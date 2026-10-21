//! S06 Committed junk (M1-T08).
//!
//! Formula (action plan, S06):
//!
//! > Tracked paths under `node_modules/`, `target/`, `__pycache__/` or `dist/`, plus `.env`
//! > (not `.env.example`) and `.DS_Store`.
//!
//! These are the files `.gitignore` exists to keep out. When they are tracked, the repository
//! carries build output nobody wrote, can carry secrets (`.env`), and tells a reader the
//! project was committed with `git add -A` rather than with attention.
//!
//! `.env.example` is deliberately not junk: it is the template that documents which variables
//! a project needs, and S08 relies on it for the same reason.
//!
//! Every path under one of the directories is counted, not just the first: 400 tracked files
//! under `node_modules/` is one mistake, and the metric should say how big it was.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// Directory segments whose contents are always junk when tracked.
const JUNK_DIRS: &[&str] = &["node_modules", "target", "__pycache__", "dist"];

/// File names that are junk wherever they appear.
const JUNK_NAMES: &[&str] = &[".env", ".DS_Store"];

/// Judges S06.
#[must_use]
pub fn committed_junk(run_id: &str, tree: &TrackedTree<'_>) -> StaticReport {
    let mut by_kind: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in tree.paths() {
        if let Some(kind) = classify(path) {
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
                CheckId::S06,
                kind,
                &format!("tracked {kind}: {path}"),
                Location::file(path),
            ));
        }
    }
    metrics.insert("occurrences".to_owned(), as_i64(total));
    let (findings, cut) = cap(findings);
    let mut report = StaticReport::reported(CheckId::S06, metrics)
        .with_findings(findings)
        .with_notes(cut.into_iter().collect());
    if by_kind.contains_key("env") {
        // A tracked `.env` is the one junk file that may hold a secret. The note says so
        // without pretending S06 read it: S11 is the check that looks inside files.
        report = report.with_note(
            "a tracked `.env` can hold live credentials; rotating them is a decision for the \
             project, not a finding substance can make",
        );
    }
    report
}

/// Which junk pattern `path` matches, if any.
#[must_use]
pub fn classify(path: &str) -> Option<&'static str> {
    let name = TrackedTree::file_name(path);
    if JUNK_NAMES.contains(&name) {
        return Some(if name == ".env" { "env" } else { "ds-store" });
    }
    path.split('/')
        .find_map(|segment| JUNK_DIRS.iter().find(|junk| **junk == segment).copied())
}
