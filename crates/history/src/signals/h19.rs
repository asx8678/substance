//! H19 Repo farms: the owner's other repositories around this one's creation.
//!
//! Formula (action plan, H19):
//!
//! > Repos the owner created in the 30 days around this repo's creation. Context only; info.
//!
//! This is the one check that is deliberately not a verdict. A prolific account is not
//! evidence about the project a reader is judging: the plan says the project is judged, not the
//! person. So this check reports numbers — how many repositories the owner created in the
//! window, how many were one-commit, how many were forks — and never fails or passes.
//!
//! It is `not_measured` without forge data, because the question is entirely about the forge's
//! listing. It is also `not_measured` when the repository's creation date is unknown: the
//! window is defined against it.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// Judges H19.
#[must_use]
pub fn repo_farms(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => return Judgment::not_measured(reason, BTreeMap::new()),
    };
    if let Some(reason) = forge.unread_reason("owner repos") {
        return Judgment::not_measured(
            format!("the owner's repositories could not be listed: {reason}"),
            BTreeMap::new(),
        );
    }
    let Some(created) = forge.repo.as_ref().and_then(|repo| repo.created_at) else {
        return Judgment::not_measured(
            "the repository's creation date is unknown, so the window has no centre",
            BTreeMap::new(),
        );
    };
    if forge.owner_repos.is_empty() {
        // The listing succeeded and held nothing in the window: that is an answer.
        let mut metrics = BTreeMap::new();
        metrics.insert("owner_repos".to_owned(), 0);
        metrics.insert("owner_repos_forks".to_owned(), 0);
        metrics.insert("owner_repos_templates".to_owned(), 0);
        return Judgment::measured(metrics).with_note(format!(
            "the owner created no repository within {} days of {}",
            super::OWNER_WINDOW_DAYS,
            created.to_rfc3339()
        ));
    }

    let mut created_before = 0u64;
    let mut created_after = 0u64;
    let mut forks = 0u64;
    let mut templates = 0u64;
    for repo in &forge.owner_repos {
        match repo.created_at {
            Some(at) if at < created => created_before += 1,
            Some(_) => created_after += 1,
            None => {}
        }
        if repo.fork {
            forks += 1;
        }
        if repo.template {
            templates += 1;
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "owner_repos".to_owned(),
        i64::try_from(forge.owner_repos.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "owner_repos_before".to_owned(),
        i64::try_from(created_before).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "owner_repos_after".to_owned(),
        i64::try_from(created_after).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "owner_repos_forks".to_owned(),
        i64::try_from(forks).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "owner_repos_templates".to_owned(),
        i64::try_from(templates).unwrap_or(i64::MAX),
    );

    // Context only: the verdict is `reported` whatever the numbers are. The plan is explicit
    // that the person is not judged, so a threshold here would be a finding about an account.
    Judgment::measured(metrics).with_note(
        "context only: this describes the owner's account, not this project, and never \
         contributes a verdict"
            .to_owned(),
    )
}
