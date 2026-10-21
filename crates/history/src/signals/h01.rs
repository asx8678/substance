//! H01 Chronology: server-side first dates against commit dates.
//!
//! Formula (action plan, H01):
//!
//! > Server-side first dates (repo created_at, first PR, issue, CI run, release) against
//! > commit dates. Commits more than 1 day older than the repo →
//! > `unverifiable("imported or backdated history")`, plus the count.
//!
//! A commit date is set by whoever runs `git commit`, so it can be anything; the forge's own
//! creation date cannot be set by the author. When a commit claims to predate the repository
//! by more than a day, the history was imported from somewhere else or backdated, and every
//! date-based signal below (H02, H03, H09) rests on numbers that mean something different.
//! That is why this check answers `unverifiable` rather than `fail`: it is not a finding
//! about the project's quality, it is a statement that the evidence cannot be read.
//!
//! The thresholds file has no key for this check, so a history that is consistent reports its
//! metrics and declines to judge (ADR-0013). Only the `unverifiable` case has a verdict.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;
use crate::signals::days_between;

/// One server-side first date, and which object it came from.
struct ServerDate {
    /// Where the date came from: `repo`, `pull`, `issue`, `workflow run`, `release`.
    source: &'static str,
    /// The instant the server recorded.
    at: Timestamp,
}

/// Judges H01.
#[must_use]
pub fn chronology(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let mut metrics = BTreeMap::new();
    // The earliest server-side date is the repository's own creation date: it needs no
    // paging, and every other object was created after it.
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => {
            metrics.insert("commits_total".to_owned(), count(&input.commits));
            return Judgment::not_measured(reason, metrics);
        }
    };
    let repo_created = forge.repo.as_ref().and_then(|repo| repo.created_at);
    let Some(repo_created) = repo_created else {
        metrics.insert("commits_total".to_owned(), count(&input.commits));
        return Judgment::not_measured("the forge did not report a creation date", metrics);
    };

    let first_commit = input.first_commit_at();
    // The formula's "more than 1 day older": a commit dated at least two whole days before
    // the repository was created. A one-day gap is a timezone and a server clock, not a
    // pasted history, so it is allowed for.
    let older = input
        .commits
        .iter()
        .filter(|commit| days_between(commit.author.at, repo_created) > 1)
        .count();

    metrics.insert("commits_total".to_owned(), count(&input.commits));
    metrics.insert(
        "commits_older_than_repo".to_owned(),
        i64::try_from(older).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "repo_created_at".to_owned(),
        repo_created.as_offset_date_time().unix_timestamp(),
    );
    if let Some(first) = first_commit {
        metrics.insert(
            "first_commit_at".to_owned(),
            first.as_offset_date_time().unix_timestamp(),
        );
        metrics.insert(
            "first_commit_days_after_creation".to_owned(),
            days_between(repo_created, first),
        );
    }

    // Every server-side first date, so a reader can compare them with the commits.
    let mut dates: Vec<ServerDate> = Vec::new();
    if let Some(first_pull) = forge.pulls.iter().filter_map(|pull| pull.created_at).min() {
        dates.push(ServerDate {
            source: "pull",
            at: first_pull,
        });
    }
    if let Some(first_issue) = forge
        .issues
        .iter()
        .filter_map(|issue| issue.created_at)
        .min()
    {
        dates.push(ServerDate {
            source: "issue",
            at: first_issue,
        });
    }
    if let Some(first_run) = forge
        .workflow_runs
        .iter()
        .filter_map(|run| run.created_at)
        .min()
    {
        dates.push(ServerDate {
            source: "workflow run",
            at: first_run,
        });
    }
    if let Some(first_release) = forge
        .releases
        .iter()
        .filter_map(|release| release.created_at)
        .min()
    {
        dates.push(ServerDate {
            source: "release",
            at: first_release,
        });
    }
    dates.sort_by(|left, right| {
        left.at
            .cmp(&right.at)
            .then_with(|| left.source.cmp(right.source))
    });
    for date in &dates {
        let key = format!("first_{}_at", date.source.replace(' ', "_"));
        metrics.insert(key, date.at.as_offset_date_time().unix_timestamp());
    }

    let mut judgment = Judgment::measured(metrics);
    if older > 0 {
        judgment = Judgment::unverifiable("imported or backdated history", judgment.metrics)
            .with_note(format!(
                "{older} commits are dated more than a day before the repository was created"
            ));
    } else {
        judgment = judgment
            .with_note("every commit is dated after the repository's creation date".to_owned());
    }
    judgment
}

/// A count as an `i64` metric.
fn count<T>(items: &[T]) -> i64 {
    i64::try_from(items.len()).unwrap_or(i64::MAX)
}
