//! H16 Claims vs. history: what the README says against what the log shows.
//!
//! Formula (action plan, H16):
//!
//! > CHANGELOG versions (`## [x.y.z]` or `## x.y.z`) with no tag or release; README badges
//! > pointing at workflow files that don't exist, or static `img.shields.io/badge/` images
//! > claiming build or coverage; "production-ready", "battle-tested" or "enterprise-grade"
//! > with active weeks below threshold.
//!
//! Three claims, each checkable against something the project already has. A CHANGELOG entry
//! with no tag and no release describes a version nobody can install. A badge that points at
//! a workflow file that does not exist, or a shields.io image that is just a picture, claims
//! a build status that nothing verified. "Production-ready" on a repository active for two
//! weeks is a sentence about an intention.
//!
//! The README has to be read for this check, so [`HistoryInput`] carries its text. A repository
//! with no README cannot be checked for claims about it, which is `not_measured` rather than a
//! pass: nothing was found because nothing was looked at.
//!
//! The sentences are matched case-insensitively as whole phrases. The threshold is
//! `history.claims_min_active_weeks`: below it, a claim is unsupported.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// The words that claim a project is finished, lowercased.
pub const CLAIMS: &[&str] = &[
    "production-ready",
    "production ready",
    "battle-tested",
    "battle tested",
    "enterprise-grade",
    "enterprise grade",
];

/// The badge markers that claim a build or coverage status.
pub const BADGE_CLAIMS: &[&str] = &["build", "coverage", "tests", "ci"];

/// Judges H16.
#[must_use]
pub fn claims_vs_history(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let Some(readme) = input.readme.as_deref() else {
        return Judgment::not_measured(
            "the repository has no README at HEAD, so it makes no claims to check",
            BTreeMap::new(),
        );
    };
    let lower = readme.to_lowercase();
    let forge = input.forge.as_ref();

    // 1. CHANGELOG versions with no tag and no release.
    let mut changelog_versions: Vec<String> = Vec::new();
    if let Some(changelog) = input.changelog.as_deref() {
        for line in changelog.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("## ") else {
                continue;
            };
            let version = rest
                .trim()
                .strip_prefix('[')
                .and_then(|inner| inner.split(']').next());
            let version = match version {
                Some(version) => version.trim(),
                // `## 1.2.3` — a version with no link.
                None => rest.split_whitespace().next().unwrap_or_default(),
            };
            if version.is_empty() || !version.starts_with(|ch: char| ch.is_ascii_digit()) {
                continue;
            }
            changelog_versions.push(version.trim_start_matches('v').to_owned());
        }
    }
    changelog_versions.sort();
    changelog_versions.dedup();
    let mut tagged: Vec<String> = Vec::new();
    let mut released: Vec<String> = Vec::new();
    if let Some(forge) = forge {
        tagged = forge
            .tags
            .iter()
            .map(|tag| tag.name.trim_start_matches('v').to_owned())
            .collect();
        released = forge
            .releases
            .iter()
            .map(|release| release.tag_name.trim_start_matches('v').to_owned())
            .collect();
    }
    let untagged: Vec<&String> = changelog_versions
        .iter()
        .filter(|version| {
            !tagged.iter().any(|tag| tag == *version) && !released.iter().any(|tag| tag == *version)
        })
        .collect();

    // 2. Badges: a workflow badge that names a file the repository does not have, or a
    //    static shields.io image claiming a build or coverage status.
    let workflows: Vec<String> = input
        .workflows
        .iter()
        .map(|path| path.to_lowercase())
        .collect();
    let mut missing_workflow_badges = 0u64;
    let mut static_badges = 0u64;
    for token in readme.split(|ch: char| ch.is_whitespace() || ch == ')' || ch == '"' || ch == '\'')
    {
        let token = token.trim();
        if !token.contains("github.com/") || !token.contains("/actions/workflows/") {
            if token.contains("img.shields.io/badge/") {
                let lowered = token.to_lowercase();
                if BADGE_CLAIMS.iter().any(|claim| lowered.contains(claim)) {
                    static_badges += 1;
                }
            }
            continue;
        }
        let Some((_, tail)) = token.split_once("/actions/workflows/") else {
            continue;
        };
        let file = tail
            .split(['?', '#', ')', '"'])
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if file.is_empty() {
            continue;
        }
        // A badge may name `.yml` or `.yaml`, and the file may be either: compare stems.
        let stem = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&file);
        let exists = workflows.iter().any(|workflow| {
            let name = workflow.rsplit('/').next().unwrap_or(workflow);
            let name_stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
            name_stem == stem
        });
        if !exists {
            missing_workflow_badges += 1;
        }
    }

    // 3. A finished-sounding claim on a young history.
    let claims: Vec<&str> = CLAIMS
        .iter()
        .copied()
        .filter(|claim| lower.contains(claim))
        .collect();
    // Active weeks over the repository's life, which is the number the claim has to survive.
    let active_weeks = super::h02::active_weeks_all_time(input);

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "changelog_versions".to_owned(),
        i64::try_from(changelog_versions.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "changelog_versions_untagged".to_owned(),
        i64::try_from(untagged.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "missing_workflow_badges".to_owned(),
        i64::try_from(missing_workflow_badges).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "static_badges".to_owned(),
        i64::try_from(static_badges).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "claims".to_owned(),
        i64::try_from(claims.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "active_weeks_all_time".to_owned(),
        i64::try_from(active_weeks).unwrap_or(i64::MAX),
    );

    let mut judgment = Judgment::measured(metrics);
    if forge.is_none() {
        judgment = judgment.with_note(
            "tags and releases are not known, so a CHANGELOG version cannot be called \
             untagged; every other part of the check still ran"
                .to_owned(),
        );
    }
    if !untagged.is_empty() {
        judgment = judgment.with_note(format!(
            "CHANGELOG versions with no tag or release: {}",
            untagged
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !claims.is_empty() {
        judgment = judgment.with_note(format!("README claims: {}", claims.join(", ")));
    }

    // The verdict rests on the unsupported claim: a finished-sounding sentence on a history
    // with fewer active weeks than the threshold allows. The other two numbers are reported
    // beside it, because whether an untagged CHANGELOG entry is a finding is a judgement the
    // report can show without this check asserting it.
    let threshold = thresholds.history().claims_min_active_weeks;
    let unsupported = !claims.is_empty()
        && threshold.is_some_and(|threshold| active_weeks < u64::from(threshold));
    if claims.is_empty() {
        return judgment.with_note(
            "the README makes no finished-sounding claim, so there is nothing to check against \
             the history"
                .to_owned(),
        );
    }
    judgment.judge(threshold, unsupported)
}
