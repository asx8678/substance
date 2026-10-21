//! H08 Refactoring signals: moved lines, copy-paste, and the age of what is changed.
//!
//! Formula (action plan, H08):
//!
//! > From hunks: moved-line permille (a removed line of 20+ trimmed chars re-added elsewhere
//! > in the same commit); copy-paste permille (an added line repeated 2+ times in one
//! > commit); median age in days of lines deleted in the last 200 commits.
//!
//! This is GitClear's set, and it describes *how* the code was written. A person who
//! refactors moves lines around and edits code that has been there for months; a generator
//! pastes the same block into six files and deletes work it wrote an hour ago.
//!
//! Three numbers, one shape: each is a share of the movement in the sampled commits, so a
//! quiet month does not read the same as a busy one. The median age of deleted lines is the
//! one that needs blame: for each deleted line, when was it written? The sample is bounded by
//! [`MAX_AGE_BLAMES`](super::MAX_AGE_BLAMES) and the note says when that bound was hit.
//!
//! One refinement the formula leaves implicit. A repeated line only counts as a paste when it
//! carries at least [`MOVE_MIN_CHARS`] characters, the same floor the moved-line rule uses:
//! `}` and `return;` repeat hundreds of times in any brace-delimited file, and counting them
//! made the metric report that 44% of this repository's code was pasted. A pasted line is a
//! line with content in it.
//!
//! The thresholds file has no key for H08; the numbers are context for a reader.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::thresholds::Thresholds;

use super::h04::nearest_rank;
use super::shape::Judgment;
use super::{HUNK_COMMITS, HistoryInput};
use crate::git::HunkLineKind;

/// How long a removed line must be to count as a move, in trimmed characters.
pub const MOVE_MIN_CHARS: usize = 20;

/// Judges H08.
#[must_use]
pub fn refactoring(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let mut added_total = 0u64;
    let mut removed_total = 0u64;
    let mut moved = 0u64;
    let mut repeated = 0u64;
    let mut contributions: Vec<(String, u64)> = Vec::new();
    let mut commits_read = 0u64;

    for commit in &input.hunks.commits {
        // Only authored files: a lockfile's churn is not refactoring.
        let files: Vec<&crate::git::FileHunks> = commit
            .files
            .iter()
            .filter(|file| !file.binary && super::path_is_authored(&file.path))
            .collect();
        if files.is_empty() {
            continue;
        }
        commits_read += 1;
        let mut added_here: Vec<String> = Vec::new();
        let mut removed_here: Vec<String> = Vec::new();
        for file in &files {
            for hunk in &file.hunks {
                for line in &hunk.lines {
                    match line.kind {
                        HunkLineKind::Added => {
                            added_total += 1;
                            added_here.push(line.text.trim().to_owned());
                        }
                        HunkLineKind::Removed => {
                            removed_total += 1;
                            removed_here.push(line.text.trim().to_owned());
                        }
                    }
                }
            }
        }
        // A removed line re-added in the same commit is a move, whatever file it landed in.
        let added_set: BTreeSet<&String> = added_here.iter().collect();
        let mut moved_here = 0u64;
        for text in &removed_here {
            if text.chars().count() >= MOVE_MIN_CHARS && added_set.contains(text) {
                moved_here += 1;
            }
        }
        moved += moved_here;
        // An added line that appears twice or more in one commit was pasted, not typed: the
        // first occurrence is the original and the rest are copies. Lines shorter than the
        // move floor are syntax, not content, and are not counted (see the module docs).
        let mut counts: BTreeMap<&String, u64> = BTreeMap::new();
        for text in &added_here {
            if text.chars().count() < MOVE_MIN_CHARS {
                continue;
            }
            *counts.entry(text).or_default() += 1;
        }
        let mut repeated_here = 0u64;
        for count in counts.values() {
            if *count >= 2 {
                repeated_here += count - 1;
            }
        }
        repeated += repeated_here;
        if moved_here + repeated_here > 0 {
            contributions.push((commit.sha.clone(), moved_here + repeated_here));
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "commits_read".to_owned(),
        i64::try_from(commits_read).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "lines_added".to_owned(),
        i64::try_from(added_total).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "lines_removed".to_owned(),
        i64::try_from(removed_total).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "lines_moved".to_owned(),
        i64::try_from(moved).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "lines_pasted".to_owned(),
        i64::try_from(repeated).unwrap_or(i64::MAX),
    );
    if let Some(share) = super::shape::permille(moved, removed_total) {
        metrics.insert(
            "moved_permille".to_owned(),
            i64::try_from(share).unwrap_or(1000),
        );
    }
    if let Some(share) = super::shape::permille(repeated, added_total) {
        metrics.insert(
            "copy_paste_permille".to_owned(),
            i64::try_from(share).unwrap_or(1000),
        );
    }
    let mut ages: Vec<u64> = input
        .hunks
        .deleted_line_ages
        .iter()
        .map(|age| u64::from(*age))
        .collect();
    ages.sort_unstable();
    if let Some(median) = nearest_rank(&ages, 50) {
        metrics.insert(
            "median_deleted_line_age_days".to_owned(),
            i64::try_from(median).unwrap_or(i64::MAX),
        );
    }
    metrics.insert(
        "deleted_lines_dated".to_owned(),
        i64::try_from(ages.len()).unwrap_or(i64::MAX),
    );

    let mut judgment = Judgment::measured(metrics);
    if input.hunks.truncated {
        judgment = judgment.with_note(format!(
            "the sample is the most recent {HUNK_COMMITS} commits; older history is not counted"
        ));
    }
    if input.hunks.ages_truncated {
        judgment = judgment.with_note(
            "dating deleted lines stopped at its blame budget, so the median age rests on a \
             sample"
                .to_owned(),
        );
    }
    if commits_read == 0 {
        return Judgment::not_measured(
            "the sampled commits touched no authored file",
            judgment.metrics,
        );
    }
    judgment
        .with_evidence(super::top_commits(contributions))
        .with_note(format!(
            "moves are removed lines of {MOVE_MIN_CHARS}+ characters re-added in the same commit; \
             pastes are added lines of {MOVE_MIN_CHARS}+ characters repeated within one commit"
        ))
}
