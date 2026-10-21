//! The git engine: commits, hunks and blame, read from the `git` CLI (ADR-0015).
//!
//! Nothing here shells out directly. Every call goes through
//! [`substance_runner::tool::run_tool_capture`], which spawns processes in exactly one
//! crate and enforces a timeout, so a hostile repository cannot wedge the pipeline and
//! no other module can start a process by accident.
//!
//! Three parser modules sit on top:
//!
//! - [`log`] reads `git log -z --numstat -M -C`, giving commits, trailers and per-file
//!   added/deleted counts with renames resolved.
//! - [`show`] reads `git show -U0`, giving per-file hunks for H08, H09 and T05.
//! - [`blame`] reads `git blame --line-porcelain`, giving the commit, author and author
//!   time of every line, with move and copy detection on.
//!
//! Every parser is total: an unexpected line becomes a [`GitError::Parse`] naming the line
//! number, never a panic. Every list is sorted by a stated key, so two runs of one commit
//! build the same model.

pub mod authored;
pub mod blame;
pub mod bots;
pub mod log;
pub mod show;

pub use authored::{
    AuthoredFile, AuthoredFilter, LISTED_LOCKFILES, LinguistAttrs, MAX_AUTHORED_LINE,
};
pub use blame::{BlameCoverage, BlameLine, BlameMap, BlameReport, blame_all};
pub use bots::{is_bot, is_bot_commit};
pub use log::{Commit, CommitIdentity, FileChange, FileStats, Trailers};
pub use show::{FileHunks, Hunk, HunkLine, HunkLineKind, PathsInCommit};

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use substance_runner::tool::{ToolError, run_tool_capture_any};

/// The binary `tools.toml` lists for everything in this module.
pub const GIT: &str = "git";

/// How long one git command may take.
///
/// A wall-clock limit is a safety cap, never a verdict input (§4.3): the command either
/// finishes or the run fails, and no check reads the elapsed time.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Which git command a [`GitError`] came from, for a message a person can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitCommand {
    /// `git log`.
    Log,
    /// `git show`.
    Show,
    /// `git blame`.
    Blame,
    /// `git ls-tree`.
    ListTree,
    /// `git check-attr`.
    CheckAttr,
    /// `git check-mailmap`.
    CheckMailmap,
}

impl GitCommand {
    /// The command as it is typed, used in error messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Log => "git log",
            Self::Show => "git show",
            Self::Blame => "git blame",
            Self::ListTree => "git ls-tree",
            Self::CheckAttr => "git check-attr",
            Self::CheckMailmap => "git check-mailmap",
        }
    }
}

/// Why the git engine could not answer.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// `git` is not installed.
    #[error("git is not installed, so the repository cannot be read")]
    Missing,
    /// git exited nonzero.
    #[error("{command} failed in {path}: {detail}")]
    Failed {
        /// The command, as typed.
        command: &'static str,
        /// The repository root it ran in.
        path: PathBuf,
        /// git's own explanation, trimmed.
        detail: String,
    },
    /// git ran, but its output did not match the format this module parses.
    #[error("cannot parse {command} output in {path}: {detail} (line {line})")]
    Parse {
        /// The command whose output failed.
        command: &'static str,
        /// The repository root it ran in.
        path: PathBuf,
        /// Which line of the output, counting from 1.
        line: usize,
        /// What was expected there.
        detail: String,
    },
    /// The command ran longer than [`TIMEOUT`].
    #[error("{command} did not finish within {seconds}s")]
    Timeout {
        /// The command, as typed.
        command: &'static str,
        /// The timeout it exceeded.
        seconds: u64,
    },
    /// The process could not be started or read.
    #[error("cannot run {command}: {source}")]
    Io {
        /// The command, as typed.
        command: &'static str,
        /// The IO error.
        source: io::Error,
    },
}

/// The parts of a failure that do not depend on which command failed.
#[derive(Debug)]
pub struct ParseFailure {
    /// The line the parser stopped on, counting from 1.
    pub line: usize,
    /// What it expected there.
    pub detail: String,
}

impl ParseFailure {
    /// A failure at `line` with `detail`.
    #[must_use]
    pub fn new(line: usize, detail: impl Into<String>) -> Self {
        Self {
            line,
            detail: detail.into(),
        }
    }
}

/// A repository root, and the git commands this crate runs in it.
///
/// The handle holds no state: `git` reads from disk on every call, so a handle never
/// goes stale, and one handle can serve several concurrent blames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    /// A handle for the work tree at `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// The work tree root.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// Runs `git` with `args` and returns stdout as text.
    ///
    /// `-c core.quotePath=false` keeps non-ASCII paths literal instead of C-quoted, so a
    /// path in the output is the path on disk; `--no-pager` stops git from trying to page.
    async fn run(&self, command: GitCommand, args: &[OsString]) -> Result<String, GitError> {
        let mut full: Vec<OsString> = vec![
            OsString::from("-c"),
            OsString::from("core.quotePath=false"),
            OsString::from("--no-pager"),
        ];
        full.extend_from_slice(args);
        let borrowed: Vec<&std::ffi::OsStr> = full.iter().map(OsString::as_os_str).collect();
        match run_tool_capture_any(GIT, &borrowed, &self.root, TIMEOUT).await {
            Ok(captured) if captured.success => Ok(captured.stdout),
            Ok(captured) => Err(GitError::Failed {
                command: command.as_str(),
                path: self.root.clone(),
                detail: captured.stderr_trimmed().to_owned(),
            }),
            Err(ToolError::Missing { .. }) => Err(GitError::Missing),
            Err(ToolError::Timeout { seconds, .. }) => Err(GitError::Timeout {
                command: command.as_str(),
                seconds,
            }),
            Err(ToolError::Io { source, .. }) => Err(GitError::Io {
                command: command.as_str(),
                source,
            }),
            Err(other) => Err(GitError::Failed {
                command: command.as_str(),
                path: self.root.clone(),
                detail: other.to_string(),
            }),
        }
    }

    /// Wraps a parser failure in a [`GitError`].
    fn parse_error(&self, command: GitCommand, failure: ParseFailure) -> GitError {
        GitError::Parse {
            command: command.as_str(),
            path: self.root.clone(),
            line: failure.line,
            detail: failure.detail,
        }
    }
}
