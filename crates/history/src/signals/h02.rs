//! H02 Active weeks: distinct ISO weeks with a human commit or merged PR.
//!
//! Formula (action plan, H02):
//!
//! > Distinct ISO weeks with at least one non-bot commit or merged PR in the 90 and 365 days
//! > before `as_of`.
//!
//! A week counts once, however many commits it holds, so fifty commits in one weekend and
//! fifty across a year read differently. Weeks are the ISO-8601 ones (Monday to Sunday), which
//! is what `time`'s week number gives and what a reader of the calendar expects; the week of a
//! commit is computed from its UTC date, never from the local time zone.
//!
//! Merged pull requests count towards the same weeks because a squash-merged branch leaves no
//! commits of its own in the default branch's history, and a project that works that way is
//! still working. Under the squash-merge rule this is the only signal that stays honest.
//!
//! The thresholds file has no key for H02: the archetype rules (M1-T06) are what read these
//! numbers, so the check reports.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::thresholds::Thresholds;
use time::Date;

use super::shape::Judgment;
use super::{HistoryInput, LONGEST_WINDOW_DAYS, SHORT_WINDOW_DAYS};

/// Judges H02.
#[must_use]
pub fn active_weeks(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let short_start = input.window_start(SHORT_WINDOW_DAYS);
    let long_start = input.window_start(LONGEST_WINDOW_DAYS);

    let mut weeks_short: BTreeSet<(i32, u8)> = BTreeSet::new();
    let mut weeks_long: BTreeSet<(i32, u8)> = BTreeSet::new();
    for commit in input.human_commits() {
        let at = commit.author.at;
        if at < long_start {
            continue;
        }
        let week = iso_week(at);
        weeks_long.insert(week);
        if at >= short_start {
            weeks_short.insert(week);
        }
    }

    // Merged PRs, when the forge was read. A merge date outside the window does not count.
    let mut prs_in_window = 0u64;
    let mut prs_short = 0u64;
    let mut forge_note = None;
    match input.forge_or_reason() {
        Ok(forge) => {
            for pull in forge.merged_pulls() {
                let Some(merged_at) = pull.merged_at else {
                    continue;
                };
                if merged_at < long_start {
                    continue;
                }
                prs_in_window += 1;
                weeks_long.insert(iso_week(merged_at));
                if merged_at >= short_start {
                    prs_short += 1;
                    weeks_short.insert(iso_week(merged_at));
                }
            }
        }
        Err(reason) => forge_note = Some(reason),
    }

    // Commits outside the long window are not part of either question, but the count says
    // how much of the history the two numbers describe.
    let commits_outside = input
        .human_commits()
        .iter()
        .filter(|commit| commit.author.at < long_start)
        .count();

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "active_weeks_90".to_owned(),
        i64::try_from(weeks_short.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "active_weeks_365".to_owned(),
        i64::try_from(weeks_long.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "human_commits_365".to_owned(),
        i64::try_from(
            input
                .human_commits()
                .iter()
                .filter(|commit| commit.author.at >= long_start)
                .count(),
        )
        .unwrap_or(i64::MAX),
    );
    metrics.insert(
        "merged_pulls_365".to_owned(),
        i64::try_from(prs_in_window).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "merged_pulls_90".to_owned(),
        i64::try_from(prs_short).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "human_commits_before_365".to_owned(),
        i64::try_from(commits_outside).unwrap_or(i64::MAX),
    );

    // The merged PRs are the only input the forge adds; the commit half is always measured,
    // so a missing forge makes the number smaller, not unmeasurable. The note says which.
    let mut judgment = Judgment::measured(metrics);
    if let Some(reason) = forge_note {
        judgment = judgment.with_note(format!("merged pull requests are not counted: {reason}"));
    }
    judgment
}

/// The number of distinct active ISO weeks over the whole history, which is what H16 needs
/// to judge a "production-ready" claim.
///
/// The same week arithmetic H02 uses, run without a window: a claim is about the project's
/// whole life, not about the last year.
#[must_use]
pub fn active_weeks_all_time(input: &HistoryInput) -> u64 {
    let mut weeks: BTreeSet<(i32, u8)> = BTreeSet::new();
    for commit in input.human_commits() {
        weeks.insert(iso_week(commit.author.at));
    }
    if let Ok(forge) = input.forge_or_reason() {
        for pull in forge.merged_pulls() {
            if let Some(merged_at) = pull.merged_at {
                weeks.insert(iso_week(merged_at));
            }
        }
    }
    u64::try_from(weeks.len()).unwrap_or(u64::MAX)
}

/// The ISO year and week of an instant's UTC date.
fn iso_week(at: substance_core::Timestamp) -> (i32, u8) {
    let date: Date = at.as_offset_date_time().date();
    // The ISO week's own year, so the week containing 1 January belongs to the year the
    // week mostly falls in rather than to the calendar year of the date.
    let week = date.iso_week();
    let year = date.year() - i32::from(week == 1 && date.month() == time::Month::December);
    (year, week)
}
