//! H04 Commit size: how big one commit is, and how much of today's code it left.
//!
//! Formula (action plan, H04):
//!
//! > Median and p90 (nearest rank) of authored added lines per commit; permille of HEAD lines
//! > from commits over 1,000 authored lines; a separate metric for giant commits after day
//! > one.
//!
//! Two questions share a name. "How big are this project's commits" is answered by the median
//! and p90 of authored added lines, and "was the code written at once" by the share of today's
//! blamed lines that come from giant commits. The second is the one with a threshold: the
//! thresholds file sets `giant_lines = 1000` and `giant_share_permille = 500`.
//!
//! The giant share is measured over *blamed* lines, so a giant commit whose work was later
//! deleted counts for nothing. That is deliberate: the question is how much of the code a
//! reader is looking at arrived in one commit, not how much work the commit represented.
//!
//! The separate `giant_after_day_one` metric is H22's: a giant commit on day one is a dump,
//! and so is a giant commit six months later, but only the second describes a working project.
//! Day one comes from [`HistoryInput::day_one`], with the private-then-public rule applied.
//!
//! Squash merges change what a commit means: under the squash-merge rule (M1-T06) the numbers
//! come from PR sizes instead. This check measures commits, and the note says which it used.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;
use crate::signals::days_between;

/// The catalog's giant-commit size, in authored added lines.
///
/// The thresholds file sets `history.giant_lines`; this constant is the fallback the metric
/// keeps when no key is present, so the number a reader compares is the one the plan means.
pub const DEFAULT_GIANT_LINES: u32 = 1000;

/// Judges H04.
#[must_use]
pub fn commit_size(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let history = thresholds.history();
    let giant_lines = history.giant_lines.unwrap_or(DEFAULT_GIANT_LINES);
    let giant_lines_u64 = u64::from(giant_lines);

    let mut per_commit: Vec<(u64, String)> = input
        .human_commits()
        .iter()
        .map(|commit| (input.authored_added(commit), commit.sha.clone()))
        .collect();
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "commits".to_owned(),
        i64::try_from(per_commit.len()).unwrap_or(i64::MAX),
    );

    // Nearest-rank median and p90 over the commit sizes; ties break on the SHA first so the
    // ordered list is deterministic before a rank is picked from it.
    per_commit.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let sizes: Vec<u64> = per_commit.iter().map(|(lines, _)| *lines).collect();
    if let Some(median) = nearest_rank(&sizes, 50) {
        metrics.insert(
            "median_added_lines".to_owned(),
            i64::try_from(median).unwrap_or(i64::MAX),
        );
    }
    if let Some(p90) = nearest_rank(&sizes, 90) {
        metrics.insert(
            "p90_added_lines".to_owned(),
            i64::try_from(p90).unwrap_or(i64::MAX),
        );
    }
    let giant_commits = sizes
        .iter()
        .filter(|lines| **lines > giant_lines_u64)
        .count();
    metrics.insert(
        "giant_commits".to_owned(),
        i64::try_from(giant_commits).unwrap_or(i64::MAX),
    );

    // Today's code, per commit that wrote it.
    let day_one = input.day_one();
    let total_blamed = input.blame.coverage.lines_blamed;
    let mut blamed_per_commit: BTreeMap<String, u64> = BTreeMap::new();
    for (_, line) in input.blame.lines() {
        *blamed_per_commit.entry(line.commit.clone()).or_default() += 1;
    }
    let size_of: BTreeMap<&str, u64> = per_commit
        .iter()
        .map(|(lines, sha)| (sha.as_str(), *lines))
        .collect();
    let mut giant_lines_today = 0u64;
    let mut giant_after_day_one = 0u64;
    for (sha, blamed) in &blamed_per_commit {
        let Some(size) = size_of.get(sha.as_str()) else {
            // A blamed commit outside the human-commit list (a merge, a bot) is not part of
            // the size distribution; its lines still count as code a reader sees.
            continue;
        };
        if *size <= giant_lines_u64 {
            continue;
        }
        giant_lines_today += blamed;
        if let Some(commit) = input.commits.iter().find(|candidate| &candidate.sha == sha)
            && days_between(day_one.at, commit.author.at) > 0
        {
            giant_after_day_one += *size;
        }
    }
    if let Some(share) = super::shape::permille(giant_lines_today, total_blamed) {
        metrics.insert(
            "giant_share_permille".to_owned(),
            i64::try_from(share).unwrap_or(1000),
        );
        metrics.insert(
            "lines_from_giant_commits".to_owned(),
            i64::try_from(giant_lines_today).unwrap_or(i64::MAX),
        );
    }
    metrics.insert(
        "giant_after_day_one".to_owned(),
        i64::try_from(giant_after_day_one).unwrap_or(i64::MAX),
    );

    let mut contributions: Vec<(String, u64)> = per_commit
        .iter()
        .map(|(lines, sha)| (sha.clone(), *lines))
        .collect();
    contributions.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let mut judgment = Judgment::measured(metrics);
    if day_one.private_then_public {
        judgment = judgment.with_note(
            "giant commits are counted from the public creation date: the repository was \
             developed privately first"
                .to_owned(),
        );
    }
    if input.blame.coverage.truncated {
        judgment = judgment.with_note(format!(
            "blame covered {} of {} authored files, so the giant share rests on part of the tree",
            input.blame.coverage.files_blamed, input.blame.coverage.files_total
        ));
    }
    if per_commit.is_empty() {
        return Judgment::not_measured("the repository has no non-bot commits", judgment.metrics);
    }

    let share = judgment
        .metrics
        .get("giant_share_permille")
        .copied()
        .map(|value| u32::try_from(value).unwrap_or(u32::MAX));
    let threshold = history.giant_share_permille;
    let exceeded = match (share, threshold) {
        (Some(share), Some(threshold)) => share >= threshold,
        _ => false,
    };
    judgment
        .with_evidence(super::top_commits(contributions))
        .judge(threshold, exceeded)
}

/// The value at `percent` by nearest rank in a sorted list.
///
/// Nearest rank is the definition the plan uses, and it needs no interpolation and no float:
/// the rank is `ceil(percent / 100 * n)`, clamped to at least 1 and at most `n`.
#[must_use]
pub fn nearest_rank(sorted: &[u64], percent: u32) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let total = u64::try_from(sorted.len()).unwrap_or(u64::MAX);
    let numerator = total.saturating_mul(u64::from(percent));
    // ceil(n * p / 100), in integers only.
    let rank = numerator.div_ceil(100).max(1);
    let index = usize::try_from(rank.min(total)).unwrap_or(sorted.len()) - 1;
    sorted.get(index).copied()
}
