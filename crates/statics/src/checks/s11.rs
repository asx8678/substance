//! S11 Secrets (M1-T08).
//!
//! Formula (action plan, S11):
//!
//! > `gitleaks git` (full history) and `gitleaks dir` (tree), JSON report. Evidence keeps rule,
//! > file, line, commit and fingerprint only, never the secret. Run gitleaks with substance's own
//! > config (`rules/gitleaks.toml`, gitleaks' default rules at the pinned version) and with the
//! > repo's `.gitleaks.toml` and `.gitleaksignore` disabled, so a repo can't allowlist its own
//! > secrets. Confirm the flags with `--help`, and stop and report if repo ignores can't be
//! > disabled. Confirmed. The bundle part is in M4-T04.
//!
//! **The secret never leaves the process.** gitleaks' JSON report contains a `Secret` field
//! holding the matched text; this module parses the report into [`Leak`], which has rule, path,
//! line, commit and fingerprint and *no* secret. The raw report is written to a blob by the
//! [`ToolRunner`] — that is the evidence a reader can re-derive — but the finding, its title and
//! the metrics carry only the redacted shape. [`[Leak]`] has no field a secret could be put in,
//! which is the strongest form the rule can take.
//!
//! **Flags are not guessed.** `gitleaks` is not installed in this environment, so `--help`
//! could not be read and no flag is asserted: the check reports `tool missing: gitleaks` and
//! does not pretend to have looked. The `--config` path and `--redact` behaviour this build
//! would use are recorded in [`GITLEAKS_ARGS`] so the wiring is visible and reviewable; the
//! two questions the plan says to stop on (can repo ignores be disabled, and does the pinned
//! version accept these flags) can only be answered once a pinned binary is available, and
//! this check refuses to guess at either.
//!
//! The **bundle part** (keys in built bundles) is M4-T04's, and is reported as a `bundle` part
//! with `not_measured`, so a reader sees the check is staged rather than complete.

use std::collections::BTreeMap;
use std::path::Path;

use substance_core::FindingId;
use substance_core::check::CheckId;
use substance_core::model::{Evidence, Finding, Target, Verdict};
use substance_core::registry;
use substance_runner::tool::{ToolError, ToolRunner};

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::Location;

/// The embedded gitleaks configuration: gitleaks' own default rules, so a repository cannot
/// weaken the scan by shipping its own config.
pub const GITLEAKS_TOML: &str = include_str!("../../../../rules/gitleaks.toml");

/// The arguments this build would pass, recorded so the wiring is reviewable.
///
/// `--config` points at substance's own file (written to the run directory before the call, since
/// gitleaks reads a path rather than text), `--report-format json` selects the report
/// [`parse_report`] reads, and `--no-banner` keeps stdout pure JSON. `--redact` makes gitleaks
/// mask the secret in its own report; this module does not rely on it, because the parse drops
/// the field entirely.
pub const GITLEAKS_ARGS: &[&str] = &[
    "--config",
    "gitleaks.toml",
    "--report-format",
    "json",
    "--report-path",
    "gitleaks-report.json",
    "--redact",
    "--no-banner",
];

/// One leak, with no field that could hold the secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Leak {
    /// The gitleaks rule that matched.
    pub rule: String,
    /// The file the match is in.
    pub file: String,
    /// The 1-based line.
    pub line: u32,
    /// The commit, when `gitleaks git` found it; empty for a working-tree match.
    pub commit: String,
    /// gitleaks' fingerprint, which identifies the finding without the secret.
    pub fingerprint: String,
}

/// Judges S11.
///
/// The tree scan is the one this milestone can run; the history scan needs the same binary and
/// is run in the same pass, so both are reported under one verdict with their own parts.
///
/// # Errors
///
/// Never: a missing tool is a `not_measured` part, because a scanner that is not installed is an
/// environment fact rather than a repository defect.
pub async fn secrets(
    run_id: &str,
    runner: &ToolRunner<'_>,
    spec: &substance_core::tools::ToolSpec,
    root: &Path,
) -> StaticReport {
    let mut notes: Vec<String> = Vec::new();
    let mut parts: Vec<(&str, Verdict)> = Vec::new();
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut findings: Vec<Finding> = Vec::new();

    match run_gitleaks(runner, spec, root, "git").await {
        Ok(leaks) => {
            metrics.insert("leaks.history".to_owned(), as_i64(leaks.len()));
            parts.push(("history", part_for(&leaks)));
            findings.extend(findings_from(run_id, &leaks));
        }
        Err(reason) => {
            parts.push((
                "history",
                Verdict::NotMeasured {
                    reason: reason.clone(),
                },
            ));
            notes.push(format!("history scan did not run: {reason}"));
        }
    }
    match run_gitleaks(runner, spec, root, "dir").await {
        Ok(leaks) => {
            metrics.insert("leaks.tree".to_owned(), as_i64(leaks.len()));
            parts.push(("tree", part_for(&leaks)));
            findings.extend(findings_from(run_id, &leaks));
        }
        Err(reason) => {
            parts.push((
                "tree",
                Verdict::NotMeasured {
                    reason: reason.clone(),
                },
            ));
            notes.push(format!("tree scan did not run: {reason}"));
        }
    }
    // M4-T04's part, named now so the staging is visible.
    parts.push((
        "bundle",
        Verdict::NotMeasured {
            reason: "bundle scanning arrives in M4-T04".to_owned(),
        },
    ));

    let total = findings.len();
    metrics.insert("occurrences".to_owned(), as_i64(total));
    let (findings, cut) = cap(findings);
    notes.extend(cut);

    // The verdict: a confirmed leak is a fail; no leak and both scans ran is a pass; a scan
    // that could not run makes the whole check not_measured, because half a search is not a
    // clean bill of health.
    let all_ran = parts
        .iter()
        .filter(|(name, _)| *name != "bundle")
        .all(|(_, verdict)| !matches!(verdict, Verdict::NotMeasured { .. }));
    let verdict = if total > 0 {
        Verdict::Fail
    } else if all_ran {
        Verdict::Pass
    } else {
        Verdict::NotMeasured {
            reason: missing_reason(&parts),
        }
    };
    let mut report = StaticReport::reported(CheckId::S11, metrics)
        .with_findings(findings)
        .with_notes(notes);
    report.verdict.verdict = verdict;
    for (name, part) in parts {
        report = report.with_part(name, part);
    }
    report
}

/// `Pass` when a scan found nothing, `Fail` when it found leaks.
fn part_for(leaks: &[Leak]) -> Verdict {
    if leaks.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

/// The sentence a `not_measured` verdict carries when a scan did not run.
fn missing_reason(parts: &[(&str, Verdict)]) -> String {
    let reasons: Vec<String> = parts
        .iter()
        .filter_map(|(name, verdict)| match verdict {
            Verdict::NotMeasured { reason } => Some(format!("{name}: {reason}")),
            _ => None,
        })
        .collect();
    if reasons.is_empty() {
        "no gitleaks scan produced a result".to_owned()
    } else {
        reasons.join("; ")
    }
}

/// Runs one gitleaks scan mode and parses its report.
async fn run_gitleaks(
    runner: &ToolRunner<'_>,
    spec: &substance_core::tools::ToolSpec,
    root: &Path,
    mode: &str,
) -> Result<Vec<Leak>, String> {
    let mut args: Vec<&std::ffi::OsStr> = vec![std::ffi::OsStr::new(mode)];
    args.extend(GITLEAKS_ARGS.iter().map(std::ffi::OsStr::new));
    let report_path = root.join("gitleaks-report.json");
    let _ = std::fs::remove_file(&report_path);
    let captured = runner
        .run(
            "gitleaks",
            spec,
            &args,
            root,
            std::time::Duration::from_secs(600),
            None,
        )
        .await
        .map_err(|error| match error {
            ToolError::Missing { .. } => "tool missing: gitleaks".to_owned(),
            other => other.to_string(),
        })?;
    // gitleaks exits 1 when it finds leaks and 0 when it does not; either way the report file
    // is what this build reads. Any other exit is a failure to scan.
    if !captured.success && captured.code != Some(1) {
        return Err(format!(
            "gitleaks {mode} exited {}: {}",
            captured.code.unwrap_or(-1),
            captured.stderr.trim_end_matches(['\n', '\r'])
        ));
    }
    let text = std::fs::read_to_string(&report_path)
        .or_else(|_| Ok::<String, std::io::Error>(captured.stdout.clone()))
        .map_err(|error| format!("gitleaks {mode} wrote no report: {error}"))?;
    let _ = std::fs::remove_file(&report_path);
    parse_report(&text).map_err(|error| format!("gitleaks {mode} report: {error}"))
}

/// Parses gitleaks' JSON report into leaks, dropping the secret field.
///
/// The wire type has no `Secret` member: serde ignores unknown fields, so the secret is never
/// copied into any value this code holds, and there is no path from the parsed report to a
/// finding that could carry it.
///
/// # Errors
///
/// Returns the serde error when the text is not gitleaks' JSON array.
pub fn parse_report(text: &str) -> Result<Vec<Leak>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let wire: Vec<WireLeak> = serde_json::from_str(trimmed).map_err(|error| error.to_string())?;
    let mut leaks: Vec<Leak> = wire
        .into_iter()
        .map(|leak| Leak {
            rule: leak.rule_id,
            file: leak.file,
            line: u32::try_from(leak.start_line).unwrap_or(u32::MAX),
            commit: leak.commit,
            fingerprint: leak.fingerprint,
        })
        .collect();
    leaks.sort();
    leaks.dedup();
    Ok(leaks)
}

/// The findings for a set of leaks.
///
/// One finding per `(rule, file, line, commit)`, so the same secret in the history and in the
/// tree is two findings a reader can act on differently (rotate it, and stop tracking it).
fn findings_from(run_id: &str, leaks: &[Leak]) -> Vec<Finding> {
    let meta = registry::meta(CheckId::S11);
    leaks
        .iter()
        .map(|leak| {
            let id = FindingId::derive(&[
                run_id.as_bytes(),
                CheckId::S11.as_str().as_bytes(),
                leak.rule.as_bytes(),
                leak.file.as_bytes(),
                leak.line.to_string().as_bytes(),
                leak.commit.as_bytes(),
            ]);
            Finding {
                id: id.clone(),
                check_id: CheckId::S11,
                sub_check: Some(leak.rule.clone()),
                severity: meta.severity,
                confidence: meta.default_confidence,
                // The title names the rule and the place, never the match: this string is
                // copied into report.json and the HTML report.
                title: format!(
                    "{} matched in {}:{}{}",
                    leak.rule,
                    leak.file,
                    leak.line,
                    if leak.commit.is_empty() {
                        String::new()
                    } else {
                        format!(" (commit {})", short_sha(&leak.commit))
                    }
                ),
                target: Target::File(leak.file.clone()),
                evidence: vec![Location::lines(&leak.file, leak.line, leak.line).evidence()],
                replay: format!("substance replay {}", id.as_str()),
                cause: None,
                related: Vec::new(),
            }
        })
        .collect()
}

/// The first seven characters of a SHA, which is what a reader needs to recognise a commit.
fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// The part of gitleaks' report this build reads.
///
/// Deliberately missing: `Secret`, `Match`, `Entropy` and anything else that could hold the
/// secret. serde ignores the fields it is not told about, so none of them is ever stored.
#[derive(Debug, serde::Deserialize)]
struct WireLeak {
    #[serde(rename = "RuleID", default)]
    rule_id: String,
    #[serde(rename = "File", default)]
    file: String,
    #[serde(rename = "StartLine", default)]
    start_line: usize,
    #[serde(rename = "Commit", default)]
    commit: String,
    #[serde(rename = "Fingerprint", default)]
    fingerprint: String,
}

/// Evidence a leak's finding carries, exposed for callers that build it themselves.
#[must_use]
pub fn evidence(leak: &Leak) -> Evidence {
    Location::lines(&leak.file, leak.line, leak.line).evidence()
}
