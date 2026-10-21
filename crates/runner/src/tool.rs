//! Running an external tool, and replaying what an earlier run saw (M1-T01).
//!
//! This is the workspace's only place that spawns a process: every other crate asks
//! this module. Two entry points:
//!
//! - [`run_tool_capture`] runs plumbing such as `git rev-parse` and keeps stdout in
//!   memory. Nothing reaches the disk.
//! - [`ToolRunner::run`] runs a tool from `tools.toml` and writes both streams to the
//!   run's content-addressed blob store, with an index entry naming them. With
//!   `--snapshot-from`, a tool whose output depends on online data is not executed at
//!   all: the saved output is replayed from that run directory.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use substance_core::hash::{canonical_json, sha256};
use substance_core::tools::ToolSpec;
use substance_core::{BlobRef, Timestamp};
use tokio::process::Command;

use crate::blobs::{BlobError, BlobStore};
use crate::permit::ExecPermit;

/// What a finished process left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// Whether the process exited 0.
    pub success: bool,
    /// The exit code, absent when the process was killed by a signal.
    pub code: Option<i32>,
    /// Standard output, as text with invalid UTF-8 replaced.
    pub stdout: String,
    /// Standard error, as text with invalid UTF-8 replaced.
    pub stderr: String,
}

impl Captured {
    /// Standard output without its trailing newline.
    #[must_use]
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim_end_matches(['\n', '\r'])
    }

    /// Standard error without its trailing newline.
    #[must_use]
    pub fn stderr_trimmed(&self) -> &str {
        self.stderr.trim_end_matches(['\n', '\r'])
    }
}

/// Why a tool could not be run to completion.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The binary is not on `PATH`.
    #[error("{command} is not installed (or not on PATH)")]
    Missing {
        /// The command that was not found.
        command: String,
    },
    /// The process ran longer than its timeout and was killed.
    #[error("{command} did not finish within {seconds}s")]
    Timeout {
        /// The command that timed out.
        command: String,
        /// The timeout it exceeded, in whole seconds.
        seconds: u64,
    },
    /// The process could not be started or its output could not be read.
    #[error("cannot run {command}: {source}")]
    Io {
        /// The command that failed.
        command: String,
        /// The IO error.
        source: io::Error,
    },
    /// The process exited nonzero where the caller required success.
    #[error("{command} exited {code}: {stderr}")]
    NonZero {
        /// The command that failed.
        command: String,
        /// Its exit code.
        code: i32,
        /// What it wrote to stderr, trimmed.
        stderr: String,
    },
    /// A tool with `exec_repo_code = true` was called without `--allow-exec`.
    #[error("{tool} runs repo-controlled code, so it needs --allow-exec")]
    NeedsExec {
        /// The tool that needed the permit.
        tool: String,
    },
    /// `--snapshot-from` was given, but that run directory has no saved output here.
    #[error("the snapshot in {path} has no saved output for {tool}")]
    SnapshotMissing {
        /// The tool whose output is missing.
        tool: String,
        /// The path that was searched.
        path: PathBuf,
    },
    /// The saved state of a run could not be read or written.
    #[error(transparent)]
    Blob(#[from] BlobError),
}

/// One tool's run, as the index records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ToolIndexEntry {
    /// The tool, as `tools.toml` names it.
    pub tool: String,
    /// The arguments, with paths rewritten relative to the repository root.
    pub args: Vec<String>,
    /// Whether the tool exited 0.
    pub success: bool,
    /// The exit code, absent when the process was killed by a signal.
    pub code: Option<i32>,
    /// The blob holding stdout.
    pub stdout: BlobRef,
    /// The blob holding stderr.
    pub stderr: BlobRef,
    /// The instant the output was captured; replay keeps the original.
    pub as_of: Timestamp,
    /// Whether this entry was replayed from `--snapshot-from` rather than executed.
    pub replayed: bool,
}

/// What a tool run produced.
///
/// [`run_tool`] fills the streams and leaves the index fields empty; the run's
/// [`ToolRunner`] adds them once the output is saved or replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// The tool's name, as `tools.toml` names it.
    pub tool: String,
    /// Whether the tool exited 0.
    pub success: bool,
    /// The exit code, absent when the process was killed by a signal.
    pub code: Option<i32>,
    /// Standard output, decoded as text.
    pub stdout: String,
    /// Standard error, decoded as text.
    pub stderr: String,
    /// The index entry that names the saved blobs, once the output is saved.
    pub entry: Option<ToolIndexEntry>,
    /// Where the index entry was written, relative to the run directory.
    pub index_path: Option<String>,
}

/// Runs `spec` once and returns its streams, without touching the disk.
///
/// This is the plan's `run_tool` shape: a missing binary is
/// [`ToolError::Missing`], which the caller reports as
/// `not_measured("tool missing: <name>")`, and a tool with `exec_repo_code = true`
/// needs `permit`. A nonzero exit is not an error: scanners usually exit nonzero when
/// they find something.
///
/// # Errors
///
/// Returns [`ToolError::Missing`], [`ToolError::NeedsExec`], [`ToolError::Timeout`] or
/// [`ToolError::Io`].
pub async fn run_tool(
    name: &str,
    spec: &ToolSpec,
    args: &[&OsStr],
    cwd: &Path,
    timeout: Duration,
    permit: Option<&ExecPermit>,
) -> Result<ToolOutput, ToolError> {
    if spec.exec_repo_code && permit.is_none() {
        return Err(ToolError::NeedsExec {
            tool: spec.bin.clone(),
        });
    }
    let captured = run_tool_capture_any(spec.bin.as_str(), args, cwd, timeout).await?;
    Ok(ToolOutput {
        tool: name.to_owned(),
        success: captured.success,
        code: captured.code,
        stdout: captured.stdout,
        stderr: captured.stderr,
        entry: None,
        index_path: None,
    })
}

impl ToolOutput {
    /// Standard output without its trailing newline.
    #[must_use]
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim_end_matches(['\n', '\r'])
    }

    /// Parses stdout as JSON, for tools that write JSON reports.
    ///
    /// # Errors
    ///
    /// Returns the serde error when stdout is not JSON.
    pub fn stdout_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(&self.stdout)
    }
}

/// Runs one tool from `tools.toml` for this run, saving its output.
///
/// The saved output lands in `blobs/` and is named by
/// `tools/<name>/<sha256(args)>.json`. Paths in the arguments are rewritten relative to
/// `repo_root` before hashing, so two checkouts of one commit at different paths produce
/// the same index key.
#[derive(Debug, Clone)]
pub struct ToolRunner<'a> {
    repo_root: &'a Path,
    run_dir: &'a Path,
    replay: Option<&'a Path>,
}

impl<'a> ToolRunner<'a> {
    /// A runner that executes tools and saves their output under `run_dir`.
    #[must_use]
    pub fn live(repo_root: &'a Path, run_dir: &'a Path) -> Self {
        Self {
            repo_root,
            run_dir,
            replay: None,
        }
    }

    /// A runner that replays the saved output found in `snapshot` instead of executing
    /// tools whose output depends on online data.
    #[must_use]
    pub fn replaying(repo_root: &'a Path, run_dir: &'a Path, snapshot: &'a Path) -> Self {
        Self {
            repo_root,
            run_dir,
            replay: Some(snapshot),
        }
    }

    /// Runs `spec` with `args` in `cwd`.
    ///
    /// A tool with `exec_repo_code = true` needs `permit`; without it the call fails with
    /// [`ToolError::NeedsExec`] before anything is spawned. A nonzero exit is not an
    /// error: most scanners report findings that way, so the caller reads `success`.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::Missing`] when the binary is absent,
    /// [`ToolError::NeedsExec`] without a permit, [`ToolError::SnapshotMissing`] when a
    /// replay finds no saved output, and [`ToolError::Io`] or [`ToolError::Timeout`] when
    /// the process itself fails.
    pub async fn run(
        &self,
        name: &str,
        spec: &ToolSpec,
        args: &[&OsStr],
        cwd: &Path,
        timeout: Duration,
        permit: Option<&ExecPermit>,
    ) -> Result<ToolOutput, ToolError> {
        if spec.exec_repo_code && permit.is_none() {
            return Err(ToolError::NeedsExec {
                tool: spec.bin.clone(),
            });
        }
        let relative_args = self.relative_args(args);
        let key = index_key(&relative_args)?;
        let index_path = format!("tools/{name}/{key}.json");
        if let Some(snapshot) = self.replay.filter(|_| spec.network) {
            return self.replay_from(snapshot, name, spec, &relative_args, &index_path);
        }
        let captured = run_tool_capture_any(spec.bin.as_str(), args, cwd, timeout).await?;
        self.save(name, spec, relative_args, captured, index_path)
    }

    /// Saves one finished run and returns it.
    fn save(
        &self,
        name: &str,
        spec: &ToolSpec,
        args: Vec<String>,
        captured: Captured,
        index_path: String,
    ) -> Result<ToolOutput, ToolError> {
        let store = BlobStore::new(self.run_dir);
        let stdout = store.put(captured.stdout.as_bytes())?;
        let stderr = store.put(captured.stderr.as_bytes())?;
        let entry = ToolIndexEntry {
            tool: spec.bin.clone(),
            args,
            success: captured.success,
            code: captured.code,
            stdout,
            stderr,
            as_of: substance_core::wall_now_utc(),
            replayed: false,
        };
        write_entry(self.run_dir, &index_path, &entry)?;
        Ok(ToolOutput {
            tool: name.to_owned(),
            success: captured.success,
            code: captured.code,
            stdout: captured.stdout,
            stderr: captured.stderr,
            entry: Some(entry),
            index_path: Some(index_path),
        })
    }

    /// Replays a saved run from `snapshot` instead of executing the tool.
    fn replay_from(
        &self,
        snapshot: &Path,
        name: &str,
        spec: &ToolSpec,
        args: &[String],
        index_path: &str,
    ) -> Result<ToolOutput, ToolError> {
        let path = snapshot.join(index_path);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ToolError::SnapshotMissing {
                    tool: spec.bin.clone(),
                    path: path.clone(),
                });
            }
            Err(source) => {
                return Err(ToolError::Blob(BlobError::Io { path, source }));
            }
        };
        let mut entry: ToolIndexEntry = serde_json::from_str(&text).map_err(|source| {
            ToolError::Blob(BlobError::Io {
                path: path.clone(),
                source: io::Error::new(io::ErrorKind::InvalidData, source),
            })
        })?;
        entry.replayed = true;
        entry.args = args.to_vec();
        let source_store = BlobStore::new(snapshot);
        let stdout = read_text(&source_store, &entry.stdout)?;
        let stderr = read_text(&source_store, &entry.stderr)?;
        Ok(ToolOutput {
            tool: name.to_owned(),
            success: entry.success,
            code: entry.code,
            stdout,
            stderr,
            entry: Some(entry),
            index_path: Some(index_path.to_owned()),
        })
    }

    /// Rewrites every argument that names a path inside the repository as a relative path.
    fn relative_args(&self, args: &[&OsStr]) -> Vec<String> {
        args.iter().map(|arg| self.relative_arg(arg)).collect()
    }

    /// Rewrites one argument, leaving anything that is not a path under the root alone.
    fn relative_arg(&self, arg: &OsStr) -> String {
        let text = arg.to_string_lossy();
        let as_path = Path::new(text.as_ref());
        match as_path.strip_prefix(self.repo_root) {
            Ok(relative) => relative.to_string_lossy().into_owned(),
            Err(_) => text.into_owned(),
        }
    }
}

/// The index key for `args`: the SHA-256 of their canonical JSON, in hex.
fn index_key(args: &[String]) -> Result<String, ToolError> {
    let bytes = canonical_json(&args.to_vec()).map_err(|error| {
        ToolError::Blob(BlobError::Io {
            path: PathBuf::from("tools index"),
            source: io::Error::new(io::ErrorKind::InvalidData, error.to_string()),
        })
    })?;
    Ok(substance_core::hash::hex(&sha256(&bytes)))
}

/// Writes one index entry under the run directory.
fn write_entry(run_dir: &Path, index_path: &str, entry: &ToolIndexEntry) -> Result<(), ToolError> {
    let path = run_dir.join(index_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            ToolError::Blob(BlobError::Io {
                path: parent.to_path_buf(),
                source,
            })
        })?;
    }
    let json = serde_json::to_value(entry).map_err(|error| {
        ToolError::Blob(BlobError::Io {
            path: path.clone(),
            source: io::Error::new(io::ErrorKind::InvalidData, error),
        })
    })?;
    let bytes = substance_core::hash::canonical_json_pretty_value(&json);
    std::fs::write(&path, bytes).map_err(|source| ToolError::Blob(BlobError::Io { path, source }))
}

/// Reads a blob as text, replacing invalid UTF-8.
fn read_text(store: &BlobStore, reference: &BlobRef) -> Result<String, ToolError> {
    let bytes = store.get(reference)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Runs `command` with `args` in `cwd` and requires a zero exit.
///
/// # Errors
///
/// Returns [`ToolError::Missing`] when the binary does not exist,
/// [`ToolError::Timeout`] when it outlives `timeout`, [`ToolError::Io`] when it cannot
/// be spawned or read, and [`ToolError::NonZero`] when it exits nonzero.
pub async fn run_tool_capture(
    command: &str,
    args: &[&OsStr],
    cwd: &Path,
    timeout: Duration,
) -> Result<Captured, ToolError> {
    let captured = capture(command, args, cwd, timeout).await?;
    if captured.success {
        return Ok(captured);
    }
    Err(ToolError::NonZero {
        command: command.to_owned(),
        code: captured.code.unwrap_or(-1),
        stderr: captured.stderr_trimmed().to_owned(),
    })
}

/// Runs `command` with `args` in `cwd` and captures its output whatever the exit code.
///
/// A nonzero exit is not an error here: `git rev-parse --is-shallow-repository` answers
/// `false` for an ordinary repository and fails for a directory that is not a repository,
/// and the caller needs both the code and the output to tell them apart.
///
/// # Errors
///
/// Returns [`ToolError::Missing`], [`ToolError::Timeout`] or [`ToolError::Io`], never
/// [`ToolError::NonZero`].
pub async fn run_tool_capture_any(
    command: &str,
    args: &[&OsStr],
    cwd: &Path,
    timeout: Duration,
) -> Result<Captured, ToolError> {
    capture(command, args, cwd, timeout).await
}

/// Spawns the process, waits for it inside `timeout`, and captures both streams.
///
/// The process is killed when the future is dropped, so a cancelled stage cannot leave
/// a child behind.
async fn capture(
    command: &str,
    args: &[&OsStr],
    cwd: &Path,
    timeout: Duration,
) -> Result<Captured, ToolError> {
    let mut child = Command::new(command);
    child
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, child.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ToolError::Missing {
                command: command.to_owned(),
            });
        }
        Ok(Err(source)) => {
            return Err(ToolError::Io {
                command: command.to_owned(),
                source,
            });
        }
        Err(_) => {
            return Err(ToolError::Timeout {
                command: command.to_owned(),
                seconds: timeout.as_secs(),
            });
        }
    };
    Ok(Captured {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
