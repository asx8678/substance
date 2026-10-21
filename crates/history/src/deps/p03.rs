//! P03 Born-stale versions (M1-T10).
//!
//! Formula (action plan, P03):
//!
//! > Per direct dependency: was the version added already a major version behind the latest
//! > release on the day it was added? Fail when the stale share is at least 500‰.
//!
//! The question is deliberately about the **day it was added**, not about today. A project
//! that adopted React 17 in 2021 and is still on it was not stale then; a project that adopted
//! React 15 in 2021 was, because 17 was already out. The difference between the two is the
//! entire signal: one is a project that has not upgraded, the other never started current.
//!
//! Two inputs make the comparison possible, and both are snapped to one instant:
//!
//! - the adoption date, from git, through [`crate::dates`];
//! - the registry's releases and their dates, from [`crate::deps::known`] — every release, not
//!   the newest 500, which is what makes the answer right for an old dependency.
//!
//! `latest_by` answers "the newest release that existed on that day", and the comparison is on
//! major numbers only: a project one minor behind on the day it adopted is normal, one major
//! behind is the finding.
//!
//! The share is permille over the dependencies that could be compared. A dependency whose
//! registry had no dated release before the adoption is excluded from the denominator and
//! counted separately, so the share never rests on a package this build could not date.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::deps::Ecosystem;
use substance_core::thresholds::Thresholds;

use crate::deps::known::{Known, KnownError};
use crate::deps::version::{VersionCore, major_behind};
use crate::deps::world::World;
use crate::signals::shape::Judgment;

/// Judges P03.
///
/// `added_at` maps a dependency to the instant the project first declared it. A dependency
/// with no recorded date cannot be judged and is counted as undated rather than as current.
#[must_use]
pub fn born_stale(
    world: &World,
    known: &Known,
    added_at: &BTreeMap<(Ecosystem, String), Timestamp>,
    thresholds: &Thresholds,
) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut findings: Vec<(Ecosystem, String, String, String)> = Vec::new();
    let mut compared = 0i64;
    let mut stale = 0i64;
    let mut undated = 0i64;
    let mut unreadable = 0i64;
    let mut unknown = 0i64;
    let mut behind_total = 0i64;

    for dep in &world.direct {
        // A dev dependency is still a dependency the project carries, and a stale test runner
        // is the same finding as a stale HTTP client, so both are judged.
        let Some(adopted) = added_at.get(&dep.key()) else {
            undated += 1;
            continue;
        };
        let package = match known.get(dep.ecosystem, &dep.name) {
            Some(Ok(_)) => match known.package(dep.ecosystem, &dep.name) {
                Some(package) => package,
                None => continue,
            },
            Some(Err(KnownError::NotFound)) | None => {
                unknown += 1;
                continue;
            }
            Some(Err(KnownError::Unavailable { .. })) => {
                unknown += 1;
                continue;
            }
        };
        let Some(adopted_core) = VersionCore::parse(&dep.version) else {
            // A lockfile that records a git reference or a tag spelling this build cannot read
            // has no major number to compare. Saying so is better than comparing zero.
            unreadable += 1;
            notes.push(format!(
                "{} {}: \"{}\" is not a version this build can compare",
                dep.ecosystem.as_str(),
                dep.name,
                dep.version
            ));
            continue;
        };
        let Some(latest) = package.latest_major_by(*adopted) else {
            unknown += 1;
            continue;
        };
        compared += 1;
        if latest > adopted_core.major {
            stale += 1;
            behind_total += i64::try_from(latest - adopted_core.major).unwrap_or(i64::MAX);
            let newest = package
                .latest_by(*adopted)
                .map(|known| known.version.clone())
                .unwrap_or_default();
            findings.push((
                dep.ecosystem,
                dep.name.clone(),
                dep.version.clone(),
                format!(
                    "{} {} was added when {newest} was already released",
                    dep.name, dep.version
                ),
            ));
        }
    }

    findings.sort();
    metrics.insert("direct_dependencies".to_owned(), as_i64(world.direct.len()));
    metrics.insert("compared".to_owned(), compared);
    metrics.insert("stale_at_adoption".to_owned(), stale);
    metrics.insert("majors_behind_total".to_owned(), behind_total);
    metrics.insert("undated".to_owned(), undated);
    metrics.insert("no_registry_date".to_owned(), unknown);
    metrics.insert("unreadable_version".to_owned(), unreadable);

    if world.direct.is_empty() {
        return Judgment::not_measured(
            "the project declares no direct dependency, so none was born stale",
            metrics,
        );
    }
    if compared == 0 {
        return Judgment::not_measured(
            "no dependency had both an adoption date and a dated release to compare against, \
             so the stale share is not measurable",
            metrics,
        );
    }
    // The share, in permille, over the dependencies that could be compared. `share_permille`
    // returns None only when the denominator is zero, which is handled above.
    let share = share_permille(stale, compared).unwrap_or(0);
    metrics.insert("stale_permille".to_owned(), share);
    if undated > 0 {
        notes.push(format!(
            "{undated} dependencies have no adoption date, so the stale share is over the \
             {compared} that could be compared"
        ));
    }
    let mut judgment = Judgment::measured(metrics);
    let floor = thresholds.history().p03_stale_permille;
    judgment = judgment.judge(floor, floor.is_some_and(|floor| share >= i64::from(floor)));
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

/// `part` as a share of `whole`, in permille, or `None` when `whole` is zero.
///
/// The same rule the history signals use (see [`crate::signals::shape`]): a ratio with no
/// denominator is not zero, it is unmeasurable, and the caller must say so.
fn share_permille(part: i64, whole: i64) -> Option<i64> {
    if whole <= 0 {
        return None;
    }
    Some(part.saturating_mul(1000) / whole)
}

/// `usize` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The major number `text` names, if any, for a caller that only wants that.
#[must_use]
pub fn major_of(text: &str) -> Option<u64> {
    VersionCore::major_of(text)
}

/// How many major versions `candidate` was behind `latest`, if both parse.
#[must_use]
pub fn behind(candidate: &str, latest: &str) -> Option<u64> {
    major_behind(candidate, latest)
}
