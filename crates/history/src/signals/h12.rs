//! H12 Review: was the code reviewed by someone other than its author?
//!
//! Formula (action plan, H12):
//!
//! > Permille of merged PRs with an APPROVED review from someone other than the author;
//! > count of rubber stamps (merged under 60 s after creation with no reviews or comments).
//!
//! A review is the one cheap check on whether anyone else looked. An approval from the
//! author is not a review, so it is excluded; a robot approving everything is a rubber stamp
//! with extra steps, which is why the review's author goes through the same bot test as a
//! commit.
//!
//! A rubber stamp is the other half: a pull request opened and merged within a minute with no
//! review and no comment. That is the trace of a person (or an agent) merging their own work
//! through the pull-request form without anyone reading it.
//!
//! The threshold is `history.reviewed_prs_permille`, and it applies only once the project has
//! `history.reviewed_prs_min_maintainers` maintainers: a solo project cannot review its own
//! work, and saying so is H11's business.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// How quickly a pull request must be merged to count as a rubber stamp: 60 seconds.
pub const RUBBER_STAMP_SECONDS: i64 = 60;

/// Judges H12.
#[must_use]
pub fn review(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => {
            return Judgment::not_measured(reason, BTreeMap::new());
        }
    };
    let merged = forge.merged_pulls();
    if merged.is_empty() {
        return Judgment::not_measured(
            "the repository has no merged pull request, so no merge could be reviewed",
            BTreeMap::new(),
        );
    }
    if let Some(reason) = forge.unread_reason("reviews") {
        return Judgment::not_measured(
            format!("the reviews could not be read: {reason}"),
            BTreeMap::new(),
        );
    }

    let mut reviewed = 0u64;
    let mut unread = 0u64;
    let mut rubber_stamps = 0u64;
    let mut evidence_commits: Vec<(String, u64)> = Vec::new();
    let mut self_approved = 0u64;
    for pull in &merged {
        let Some(reviews) = forge.reviews_of(pull.number) else {
            unread += 1;
            continue;
        };
        let author = pull.author.login.to_lowercase();
        let approved = reviews.iter().any(|review| {
            review.state.eq_ignore_ascii_case("APPROVED")
                && !review.author.login.to_lowercase().eq(&author)
                && !review.author.is_bot()
        });
        if approved {
            reviewed += 1;
            if let Some(sha) = &pull.merge_commit_sha {
                evidence_commits.push((sha.clone(), 1));
            }
        }
        let self_approval = reviews.iter().any(|review| {
            review.state.eq_ignore_ascii_case("APPROVED")
                && review.author.login.to_lowercase().eq(&author)
        });
        if self_approval {
            self_approved += 1;
        }
        // A rubber stamp: opened and merged within a minute, with nothing said about it.
        let quick = match (pull.created_at, pull.merged_at) {
            (Some(created), Some(merged_at)) => {
                merged_at.as_offset_date_time().unix_timestamp()
                    - created.as_offset_date_time().unix_timestamp()
                    <= RUBBER_STAMP_SECONDS
            }
            _ => false,
        };
        if quick && reviews.is_empty() {
            rubber_stamps += 1;
        }
    }

    let read = u64::try_from(merged.len())
        .unwrap_or(u64::MAX)
        .saturating_sub(unread);
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "merged_pulls".to_owned(),
        i64::try_from(merged.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "pulls_with_reviews".to_owned(),
        i64::try_from(read).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "reviewed_pulls".to_owned(),
        i64::try_from(reviewed).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "rubber_stamps".to_owned(),
        i64::try_from(rubber_stamps).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "self_approved".to_owned(),
        i64::try_from(self_approved).unwrap_or(i64::MAX),
    );
    let permille = super::shape::permille(reviewed, read);
    if let Some(permille) = permille {
        metrics.insert(
            "reviewed_permille".to_owned(),
            i64::try_from(permille).unwrap_or(1000),
        );
    }

    let mut judgment = Judgment::measured(std::mem::take(&mut metrics));
    if unread > 0 {
        judgment = judgment.with_note(format!(
            "{unread} of {} merged pull requests could not have their reviews read, so the \
             share describes the rest",
            merged.len()
        ));
    }
    if read == 0 {
        return Judgment::not_measured(
            "no merged pull request had its reviews read",
            judgment.metrics,
        );
    }

    // The threshold needs a maintainer count, which is H11's number. It is computed here the
    // same way so this check does not depend on another check having run.
    let maintainers = merged
        .iter()
        .filter_map(|pull| {
            pull.merged_by
                .as_deref()
                .or(Some(pull.author.login.as_str()))
        })
        .filter(|login| !crate::git::is_bot(login, "", None))
        .map(str::to_lowercase)
        .collect::<std::collections::BTreeSet<String>>();
    let maintainers = u64::try_from(maintainers.len()).unwrap_or(0);
    metrics.insert(
        "maintainers".to_owned(),
        i64::try_from(maintainers).unwrap_or(i64::MAX),
    );
    judgment.metrics = metrics;

    let history = thresholds.history();
    let min_maintainers = history.reviewed_prs_min_maintainers.unwrap_or(2);
    // A solo project cannot review its own work: the threshold does not apply, and the
    // verdict reports rather than passing, because the question is not answerable here.
    let applies = maintainers >= u64::from(min_maintainers);
    let threshold = history.reviewed_prs_permille.filter(|_| applies);
    let exceeded = match (threshold, permille) {
        (Some(threshold), Some(permille)) => permille < u64::from(threshold),
        _ => false,
    };
    judgment
        .with_evidence(super::top_commits(evidence_commits))
        .with_note(if applies {
            format!(
                "{maintainers} maintainers merged pull requests, so the {min_maintainers}-maintainer \
                 floor applies"
            )
        } else {
            format!(
                "only {maintainers} maintainers merged pull requests: the review threshold does \
                 not apply to a solo project"
            )
        })
        .judge(threshold, exceeded)
}
