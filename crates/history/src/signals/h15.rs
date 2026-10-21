//! H15 Silent tracker: a repository whose issues only its author ever touches.
//!
//! Formula (action plan, H15):
//!
//! > Fail when the repo has at least 3 issues and only the owner or author accounts ever
//! > opened or commented on them.
//!
//! A public issue tracker that only the owner uses is a support channel that does not exist.
//! The formula is deliberately narrow: three issues is a low bar, and "only the owner or the
//! issue's own author" is a high one — one comment from anyone else clears the repository.
//!
//! This is one of the few history checks with a verdict built into its formula rather than
//! into a threshold, so it fails or passes without consulting the file. A repository with
//! fewer than three issues cannot fail it, and says so with `not_measured` rather than a pass:
//! three issues is the formula's own precondition.

use std::collections::BTreeSet;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// How many issues a repository needs before the question can be asked.
pub const MIN_ISSUES: usize = 3;

/// Judges H15.
#[must_use]
pub fn silent_tracker(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => return Judgment::not_measured(reason, std::collections::BTreeMap::new()),
    };
    let issues = &forge.issues;
    let mut metrics = std::collections::BTreeMap::new();
    metrics.insert(
        "issues".to_owned(),
        i64::try_from(issues.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "comments".to_owned(),
        i64::try_from(forge.issue_comments.len()).unwrap_or(i64::MAX),
    );

    if issues.len() < MIN_ISSUES {
        return Judgment::not_measured(
            format!(
                "the repository has {} issues, fewer than the {MIN_ISSUES} the formula needs",
                issues.len()
            ),
            metrics,
        );
    }

    // Everyone who opened an issue, and everyone who commented on one.
    let mut authors: BTreeSet<String> = issues
        .iter()
        .map(|issue| issue.author.login.to_lowercase())
        .collect();
    let mut commenters: BTreeSet<String> = forge
        .issue_comments
        .iter()
        .map(|comment| comment.author.login.to_lowercase())
        .filter(|login| !login.is_empty())
        .collect();
    // The owner account, for the "or owner" half of the formula. The forge client knows
    // it; the repository object does not carry it, so the slug's owner is the answer.
    let owner = input
        .forge
        .as_ref()
        .and_then(|data| data.owner.clone())
        .unwrap_or_default();
    if !owner.is_empty() {
        authors.insert(owner.to_lowercase());
        commenters.insert(owner.to_lowercase());
    }

    // Outsiders: anyone who is neither an issue's author nor the owner. The formula allows
    // the author of *each* issue, so the set to subtract is every issue author, not one.
    let mut outside_commenters: BTreeSet<String> = BTreeSet::new();
    for comment in &forge.issue_comments {
        let login = comment.author.login.to_lowercase();
        if login.is_empty() {
            continue;
        }
        if !authors.contains(&login) {
            outside_commenters.insert(login);
        }
    }

    metrics.insert(
        "issue_authors".to_owned(),
        i64::try_from(authors.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "outside_commenters".to_owned(),
        i64::try_from(outside_commenters.len()).unwrap_or(i64::MAX),
    );
    let mut judgment = Judgment::measured(metrics);
    let silent = outside_commenters.is_empty();
    judgment = judgment.with_note(if silent {
        "every issue was opened by its author or the owner, and only they commented".to_owned()
    } else {
        format!(
            "{} people outside the issue authors commented on an issue",
            outside_commenters.len()
        )
    });
    // The formula states the verdict directly: there is no threshold to consult.
    judgment.verdict = if silent {
        substance_core::model::Verdict::Fail
    } else {
        substance_core::model::Verdict::Pass
    };
    judgment
}
