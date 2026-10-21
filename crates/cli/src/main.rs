//! The `substance` command-line binary.
//!
//! Commands land over several milestones. Every subcommand in the plan's synopsis
//! parses from M0-T06 on; the ones without a body print `not implemented yet (Mx)`
//! and exit 3, so a caller can wire the CLI into CI before the stages exist.

mod checks;
mod cli;
mod error;
mod output;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{ChecksCommand, Cli, Command, SchemaCommand};
use crate::error::{CliError, EXIT_OK};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => error.report(),
    }
}

/// Parses the command line and runs the command it names.
///
/// A clap failure is printed the way clap prints it, and exits 2 for a usage error
/// and 0 for `--help`/`--version`, as clap's own exit codes prescribe.
fn run() -> Result<ExitCode, CliError> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(parse_error) => {
            let code = parse_error.exit_code();
            parse_error.print().map_err(CliError::from_write)?;
            return Ok(ExitCode::from(u8::try_from(code).unwrap_or(EXIT_OK)));
        }
    };
    dispatch(cli.command)
}

/// Runs one subcommand.
fn dispatch(command: Command) -> Result<ExitCode, CliError> {
    match command {
        Command::Run(_args) => not_yet("M0"),
        Command::Replay(_args) => not_yet("M3"),
        Command::Bench(_args) => not_yet("M5"),
        Command::Report(_args) => not_yet("M13"),
        Command::Schema { command } => match command {
            SchemaCommand::Export(_args) => not_yet("M0"),
            SchemaCommand::Validate(_args) => not_yet("M0"),
        },
        Command::Checks { command } => match command {
            ChecksCommand::List(args) => {
                checks::list(&args)?;
                Ok(ExitCode::from(EXIT_OK))
            }
        },
        Command::Doctor(_args) => not_yet("M2"),
    }
}

/// Reports a stage that this build does not have yet, as exit 3.
fn not_yet(milestone: &'static str) -> Result<ExitCode, CliError> {
    Err(CliError::NotImplemented(milestone))
}
