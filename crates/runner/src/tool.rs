//! Running an external tool and capturing its output in memory (M0-T08).
//!
//! This is the workspace's only place that spawns a process: every other crate asks
//! this module. Nothing here touches the disk, so it suits high-volume plumbing such
//! as `git rev-parse`; M1-T01 adds `run_tool`, which saves output as blobs.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

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
        /// The underlying IO error.
        source: io::Error,
    },
    /// The process ran and exited nonzero.
    #[error("{command} exited with {code}: {stderr}")]
    NonZero {
        /// The command that failed.
        command: String,
        /// The exit code, or `-1` when the process was killed by a signal.
        code: i32,
        /// The captured error stream, trimmed.
        stderr: String,
    },
}

/// Runs `command` with `args` in `cwd` and captures its output, requiring exit 0.
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
/// a child behind. Nothing is written to disk.
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
