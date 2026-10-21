//! H21 OpenSSF Scorecard: what an outside tool says about the repository's posture.
//!
//! Formula (action plan, H21):
//!
//! > `scorecard --repo=github.com/<o>/<r> --format=json` from PATH (needs
//! > `GITHUB_AUTH_TOKEN`). Import Maintained, Code-Review, Contributors, Branch-Protection,
//! > CI-Tests, Dangerous-Workflow and Pinned-Dependencies as 0–10 integers.
//!
//! Scorecard is a separate program that reads the forge. Substance does not reimplement it:
//! it runs the binary when it is on `PATH`, parses the JSON it prints, and imports the seven
//! checks the plan names. Each score is already a 0–10 integer in Scorecard's output, so
//! nothing is scaled or reinterpreted here.
//!
//! The run happens in [`crate::signals::HistoryInput::gather`], through the [`ToolRunner`],
//! and the judge below reads only its result. Two properties follow, and both are why the
//! call does not live here:
//!
//! - The tool's stdout and stderr are saved as blobs with an entry under `tools/scorecard/`,
//!   so a verdict can be traced to the bytes a tool printed (M1-T01).
//! - Under `--snapshot-from` a `network = true` tool is *replayed* from the earlier run
//!   rather than executed, which is what makes a run reproducible offline (§3 issue 29).
//!
//! The token is the `GITHUB_AUTH_TOKEN` environment variable, which Scorecard reads itself.
//! Substance never puts it on a command line — an argument is readable by every process on
//! the machine — and never stores it: the child inherits the environment, and the command
//! recorded in evidence is the one [`scorecard_command`] returns, which has no token in it.
//!
//! A missing binary is `not_measured("tool missing: scorecard")`, the plan's own wording. A
//! binary that fails, times out, or prints something that is not Scorecard's JSON is
//! `not_measured` with the reason: a run that did not happen must not read as a clean score.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use substance_core::BlobRef;
use substance_core::model::Evidence;
use substance_core::thresholds::Thresholds;
use substance_core::tools::{ToolSpec, meets_floor};
use substance_runner::tool::{ToolError, ToolOutput, ToolRunner};

use super::HistoryInput;
use super::shape::Judgment;

/// The program this check runs, as `PATH` spells it (and as `tools.toml` names it).
pub const SCORECARD: &str = "scorecard";

/// How long the tool may run, in seconds.
///
/// Scorecard makes dozens of API calls, so two minutes is generous; a longer run is a hung
/// network rather than a large repository.
pub const SCORECARD_TIMEOUT_SECONDS: u64 = 120;

/// How long the version probe may take. It starts a binary and prints one line.
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

/// The Scorecard checks the plan imports, as `(metric key, Scorecard's check name)`.
pub const IMPORTED_CHECKS: &[(&str, &str)] = &[
    ("maintained", "Maintained"),
    ("code_review", "Code-Review"),
    ("contributors", "Contributors"),
    ("branch_protection", "Branch-Protection"),
    ("ci_tests", "CI-Tests"),
    ("dangerous_workflow", "Dangerous-Workflow"),
    ("pinned_dependencies", "Pinned-Dependencies"),
];

/// The command line, spelled out so a reader can reproduce it. It carries no token.
#[must_use]
pub fn scorecard_command(slug: &str) -> String {
    format!("{SCORECARD} --repo=github.com/{slug} --format=json")
}

/// What running Scorecard produced.
///
/// The two variants are the two things a judge can do with a tool: import its numbers, or
/// say why there are none. The reason travels as text because it becomes the verdict's
/// `not_measured` reason verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScorecardRun {
    /// The tool ran and its report held at least one of the imported checks.
    Scores {
        /// The 0–10 scores, keyed by metric name, sorted.
        scores: BTreeMap<String, i64>,
        /// The tool's stdout, for the evidence reference.
        blob: BlobRef,
        /// The exact command line, without a token.
        command: String,
        /// Whether the numbers came from a snapshot rather than a live run.
        replayed: bool,
    },
    /// The tool could not be run, or did not print a report this build understands.
    Unavailable {
        /// The sentence the verdict carries.
        reason: String,
    },
}

impl ScorecardRun {
    /// The absence, spelled the plan's way.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    /// The absence of the repository's name, which no tool can be asked about.
    #[must_use]
    pub fn no_slug() -> Self {
        Self::unavailable(
            "scorecard needs the GitHub repository name, which the forge did not provide",
        )
    }
}

/// Runs Scorecard in `root` and returns the seven imported scores.
///
/// The call goes through the run's [`ToolRunner`], which is what makes the output a blob and
/// what makes `--snapshot-from` replay it instead of executing it. The arguments are exactly
/// the documented ones: `--repo` names the repository by its host-qualified slug, and
/// `--format=json` makes the output the document parsed below.
///
/// # Errors
///
/// Part of [`HistoryInput::gather`]: it never fails the run. Every failure — a missing
/// binary, a timeout, a nonzero exit, a report this build cannot read — becomes
/// [`ScorecardRun::Unavailable`] with the sentence a `not_measured` verdict carries.
pub async fn gather(
    runner: &ToolRunner<'_>,
    spec: &ToolSpec,
    tool_name: &str,
    root: &Path,
    slug: &str,
) -> ScorecardRun {
    // Ask the version first: a Scorecard older than `tools.toml`'s floor reports different
    // check names, so its numbers would not be the seven this build imports. The probe goes
    // through the same runner, so its output is a blob too, and a snapshot carries it.
    let version_args: Vec<&OsStr> = spec.version_args.iter().map(OsStr::new).collect();
    if !version_args.is_empty() {
        match runner
            .run(tool_name, spec, &version_args, root, VERSION_TIMEOUT, None)
            .await
        {
            Ok(captured) => {
                if let Some(reason) = version_reason(spec, captured.stdout_trimmed()) {
                    return ScorecardRun::unavailable(reason);
                }
            }
            Err(error) => return ScorecardRun::unavailable(reason_for(&error)),
        }
    }
    let target = format!("github.com/{slug}");
    let args: [&OsStr; 3] = [
        OsStr::new("--repo"),
        OsStr::new(&target),
        OsStr::new("--format=json"),
    ];
    let captured = match runner
        .run(
            tool_name,
            spec,
            &args,
            root,
            Duration::from_secs(SCORECARD_TIMEOUT_SECONDS),
            None,
        )
        .await
    {
        Ok(captured) => captured,
        Err(error) => return ScorecardRun::unavailable(reason_for(&error)),
    };
    parse(&captured, slug)
}

/// Reads Scorecard's report out of a finished run.
///
/// Split from the call so the parsing rules are readable on their own: a nonzero exit, a
/// document that is not the report, or a score outside 0–10 all mean "no numbers", never a
/// number invented to fill the gap.
fn parse(captured: &ToolOutput, slug: &str) -> ScorecardRun {
    if !captured.success {
        return ScorecardRun::unavailable(format!(
            "scorecard exited {}: {}",
            captured.code.unwrap_or(-1),
            captured.stderr.trim_end_matches(['\n', '\r'])
        ));
    }
    let wire: WireScorecard = match serde_json::from_str(&captured.stdout) {
        Ok(wire) => wire,
        Err(error) => {
            return ScorecardRun::unavailable(format!(
                "scorecard printed something that is not its JSON report: {error}"
            ));
        }
    };
    let mut scores: BTreeMap<String, i64> = BTreeMap::new();
    for (key, name) in IMPORTED_CHECKS {
        let found = wire
            .checks
            .iter()
            .find(|check| check.name.eq_ignore_ascii_case(name));
        match found.and_then(|check| check.score) {
            // The scores are 0–10 by the tool's own definition; a value outside that range
            // means the document is not what this build parses, and clipping it would invent
            // a number the tool did not report.
            Some(score) if (0..=10).contains(&score) => {
                scores.insert((*key).to_owned(), i64::from(score));
            }
            Some(score) => {
                return ScorecardRun::unavailable(format!(
                    "scorecard reported {score} for {name}, which is outside 0–10"
                ));
            }
            None => {}
        }
    }
    if scores.is_empty() {
        return ScorecardRun::unavailable(format!(
            "scorecard's report named none of the {} checks this build imports",
            IMPORTED_CHECKS.len()
        ));
    }
    // The blob reference is the saved stdout when there is one: the runner hashes the bytes
    // it stored, so the reference names exactly what was printed. A runner without a run
    // directory (a probe) leaves it to be hashed here.
    let stdout = captured.stdout.as_bytes();
    let blob = captured
        .entry
        .as_ref()
        .map_or_else(|| BlobRef::from_bytes(stdout), |entry| entry.stdout.clone());
    let replayed = captured.entry.as_ref().is_some_and(|entry| entry.replayed);
    ScorecardRun::Scores {
        scores,
        blob,
        command: scorecard_command(slug),
        replayed,
    }
}

/// The sentence a `not_measured` verdict carries for one tool error.
///
/// A missing binary uses the plan's own wording, `tool missing: scorecard`, so a reader
/// learns the tool is absent rather than that the repository failed. A version below
/// `tools.toml`'s floor names both versions: an old Scorecard reports different check names,
/// so its numbers would not be the ones this check imports.
fn reason_for(error: &ToolError) -> String {
    match error {
        ToolError::Missing { .. } => "tool missing: scorecard".to_owned(),
        ToolError::SnapshotMissing { .. } => {
            "snapshot missing: this run directory has no saved scorecard output".to_owned()
        }
        other => other.to_string(),
    }
}

/// Why a found Scorecard is still unusable, when it is.
///
/// # Errors
///
/// Returns the sentence for the verdict when the version printed is below the `min_version`
/// floor, or could not be read at all.
pub fn version_reason(spec: &ToolSpec, version: &str) -> Option<String> {
    if meets_floor(version, &spec.min_version) {
        return None;
    }
    Some(format!(
        "the scorecard on PATH reports \"{version}\", older than the {floor} this build \
         needs: an older scorecard names its checks differently, so its scores are not the \
         ones this check imports",
        floor = spec.min_version
    ))
}

/// Judges H21.
///
/// Pure: everything the tool said was gathered before judging, in
/// [`HistoryInput::gather`]. A run that did not happen is `not_measured` with its reason and
/// no scores, because a missing input must never read as a clean 10.
#[must_use]
pub fn scorecard(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    match &input.scorecard {
        ScorecardRun::Scores {
            scores,
            blob,
            command,
            replayed,
        } => {
            let mut judgment = Judgment::measured(scores.clone());
            judgment
                .evidence
                .push(Evidence::ToolOutput(substance_core::model::ToolOutput {
                    tool: SCORECARD.to_owned(),
                    command: command.clone(),
                    blob: blob.clone(),
                }));
            let mut note = format!(
                "imported from `{command}`; the token is read by scorecard from \
                 GITHUB_AUTH_TOKEN and never passed on the command line"
            );
            if *replayed {
                note.push_str("; replayed from --snapshot-from, not executed");
            }
            judgment.notes.push(note);
            judgment
        }
        ScorecardRun::Unavailable { reason } => {
            Judgment::not_measured(reason.clone(), BTreeMap::new())
        }
    }
}

/// Scorecard's JSON report, reduced to the members this build reads.
#[derive(Debug, Deserialize)]
struct WireScorecard {
    #[serde(default)]
    checks: Vec<WireCheck>,
}

#[derive(Debug, Deserialize)]
struct WireCheck {
    name: String,
    #[serde(default)]
    score: Option<i32>,
}
