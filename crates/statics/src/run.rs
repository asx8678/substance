//! Running every static check over one commit (M1-T08).
//!
//! This is the module the orchestrator calls: it does the IO once, hands each check the view it
//! needs, and returns the twelve reports in ID order. The split matters because the checks are
//! where the judgement lives and this is where the plumbing lives:
//!
//! - the rule pack is loaded once and the tree walked once ([`crate::scan`]);
//! - the tracked tree is listed once and its files read through one memoised cache
//!   ([`crate::tree`]);
//! - S01 and S11 need a [`ToolRunner`], so the caller passes the run's own, which is what makes
//!   `--snapshot-from` replay them like any other tool;
//! - S02, S03 and S09's forwarding half read the scan's matches, so they share one walk.
//!
//! A failure that stops the whole run is returned as [`StaticError`]; a failure that only stops
//! one check is that check's `not_measured` reason. The distinction is the plan's principle 8:
//! a missing tool must not read as a clean repository, and must not take the other checks down
//! with it either.

use std::collections::BTreeMap;
use std::path::Path;

use substance_core::check::CheckId;
use substance_core::model::{CheckVerdict, Finding, Verdict};
use substance_core::tools::ToolsFile;
use substance_history::git::{GitError, GitRepo};
use substance_runner::tool::ToolRunner;

use crate::checks::verdict::StaticReport;
use crate::checks::{
    agent_leftovers, committed_junk, config_contradictions, data_access_sprawl, docs_honesty,
    escape_hatches, evaluator_manipulation, fake_ci, llm_keys_in_client, locked_install,
    over_abstraction, scaffold_leftovers, secrets, skipped_and_weak_tests, slop_scan,
    startup_schema_traps, swallowed_writes,
};
use crate::rulepack::RulePack;
use crate::scan::{ScanReport, scan};
use crate::tree::TrackedTree;
use substance_core::scaffolds::ScaffoldPack;

/// Why the static pass could not run at all.
#[derive(Debug, thiserror::Error)]
pub enum StaticError {
    /// The tree or a blob could not be read, so no file-backed check has anything to look at.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The embedded rule pack is malformed, which is a build error.
    #[error("rule pack: {0}")]
    RulePack(#[from] crate::rulepack::RulePackError),
    /// The embedded `rules/scaffolds.toml` is malformed.
    #[error("scaffolds.toml: {0}")]
    Scaffolds(String),
}

/// Everything one static pass produced.
#[derive(Debug)]
pub struct StaticOutcome {
    /// The twelve verdicts, in ID order.
    pub verdicts: Vec<CheckVerdict>,
    /// Every finding, sorted by `(check, id)`.
    pub findings: Vec<Finding>,
    /// Notes per check, in the same order as `verdicts`.
    pub notes: Vec<(CheckId, Vec<String>)>,
    /// What the rule walk covered.
    pub scan: ScanCoverage,
}

/// The scan's coverage, kept so a report can say how much it looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanCoverage {
    /// Files whose language a rule reads.
    pub considered: u32,
    /// Files parsed.
    pub scanned: u32,
    /// Files skipped for size.
    pub skipped_large: u32,
    /// Authored lines across the tree.
    pub authored_lines: u64,
    /// How many rules the pack held.
    pub rules: usize,
}

/// Runs every static check over `rev` in `repo`.
///
/// `tools` is the run's tool table (for S01 and S11) and `runner` its [`ToolRunner`], so a
/// network or snapshot-aware invocation is replayed rather than executed where the run says so.
///
/// # Errors
///
/// Returns [`StaticError`] when the tree cannot be read or an embedded file is malformed — the
/// two failures that leave nothing to report. Every other failure belongs to one check.
pub async fn run_static_checks(
    repo: &GitRepo,
    rev: &str,
    run_id: &str,
    tools: &ToolsFile,
    runner: &ToolRunner<'_>,
) -> Result<StaticOutcome, StaticError> {
    let pack = RulePack::load()?;
    let scaffolds = ScaffoldPack::load().map_err(StaticError::Scaffolds)?;
    let root = repo.root();

    let scan_report = scan(repo, rev, &pack).await?;
    let mut tree = TrackedTree::load(repo, rev).await?;

    let mut reports: Vec<StaticReport> = Vec::new();
    reports.push(slop_scan(runner, tools, root).await);
    reports.push(escape_hatches(run_id, &scan_report));
    reports.push(swallowed_writes(run_id, &scan_report));
    reports.push(
        scaffold_leftovers(run_id, &mut tree, &scaffolds)
            .await
            .map_err(StaticError::Scaffolds)?,
    );
    reports.push(agent_leftovers(run_id, &tree));
    reports.push(committed_junk(run_id, &tree));
    reports.push(config_contradictions(run_id, &mut tree).await);
    reports.push(docs_honesty(run_id, &mut tree).await);
    reports.push(over_abstraction(run_id, &mut tree, &scan_report).await);
    reports.push(data_access_sprawl(run_id, &mut tree).await);
    reports.push(startup_schema_traps(run_id, &mut tree, &scan_report.matches).await);
    reports.push(locked_install(run_id, &mut tree).await);
    reports.push(fake_ci(run_id, &mut tree).await);
    reports.push(skipped_and_weak_tests(run_id, &mut tree, &scan_report.matches).await);
    reports.push(llm_keys_in_client(run_id, &mut tree).await);
    reports.push(match tools.spec("gitleaks") {
        Some(spec) => secrets(run_id, runner, spec, root).await,
        None => {
            let mut report = StaticReport::not_measured(CheckId::S11, "tool missing: gitleaks");
            report.verdict.parts.insert(
                "history".to_owned(),
                Verdict::NotMeasured {
                    reason: "tool missing: gitleaks".to_owned(),
                },
            );
            report.verdict.parts.insert(
                "tree".to_owned(),
                Verdict::NotMeasured {
                    reason: "tool missing: gitleaks".to_owned(),
                },
            );
            report.verdict.parts.insert(
                "bundle".to_owned(),
                Verdict::NotMeasured {
                    reason: "bundle scanning arrives in M4-T04".to_owned(),
                },
            );
            report
        }
    });
    reports.push(evaluator_manipulation(run_id, &mut tree).await);

    debug_assert_eq!(
        reports.len(),
        17,
        "twelve S checks, D04, O01, O10, T04 and L05"
    );
    Ok(assemble(reports, &scan_report, pack.len()))
}

/// Splits the reports into verdicts, findings and notes, in ID order.
fn assemble(reports: Vec<StaticReport>, scan_report: &ScanReport, rules: usize) -> StaticOutcome {
    let mut verdicts: Vec<CheckVerdict> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut notes: Vec<(CheckId, Vec<String>)> = Vec::new();
    for report in reports {
        notes.push((report.verdict.check_id, report.notes));
        findings.extend(report.findings);
        verdicts.push(report.verdict);
    }
    verdicts.sort_by_key(|verdict| verdict.check_id);
    findings.sort_by(|left, right| {
        left.check_id
            .cmp(&right.check_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    notes.sort_by_key(|(check, _)| *check);
    StaticOutcome {
        verdicts,
        findings,
        notes,
        scan: ScanCoverage {
            considered: scan_report.stats.considered,
            scanned: scan_report.stats.scanned,
            skipped_large: scan_report.stats.skipped_large,
            authored_lines: scan_report.stats.authored_lines,
            rules,
        },
    }
}

/// The rule pack's hash, for a caller that wants the manifest value without loading the pack.
#[must_use]
pub fn rule_pack_hash() -> String {
    crate::rulepack::hash_files(crate::rulepack::FILES)
}

/// The metrics map for a check with nothing to say, used by callers that need the shape.
#[must_use]
pub fn empty_metrics() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

/// Whether every check this pass runs reported a verdict.
///
/// The set is the twelve S checks plus the five M1-T09 added; a caller that wires a new one
/// without adding it here would otherwise believe its own pass covered it.
#[must_use]
pub fn covers_all_checks(outcome: &StaticOutcome) -> bool {
    let expected = [
        CheckId::D04,
        CheckId::L05,
        CheckId::O01,
        CheckId::O10,
        CheckId::S01,
        CheckId::S02,
        CheckId::S03,
        CheckId::S04,
        CheckId::S05,
        CheckId::S06,
        CheckId::S07,
        CheckId::S08,
        CheckId::S09,
        CheckId::S10,
        CheckId::S11,
        CheckId::S12,
        CheckId::T04,
    ];
    expected.iter().all(|check| {
        outcome
            .verdicts
            .iter()
            .any(|verdict| verdict.check_id == *check)
    })
}

/// A path relative to the repository root, for callers that log one.
#[must_use]
pub fn display(root: &Path) -> String {
    root.display().to_string()
}
