//! H20 Velocity: authored lines per person-day.
//!
//! Formula (action plan, H20):
//!
//! > Authored lines per active person-day, where a person-day is a distinct (author, UTC
//! > date).
//!
//! The unit is the person-day, not the day, so a team of five writing 2,000 lines in a day is
//! 400 lines each — unremarkable — while one person writing 2,000 lines in a day is not. Ten
//! commits in one afternoon are still one person-day, because the question is how much a
//! person produced in a day, not how often they committed.
//!
//! The median is over person-days, and the plan's threshold applies only once there are enough
//! days to read one: `history.person_day_min_active_days`, then
//! `history.person_day_median_lines`. A repository two days old has a median and no evidence.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::common::person_days;
use super::h04::nearest_rank;
use super::shape::Judgment;

/// Judges H20.
#[must_use]
pub fn velocity(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let commits = super::common::own_commits(input);
    let days = person_days(&commits);

    // Authored added lines per (author, day), summed over that person-day's commits.
    let mut lines_by_day: BTreeMap<(String, i64), u64> = BTreeMap::new();
    let human: BTreeSet<String> = days.iter().map(|(author, _)| author.clone()).collect();
    for commit in &commits {
        let key = (
            commit.author.name.clone(),
            i64::from(
                commit
                    .author
                    .at
                    .as_offset_date_time()
                    .date()
                    .to_julian_day(),
            ),
        );
        if !human.contains(&key.0) {
            continue;
        }
        *lines_by_day.entry(key).or_default() += input.authored_added(commit);
    }
    let mut per_day: Vec<u64> = lines_by_day.values().copied().collect();
    per_day.sort_unstable();
    let median = nearest_rank(&per_day, 50);

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "person_days".to_owned(),
        i64::try_from(days.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "authors".to_owned(),
        i64::try_from(human.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "lines".to_owned(),
        i64::try_from(per_day.iter().sum::<u64>()).unwrap_or(i64::MAX),
    );
    if let Some(median) = median {
        metrics.insert(
            "median_lines_per_person_day".to_owned(),
            i64::try_from(median).unwrap_or(i64::MAX),
        );
    }
    if let Some(max) = per_day.last() {
        metrics.insert(
            "max_lines_per_person_day".to_owned(),
            i64::try_from(*max).unwrap_or(i64::MAX),
        );
    }

    let history = thresholds.history();
    let min_days = history.person_day_min_active_days.unwrap_or(14);
    let judgment = Judgment::measured(metrics);
    if days.len() < usize::try_from(min_days).unwrap_or(usize::MAX) {
        return Judgment::not_measured(
            format!(
                "{} person-days is fewer than the {min_days} a median needs",
                days.len()
            ),
            judgment.metrics,
        );
    }
    // Evidence: the person-days with the most lines, as notes, since a person-day is not a
    // commit. The commit list would answer a different question.
    let mut heaviest: Vec<((String, i64), u64)> = lines_by_day.into_iter().collect();
    heaviest.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let top: Vec<String> = heaviest
        .iter()
        .take(5)
        .map(|((author, day), lines)| {
            let date = time::Date::from_julian_day(i32::try_from(*day).unwrap_or(0))
                .map(|date| date.to_string())
                .unwrap_or_else(|_| "an unknown day".to_owned());
            format!("{author} on {date}: {lines} lines")
        })
        .collect();

    let threshold = history.person_day_median_lines;
    let exceeded = match (threshold, median) {
        (Some(threshold), Some(median)) => median >= u64::from(threshold),
        _ => false,
    };
    judgment
        .with_note(format!("heaviest person-days: {}", top.join("; ")))
        .judge(threshold, exceeded)
}
