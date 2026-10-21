//! The working copy: the commit under test, exported where nothing else can see it (M2-T01).
//!
//! §4.1: "substance never runs code in the user's checkout. It exports the commit under test to
//! `runs/<run_id>/work/`." Two export methods are allowed, and the choice between them is not
//! cosmetic:
//!
//! - `git worktree add --detach` is fast, but it writes into the **source repository**:
//!   `.git/worktrees/<name>/`, plus a `.git` file in the new directory. That is a write to the
//!   user's repository, which a tool that promises to leave the checkout alone should not make,
//!   and it leaves state behind if the run is killed. It also refuses to overwrite a directory
//!   outside its bookkeeping, so recovering from an interrupted run means editing the user's git
//!   directory.
//! - `git archive <sha> | tar -x` reads the commit and writes only under the run directory. The
//!   price is a full copy of the tree per run.
//!
//! This module takes the second. "Never touch the user's checkout files" is the stronger promise,
//! and a run directory that can be deleted with `rm -rf` is worth the copy.
//!
//! `git archive` writes a tar to a file first and `tar` unpacks it in a second process, rather
//! than piping one into the other: a pipe needs a thread or an `unsafe` descriptor shuffle to
//! wire up under `tokio`, and the plan forbids `unsafe` outright. The tarball is deleted as soon
//! as it is unpacked, so a run does not keep two copies of the tree.
//!
//! ## Named exports
//!
//! A run may need a second copy of the same commit: the README rung of the boot ladder (M2-T02)
//! runs arbitrary shell from a document, and a failed install there must not leave a half-built
//! `node_modules` behind for the rung that follows. [`WorkCopy::create`] is the run's own copy and
//! always lives at `work/`; [`WorkCopy::create_named`] names its directory, so the second copy is
//! `work-readme/` and the two cannot be confused. Both export the same commit by the same means,
//! and both are removed the same way: nothing about a named export is second class except its name.
//!
//! ## What an export is not
//!
//! `git archive` contains tracked files at that commit and nothing else: no `.git`, no untracked
//! files, no ignored build output. That is exactly what a reproducible run needs — the commit
//! identifies the content — and it is why the caller must not expect `git` commands to work
//! inside the work copy. Anything needing history reads the source repository.

use std::io;
use std::path::{Path, PathBuf};

use substance_core::RunId;

use crate::tool::{ToolError, run_tool_capture_any};

/// A work tree exported under a run directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCopy {
    /// The exported tree, `runs/<run_id>/work/`.
    root: PathBuf,
    /// The source repository the tree came from, for removal and for later stages that need
    /// history.
    source: PathBuf,
    /// The commit the tree holds.
    commit_sha: String,
    /// The run directory the tree lives in.
    run_dir: PathBuf,
}

/// Why a work copy could not be created or removed.
#[derive(Debug, thiserror::Error)]
pub enum WorkCopyError {
    /// The source is not a git work tree, so there is no commit to export.
    #[error("{path} is not a git work tree")]
    NotARepository {
        /// The path that was checked.
        path: PathBuf,
    },
    /// The commit is not in the source repository.
    #[error("commit {sha} is not in {path}")]
    NoSuchCommit {
        /// The commit that was asked for.
        sha: String,
        /// The repository that was searched.
        path: PathBuf,
    },
    /// `git archive` or `tar` failed.
    #[error("{command} exited {code}: {stderr}")]
    Command {
        /// The command that failed.
        command: String,
        /// Its exit code.
        code: i32,
        /// What it wrote to stderr, trimmed.
        stderr: String,
    },
    /// A program needed for the export is missing from `PATH`.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// A directory or file could not be created or removed.
    #[error("cannot prepare {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
}

/// How long one export step may take.
///
/// `git archive` on a large repository is IO-bound and can legitimately run for minutes. This cap
/// exists so a wedged process cannot hang the run forever, not as a performance target.
const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

impl WorkCopy {
    /// The directory name a run's own work copy lives under.
    ///
    /// Named because a second export needs a different name, and a reader of a run directory
    /// should be able to tell which copy is which without opening a manifest.
    pub const DIR: &'static str = "work";

    /// Exports `commit_sha` from the repository at `source` into `<run_dir>/work/`.
    ///
    /// The directory is created fresh; an existing `work/` is removed first, so a retry in a
    /// reused run directory cannot mix two trees.
    ///
    /// # Errors
    ///
    /// Returns [`WorkCopyError::NoSuchCommit`] when the commit is not in `source`,
    /// [`WorkCopyError::Command`] when git or tar fails, [`WorkCopyError::Tool`] when either
    /// program is missing, and [`WorkCopyError::Io`] when the directories cannot be made.
    pub async fn create(
        source: &Path,
        commit_sha: &str,
        run_dir: &Path,
        run_id: &RunId,
    ) -> Result<Self, WorkCopyError> {
        Self::create_named(source, commit_sha, run_dir, run_id, Self::DIR).await
    }

    /// Exports `commit_sha` into `<run_dir>/<name>/`.
    ///
    /// For a copy that must stay independent of the run's own tree: the boot ladder's README rung
    /// is handed `"work-readme"` so its install cannot dirty the copy rungs 2 and 3 use. `name`
    /// must be a single path component -- a caller that passed `../x` would be exporting outside
    /// the run directory, which is the one thing this module exists to prevent.
    ///
    /// # Errors
    ///
    /// Returns [`WorkCopyError::Io`] when `name` is not a single component, along with everything
    /// [`Self::create`] returns.
    pub async fn create_named(
        source: &Path,
        commit_sha: &str,
        run_dir: &Path,
        run_id: &RunId,
        name: &str,
    ) -> Result<Self, WorkCopyError> {
        let _ = run_id;
        ensure_repository(source).await?;
        if Path::new(name).components().count() != 1 || name.starts_with('.') {
            return Err(WorkCopyError::Io {
                path: run_dir.join(name),
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a work copy's name must be a single path component",
                ),
            });
        }
        let root = run_dir.join(name);
        remove_dir(&root)?;
        create_dir(&root)?;

        // `git archive` names the output with `-o` rather than writing to a pipe, so no stdin
        // wiring is needed and the two programs stay separate. The tarball is named after the
        // copy, so two exports in one run cannot overwrite each other's.
        let tarball = run_dir.join(format!("{name}.tar"));
        let tarball_arg = tarball.as_os_str();
        let out = run_tool_capture_any(
            "git",
            &[
                std::ffi::OsStr::new("archive"),
                std::ffi::OsStr::new("--format=tar"),
                std::ffi::OsStr::new("-o"),
                tarball_arg,
                std::ffi::OsStr::new(commit_sha),
            ],
            source,
            EXPORT_TIMEOUT,
        )
        .await?;
        if !out.success {
            let _ = std::fs::remove_file(&tarball);
            return Err(command_error("git archive", &out));
        }
        let unpacked = run_tool_capture_any(
            "tar",
            &[
                std::ffi::OsStr::new("-xf"),
                tarball_arg,
                std::ffi::OsStr::new("-C"),
                root.as_os_str(),
            ],
            run_dir,
            EXPORT_TIMEOUT,
        )
        .await;
        // The tarball is removed either way: on success it is redundant, on failure the error
        // message is about tar and a stray copy of the tree would only confuse the next reader.
        let _ = std::fs::remove_file(&tarball);
        let out = unpacked?;
        if !out.success {
            return Err(command_error("tar -xf", &out));
        }
        Ok(Self {
            root,
            source: source.to_path_buf(),
            commit_sha: commit_sha.to_owned(),
            run_dir: run_dir.to_path_buf(),
        })
    }

    /// The exported tree's root: the directory every command runs in.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The commit the tree holds.
    #[must_use]
    pub fn commit_sha(&self) -> &str {
        &self.commit_sha
    }

    /// The source repository, for stages that need history or the origin URL.
    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The run directory the tree lives under.
    #[must_use]
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// Deletes the exported tree.
    ///
    /// Nothing in the source repository is touched, which is the whole point of the export: a
    /// failed run leaves the user's checkout exactly as it was.
    ///
    /// # Errors
    ///
    /// Returns [`WorkCopyError::Io`] when the directory cannot be removed.
    pub fn remove(&self) -> Result<(), WorkCopyError> {
        remove_dir(&self.root)
    }
}

/// Whether `path` is inside a git work tree.
async fn ensure_repository(path: &Path) -> Result<(), WorkCopyError> {
    let out = run_tool_capture_any(
        "git",
        &[
            std::ffi::OsStr::new("rev-parse"),
            std::ffi::OsStr::new("--git-dir"),
        ],
        path,
        EXPORT_TIMEOUT,
    )
    .await?;
    if out.success {
        Ok(())
    } else {
        Err(WorkCopyError::NotARepository {
            path: path.to_path_buf(),
        })
    }
}

/// Turns a failed tool run into the matching error.
///
/// The stderr is carried through rather than classified: `git archive` reports a bad commit and a
/// missing repository with the same exit code, and the sentence it prints is what tells them
/// apart. A caller that needs to know reads the message; nothing here pretends to.
fn command_error(command: &str, out: &crate::tool::Captured) -> WorkCopyError {
    WorkCopyError::Command {
        command: command.to_owned(),
        code: out.code.unwrap_or(-1),
        stderr: out.stderr_trimmed().to_owned(),
    }
}

/// Creates `dir` and its parents.
fn create_dir(dir: &Path) -> Result<(), WorkCopyError> {
    std::fs::create_dir_all(dir).map_err(|source| WorkCopyError::Io {
        path: dir.to_path_buf(),
        source,
    })
}

/// Removes `dir` if it exists.
fn remove_dir(dir: &Path) -> Result<(), WorkCopyError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(WorkCopyError::Io {
            path: dir.to_path_buf(),
            source,
        }),
    }
}
