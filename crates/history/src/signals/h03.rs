//! H03 Big-bang ratio: the share of today's lines written on day one.
//!
//! Formula (action plan, H03):
//!
//! > Permille of blamed authored HEAD lines whose commit falls on the UTC calendar day of the
//! > first non-bot commit.
//!
//! This is the cheapest strong signal in the set. A person who writes a project over months
//! leaves lines all over the history; an agent that dumps a finished application leaves nearly
//! every surviving line on the day the repository appeared. The blame is what makes it
//! expensive to fake: a line's commit is a fact about the tree, not a claim in a message.
//!
//! Day one comes from [`HistoryInput::day_one`], which applies the private-then-public rule:
//! a repository created more than 30 days after its first commit was developed privately, so
//! its first public day is the honest baseline and the note says so.
//!
//! The thresholds file has no H03 key (the archetype rules read `big_bang`), so the check
//! reports.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;
use crate::signals::days_between;

/// Judges H03.
#[must_use]
pub fn big_bang(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let day_one = input.day_one();
    let mut metrics = BTreeMap::new();
    let total = input.blame.coverage.lines_blamed;
    metrics.insert(
        "blamed_lines".to_owned(),
        i64::try_from(total).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "blamed_files".to_owned(),
        i64::from(input.blame.coverage.files_blamed),
    );
    metrics.insert(
        "authored_files".to_owned(),
        i64::from(input.blame.coverage.files_total),
    );

    let mut judgment = Judgment::measured(metrics);
    if day_one.private_then_public {
        judgment = judgment.with_note(format!(
            "day one is the public creation date {}: the repository was created more than 30 \
             days after its first commit, so the first commit day is not a public baseline",
            day_one.date()
        ));
    }
    if input.blame.coverage.truncated {
        judgment = judgment.with_note(format!(
            "blame covered {} of {} authored files, so the ratio rests on part of the tree",
            input.blame.coverage.files_blamed, input.blame.coverage.files_total
        ));
    }
    if total == 0 {
        return Judgment::not_measured(
            "no authored line could be blamed, so there is no big-bang ratio",
            judgment.metrics,
        );
    }

    let mut from_day_one = 0u64;
    let mut contributions: Vec<(String, u64)> = Vec::new();
    let mut per_commit: BTreeMap<String, u64> = BTreeMap::new();
    for (_, line) in input.blame.lines() {
        if days_between(day_one.at, line.at) == 0 {
            from_day_one += 1;
            *per_commit.entry(line.commit.clone()).or_default() += 1;
        }
    }
    for (sha, lines) in per_commit {
        contributions.push((sha, lines));
    }

    let permille = from_day_one.saturating_mul(1000) / total;
    judgment.metrics.insert(
        "big_bang".to_owned(),
        i64::try_from(permille).unwrap_or(1000),
    );
    judgment.metrics.insert(
        "lines_from_day_one".to_owned(),
        i64::try_from(from_day_one).unwrap_or(i64::MAX),
    );
    // H03 has no threshold key of its own: the archetype rule `one_shot` reads the permille.
    // Reporting is the correct answer for a metric the report shows and M1-T06 interprets.
    judgment
        .with_evidence(super::top_commits(contributions))
        .with_note(format!(
            "{from_day_one} of {total} blamed lines ({permille} permille) date from day one"
        ))
}
