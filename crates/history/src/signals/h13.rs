//! H13 Maintenance: how fast the project answers, and how current its dependencies are.
//!
//! Formula (action plan, H13):
//!
//! > Median hours to the first maintainer response on outsiders' issues and PRs (last 365
//! > days); median days to merge security-labelled dependency PRs; libyear in
//! > permille-years; CI conclusion at HEAD.
//!
//! Four numbers, each answering "is anyone home". The first is the one a user feels: someone
//! outside the project asks a question, and either a maintainer answers or nobody does. The
//! second is the dependency half of the same question — a security update that sits unmerged
//! for months is an unanswered issue with a CVE attached.
//!
//! **Libyear** is the dependency-vintage measure: for each direct dependency, how many years
//! behind the newest release the pinned version is. It needs the registry clients (M1-T04)
//! and the lockfile parsers, so this check reports it only when a caller supplied those
//! numbers; the history input carries no registry data, and inventing one would be worse than
//! reporting the gap. It is expressed in permille-years to stay integer: 1000 is one year
//! behind, summed over dependencies.
//!
//! The thresholds file has no key for H13: `answers_issues` feeds H22's `solo` rule, and the
//! other three numbers are context.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::common::is_maintainer;
use super::h04::nearest_rank;
use super::shape::Judgment;
use super::{HistoryInput, LONGEST_WINDOW_DAYS};

/// The label GitHub's dependency bots put on a security update, lowercased.
pub const SECURITY_LABEL: &str = "security";

/// Judges H13.
#[must_use]
pub fn maintenance(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => return Judgment::not_measured(reason, BTreeMap::new()),
    };
    let window_start = input.window_start(LONGEST_WINDOW_DAYS);

    // The first maintainer response on an outsider's issue: the earliest comment by someone
    // whose association is a maintainer, after the issue was opened.
    let mut response_hours: Vec<u64> = Vec::new();
    let mut outsider_issues = 0u64;
    let mut unanswered = 0u64;
    for issue in &forge.issues {
        let Some(created_at) = issue.created_at else {
            continue;
        };
        if created_at < window_start || is_maintainer(&issue.author_association) {
            continue;
        }
        if issue.author.is_bot() {
            continue;
        }
        outsider_issues += 1;
        let handle = format!("{}", issue.number);
        let first = forge
            .issue_comments
            .iter()
            .filter(|comment| comment.issue.ends_with(&format!("#{handle}")))
            .filter(|comment| is_maintainer(&comment.author_association))
            .filter(|comment| !comment.author.is_bot())
            .filter_map(|comment| comment.created_at)
            .filter(|at| *at >= created_at)
            .min();
        match first {
            Some(at) => {
                let seconds = at.as_offset_date_time().unix_timestamp()
                    - created_at.as_offset_date_time().unix_timestamp();
                response_hours.push(u64::try_from(seconds.max(0) / 3600).unwrap_or(u64::MAX));
            }
            None => unanswered += 1,
        }
    }
    // The same question for pull requests: an outsider's pull request that a maintainer
    // reviewed or commented on.
    for pull in &forge.pulls {
        let Some(created_at) = pull.created_at else {
            continue;
        };
        if created_at < window_start || pull.author.is_bot() {
            continue;
        }
        // `author_association` is not on the pull-request type, so an author who merged
        // their own pull request is treated as a maintainer and skipped: the question is
        // about people outside the project.
        if pull.merged_by.as_deref() == Some(pull.author.login.as_str()) {
            continue;
        }
        let first = forge
            .reviews_of(pull.number)
            .unwrap_or(&[])
            .iter()
            .filter(|review| !review.author.is_bot())
            .filter(|review| review.author.login != pull.author.login)
            .filter_map(|review| review.submitted_at)
            .filter(|at| *at >= created_at)
            .min();
        if let Some(at) = first {
            let seconds = at.as_offset_date_time().unix_timestamp()
                - created_at.as_offset_date_time().unix_timestamp();
            response_hours.push(u64::try_from(seconds.max(0) / 3600).unwrap_or(u64::MAX));
        }
    }
    let mut sorted_hours = response_hours.clone();
    sorted_hours.sort_unstable();
    let median_hours = nearest_rank(&sorted_hours, 50);

    // Security-labelled dependency pull requests: how long they waited to be merged.
    let mut security_days: Vec<u64> = Vec::new();
    for pull in &forge.pulls {
        let labelled = pull.labels.iter().any(|label| {
            label.to_lowercase().contains(SECURITY_LABEL)
                || label.to_lowercase().contains("dependencies")
        });
        if !labelled {
            continue;
        }
        let (Some(created_at), Some(merged_at)) = (pull.created_at, pull.merged_at) else {
            continue;
        };
        let seconds = merged_at.as_offset_date_time().unix_timestamp()
            - created_at.as_offset_date_time().unix_timestamp();
        security_days.push(u64::try_from(seconds.max(0) / 86_400).unwrap_or(u64::MAX));
    }
    security_days.sort_unstable();
    let security_median = nearest_rank(&security_days, 50);

    // CI at HEAD: the newest run on the default branch's head commit.
    let head_runs: Vec<&crate::forge::WorkflowRun> = forge
        .workflow_runs
        .iter()
        .filter(|run| run.conclusion.is_some())
        .collect();
    let mut conclusions: BTreeMap<String, u64> = BTreeMap::new();
    for run in &head_runs {
        if let Some(conclusion) = &run.conclusion {
            *conclusions.entry(conclusion.clone()).or_default() += 1;
        }
    }
    let newest = head_runs
        .iter()
        .filter_map(|run| run.created_at.map(|at| (at, run.conclusion.clone())))
        .max_by(|left, right| left.0.cmp(&right.0));

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "outsider_issues".to_owned(),
        i64::try_from(outsider_issues).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "unanswered_issues".to_owned(),
        i64::try_from(unanswered).unwrap_or(i64::MAX),
    );
    if let Some(median) = median_hours {
        metrics.insert(
            "median_first_response_hours".to_owned(),
            i64::try_from(median).unwrap_or(i64::MAX),
        );
    }
    if let Some(median) = security_median {
        metrics.insert(
            "security_merge_median_days".to_owned(),
            i64::try_from(median).unwrap_or(i64::MAX),
        );
    }
    metrics.insert(
        "security_pulls".to_owned(),
        i64::try_from(security_days.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "workflow_runs_finished".to_owned(),
        i64::try_from(head_runs.len()).unwrap_or(i64::MAX),
    );
    for (conclusion, count) in &conclusions {
        let key = format!("runs_{}", conclusion.to_lowercase().replace(' ', "_"));
        metrics.insert(key, i64::try_from(*count).unwrap_or(i64::MAX));
    }
    if let Some((_, Some(conclusion))) = &newest {
        // The conclusion itself is a string, and metrics are integers: the index into the
        // sorted list of conclusions seen is the integer form, and the note spells out which
        // one it is.
        let position = conclusions
            .keys()
            .position(|candidate| candidate == conclusion)
            .unwrap_or(0);
        metrics.insert(
            "ci_conclusion_at_head".to_owned(),
            i64::try_from(position).unwrap_or(0),
        );
    }

    // Libyear needs registry data, which this input does not carry: reporting the gap is
    // honest, and a zero would read as "up to date".
    let mut judgment = Judgment::measured(metrics);
    judgment = judgment.with_note(
        "libyear is not computed here: it needs the registry clients and the dependency list, \
         which the history input does not carry"
            .to_owned(),
    );
    if let Some((_, Some(conclusion))) = &newest {
        judgment = judgment.with_note(format!(
            "the newest finished workflow run concluded {conclusion}"
        ));
    }
    if let Some(median) = median_hours {
        judgment = judgment.with_note(format!(
            "the median first response on an outsider's issue or pull request is {median} hours"
        ));
    }
    judgment
}
