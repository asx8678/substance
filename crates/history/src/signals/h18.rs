//! H18 Star spikes: stars that arrived from accounts created days earlier.
//!
//! Formula (action plan, H18):
//!
//! > Days with 50+ stars where at least 50% of the stargazers' accounts were under 30 days
//! > old (cap 200 lookups). Never a positive signal.
//!
//! A star can be bought, and the cheapest way to buy one is an account made for it. The
//! signal is a burst with young accounts behind it: fifty stars in a day is a launch or a
//! purchase, and the ages say which. The lookup cap is the plan's 200, spent on the stargazers
//! of the burst days so the requests go where the question is.
//!
//! "Never a positive signal" is in the formula for a reason: a quiet repository with no spike
//! does not acquire a good mark from this check. It reports, and the report shows the days.
//!
//! The thresholds file has no key for H18; the numbers are context.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::shape::Judgment;
use super::{HistoryInput, USER_LOOKUPS_H18};

/// How many stars in one UTC day make a burst.
pub const BURST_STARS: u64 = 50;

/// How old an account must be to count as established, in days.
pub const ESTABLISHED_DAYS: i64 = 30;

/// Judges H18.
#[must_use]
pub fn star_spikes(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => return Judgment::not_measured(reason, BTreeMap::new()),
    };
    if forge.stargazers.is_empty() {
        return Judgment::not_measured(
            "the repository has no stargazer whose star instant the forge reported",
            BTreeMap::new(),
        );
    }
    if input.blame.coverage.lines_blamed == 0 {
        // Not the point of this check, but a reader should know the repository is empty.
    }

    // Stars per UTC day, sorted by day.
    let mut by_day: BTreeMap<i64, Vec<&crate::forge::Stargazer>> = BTreeMap::new();
    for star in &forge.stargazers {
        let Some(at) = star.starred_at else {
            continue;
        };
        let day = i64::from(at.as_offset_date_time().date().to_julian_day());
        by_day.entry(day).or_default().push(star);
    }

    let mut burst_days = 0u64;
    let mut questioned = 0u64;
    let mut young = 0u64;
    let mut unknown = 0u64;
    let mut evidence_days: Vec<String> = Vec::new();
    for (day, stars) in &by_day {
        let count = u64::try_from(stars.len()).unwrap_or(0);
        if count < BURST_STARS {
            continue;
        }
        burst_days += 1;
        for star in stars {
            if questioned >= u64::from(USER_LOOKUPS_H18 as u32) {
                break;
            }
            let Some(at) = star.starred_at else {
                continue;
            };
            questioned += 1;
            match forge.account_age_days(&star.login, at) {
                Some(age) if age < ESTABLISHED_DAYS => young += 1,
                Some(_) => {}
                None => unknown += 1,
            }
        }
        let julian = i32::try_from(*day).unwrap_or(0);
        let date = time::Date::from_julian_day(julian).map(|date| date.to_string());
        evidence_days.push(format!(
            "{count} stars on {}",
            date.unwrap_or_else(|_| "an unknown day".to_owned())
        ));
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "stargazers".to_owned(),
        i64::try_from(forge.stargazers.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "star_days".to_owned(),
        i64::try_from(by_day.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "burst_days".to_owned(),
        i64::try_from(burst_days).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "accounts_looked_up".to_owned(),
        i64::try_from(questioned).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "young_accounts".to_owned(),
        i64::try_from(young).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "accounts_without_age".to_owned(),
        i64::try_from(unknown).unwrap_or(i64::MAX),
    );
    if let Some(share) = super::shape::permille(young, questioned) {
        metrics.insert(
            "young_share_permille".to_owned(),
            i64::try_from(share).unwrap_or(1000),
        );
    }

    let mut judgment = Judgment::measured(std::mem::take(&mut metrics));
    if burst_days == 0 {
        return Judgment::not_measured(
            format!("no day has {BURST_STARS} or more stars, so there is no burst to question"),
            judgment.metrics,
        );
    }
    let flagged = young.saturating_mul(2) >= questioned && questioned > 0;
    if flagged {
        judgment = judgment.with_note(format!(
            "at least half of the {} accounts behind the bursts were created less than \
             {ESTABLISHED_DAYS} days before starring",
            questioned
        ));
    } else {
        judgment = judgment.with_note(format!(
            "the {burst_days} burst days are not explained by fresh accounts"
        ));
    }
    if unknown > 0 {
        judgment = judgment.with_note(format!(
            "{unknown} accounts could not be looked up, so their age is unknown"
        ));
    }
    metrics.insert("spike".to_owned(), i64::from(flagged));
    judgment.metrics = metrics;
    judgment.with_note(format!("burst days: {}", evidence_days.join("; ")))
}
