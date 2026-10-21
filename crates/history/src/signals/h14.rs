//! H14 Real usage: are the people asking questions real, and does anyone depend on this?
//!
//! Formula (action plan, H14):
//!
//! > Outside issue authors whose accounts were 180+ days old when they opened the issue (cap
//! > 50 user lookups); deps.dev dependents; registry downloads.
//!
//! The question behind it is whether the apparent users are users. An account created last
//! week that opens a detailed issue on a project it has never seen is not a customer; an
//! account that has existed for years and asks a small, specific question usually is. The
//! lookup cap is the plan's 50, and it is spent on authors sorted by login so two runs ask
//! the same questions (ADR-0009).
//!
//! The registry half — dependents from deps.dev and download counts — needs the package's
//! name and version and the M1-T04 clients, so this check reports that part as not measured
//! with the reason; a zero there would say "nobody uses this", which is a much stronger claim
//! than "substance did not ask".

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::common::is_maintainer;
use super::shape::Judgment;
use super::{HistoryInput, USER_LOOKUPS_H14};

/// How old an account must have been when it opened the issue, in days.
pub const REAL_ACCOUNT_DAYS: i64 = 180;

/// Judges H14.
#[must_use]
pub fn real_usage(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => return Judgment::not_measured(reason, BTreeMap::new()),
    };

    // Outsiders whose issues name an account age, sorted by login so the sample is stable.
    let mut outsiders: Vec<&crate::forge::Issue> = forge
        .issues
        .iter()
        .filter(|issue| !is_maintainer(&issue.author_association))
        .filter(|issue| !issue.author.is_bot())
        .filter(|issue| !issue.author.login.is_empty())
        .collect();
    outsiders.sort_by(|left, right| {
        left.author
            .login
            .cmp(&right.author.login)
            .then_with(|| left.number.cmp(&right.number))
    });
    outsiders.dedup_by(|left, right| left.author.login == right.author.login);

    let mut looked_up = 0u64;
    let mut established = 0u64;
    let mut unknown = 0u64;
    for issue in &outsiders {
        if looked_up >= u64::from(USER_LOOKUPS_H14 as u32) {
            break;
        }
        let Some(created_at) = issue.created_at else {
            continue;
        };
        looked_up += 1;
        match forge.account_age_days(&issue.author.login, created_at) {
            Some(age) if age >= REAL_ACCOUNT_DAYS => established += 1,
            Some(_) => {}
            None => unknown += 1,
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "outside_issue_authors".to_owned(),
        i64::try_from(outsiders.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "accounts_looked_up".to_owned(),
        i64::try_from(looked_up).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "established_accounts".to_owned(),
        i64::try_from(established).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "accounts_without_age".to_owned(),
        i64::try_from(unknown).unwrap_or(i64::MAX),
    );

    let mut judgment = Judgment::measured(metrics);
    if outsiders.len() > usize::try_from(looked_up).unwrap_or(usize::MAX) {
        judgment = judgment.with_note(format!(
            "the {} lookups are the first of {} outside issue authors, in login order",
            USER_LOOKUPS_H14,
            outsiders.len()
        ));
    }
    if unknown > 0 {
        judgment = judgment.with_note(format!(
            "{unknown} accounts could not be looked up, so their age is unknown rather than young"
        ));
    }
    judgment = judgment.with_note(
        "dependents and downloads are not measured here: they need the package registry \
         clients and the dependency list"
            .to_owned(),
    );
    if looked_up == 0 {
        return Judgment::not_measured(
            "no outside issue author could be looked up, so the question of real usage is \
             unanswered",
            judgment.metrics,
        );
    }
    judgment
}
