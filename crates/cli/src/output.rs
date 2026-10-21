//! Writing to stdout and stderr without panickingon a closed pipe.
//!
//! `std::println!` panics when the reader went away (`substance checks list | head`),
//! so every write here checks the result and reports a broken pipe as success.

use std::io::Write;

use crate::error::CliError;

/// Writes `text` to stdout, adding a newline.
///
/// # Errors
///
/// Returns [`CliError::BrokenPipe`] when the reader closed the pipe, and
/// [`CliError::Internal`] when the write fails for another reason.
pub fn stdout_line(text: &str) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .map_err(CliError::from_write)
}

/// Writes `text` to stderr, adding a newline, and ignores the outcome.
///
/// This is used while reporting a failure, when a second failure has nowhere to go.
pub fn stderr_line(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.write_all(b"\n");
}
