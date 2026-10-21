//! `substance replay`: rerun one recorded action and diff it (M3-T07).
//!
//! | code | when |
//! | --- | --- |
//! | 0 | the replayed record normalized to the same bytes |
//! | 1 | it normalized to different bytes; the diff is printed |
//! | 2 | the id, the run directory, the source or the flags cannot be used |
//! | 3 | the replay cannot run on this host, or a finding has no record to rerun |

use std::process::ExitCode;

use substance_orchestrator::replay::{Ending, ReplayOptions, replay};
use substance_orchestrator::{RunError, tools};

use crate::cli::ReplayArgs;
use crate::error::{CliError, EXIT_OK};
use crate::output::stdout_line;

/// Runs `substance replay`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] for a bad argument (exit 2), and [`CliError::Internal`] for a stage of
/// the replay that failed (exit 3). The two verdicts -- identical and different -- are `Ok` exit
/// codes, because a replay that found a difference did what it was asked.
pub fn replay_command(args: &ReplayArgs) -> Result<ExitCode, CliError> {
    let tools_file = tools::table().map_err(|error| CliError::Internal(error.to_string()))?;
    let table: Vec<(String, substance_core::tools::ToolSpec)> = tools_file
        .tools
        .iter()
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();
    // The permit is granted for this invocation -- never read back from the run's `invocation.json`.
    // A recorded `--allow-exec` was a human's decision about *that* run; repeating it later,
    // unattended, would make the flag mean "forever".
    let permit = substance_orchestrator::permit_for(args.allow_exec);
    if permit.is_none() {
        return Err(CliError::Usage(
            "replay boots the repository's code, so it needs --allow-exec".to_owned(),
        ));
    }
    let options = ReplayOptions {
        run: args.run.clone(),
        id: args.id.clone(),
        allow_exec: args.allow_exec,
        source: args.source.clone(),
        tools: table,
        permit,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Internal(format!("cannot start the async runtime: {error}")))?;
    match runtime.block_on(replay(&options)) {
        Ok(ending) => report(&ending),
        Err(error @ (RunError::Usage(_) | RunError::Replay(_))) => Err(map_error(&error)),
        Err(error) => Err(CliError::Internal(error.to_string())),
    }
}

/// Prints the ending and returns its exit code.
fn report(ending: &Ending) -> Result<ExitCode, CliError> {
    match ending {
        Ending::Identical { record, written } => {
            stdout_line(&format!(
                "{}: identical after normalization; the new record is {}",
                record.as_str(),
                written.display()
            ))?;
            Ok(ExitCode::from(EXIT_OK))
        }
        Ending::Different {
            record,
            written,
            diff,
        } => {
            // The diff goes to stdout rather than stderr: it is the command's output, and a caller
            // that did not ask for it should still be able to pipe it into `diffstat`.
            stdout_line(&format!(
                "{}: different after normalization; the new record is {}",
                record.as_str(),
                written.display()
            ))?;
            stdout_line(diff)?;
            Ok(ExitCode::from(
                substance_orchestrator::replay::Ending::exit_code(ending),
            ))
        }
    }
}

/// Maps a run error to the CLI error that carries its exit code.
fn map_error(error: &RunError) -> CliError {
    match error {
        RunError::Usage(_) => CliError::Usage(error.to_string()),
        other => CliError::Internal(other.to_string()),
    }
}
