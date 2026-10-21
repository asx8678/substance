//! P02 Dependency reputation (M1-T10).
//!
//! Formula (action plan, P02):
//!
//! > Package not found in its registry; first published less than 30 days before the project
//! > added it; no source repo; downloads and version count below thresholds.
//!
//! Four signals, all about whether a package looks like a real project or like something
//! created to be depended on. Each is judged on its own and each carries its own finding,
//! because they mean different things:
//!
//! - **not found** is the strongest: the registry the lockfile names has never heard of the
//!   package. A lockfile can name a package that was deleted, or one that never existed.
//! - **first published days before adoption** is the typosquat and the slop-package signal. The
//!   date comes from the registry; the adoption date comes from git, through
//!   [`crate::dates`], because the manifest's own commit is when the project took it on.
//! - **no source repository** is a weaker hint: plenty of honest packages live inside a
//!   monorepo without a `repository` field, so the finding is a `hint`, not a `confirmed` one.
//! - **downloads and version count below thresholds** is the popularity signal. A package with
//!   three versions and forty downloads a month is either new or nobody's.
//!
//! A missing threshold means the metric is reported, not judged (ADR-0013), which is why the
//! four thresholds are looked up separately instead of one verdict for the whole check.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::deps::Ecosystem;
use substance_core::model::Verdict;
use substance_core::thresholds::Thresholds;

use crate::deps::known::{Known, KnownError, KnownPackage};
use crate::deps::world::{Direct, World};
use crate::signals::shape::Judgment;

/// How young a package may be, in days, before its adoption is suspicious.
///
/// The plan's own number. A package first published a month before a project adopted it has
/// had no time to be reviewed by anyone, which is the whole point of the signal.
pub const MIN_AGE_DAYS: u32 = 30;

/// Judges P02.
///
/// `added_at` maps a dependency to the instant the project first declared it, from
/// [`crate::dates::walks`]. A package with no recorded adoption date simply skips the
/// age signal: the other three still run, and the note says which one could not.
#[must_use]
pub fn reputation(
    world: &World,
    known: &Known,
    added_at: &BTreeMap<(Ecosystem, String), Timestamp>,
    thresholds: &Thresholds,
) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut findings: Vec<(Ecosystem, String, String, String)> = Vec::new();
    let mut missing = 0i64;
    let mut young = 0i64;
    let mut no_repo = 0i64;
    let mut unpopular = 0i64;
    let mut checked = 0i64;
    let mut undated = 0i64;

    let min_downloads = thresholds.history().p02_min_monthly_downloads;
    let min_versions = thresholds.history().p02_min_versions;

    for dep in &world.direct {
        match known.get(dep.ecosystem, &dep.name) {
            Some(Ok(_package)) => checked += 1,
            Some(Err(KnownError::NotFound)) => {
                missing += 1;
                findings.push((
                    dep.ecosystem,
                    dep.name.clone(),
                    dep.version.clone(),
                    format!(
                        "{} is not published on {}",
                        dep.name,
                        dep.ecosystem.as_str()
                    ),
                ));
                continue;
            }
            _ => continue,
        }
        let package = match known.package(dep.ecosystem, &dep.name) {
            Some(package) => package,
            None => continue,
        };
        young_or_old(
            package,
            dep,
            added_at,
            &mut young,
            &mut undated,
            &mut findings,
        );
        if package.repository.is_none() {
            no_repo += 1;
            findings.push((
                dep.ecosystem,
                dep.name.clone(),
                dep.version.clone(),
                format!(
                    "{} declares no source repository on {}",
                    dep.name,
                    dep.ecosystem.as_str()
                ),
            ));
        }
        // Downloads and version count are separate thresholds, and a registry that reports
        // neither (PyPI) leaves both unjudged rather than accused.
        let too_few_versions =
            min_versions.is_some_and(|floor| versions_below(package.total_versions, floor));
        let too_few_downloads = match (min_downloads, package.downloads) {
            (Some(floor), Some(downloads)) => downloads < u64::from(floor),
            _ => false,
        };
        if too_few_versions || too_few_downloads {
            unpopular += 1;
            findings.push((
                dep.ecosystem,
                dep.name.clone(),
                dep.version.clone(),
                describe_unpopular(package, min_versions, min_downloads),
            ));
        }
    }

    findings.sort();
    metrics.insert("direct_dependencies".to_owned(), as_i64(world.direct.len()));
    metrics.insert("packages_checked".to_owned(), checked);
    metrics.insert("not_found".to_owned(), missing);
    metrics.insert("young_at_adoption".to_owned(), young);
    metrics.insert("no_source_repository".to_owned(), no_repo);
    metrics.insert("below_thresholds".to_owned(), unpopular);
    metrics.insert("undated".to_owned(), undated);

    if world.direct.is_empty() {
        return Judgment::not_measured(
            "the project declares no direct dependency, so there is no reputation to check",
            metrics,
        );
    }
    if checked == 0 && missing == 0 {
        return Judgment::not_measured(
            "no package document could be read, so nothing about these dependencies is known",
            metrics,
        );
    }
    if undated > 0 {
        notes.push(format!(
            "{undated} dependencies have no recorded adoption date, so the \"published days \
             before adoption\" signal did not run for them"
        ));
    }
    let mut judgment = Judgment::measured(metrics);
    judgment.verdict = if findings.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    judgment.evidence = findings
        .iter()
        .map(|(ecosystem, name, version, _text)| {
            substance_core::model::Evidence::RegistryObject(substance_core::model::RegistryObject {
                registry: ecosystem.as_str().to_owned(),
                package: name.clone(),
                version: version.clone(),
            })
        })
        .collect();
    judgment.notes = notes;
    judgment
}

/// The `first published days before adoption` signal for one package.
fn young_or_old(
    package: &KnownPackage,
    dep: &Direct,
    added_at: &BTreeMap<(Ecosystem, String), Timestamp>,
    young: &mut i64,
    undated: &mut i64,
    findings: &mut Vec<(Ecosystem, String, String, String)>,
) {
    let Some(adopted) = added_at.get(&dep.key()) else {
        *undated += 1;
        return;
    };
    let Some(published) = package.first_published else {
        *undated += 1;
        return;
    };
    // Days between the first publish and the adoption, floored at zero: a package published
    // after the project adopted it is a strange but real case (a rewrite, a republish), and a
    // negative age would read as "long established".
    let days = (adopted.as_offset_date_time() - published.as_offset_date_time())
        .whole_days()
        .max(0);
    if days < i64::from(MIN_AGE_DAYS) {
        *young += 1;
        findings.push((
            dep.ecosystem,
            dep.name.clone(),
            dep.version.clone(),
            format!(
                "{} was first published {days} days before this project adopted it",
                dep.name
            ),
        ));
    }
}

/// The sentence for a package below the popularity thresholds.
fn describe_unpopular(
    package: &KnownPackage,
    min_versions: Option<u32>,
    min_downloads: Option<u32>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if min_versions.is_some_and(|floor| versions_below(package.total_versions, floor)) {
        parts.push(format!("{} versions", package.total_versions));
    }
    if min_downloads.is_some_and(|floor| {
        package
            .downloads
            .is_some_and(|downloads| downloads < u64::from(floor))
    }) {
        let downloads = package.downloads.unwrap_or(0);
        parts.push(format!("{downloads} downloads"));
    }
    if parts.is_empty() {
        parts.push("below a threshold".to_owned());
    }
    format!("{} has {}", package.name, parts.join(" and "))
}

/// Whether a version count is below `floor`.
///
/// A count too large for `u32` is not below any `u32` floor, so the conversion's failure means
/// "no" rather than an error: a package with billions of versions is not too new.
fn versions_below(total: usize, floor: u32) -> bool {
    u32::try_from(total).is_ok_and(|count| count < floor)
}

/// `usize` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
