//! S01 Slop pattern scan (M1-T08).
//!
//! Formula (action plan, S01):
//!
//! > Run aislop, antislop and AI-SLOP Detector, whichever are on PATH (confirm their flags
//! > with `--help`), and parse their output. None present → not_measured.
//!
//! Substance does not reimplement these scanners. It runs whichever are installed, collects
//! what they report, and names them so a reader knows which tool disagreed. The three are
//! independent projects with their own ideas of "slop", and running all three is the point:
//! agreement between two is stronger evidence than one tool's opinion.
//!
//! **Flags are not guessed.** The plan says to confirm them with `--help`, and in this
//! environment none of the three binaries is installed, so no flag has been confirmed and
//! none is asserted here. Until a version is pinned in `tools.toml` and its output parsed
//! from a real run, every tool reports `not_measured` with `tool missing: <name>`; when one is
//! present but this build cannot confirm how to read it, the verdict says that instead. That
//! is deliberate: inventing `--json` for a tool whose real flag is `--format json` would
//! produce a confident zero.
//!
//! The metrics that do not depend on a tool's output are always reported: whether each
//! scanner was found, which is what a reader needs to interpret an all-clear.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Verdict;
use substance_core::tools::{ToolSpec, ToolsFile};
use substance_runner::tool::{ToolError, ToolRunner};

use crate::checks::verdict::StaticReport;

/// The tools S01 runs, in the order the plan lists them.
pub const SLOP_TOOLS: &[&str] = &["aislop", "antislop", "ai-slop-detector"];

/// What one scanner contributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScannerRun {
    /// The tool ran and its report was read.
    Reported {
        /// How many findings it produced.
        findings: u32,
    },
    /// The tool could not be run, with the sentence the verdict carries.
    Unavailable {
        /// Why there is no result.
        reason: String,
    },
}

impl ScannerRun {
    /// The reason, when there is one.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Reported { .. } => None,
            Self::Unavailable { reason } => Some(reason.as_str()),
        }
    }
}

/// Judges S01.
///
/// The probe is the version call `tools.toml` already defines: it answers "is this binary
/// here, and is it new enough to be the one the table names" without guessing any flag. A
/// tool that answers is recorded as present and still contributes nothing to the count until
/// its report format is pinned, which is what the per-part verdict says.
///
/// # Errors
///
/// Never: every failure is a part, because S01's job is to report what the environment
/// allowed, and a missing scanner is not an error in the repository.
pub async fn slop_scan(
    runner: &ToolRunner<'_>,
    table: &ToolsFile,
    root: &std::path::Path,
) -> StaticReport {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut parts: Vec<(&str, Verdict)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut present: Vec<String> = Vec::new();
    for name in SLOP_TOOLS {
        let Some(spec) = table.spec(name) else {
            metrics.insert(format!("found.{name}"), 0);
            parts.push((
                name,
                Verdict::NotMeasured {
                    reason: format!("{name} is not listed in tools.toml"),
                },
            ));
            continue;
        };
        let outcome = probe(runner, name, spec, root).await;
        match &outcome {
            ScannerRun::Reported { findings } => {
                present.push((*name).to_owned());
                metrics.insert(format!("found.{name}"), 1);
                metrics.insert(format!("findings.{name}"), i64::from(*findings));
                parts.push((name, Verdict::Reported));
            }
            ScannerRun::Unavailable { reason } => {
                metrics.insert(format!("found.{name}"), 0);
                missing.push((*name).to_owned());
                parts.push((
                    name,
                    Verdict::NotMeasured {
                        reason: reason.clone(),
                    },
                ));
            }
        }
    }
    let mut report = StaticReport::not_measured(
        CheckId::S01,
        if missing.is_empty() {
            "no slop scanner produced a report this build can read".to_owned()
        } else {
            format!("tool missing: {}", missing.join(", "))
        },
    );
    report.verdict.metrics = metrics;
    for (part, verdict) in parts {
        report = report.with_part(part, verdict);
    }
    if !present.is_empty() {
        notes.push(format!(
            "found on PATH: {}; their report formats are not pinned yet, so their findings \
             are not counted",
            present.join(", ")
        ));
    }
    report.with_notes(notes)
}

/// Finds out whether one scanner can run, without guessing its report flags.
///
/// The version call is the only invocation `tools.toml` defines and the only one this build
/// may make: it starts the binary, proves it exists, and confirms the version floor. Nothing
/// is parsed out of it beyond presence.
async fn probe(
    runner: &ToolRunner<'_>,
    name: &str,
    spec: &ToolSpec,
    root: &std::path::Path,
) -> ScannerRun {
    if spec.version_args.is_empty() {
        return ScannerRun::Unavailable {
            reason: format!("tool missing: {name}"),
        };
    }
    let args: Vec<&std::ffi::OsStr> = spec.version_args.iter().map(std::ffi::OsStr::new).collect();
    match runner
        .run(
            name,
            spec,
            &args,
            root,
            std::time::Duration::from_secs(20),
            None,
        )
        .await
    {
        Ok(_) => ScannerRun::Unavailable {
            reason: format!(
                "{name} is installed, but this build cannot yet read its report: its output \
                 format is unconfirmed (the pin is TBD)"
            ),
        },
        Err(ToolError::Missing { .. }) => ScannerRun::Unavailable {
            reason: format!("tool missing: {name}"),
        },
        Err(error) => ScannerRun::Unavailable {
            reason: error.to_string(),
        },
    }
}
