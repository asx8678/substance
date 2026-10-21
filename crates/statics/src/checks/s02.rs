//! S02 Escape hatches (M1-T08).
//!
//! Formula (action plan, S02):
//!
//! > `rules/{ts,rust,python}/escape-hatches.yml` with the catalog's patterns. Rust
//! > `unwrap()`/`expect()` count only inside functions that take Axum or Actix extractor
//! > parameters. Metric: count per 1,000 authored lines.
//!
//! The counting is the rule engine's: each match the pack produces for an S02 rule belongs
//! here, and the sub-check its metadata names is the pattern's name (`ts-any`,
//! `rust-unwrap-handler`). Two numbers come out of it: the total, and the total per thousand
//! authored lines, which is the metric the plan asks for and the only one that lets a 40-line
//! script and a 40,000-line app be compared.
//!
//! The denominator is the authored line count [`ScanReport`] measured while scanning — the
//! same authored rule the history signals use, so a lockfile's or a vendored tree's lines are
//! not in either side of the ratio.
//!
//! No threshold exists for this metric yet (the plan gives none), so the verdict is
//! `Reported` (ADR-0013).

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::verdict::{StaticReport, cap};
use crate::finding::to_finding;
use crate::scan::{RuleMatch, ScanReport};

/// The number of authored lines the rate is per.
pub const PER_LINES: i64 = 1_000;

/// Judges S02 from the scan's matches.
#[must_use]
pub fn escape_hatches(run_id: &str, scan: &ScanReport) -> StaticReport {
    let matches = matching(scan, CheckId::S02);
    let metrics = metrics(&matches, scan.stats.authored_lines);
    let mut notes = Vec::new();
    if scan.stats.authored_lines == 0 {
        notes.push(
            "the commit has no authored lines, so the per-1,000-lines rate is not reported"
                .to_owned(),
        );
    }
    let findings = findings(run_id, &matches);
    let (findings, cut) = cap(findings);
    notes.extend(cut);
    StaticReport::reported(CheckId::S02, metrics)
        .with_findings(findings)
        .with_notes(notes)
}

/// The matches belonging to `check`, in scan order.
#[must_use]
pub fn matching(scan: &ScanReport, check: CheckId) -> Vec<&RuleMatch> {
    scan.matches
        .iter()
        .filter(|found| found.check_id == check)
        .collect()
}

/// The metrics: the total, one count per sub-check, and the rate.
#[must_use]
pub fn metrics(matches: &[&RuleMatch], authored_lines: u64) -> BTreeMap<String, i64> {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("occurrences".to_owned(), as_i64(matches.len()));
    for found in matches {
        let key = format!("count.{}", found.sub_check);
        let entry = metrics.entry(key).or_insert(0);
        *entry += 1;
    }
    if authored_lines > 0 {
        metrics.insert(
            "per_1000_lines".to_owned(),
            rate(matches.len(), authored_lines),
        );
        metrics.insert("authored_lines".to_owned(), as_i64_u64(authored_lines));
    }
    metrics
}

/// `count * 1000 / lines`, floored: an integer rate, never a float.
#[must_use]
pub fn rate(count: usize, authored_lines: u64) -> i64 {
    if authored_lines == 0 {
        return 0;
    }
    let scaled = as_i64(count).saturating_mul(PER_LINES);
    scaled / as_i64_u64(authored_lines)
}

/// The findings one check's matches produce.
#[must_use]
pub fn findings(run_id: &str, matches: &[&RuleMatch]) -> Vec<Finding> {
    matches
        .iter()
        .map(|found| to_finding(run_id, found))
        .collect()
}

/// A `usize` count as a metric value.
///
/// Every metric is an `i64` in the model and must stay within ±2^53; a match count above
/// that is not a repository, so saturation is the honest answer rather than a panic.
#[must_use]
pub fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// A `u64` count as a metric value, saturating for the same reason.
#[must_use]
pub fn as_i64_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
