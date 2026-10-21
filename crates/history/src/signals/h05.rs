//! H05 Machine-speed commits: lines written per minute since the same author's last commit.
//!
//! Formula (action plan, H05):
//!
//! > Per commit: authored added lines ÷ max(1, minutes since the same author's previous
//! > commit); p90, and the count above threshold.
//!
//! A person types tens of lines a minute at most, and not for hours. An agent that writes an
//! application commits thousands of lines three minutes apart, and that rate is the whole
//! signal: `max(1, minutes)` stops a burst of commits in the same minute from dividing by zero
//! and turning a two-line commit into an infinite rate.
//!
//! "The same author" is the mailmap-normalized name, so one person with two addresses is one
//! author. Commits are ordered by author instant, and the first commit of an author has no
//! previous commit: it contributes no rate, because there is no interval to divide by.
//!
//! The threshold is `history.machine_speed_p90_lines_per_minute`, compared against the p90 that
//! nearest rank reports; the count above it is reported whatever the verdict.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::h04::nearest_rank;
use super::shape::Judgment;

/// Judges H05.
#[must_use]
pub fn machine_speed(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    // Oldest first, so "the same author's previous commit" is the entry before this one.
    let mut commits: Vec<(&str, i64, u64, &str)> = input
        .human_commits()
        .iter()
        .map(|commit| {
            (
                commit.author.name.as_str(),
                commit.author.at.as_offset_date_time().unix_timestamp(),
                input.authored_added(commit),
                commit.sha.as_str(),
            )
        })
        .collect();
    commits.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| left.0.cmp(right.0))
            .then_with(|| left.3.cmp(right.3))
    });

    let mut last_by_author: BTreeMap<&str, i64> = BTreeMap::new();
    let mut rates: Vec<u64> = Vec::new();
    let mut contributions: Vec<(String, u64)> = Vec::new();
    let mut authors: BTreeSet<&str> = BTreeSet::new();
    for (author, at, lines, sha) in &commits {
        authors.insert(author);
        if let Some(previous) = last_by_author.get(author) {
            let seconds = at.saturating_sub(*previous).max(1);
            let minutes = u64::try_from(seconds)
                .unwrap_or(u64::MAX)
                .div_ceil(60)
                .max(1);
            let rate = lines / minutes;
            rates.push(rate);
            if rate > 0 {
                contributions.push(((*sha).to_owned(), rate));
            }
        }
        last_by_author.insert(author, *at);
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "commits_with_rate".to_owned(),
        i64::try_from(rates.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "authors".to_owned(),
        i64::try_from(authors.len()).unwrap_or(i64::MAX),
    );
    let mut sorted = rates.clone();
    sorted.sort_unstable();
    let p90 = nearest_rank(&sorted, 90);
    if let Some(p90) = p90 {
        metrics.insert(
            "p90_lines_per_minute".to_owned(),
            i64::try_from(p90).unwrap_or(i64::MAX),
        );
    }
    let threshold = thresholds.history().machine_speed_p90_lines_per_minute;
    let above = match (threshold, p90) {
        (Some(threshold), Some(_)) => rates
            .iter()
            .filter(|rate| **rate >= u64::from(threshold))
            .count(),
        _ => 0,
    };
    metrics.insert(
        "commits_above_threshold".to_owned(),
        i64::try_from(above).unwrap_or(i64::MAX),
    );

    let judgment = Judgment::measured(metrics);
    if rates.is_empty() {
        return Judgment::not_measured(
            "no author has two commits, so there is no interval between commits",
            judgment.metrics,
        );
    }
    let exceeded = match (threshold, p90) {
        (Some(threshold), Some(p90)) => p90 >= u64::from(threshold),
        _ => false,
    };
    judgment
        .with_evidence(super::top_commits(contributions))
        .with_note(format!(
            "the rate of a commit is its authored added lines over max(1, minutes since the \
             same author's previous commit); {} commits have a previous commit",
            rates.len()
        ))
        .judge(threshold, exceeded)
}
