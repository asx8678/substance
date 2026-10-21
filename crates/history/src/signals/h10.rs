//! H10 Message vs. diff: generic subjects on large diffs.
//!
//! Formula (action plan, H10):
//!
//! > Share of commits whose whole subject, case-insensitive, is in
//! > `rules/history/generic-messages.txt` (update, fix, wip, changes, minor, cleanup, stuff,
//! > commit, `.`); also the count of those on diffs over 300 authored lines.
//!
//! A message says what the author thought they were doing. "update" says nothing, and a
//! 2,000-line "update" is the clearest sign that nobody described — or read — the change. The
//! match is the *whole* subject, so `Update dependencies to fix CVE-2024-1` is not generic and
//! `update` is; `.` and `wip` are included because generators emit them.
//!
//! The share is measured over all non-bot, non-merge commits; the second number counts the
//! generic messages that sat on a diff of more than 300 authored lines, which is the pairing
//! the catalog cares about.
//!
//! The threshold is `history.generic_message_permille`.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// How many authored added lines make a diff "large" for the pairing metric.
pub const LARGE_DIFF_LINES: u64 = 300;

/// Judges H10.
#[must_use]
pub fn message_vs_diff(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let commits = input.human_commits();
    let mut generic = 0u64;
    let mut generic_large = 0u64;
    let mut generic_large_lines = 0u64;
    let mut contributions: Vec<(String, u64)> = Vec::new();

    for commit in &commits {
        let subject = commit.subject.trim().to_lowercase();
        if !input.generic_subjects.iter().any(|known| known == &subject) {
            continue;
        }
        generic += 1;
        let added = input.authored_added(commit);
        contributions.push((commit.sha.clone(), added.max(1)));
        if added > LARGE_DIFF_LINES {
            generic_large += 1;
            generic_large_lines += added;
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "commits".to_owned(),
        i64::try_from(commits.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "generic".to_owned(),
        i64::try_from(generic).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "generic_large".to_owned(),
        i64::try_from(generic_large).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "generic_large_lines".to_owned(),
        i64::try_from(generic_large_lines).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "generic_subjects".to_owned(),
        i64::try_from(input.generic_subjects.len()).unwrap_or(i64::MAX),
    );
    let total = u64::try_from(commits.len()).unwrap_or(u64::MAX);
    let permille = super::shape::permille(generic, total);
    if let Some(permille) = permille {
        metrics.insert(
            "generic_permille".to_owned(),
            i64::try_from(permille).unwrap_or(1000),
        );
    }

    let judgment = Judgment::measured(metrics);
    if input.generic_subjects.is_empty() {
        return Judgment::not_measured(
            "rules/history/generic-messages.txt is empty, so no subject is generic",
            judgment.metrics,
        );
    }
    if commits.is_empty() {
        return Judgment::not_measured("the repository has no non-bot commits", judgment.metrics);
    }
    let threshold = thresholds.history().generic_message_permille;
    let exceeded = match (threshold, permille) {
        (Some(threshold), Some(permille)) => permille >= u64::from(threshold),
        _ => false,
    };
    judgment
        .with_evidence(super::top_commits(contributions))
        .with_note(format!(
            "a subject is generic when its whole text, lowercased, is one of the {} listed \
             messages; a large diff is over {LARGE_DIFF_LINES} authored added lines",
            input.generic_subjects.len()
        ))
        .judge(threshold, exceeded)
}
