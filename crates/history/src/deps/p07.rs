//! P07 Dependency vitality (M1-T10).
//!
//! Formula (action plan, P07):
//!
//! > Forge-only history (H02, H11, H13, plus the archetype without blame) for each direct
//! > dependency's GitHub repo; first 50 by name, with coverage recorded. Abandoned = no commit
//! > or release in 365 days.
//!
//! A dependency on an abandoned package is a dependency nobody will fix. The evidence is the
//! package's own repository, read through the forge: its last commit and its last release.
//!
//! ## Why the checks are "forge-only"
//!
//! H02, H11 and H13 were written for the project under evaluation, where substance has the git
//! history and the blame. A dependency's repository is a different shape: it is not checked out,
//! there is no git engine pointed at it, and cloning fifty repositories to answer one question
//! would be absurd. So this check reads only what the forge API reports — commit activity, the
//! contributors list, the releases — which is exactly the subset those three checks need, and
//! the plan's phrase for it. The archetype's contribution is the same: what a forge can tell
//! about a repository's shape without a blame.
//!
//! ## Coverage
//!
//! Fifty repositories by name is the plan's bound, and it is a request budget as much as a
//! sample: each repository costs several requests against `forge_max_requests`. The check
//! records how many it read and how many dependencies have no GitHub repository to read at all,
//! so a reader can see whether the vitality picture covers the dependency list or a corner of
//! it. A dependency whose repository is not on GitHub is not abandoned; it is unmeasured, and
//! the note says so.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::check::CheckId;
use substance_core::deps::Ecosystem;
use substance_core::model::Verdict;
use substance_core::thresholds::Thresholds;

use crate::deps::known::Known;
use crate::deps::world::World;
use crate::forge::{Forge, ForgeError};
use crate::signals::shape::Judgment;

/// How many dependencies' repositories the check reads, by name order.
///
/// The plan's "first 50 by name". Sorted by `(ecosystem, name)` so the same fifty are read on
/// every run: taking the first fifty in whatever order the lockfile listed them would make two
/// runs disagree about which dependencies were measured.
pub const MAX_REPOS: usize = 50;

/// The reason recorded for a dependency whose registry metadata names no repository.
pub const NO_REPOSITORY: &str = "the package declares no source repository";

/// How long without a commit or release makes a dependency abandoned, in days.
pub const ABANDONED_DAYS: u32 = 365;

/// What the forge said about one dependency's repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoVitality {
    /// The dependency this repository belongs to.
    pub ecosystem: Ecosystem,
    /// The package name.
    pub package: String,
    /// The repository slug, `owner/name`.
    pub slug: String,
    /// The newest commit the forge reported, when it reported one.
    pub last_commit_at: Option<Timestamp>,
    /// The newest release, when the package has one.
    pub last_release_at: Option<Timestamp>,
    /// How many distinct human contributors the forge listed.
    pub maintainers: u32,
    /// Whether the repository is archived, which is the forge's own "abandoned" flag.
    pub archived: bool,
}

impl RepoVitality {
    /// The newest activity the repository shows: a commit or a release.
    #[must_use]
    pub fn last_activity(&self) -> Option<Timestamp> {
        match (self.last_commit_at, self.last_release_at) {
            (Some(commit), Some(release)) => Some(commit.max(release)),
            (Some(commit), None) => Some(commit),
            (None, release) => release,
        }
    }
}

/// Judges P07.
///
/// `repos` is what the forge reported for the dependencies it was asked about; `skipped` names
/// the dependencies that have no GitHub repository, with the reason. `as_of` is the run's
/// instant: "no commit in 365 days" is measured from the evaluation date, never a wall clock.
#[must_use]
pub fn vitality(
    world: &World,
    _known: &Known,
    repos: &[RepoVitality],
    skipped: &[(String, String)],
    as_of: Timestamp,
    budget_exhausted: bool,
    _thresholds: &Thresholds,
) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut findings: Vec<(String, String)> = Vec::new();
    let mut abandoned = 0i64;
    let mut archived = 0i64;
    let mut unmeasured = 0i64;

    let cutoff = substance_core::clock::window_start(as_of, ABANDONED_DAYS);
    for repo in repos {
        if repo.archived {
            archived += 1;
            abandoned += 1;
            findings.push((
                repo.slug.clone(),
                format!(
                    "{} depends on {} ({}), whose repository is archived",
                    repo.package, repo.package, repo.slug
                ),
            ));
            continue;
        }
        match repo.last_activity() {
            Some(last) if last < cutoff => {
                abandoned += 1;
                let days = (as_of.as_offset_date_time() - last.as_offset_date_time())
                    .whole_days()
                    .max(0);
                findings.push((
                    repo.slug.clone(),
                    format!(
                        "{} depends on {} ({}), which has had no commit or release in {days} days",
                        repo.package, repo.package, repo.slug
                    ),
                ));
            }
            Some(_) => {}
            None => {
                unmeasured += 1;
                notes.push(format!(
                    "{} ({}): the forge reported neither a commit nor a release date",
                    repo.package, repo.slug
                ));
            }
        }
    }

    findings.sort();
    metrics.insert("direct_dependencies".to_owned(), as_i64(world.direct.len()));
    metrics.insert("repositories_read".to_owned(), as_i64(repos.len()));
    metrics.insert("abandoned".to_owned(), abandoned);
    metrics.insert("archived".to_owned(), archived);
    metrics.insert("no_repository".to_owned(), as_i64(skipped.len()));
    metrics.insert("undated".to_owned(), unmeasured);
    // Coverage, in the plan's terms: how much of the dependency list the vitality picture
    // actually covers, and why the rest is missing.
    metrics.insert("deps_total".to_owned(), as_i64(world.direct.len()));
    metrics.insert(
        "deps_with_github".to_owned(),
        as_i64(
            repos.len()
                + skipped
                    .iter()
                    .filter(|(_, reason)| reason != NO_REPOSITORY)
                    .count(),
        ),
    );
    metrics.insert("deps_examined".to_owned(), as_i64(repos.len()));
    metrics.insert("deps_skipped".to_owned(), as_i64(skipped.len()));

    if budget_exhausted {
        // A run that stopped at the request cap is not a clean dependency list: whatever was
        // not reached is unmeasured, and saying so is the only honest answer.
        let mut judgment = Judgment::not_measured(
            "budget exhausted: the forge request budget ran out before every dependency's \
             repository could be read",
            metrics,
        );
        judgment.evidence = findings
            .iter()
            .map(|(slug, _text)| {
                substance_core::model::Evidence::ForgeObject(substance_core::model::ForgeObject {
                    kind: "repository".to_owned(),
                    id: slug.clone(),
                    url: format!("https://github.com/{slug}"),
                })
            })
            .collect();
        judgment.notes = notes;
        return judgment;
    }

    if repos.is_empty() {
        let reason = skipped.first().map_or_else(
            || "no dependency names a GitHub repository, and P07 reads nothing else".to_owned(),
            |(package, reason)| {
                format!("{package}: {reason}, and no other dependency names a GitHub repository")
            },
        );
        return Judgment::not_measured(reason, metrics);
    }

    let mut judgment = Judgment::measured(metrics);
    judgment.verdict = if findings.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    judgment.evidence = findings
        .iter()
        .map(|(slug, _text)| {
            substance_core::model::Evidence::ForgeObject(substance_core::model::ForgeObject {
                kind: "repository".to_owned(),
                id: slug.clone(),
                url: format!("https://github.com/{slug}"),
            })
        })
        .collect();
    // Coverage is the plan's own requirement, so it travels as a note rather than as a comment.
    if world.direct.len() > MAX_REPOS {
        notes.push(format!(
            "read {} of {} dependencies' repositories: the check reads the first {MAX_REPOS} by \
             name",
            repos.len(),
            world.direct.len()
        ));
    }
    if !skipped.is_empty() {
        notes.push(format!(
            "{} dependencies have no GitHub repository the forge could read, so their vitality is \
             unmeasured rather than good",
            skipped.len()
        ));
    }
    if unmeasured > 0 {
        notes.push(format!(
            "{unmeasured} repositories reported no activity date; they are neither abandoned nor \
             active as far as this check can tell"
        ));
    }
    judgment.notes = notes;
    judgment
}

/// The dependencies whose repository P07 should read: the first [`MAX_REPOS`] by name.
///
/// Kept here so the geometry of the sample is visible beside the bound that sets it, and so a
/// caller cannot page the list differently and call the result the same check.
#[must_use]
pub fn sample(world: &World) -> Vec<(Ecosystem, String)> {
    let mut keys: Vec<(Ecosystem, String)> = world
        .direct
        .iter()
        .map(|dep| (dep.ecosystem, dep.name.clone()))
        .collect();
    keys.sort();
    keys.dedup();
    keys.truncate(MAX_REPOS);
    keys
}

/// The registry a package's repository is likely to be found through.
///
/// A package with no `repository` field has no forge repository to read; the check says so
/// rather than guessing a URL from the package name, which would point at a repository that may
/// belong to someone else entirely.
#[must_use]
pub fn repository_of(known: &Known, ecosystem: Ecosystem, package: &str) -> Option<String> {
    known
        .package(ecosystem, package)
        .and_then(|known| known.repository.clone())
}

/// `usize` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The check id, for a caller that names it.
#[must_use]
pub const fn check_id() -> CheckId {
    CheckId::P07
}

/// What P07's gathering produced.
#[derive(Debug, Clone, Default)]
pub struct VitalityResult {
    /// One record per dependency repository the forge answered for.
    pub repos: Vec<RepoVitality>,
    /// The dependencies that were not read, with the reason, sorted by package.
    pub skipped: Vec<(String, String)>,
    /// Whether the request budget stopped the walk.
    pub budget_exhausted: bool,
    /// Why nothing was read at all, when that is the case.
    pub unavailable: Option<String>,
}

impl VitalityResult {
    /// The absence, with the reason P07 reports.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            unavailable: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// Reads each direct dependency's own repository, for P07.
///
/// The sample is [`MAX_REPOS`] by name, so the same fifty dependencies are read on every run.
/// Per repository, four requests are the most it can cost, and each answers one of the signals
/// the plan names:
///
/// | Request | What it answers | Which signal |
/// | --- | --- | --- |
/// | `GET /repos/{o}/{r}` | `created_at`, `pushed_at`, `archived`, licence | H02's activity, H13's posture |
/// | `GET /repos/{o}/{r}/commits?per_page=1` | the newest commit's date | H02's commit activity, the abandoned test |
/// | `GET /repos/{o}/{r}/releases` | the newest stable release | H13's release cadence, the abandoned test |
/// | `GET /repos/{o}/{r}/contributors` | how many people work on it | H11's maintainers |
///
/// That is the plan's "forge-only history (H02, H11, H13, plus the archetype without blame)":
/// every one of those signals is a question the forge API answers about a repository that is
/// not checked out, and none needs a blame — which is the part that would require a clone.
///
/// A budget exhaustion stops the walk where it stands: the dependencies already read keep
/// their records and the judge reports `not_measured("budget exhausted: …")`, because a
/// vitality picture covering half the list must not read as a clean one.
pub async fn gather(
    world: &World,
    known: &Known,
    http: &crate::snapshot::SnapshotHttp,
    as_of: Timestamp,
) -> VitalityResult {
    let mut result = VitalityResult::default();
    for (ecosystem, package) in sample(world) {
        // The repository URL comes from the registry facts: a package that declares none has no
        // forge repository to read, and guessing one from the package name would point at a
        // repository that may belong to someone else entirely.
        let Some(url) = known
            .package(ecosystem, &package)
            .and_then(|known| known.repository.clone())
        else {
            result.skipped.push((package, NO_REPOSITORY.to_owned()));
            continue;
        };
        let repo_ref = match crate::forge::GitHubRepoRef::parse(&url) {
            Ok(repo_ref) => repo_ref,
            Err(error) => {
                // A GitLab, Bitbucket or self-hosted repository is not something this build
                // reads: the answer is the reason, not a wrong repository.
                result.skipped.push((package, error.to_string()));
                continue;
            }
        };
        // The clone is a second handle onto the same `forge_max_requests` budget, not a
        // second budget: `SnapshotHttp` keeps its counter behind a shared atomic.
        let mut client = crate::forge::GitHub::new(repo_ref.clone(), http.clone(), None, as_of);
        let info = match client.repo().await {
            Ok(info) => info,
            Err(ForgeError::Budget { .. }) => return out_of_budget(result, package),
            Err(error) => {
                result.skipped.push((package, error.to_string()));
                continue;
            }
        };
        let last_commit = match client.last_commit().await {
            Ok(when) => when,
            Err(ForgeError::Budget { .. }) => return out_of_budget(result, package),
            // One failed request is that signal's absence, not the repository's: the record is
            // still worth keeping for the signals that did answer. `pushed_at` is the same
            // repository document's answer, so it stands in when the listing could not be read.
            Err(_) => info.pushed_at,
        };
        let last_release = match client.releases().await {
            Ok(releases) => releases
                .iter()
                .filter(|release| !release.draft && !release.prerelease)
                .filter_map(|release| release.created_at)
                .max(),
            Err(ForgeError::Budget { .. }) => return out_of_budget(result, package),
            Err(_) => None,
        };
        let maintainers = match client.contributors().await {
            Ok(contributors) => u32::try_from(
                contributors
                    .iter()
                    .filter(|contributor| !contributor.is_bot())
                    .count(),
            )
            .unwrap_or(u32::MAX),
            Err(ForgeError::Budget { .. }) => return out_of_budget(result, package),
            Err(_) => 0,
        };
        result.repos.push(RepoVitality {
            ecosystem,
            package,
            slug: repo_ref.slug(),
            last_commit_at: last_commit,
            last_release_at: last_release,
            maintainers,
            archived: info.archived,
        });
    }
    result.skipped.sort();
    result.repos.sort_by(|left, right| {
        left.ecosystem
            .cmp(&right.ecosystem)
            .then_with(|| left.package.cmp(&right.package))
    });
    result
}

/// Records that the budget stopped the walk at `package`, and returns what was read.
fn out_of_budget(mut result: VitalityResult, package: String) -> VitalityResult {
    result.budget_exhausted = true;
    result
        .skipped
        .push((package, "budget exhausted".to_owned()));
    result.skipped.sort();
    result.repos.sort_by(|left, right| {
        left.ecosystem
            .cmp(&right.ecosystem)
            .then_with(|| left.package.cmp(&right.package))
    });
    result
}
