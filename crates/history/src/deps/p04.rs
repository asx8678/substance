//! P04 Deprecated or yanked (M1-T10).
//!
//! Formula (action plan, P04):
//!
//! > npm `deprecated` on the version in use, or a yanked release on PyPI or crates.io.
//!
//! The check is about the version the project actually installs, not about the package: a
//! package whose newest release is deprecated is fine if the project pins an older one that
//! is not, and a package with one yanked release is fine if the project is not on it. So the
//! question is asked once per direct dependency, on the version the lockfile resolved.
//!
//! Three registries answer in two shapes: npm sets a `deprecated` message on a version, PyPI
//! yanks a release (all files yanked, which is why [`crate::registry::PyPiRelease`] only calls a
//! release yanked when every file is), and crates.io sets `yanked` on a version. Both mean the
//! same thing to a project — the release is withdrawn and may not be installable, or installs
//! with a warning — so both produce one finding kind with the registry named in the message.
//!
//! A package the registries could not answer for is not a pass. The verdict counts how many
//! dependencies it could actually look at, and when it could look at none it reports
//! `not_measured` with the reason: a network that failed must not read as a clean bill.

use std::collections::BTreeMap;

use substance_core::deps::Ecosystem;
use substance_core::model::Verdict;
use substance_core::thresholds::Thresholds;

use crate::deps::known::{Known, KnownError};
use crate::deps::world::World;
use crate::signals::shape::Judgment;

/// Judges P04.
///
/// Pure: what the registries said was gathered before judging. The verdict is `fail` when any
/// dependency's version in use is deprecated or yanked, `pass` when every dependency was looked
/// at and none was, and `not_measured` when nothing could be looked at.
#[must_use]
pub fn deprecated_or_yanked(world: &World, known: &Known, _thresholds: &Thresholds) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut checked = 0i64;
    let mut deprecated = 0i64;
    let mut yanked = 0i64;
    let mut not_found = 0i64;
    let mut unavailable = 0i64;
    let mut notes: Vec<String> = Vec::new();
    let mut findings: Vec<(Ecosystem, String, String, String)> = Vec::new();

    for dep in &world.direct {
        match known.get(dep.ecosystem, &dep.name) {
            Some(Ok(package)) => {
                let Some(version) = package.version(&dep.version) else {
                    // The client kept the list, so a missing version means the registry does not
                    // publish the version the lockfile names: an unpublished or rewritten
                    // release. That is worth a note, not a P04 finding — P02 asks whether the
                    // package exists at all, and this one does.
                    notes.push(format!(
                        "{} {}: the registry does not list version {}",
                        dep.ecosystem.as_str(),
                        dep.name,
                        display_version(&dep.version)
                    ));
                    continue;
                };
                checked += 1;
                if let Some(message) = &version.deprecated {
                    deprecated += 1;
                    let text = message.trim();
                    findings.push((
                        dep.ecosystem,
                        dep.name.clone(),
                        version.version.clone(),
                        if text.is_empty() {
                            format!("{} {} is deprecated", dep.name, version.version)
                        } else {
                            format!("{} {} is deprecated: {text}", dep.name, version.version)
                        },
                    ));
                }
                if version.yanked {
                    yanked += 1;
                    findings.push((
                        dep.ecosystem,
                        dep.name.clone(),
                        version.version.clone(),
                        format!(
                            "{} {} was yanked by the {}",
                            dep.name,
                            version.version,
                            dep.ecosystem.as_str()
                        ),
                    ));
                }
            }
            Some(Err(KnownError::NotFound)) => not_found += 1,
            Some(Err(KnownError::Unavailable { .. })) => unavailable += 1,
            None => unavailable += 1,
        }
    }

    findings.sort();
    metrics.insert("direct_dependencies".to_owned(), as_i64(world.direct.len()));
    metrics.insert("versions_checked".to_owned(), checked);
    metrics.insert("deprecated".to_owned(), deprecated);
    metrics.insert("yanked".to_owned(), yanked);
    metrics.insert("not_found".to_owned(), not_found);
    metrics.insert("unavailable".to_owned(), unavailable);

    if world.direct.is_empty() {
        return Judgment::not_measured(
            "the project declares no direct dependency, so nothing is deprecated or yanked",
            metrics,
        );
    }
    if checked == 0 {
        let reason = match known.unavailable().first() {
            Some((_, _, reason)) => format!(
                "no dependency version could be looked up: {} answered \"{reason}\"",
                world.direct.len()
            ),
            None => "no dependency version could be looked up: every package is missing from its \
                     registry"
                .to_owned(),
        };
        return Judgment::not_measured(reason, metrics);
    }

    let mut judgment = Judgment::measured(metrics);
    judgment.verdict = if findings.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    // One evidence line per finding, naming the package and version but carrying no registry
    // payload: what a reader needs is which dependency to change.
    // A registry object, not a sentence: the evidence type says "a package at a version in a
    // registry", which is exactly what a reader has to go and look at.
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

/// A version string, or a sentence when the lockfile recorded none.
///
/// A `package.json` requirement with no lockfile names a specifier (`^4.17.20`) and a bare
/// dependency has no version at all; both are said plainly rather than printed as an empty
/// string.
#[must_use]
fn display_version(version: &str) -> &str {
    if version.is_empty() {
        "(no version in the manifest)"
    } else {
        version
    }
}

/// `usize` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
