//! S05 Agent leftovers (M1-T08).
//!
//! Formula (action plan, S05):
//!
//! > The catalog's filename patterns: report-style markdown in the root, one-off scripts,
//! > duplicate versions.
//!
//! Three shapes, each a name rather than a parse:
//!
//! - **Report-style markdown in the root**: `IMPLEMENTATION_SUMMARY.md`, `FIXES_APPLIED.md`,
//!   `*_COMPLETE.md`. These are notes written for the person who did the work, left where a
//!   reader will find them and mistake them for documentation.
//! - **One-off scripts**: `test_fix.py`, `debug_*.js`, `verify_*.sh`. A script written to check
//!   one thing once is not a tool; it is debris from a debugging session.
//! - **Duplicate versions**: `App_old.tsx`, `main_v2.py`, `utils_fixed.rs`, `*.backup`. Two
//!   copies of one file mean nobody decided which is real.
//!
//! A directory named `node_modules` or `vendor` is skipped: a dependency's own test script is
//! not the project's leftover. Everything else in the tree is fair game, root markdown being
//! the only pattern restricted to the root (the catalog says "in the root", and a nested
//! `docs/ADDITIONS_COMPLETE.md` is a documentation page, not a leftover).

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// Directories whose contents are never the project's own leftovers.
const SKIPPED_DIRS: &[&str] = &["node_modules", "vendor", "third_party"];

/// Report-style markdown names that end with a phrase rather than a word boundary.
const REPORT_SUFFIXES: &[&str] = &["_COMPLETE.md", "_SUMMARY.md", "_REPORT.md"];

/// One-off script prefixes.
const ONEOFF_PREFIXES: &[&str] = &["debug_", "verify_", "test_fix"];

/// Duplicate-version markers in a file name.
const DUPLICATE_MARKERS: &[&str] = &["_old.", "_v2.", "_fixed.", ".backup"];

/// Judges S05.
#[must_use]
pub fn agent_leftovers(run_id: &str, tree: &TrackedTree<'_>) -> StaticReport {
    let mut by_kind: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in tree.paths() {
        if path
            .split('/')
            .any(|segment| SKIPPED_DIRS.contains(&segment))
        {
            continue;
        }
        if let Some(kind) = classify(path, tree.root_files().contains(&path)) {
            by_kind.entry(kind).or_default().push(path);
        }
    }
    let mut findings: Vec<Finding> = Vec::new();
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut total = 0_usize;
    for (kind, paths) in &by_kind {
        total += paths.len();
        metrics.insert(format!("count.{kind}"), as_i64(paths.len()));
        for path in paths {
            findings.push(file_finding::at(
                run_id,
                CheckId::S05,
                kind,
                &format!("{kind} file: {path}"),
                Location::file(path),
            ));
        }
    }
    metrics.insert("occurrences".to_owned(), as_i64(total));
    let (findings, cut) = cap(findings);
    StaticReport::reported(CheckId::S05, metrics)
        .with_findings(findings)
        .with_notes(cut.into_iter().collect())
}

/// Which pattern `path` matches, if any.
///
/// The order is the catalog's: report markdown, then one-off scripts, then duplicates, so a
/// file that is somehow both is reported once under the first rule that names it.
#[must_use]
pub fn classify(path: &str, at_root: bool) -> Option<&'static str> {
    let name = TrackedTree::file_name(path);
    let lower = name.to_ascii_lowercase();
    if at_root && is_report_markdown(&lower) {
        return Some("report-markdown");
    }
    let one_off_place = !path.contains('/') || is_script_dir(path);
    if one_off_place
        && ONEOFF_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(&prefix.to_ascii_lowercase()))
    {
        return Some("one-off-script");
    }
    if DUPLICATE_MARKERS
        .iter()
        .any(|marker| lower.contains(&marker.to_ascii_lowercase()))
    {
        return Some("duplicate-version");
    }
    None
}

/// Report-style markdown: the named files, or any name ending in a summary suffix.
fn is_report_markdown(lower: &str) -> bool {
    matches!(
        lower,
        "implementation_summary.md" | "fixes_applied.md" | "changes_made.md" | "work_summary.md"
    ) || REPORT_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(&suffix.to_ascii_lowercase()))
}

/// Whether the path sits in a directory where a one-off script is still the project's own.
fn is_script_dir(path: &str) -> bool {
    let mut segments: Vec<&str> = path.split('/').collect();
    segments.pop();
    !segments.is_empty()
        && segments
            .iter()
            .all(|segment| matches!(*segment, "scripts" | "tools" | "bin"))
}
