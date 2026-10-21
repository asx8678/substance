//! What the H11–H22 signals share: the false-positive rules and the derived facts
//! (M1-T06).
//!
//! Four of the plan's false-positive rules touch more than one check, so they live here
//! once and every check reads the same answer:
//!
//! - **Forks.** A fork's history contains its upstream's commits. Those are not this
//!   project's work, so they are excluded from the commit-level counts when the forge's
//!   compare said how many commits came from upstream.
//! - **Squash merges.** A project that squash-merges has few commits and many pull
//!   requests. When at least 70% of its merged pull requests were squashed, H02 and H04 read
//!   pull-request dates and sizes instead of commits.
//! - **Private, then public.** Handled where day one is decided
//!   ([`HistoryInput::day_one`]), because it changes what "day one" means rather than what
//!   is counted.
//! - **Bus factor 1.** A flag, never a verdict: H11 reports it and no threshold reads it.
//!
//! The rest are definitions: a person-day, a test path, and whether an association names a
//! maintainer.

use std::collections::BTreeSet;

use substance_core::Timestamp;

use super::{HistoryInput, MAINTAINER_ASSOCIATIONS};
use crate::git::Commit;

/// The share of merged pull requests that must be squashed before H02 and H04 read pull
/// requests instead of commits, in permille: 70%.
pub const SQUASH_SHARE_PERMILLE: u64 = 700;

/// Whether a commit's subject looks like GitHub's squash-merge subject.
///
/// GitHub writes `<title> (#123)` for a squash merge, and appends the pull request's number
/// in parentheses. A project can rewrite that, so this is only evidence — the authoritative
/// test is the forge's own `merge_commit_sha` pointing at the commit, which needs one pull
/// request's data rather than a pattern.
#[must_use]
pub fn is_squash_subject(subject: &str) -> bool {
    let trimmed = subject.trim_end();
    let Some(rest) = trimmed.strip_suffix(')') else {
        return false;
    };
    let Some((_, number)) = rest.rsplit_once("(#") else {
        return false;
    };
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
}

/// Whether the project squash-merges enough that commits are the wrong unit (the plan's 70%
/// rule).
///
/// The test is the plan's: at least 70% of merged pull requests squashed. A repository with
/// no merged pull requests does not squash-merge, whatever its commit subjects look like.
#[must_use]
pub fn squash_merges(input: &HistoryInput) -> bool {
    let Ok(forge) = input.forge_or_reason() else {
        return false;
    };
    let merged = forge.merged_pulls();
    if merged.is_empty() {
        return false;
    }
    let squashed = merged
        .iter()
        .filter(|pull| {
            pull.merge_commit_sha
                .as_deref()
                .and_then(|sha| input.commits.iter().find(|commit| commit.sha == sha))
                .is_some_and(|commit| commit.parents.len() <= 1)
        })
        .count();
    let total = u64::try_from(merged.len()).unwrap_or(u64::MAX);
    let squashed = u64::try_from(squashed).unwrap_or(0);
    super::shape::permille(squashed, total).is_some_and(|share| share >= SQUASH_SHARE_PERMILLE)
}

/// Whether `commit` came from the fork's upstream, according to the compare the forge ran.
///
/// `compare(base=upstream, head=this fork)` reports `ahead_by`: how many commits this fork
/// has that upstream does not. The fork's own work is therefore the `ahead_by` newest
/// commits in `--date-order`; everything older came from upstream. That is an ordering
/// assumption, and it is the one the API's own numbers support — a fork cannot commit into
/// its own past.
#[must_use]
pub fn from_upstream(input: &HistoryInput, index: usize) -> bool {
    let Some(forge) = input.forge.as_ref() else {
        return false;
    };
    let Some(compare) = forge.upstream_compare.as_ref() else {
        return false;
    };
    let ahead = usize::try_from(compare.ahead_by).unwrap_or(usize::MAX);
    index >= ahead
}

/// The commits a check counts: human, and (for a fork) not upstream's.
///
/// `--date-order` lists a fork's own commits before upstream's, so this is a prefix filter
/// over the history rather than a set membership test.
#[must_use]
pub fn own_commits(input: &HistoryInput) -> Vec<&Commit> {
    input
        .commits
        .iter()
        .enumerate()
        .filter(|(index, commit)| {
            !commit.is_merge()
                && !crate::git::is_bot_commit(&commit.author)
                && !from_upstream(input, *index)
        })
        .map(|(_, commit)| commit)
        .collect()
}

/// Whether a path is a test file, by the rules in `rules/history/test-paths.txt`.
///
/// The patterns are directory markers and file-name globs, matched without a glob crate:
/// a path matches a directory rule when one of its segments equals the rule without its
/// trailing slash, and a file-name rule when the name matches the rule's `*` positions.
#[must_use]
pub fn is_test_path(path: &str, rules: &TestPathRules) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    for rule in &rules.directories {
        if path.split('/').any(|segment| segment == rule) {
            return true;
        }
    }
    rules
        .names
        .iter()
        .any(|pattern| glob_matches(pattern, name))
}

/// Matches a name against a pattern containing `*` wildcards, without a glob crate.
///
/// `*` matches any run of characters, including none, and everything else matches itself.
/// Only names are matched, never paths with separators, so `*` cannot span directories.
#[must_use]
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let Some(first) = parts.first() else {
        return false;
    };
    let Some(last) = parts.last() else {
        return false;
    };
    if !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len().saturating_sub(last.len())];
    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The test-path rules, parsed from `rules/history/test-paths.txt`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestPathRules {
    /// Directory names whose contents are tests, without the trailing slash.
    pub directories: Vec<String>,
    /// File-name patterns, with `*` wildcards.
    pub names: Vec<String>,
}

impl TestPathRules {
    /// Parses the rules file: comments and blanks are dropped, a line ending in `/` is a
    /// directory rule and anything else is a name pattern. Both lists come out sorted.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut directories: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.strip_suffix('/') {
                Some(directory) => directories.push(directory.to_owned()),
                None => names.push(line.to_owned()),
            }
        }
        directories.sort();
        directories.dedup();
        names.sort();
        names.dedup();
        Self { directories, names }
    }

    /// Whether the rules file listed anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.directories.is_empty() && self.names.is_empty()
    }
}

/// The distinct (author, UTC date) pairs in a commit list, sorted.
///
/// A person-day is one person on one calendar day, however many commits they made: ten
/// commits on Tuesday are one person-day, not ten. The date is UTC, so the answer does not
/// depend on where the run happened.
#[must_use]
pub fn person_days(commits: &[&Commit]) -> Vec<(String, i64)> {
    let mut days: BTreeSet<(String, i64)> = BTreeSet::new();
    for commit in commits {
        days.insert((
            commit.author.name.clone(),
            i64::from(
                commit
                    .author
                    .at
                    .as_offset_date_time()
                    .date()
                    .to_julian_day(),
            ),
        ));
    }
    days.into_iter().collect()
}

/// The distinct UTC dates in a commit list, sorted.
#[must_use]
pub fn active_days(commits: &[&Commit]) -> Vec<i64> {
    let mut days: BTreeSet<i64> = BTreeSet::new();
    for commit in commits {
        days.insert(i64::from(
            commit
                .author
                .at
                .as_offset_date_time()
                .date()
                .to_julian_day(),
        ));
    }
    days.into_iter().collect()
}

/// Whether `association` names a maintainer.
#[must_use]
pub fn is_maintainer(association: &str) -> bool {
    MAINTAINER_ASSOCIATIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(association))
}

/// How many whole days `later` is after `earlier`, clamped at zero.
#[must_use]
pub fn non_negative_days(earlier: Timestamp, later: Timestamp) -> u32 {
    u32::try_from(super::days_between(earlier, later).max(0)).unwrap_or(u32::MAX)
}
