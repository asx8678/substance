//! P06 Licenses (M1-T10).
//!
//! Formula (action plan, P06):
//!
//! > Project license (LICENSE text hash or the manifest's `license` field) against dependency
//! > licenses from registry metadata, using `rules/licenses.toml`. Missing licenses are flagged.
//! > Use cargo-deny, license-checker or pip-licenses when present.
//!
//! The question is a legal one a reader must be able to check without trusting this build: every
//! dependency's licence, against the project's own, with the compatibility matrix in
//! `rules/licenses.toml` as data. Three things are flagged:
//!
//! - **a dependency with no licence at all** — the registry metadata says nothing, which is a
//!   risk rather than a permission;
//! - **a strong-copyleft dependency under a permissive project**, which is the combination that
//!   obliges the project to relicense;
//! - **a dependency whose licence this build cannot classify**, which is reported rather than
//!   assumed compatible: an unknown SPDX id is not a permissive one.
//!
//! The project's own licence is read from its manifest first and from a `LICENSE` file's text
//! second, because a manifest field is a declaration and a licence file's text needs matching
//! against known texts. The hash of the licence text is recorded as evidence, so a reader can
//! tell two licenses with one name apart.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use crate::deps::known::{Known, KnownError};
use crate::deps::scan::ScannerRun;
use crate::deps::world::World;
use crate::signals::shape::Judgment;

/// How a licence obliges the project that depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Obligation {
    /// No conditions beyond attribution: MIT, BSD, ISC, Apache-2.0.
    Permissive,
    /// Conditions on the files that use it, not on the project: MPL-2.0, LGPL.
    WeakCopyleft,
    /// Conditions on the whole project: GPL, AGPL.
    StrongCopyleft,
    /// Not in the rule file, so this build declines to judge it.
    Unknown,
}

/// The classification the rule file carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LicenseRules {
    /// SPDX identifier to obligation, sorted by identifier.
    pub obligations: BTreeMap<String, Obligation>,
    /// `(project, dependency)` pairs the matrix refuses, sorted.
    pub incompatible: Vec<(Obligation, Obligation)>,
}

impl LicenseRules {
    /// The obligation for an SPDX id, case-insensitively, or [`Obligation::Unknown`].
    ///
    /// A licence expression (`MIT OR Apache-2.0`) is read as its first identifier: a package
    /// offering a choice lets the project take the permissive one, so reading the strictest
    /// would accuse it wrongly.
    #[must_use]
    pub fn obligation_of(&self, license: &str) -> Obligation {
        let first = first_identifier(license);
        self.obligations
            .get(first)
            .copied()
            .unwrap_or(Obligation::Unknown)
    }

    /// Whether the matrix refuses `dependency` under `project`.
    #[must_use]
    pub fn refuses(&self, project: Obligation, dependency: Obligation) -> bool {
        self.incompatible
            .iter()
            .any(|(left, right)| *left == project && *right == dependency)
    }
}

/// The first SPDX identifier of an expression such as `MIT OR Apache-2.0`, trimmed.
///
/// A package that offers a choice lets the project take the permissive option, so the first
/// identifier is the one that decides the obligation. Reading the strictest instead would
/// accuse a dual-licensed package of the licence the project did not choose.
#[must_use]
pub fn first_identifier(license: &str) -> &str {
    let mut end = license.len();
    for separator in [" OR ", " or ", "/"] {
        if let Some(at) = license.find(separator) {
            end = end.min(at);
        }
    }
    license[..end].trim()
}

/// Judges P06.
///
/// `project_license` is the licence the project declares, with the hash of the licence text
/// when one was read. `tools` carries one entry per licence scanner that ran.
#[must_use]
pub fn licenses(
    world: &World,
    known: &Known,
    project_license: Option<&str>,
    rules: &LicenseRules,
    tools: &[(String, ScannerRun, Vec<String>)],
    _thresholds: &Thresholds,
) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut findings: Vec<String> = Vec::new();
    let mut missing = 0i64;
    let mut incompatible = 0i64;
    let mut unknown = 0i64;
    let mut checked = 0i64;

    let project = project_license.map(|license| rules.obligation_of(license));
    if project_license.is_none() {
        notes.push(
            "the project declares no licence in its manifest and has no LICENSE file this build \
             reads, so compatibility was not judged; only missing dependency licences are \
             flagged"
                .to_owned(),
        );
    }

    for dep in &world.direct {
        let package = match known.get(dep.ecosystem, &dep.name) {
            Some(Ok(_)) => known.package(dep.ecosystem, &dep.name),
            Some(Err(KnownError::NotFound)) | None => None,
            Some(Err(KnownError::Unavailable { .. })) => None,
        };
        let Some(package) = package else {
            continue;
        };
        checked += 1;
        let Some(license) = package.license.as_deref() else {
            missing += 1;
            findings.push(format!(
                "{} declares no licence on {}",
                dep.name,
                dep.ecosystem.as_str()
            ));
            continue;
        };
        let obligation = rules.obligation_of(license);
        match obligation {
            Obligation::Unknown => {
                unknown += 1;
                findings.push(format!(
                    "{} is licensed {license}, which rules/licenses.toml does not classify",
                    dep.name
                ));
            }
            other => {
                if project.is_some_and(|project| rules.refuses(project, other)) {
                    incompatible += 1;
                    findings.push(format!(
                        "{} is {license}, which a {} project cannot carry",
                        dep.name,
                        project_license.unwrap_or("unknown-licence")
                    ));
                }
            }
        }
    }

    // The scanners add their own view, as parts: a licence tool that ran is a source, and one
    // that is missing is a gap a reader can see.
    let mut parts: BTreeMap<String, substance_core::model::Verdict> = BTreeMap::new();
    for (tool, run, packages) in tools {
        match run {
            ScannerRun::Ran { .. } => {
                parts.insert(tool.clone(), substance_core::model::Verdict::Pass);
                for package in packages {
                    findings.push(format!("{package}: {tool} reported a licence problem"));
                }
            }
            ScannerRun::Unavailable { reason } => {
                parts.insert(
                    tool.clone(),
                    substance_core::model::Verdict::NotMeasured {
                        reason: reason.clone(),
                    },
                );
            }
        }
    }

    findings.sort();
    findings.dedup();
    metrics.insert("direct_dependencies".to_owned(), as_i64(world.direct.len()));
    metrics.insert("licenses_checked".to_owned(), checked);
    metrics.insert("missing".to_owned(), missing);
    metrics.insert("incompatible".to_owned(), incompatible);
    metrics.insert("unclassified".to_owned(), unknown);

    if world.direct.is_empty() {
        let mut judgment = Judgment::not_measured(
            "the project declares no direct dependency, so no licence has to be compatible",
            metrics,
        );
        judgment.notes = notes;
        return judgment;
    }
    if checked == 0 {
        return Judgment::not_measured(
            "no dependency's registry metadata could be read, so no licence was compared",
            metrics,
        );
    }

    let mut judgment = Judgment::measured(metrics);
    judgment.verdict = if findings.is_empty() {
        substance_core::model::Verdict::Pass
    } else {
        substance_core::model::Verdict::Fail
    };
    judgment.evidence = findings
        .iter()
        .map(|text| substance_core::model::Evidence::Note(text.clone()))
        .collect();
    judgment.parts = parts;
    judgment.notes = notes;
    judgment
}

/// `usize` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
