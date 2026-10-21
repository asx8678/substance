//! H09 Short-lived code: lines added and then deleted again within seven days.
//!
//! Formula (action plan, H09):
//!
//! > Permille of lines added in the last 365 days that a later commit deletes, identically and
//! > in the same file, within 7 days.
//!
//! Code that survives a week was thought about; code written and deleted the same afternoon was
//! scaffolding an agent left behind, or a wrong turn it took and undid. The share is measured
//! over the last year so a project's early churn does not dominate it, and the identity test is
//! exact — a line that comes back rewritten is not the same line, and counting it would make a
//! refactor look like churn.
//!
//! The threshold is `history.short_lived_permille`.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::shape::Judgment;
use super::{HistoryInput, LONGEST_WINDOW_DAYS};
use crate::git::HunkLineKind;
use crate::signals::days_between;

/// How many days a line may survive and still count as short-lived.
pub const SHORT_LIVED_DAYS: i64 = 7;

/// Judges H09.
#[must_use]
pub fn short_lived(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let window_start = input.window_start(LONGEST_WINDOW_DAYS);
    // The commits are newest first; walking them newest first lets one pass record every
    // (file, line) that was deleted and when, and then match the additions against it.
    let mut deleted: BTreeMap<(String, String), Vec<substance_core::Timestamp>> = BTreeMap::new();
    for commit in &input.hunks.commits {
        for file in &commit.files {
            if file.binary || !super::path_is_authored(&file.path) {
                continue;
            }
            for hunk in &file.hunks {
                for line in &hunk.lines {
                    if line.kind != HunkLineKind::Removed {
                        continue;
                    }
                    let text = line.text.trim().to_owned();
                    if text.is_empty() {
                        continue;
                    }
                    deleted
                        .entry((file.path.clone(), text))
                        .or_default()
                        .push(commit.at);
                }
            }
        }
    }

    let mut added = 0u64;
    let mut short = 0u64;
    let mut contributions: Vec<(String, u64)> = Vec::new();
    for commit in &input.hunks.commits {
        if commit.at < window_start {
            continue;
        }
        let mut short_here = 0u64;
        for file in &commit.files {
            if file.binary || !super::path_is_authored(&file.path) {
                continue;
            }
            for hunk in &file.hunks {
                for line in &hunk.lines {
                    if line.kind != HunkLineKind::Added {
                        continue;
                    }
                    let text = line.text.trim().to_owned();
                    if text.is_empty() {
                        continue;
                    }
                    added += 1;
                    // A later commit that deleted this exact line in this exact file, within
                    // the window: the line did not survive its first week.
                    let key = (file.path.clone(), text);
                    let gone_soon = deleted.get(&key).is_some_and(|instants| {
                        instants.iter().any(|deleted_at| {
                            *deleted_at > commit.at
                                && days_between(commit.at, *deleted_at) <= SHORT_LIVED_DAYS
                        })
                    });
                    if gone_soon {
                        short += 1;
                        short_here += 1;
                    }
                }
            }
        }
        if short_here > 0 {
            contributions.push((commit.sha.clone(), short_here));
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "lines_added".to_owned(),
        i64::try_from(added).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "lines_short_lived".to_owned(),
        i64::try_from(short).unwrap_or(i64::MAX),
    );
    let permille = super::shape::permille(short, added);
    if let Some(permille) = permille {
        metrics.insert(
            "short_lived_permille".to_owned(),
            i64::try_from(permille).unwrap_or(1000),
        );
    }

    let mut judgment = Judgment::measured(metrics);
    if input.hunks.truncated {
        judgment = judgment.with_note(
            "only the most recent commits were read, so older additions are not counted".to_owned(),
        );
    }
    if added == 0 {
        return Judgment::not_measured(
            "no authored lines were added in the sampled window",
            judgment.metrics,
        );
    }
    let threshold = thresholds.history().short_lived_permille;
    let exceeded = match (threshold, permille) {
        (Some(threshold), Some(permille)) => permille >= u64::from(threshold),
        _ => false,
    };
    judgment
        .with_evidence(super::top_commits(contributions))
        .with_note(format!(
            "a line is short-lived when a later commit deletes it identically, in the same file, \
             within {SHORT_LIVED_DAYS} days of its addition"
        ))
        .judge(threshold, exceeded)
}
