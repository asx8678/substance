//! Why a command stopped, and the exit code the plan assigns to it.
//!
//! | code | meaning |
//! | --- | --- |
//! | 0 | the command did what it was asked |
//! | 1 | the document is invalid, or the `--fail-on` threshold was hit |
//! | 2 | a usage, config or intake error (clap reports these itself) |
//! | 3 | an internal error, including a stage this build does not have |
//!
//! A closed pipe is not a failure: `substance checks list | head` exits 0.

use std::process::ExitCode;

/// Exit code: the command did what it was asked.
pub const EXIT_OK: u8 = 0;
/// Exit code: the input is well-formed but invalid, or the threshold was hit.
pub const EXIT_FAILURE: u8 = 1;
/// Exit code: a usage, config or intake error.
pub const EXIT_USAGE: u8 = 2;
/// Exit code: an internal error, or a stage that is not built yet.
pub const EXIT_INTERNAL: u8 = 3;

/// A reason to stop before finishing the command.
#[derive(Debug)]
pub enum CliError {
    /// The caller asked for a stage this build does not have yet.
    NotImplemented(&'static str),
    /// The command line, the config or the input cannot be used.
    Usage(String),
    /// Something inside substance failed.
    Internal(String),
    /// Whoever was reading our output went away; that is not our failure.
    BrokenPipe,
}

impl CliError {
    /// Maps a failed write to stdout or stderr.
    ///
    /// A closed pipe is reported as [`CliError::BrokenPipe`], so `substance … | head`
    /// exits 0 instead of failing; every other write error is internal.
    #[must_use]
    pub fn from_write(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            Self::BrokenPipe
        } else {
            Self::Internal(format!("cannot write output: {error}"))
        }
    }

    /// The exit code for this reason.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotImplemented(_) | Self::Internal(_) => EXIT_INTERNAL,
            Self::Usage(_) => EXIT_USAGE,
            Self::BrokenPipe => EXIT_OK,
        }
    }

    /// The line written to stderr.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NotImplemented(milestone) => format!("not implemented yet ({milestone})"),
            Self::Usage(text) | Self::Internal(text) => text.clone(),
            Self::BrokenPipe => String::new(),
        }
    }

    /// Prints this reason and turns it into the process exit code.
    #[must_use]
    pub fn report(self) -> ExitCode {
        if !matches!(self, Self::BrokenPipe) {
            crate::output::stderr_line(self.message().as_str());
        }
        ExitCode::from(self.exit_code())
    }
}
