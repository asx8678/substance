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
use crate::error::EXIT_INTERNAL;
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
        // An interrupt is reported as "interrupted", on one line, with exit 3. It goes through the
        // error path rather than the outcome path because a run that stopped halfway has not
        // produced the report `finish` would print a summary of.
        Err(error @ RunError::Interrupted { .. }) => {
            stderr_line(&format!("substance run: {error}"));
            Ok(ExitCode::from(EXIT_INTERNAL))
        }
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
            Format::Html => match substance_report::render(&outcome.report, calibrated()) {
                Ok(page) => {
                    let path = outcome.run_dir.join("report.html");
                    if let Err(error) = std::fs::write(&path, page) {
                        // A report that could not be written is an internal error, not a verdict:
                        // exit 3 rather than letting a missing file read as a clean run.
                        stderr_line(&format!("cannot write {}: {error}", path.display()));
                        return ExitCode::from(EXIT_INTERNAL);
                    }
                }
                Err(error) => {
                    stderr_line(&format!("report.html: {error}"));
                    return ExitCode::from(EXIT_INTERNAL);
                }
            },
            Format::Sarif => stderr_line("sarif: not implemented yet (M13-T02)"),
        }
    }
    stderr_line(&summary(outcome));
    // A hint or an unconfirmed finding never trips --fail-on: the plan's thresholds are about
    // confirmed findings, and a hint is a lead rather than a defect.
    let severities: Vec<substance_core::Severity> = outcome
        .report
        .findings
        .iter()
        .filter(|finding| !matches!(finding.confidence, substance_core::Confidence::Hint))
        .map(|finding| finding.severity)
        .collect();
    if args
        .fail_on
        .wants_failure_for(outcome.gates_hit.len(), &severities)
    {
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
        RunError::Usage(text) => CliError::Usage(text),
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

/// Whether the embedded thresholds are calibrated (ADR-0016).
///
/// Read from the same embedded file the run used, so the page's banner cannot disagree with the
/// numbers it is printed beside. A malformed file leaves the banner on: telling a reader the
/// numbers are provisional is the safe direction when the file cannot be read.
fn calibrated() -> bool {
    substance_orchestrator::thresholds::thresholds()
        .map(|thresholds| thresholds.calibrated)
        .unwrap_or(false)
}
