//! `substance run`: one evaluation, and the exit code that follows (M0-T08).
//!
//! | code | when |
//! | --- | --- |
//! | 0 | the run finished and no threshold was hit |
//! | 1 | `--fail-on` was hit (no gate can fire until M1) |
//! | 2 | the input, the config or the command line cannot be used |
//! | 3 | something inside substance failed |

use std::process::ExitCode;

use substance_orchestrator::{RunError, RunOptions, RunOutcome};

use crate::cli::{Format, RunArgs};
use crate::error::{CliError, EXIT_FAILURE, EXIT_OK};
use crate::output::stderr_line;

/// Runs `substance run`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] for an intake or config problem (exit 2) and
/// [`CliError::Internal`] for a failure inside substance (exit 3).
pub fn run_command(args: &RunArgs) -> Result<ExitCode, CliError> {
    let options = options_of(args);
    let outcome = runtime()?.block_on(substance_orchestrator::run(&options));
    match outcome {
        Ok(outcome) => Ok(finish(args, &outcome)),
        Err(error) => Err(map_error(error)),
    }
}

/// Builds the orchestrator's options from the parsed flags.
fn options_of(args: &RunArgs) -> RunOptions {
    RunOptions {
        repo: args.repo.clone(),
        rev: args.rev.clone(),
        out: args.out.clone(),
        as_of: args.as_of,
        seed: args.seed,
        config: args.config.clone(),
        triage_only: args.triage_only,
        allow_exec: args.allow_exec,
        snapshot_from: args.snapshot_from.clone(),
    }
}

/// Reports the outcome and decides the exit code.
///
/// The formats this build cannot write yet are named on stderr and skipped, so
/// `--format json,html` still produces the JSON report instead of failing.
fn finish(args: &RunArgs, outcome: &RunOutcome) -> ExitCode {
    for format in &args.format {
        match format {
            Format::Json => {}
            Format::Html => stderr_line("html: not implemented yet (M1-T12)"),
            Format::Sarif => stderr_line("sarif: not implemented yet (M13-T02)"),
        }
    }
    stderr_line(&summary(outcome));
    let gates = outcome.gates_hit.len();
    if args.fail_on.wants_failure(gates) {
        ExitCode::from(EXIT_FAILURE)
    } else {
        ExitCode::from(EXIT_OK)
    }
}

/// One line a person can read without opening the report.
fn summary(outcome: &RunOutcome) -> String {
    let coverage = &outcome.report.coverage;
    format!(
        "{} checks: {} measured, {} not measured; report in {}",
        coverage.checks_total,
        coverage.checks_measured,
        coverage
            .checks_total
            .saturating_sub(coverage.checks_measured),
        outcome.run_dir.display(),
    )
}

/// Maps an orchestrator failure to the CLI's exit codes.
fn map_error(error: RunError) -> CliError {
    match error {
        RunError::Intake(inner) => CliError::Usage(inner.to_string()),
        RunError::Config(inner) => CliError::Usage(inner.to_string()),
        other => CliError::Internal(other.to_string()),
    }
}

/// The async runtime one command runs on.
///
/// A multi-threaded runtime with the time driver: intake waits on child processes with
/// timeouts, which needs both.
fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Internal(format!("cannot start the async runtime: {error}")))
}
