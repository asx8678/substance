//! H07 Fix chains: commit bursts that keep repairing themselves.
//!
//! Formula (action plan, H07):
//!
//! > Chains of 3 or more commits by one author, each within 30 min of the last, touching
//! > overlapping files, subject matching `(?i)\bfix`, with a failing CI run between them.
//!
//! A person who commits `fix`, waits for CI, commits `fix again`, and repeats five times in
//! forty minutes is not developing: they are steering generated code until the tests go green.
//! The four conditions together are what make the signal mean something — a fast sequence of
//! commits on unrelated files is just fast typing, and a chain with no CI history is only a
//! quirk of workflow.
//!
//! The CI half needs forge workflow runs. Without them a chain cannot be established, so the
//! check answers `not_measured` with that reason rather than counting chains the formula would
//! not accept.
//!
//! The threshold is `history.fix_chains`.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// How many seconds may separate two commits of one chain: 30 minutes.
pub const CHAIN_GAP_SECONDS: i64 = 30 * 60;

/// The shortest run of commits that is a chain.
pub const CHAIN_MIN_COMMITS: usize = 3;

/// One commit as the chain walker sees it.
struct Candidate<'a> {
    author: &'a str,
    at: i64,
    files: BTreeSet<String>,
    sha: &'a str,
}

/// Judges H07.
#[must_use]
pub fn fix_chains(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => {
            let mut metrics = BTreeMap::new();
            metrics.insert("chains".to_owned(), 0);
            return Judgment::not_measured(
                format!("a chain needs a failing CI run between its commits: {reason}"),
                metrics,
            );
        }
    };

    // When CI failed, so a chain can ask whether a failure fell between two of its commits.
    let mut failed_runs: Vec<i64> = forge
        .workflow_runs
        .iter()
        .filter(|run| matches!(run.conclusion.as_deref(), Some("failure" | "timed_out")))
        .filter_map(|run| run.created_at)
        .map(|at| at.as_offset_date_time().unix_timestamp())
        .collect();
    failed_runs.sort_unstable();

    // Oldest first, so a chain is read in the order it happened. Commits that do not match
    // `fix` are left out entirely: they can neither join nor extend a chain.
    let mut candidates: Vec<Candidate<'_>> = input
        .commits
        .iter()
        .filter(|commit| !commit.is_merge() && !crate::git::is_bot_commit(&commit.author))
        .filter(|commit| subject_matches_fix(&commit.subject))
        .map(|commit| {
            let mut files: BTreeSet<String> = BTreeSet::new();
            for file in &commit.files {
                files.insert(file.path.clone());
                if let Some(old) = &file.old_path {
                    files.insert(old.clone());
                }
            }
            Candidate {
                author: commit.author.name.as_str(),
                at: commit.author.at.as_offset_date_time().unix_timestamp(),
                files,
                sha: commit.sha.as_str(),
            }
        })
        .collect();
    candidates.sort_by(|left, right| left.at.cmp(&right.at).then_with(|| left.sha.cmp(right.sha)));

    // Group the candidates into maximal chains: each commit continues the current chain when
    // the author matches, the gap is at most 30 minutes and the files overlap.
    let mut chains: Vec<Vec<&Candidate<'_>>> = Vec::new();
    for candidate in &candidates {
        let continues = chains
            .last()
            .and_then(|chain| chain.last())
            .is_some_and(|last| {
                last.author == candidate.author
                    && candidate.at - last.at <= CHAIN_GAP_SECONDS
                    && overlaps(&last.files, &candidate.files)
            });
        if continues {
            if let Some(chain) = chains.last_mut() {
                chain.push(candidate);
            }
        } else {
            chains.push(vec![candidate]);
        }
    }

    let mut chains_found = 0u64;
    let mut chains_with_failing_ci = 0u64;
    let mut chain_commits = 0u64;
    let mut contributions: Vec<(String, u64)> = Vec::new();
    for chain in &chains {
        if chain.len() < CHAIN_MIN_COMMITS {
            continue;
        }
        chains_found += 1;
        chain_commits += u64::try_from(chain.len()).unwrap_or(0);
        let first = chain.first().map(|candidate| candidate.at).unwrap_or(0);
        let last = chain.last().map(|candidate| candidate.at).unwrap_or(0);
        // A failing run strictly after the first commit and before the last: CI had to say
        // "no" at some point inside the burst, which is what makes it steering rather than
        // a series of quick typo fixes.
        let failing_between = failed_runs.iter().any(|at| *at > first && *at < last);
        if !failing_between {
            continue;
        }
        chains_with_failing_ci += 1;
        let weight = u64::try_from(chain.len()).unwrap_or(0);
        for candidate in chain {
            contributions.push((candidate.sha.to_owned(), weight));
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "chains".to_owned(),
        i64::try_from(chains_found).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "chains_with_failing_ci".to_owned(),
        i64::try_from(chains_with_failing_ci).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "chain_commits".to_owned(),
        i64::try_from(chain_commits).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "failed_runs".to_owned(),
        i64::try_from(failed_runs.len()).unwrap_or(i64::MAX),
    );

    // The formula's own verdict rests on chains that satisfy every condition, including the
    // CI failure; the unqualified count is reported beside it so the two numbers can be read
    // together.
    let threshold = thresholds.history().fix_chains;
    let exceeded = match threshold {
        Some(threshold) => chains_with_failing_ci >= u64::from(threshold),
        None => false,
    };
    Judgment::measured(metrics)
        .with_evidence(super::top_commits(contributions))
        .with_note(
            "a chain is three or more \"fix\" commits by one author, each within 30 minutes of \
             the last, touching an overlapping file, with a failing workflow run between the \
             chain's first and last commit"
                .to_owned(),
        )
        .judge(threshold, exceeded)
}

/// Whether a subject matches the formula's `(?i)\bfix`: the word `fix` anywhere, in any case.
fn subject_matches_fix(subject: &str) -> bool {
    subject
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word == "fix")
}

/// Whether two commits' file sets overlap.
fn overlaps(left: &BTreeSet<String>, right: &BTreeSet<String>) -> bool {
    left.iter().any(|path| right.contains(path))
}
