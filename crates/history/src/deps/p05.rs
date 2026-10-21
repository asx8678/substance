//! P05 Bloat and overlap (M1-T10).
//!
//! Formula (action plan, P05):
//!
//! > Dependencies per 1,000 authored lines; overlap groups from `rules/overlap.toml` (HTTP
//! > clients, date libraries, state managers, per language); unused dependencies via knip
//! > (needs `--allow-exec`), cargo-machete and deptry.
//!
//! Three questions about the dependency list itself, answered from the project and its
//! manifest, plus one from external tools:
//!
//! - **density**: dependencies per 1,000 authored lines. A small project with forty
//!   dependencies is carrying more surface than its own code justifies. The metric is a rate,
//!   so the denominator is authored lines — the same number H04 and H05 use — and a project
//!   with no authored lines has no rate rather than an infinite one.
//! - **overlap**: two packages from one group in `rules/overlap.toml` solve one problem twice.
//!   The groups are data, not code: adding a library to a group is a one-line rule change, and
//!   the file names the language so the same group does not fire on a project that does not use
//!   that language.
//! - **unused**: the three scanners, each for one ecosystem. A missing scanner is a
//!   `not_measured` part, never an empty finding list.
//!
//! The density and overlap halves are `confirmed`; the unused half is `hint`, because every one
//! of those tools has false positives (a dependency used only through a build step, or named in
//! a config the tool does not read).

use std::collections::BTreeMap;

use substance_core::deps::Ecosystem;
use substance_core::thresholds::Thresholds;

use crate::deps::scan::ScannerRun;
use crate::deps::world::World;
use crate::signals::shape::Judgment;

/// One overlap group: a problem several packages solve, and the packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlapGroup {
    /// The group's name, as the rule file spells it.
    pub name: String,
    /// The language the group applies to: `ts`, `python` or `rust`.
    pub language: String,
    /// The packages in the group, sorted.
    pub packages: Vec<String>,
}

/// Judges P05.
///
/// `authored_lines` is the repository's authored line count, from the blame the history stage
/// already gathered. `unused` carries one entry per scanner that looked for unused
/// dependencies.
#[must_use]
pub fn bloat(
    world: &World,
    groups: &[OverlapGroup],
    authored_lines: u64,
    unused: &[(String, ScannerRun, Vec<String>)],
    thresholds: &Thresholds,
) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut findings: Vec<(String, String)> = Vec::new();

    let dependencies = world.direct.len();
    // Dependencies per 1,000 authored lines, rounded down. The multiply is saturating so a
    // huge dependency count cannot overflow; a zero line count means no rate at all, which
    // the note says rather than a zero that reads like "no dependencies".
    // A checked division: no authored lines means no rate at all, which the note says rather
    // than a zero that reads like "no dependencies".
    let density = u64::try_from(dependencies)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000)
        .checked_div(authored_lines);
    if density.is_none() {
        notes.push(
            "the repository has no authored lines, so dependencies per 1,000 lines has no              denominator and was not computed"
                .to_owned(),
        );
    }
    if let Some(density) = density {
        metrics.insert("dependencies_per_1000_lines".to_owned(), as_i64(density));
    }
    metrics.insert(
        "direct_dependencies".to_owned(),
        as_i64(dependencies as u64),
    );

    // Overlap: a group fires when two or more of its packages are direct dependencies and the
    // project's manifests are in the group's language.
    for group in groups {
        let present: Vec<&str> = group
            .packages
            .iter()
            .filter(|package| {
                world.direct.iter().any(|dep| {
                    dep.name == **package
                        && language_of(dep.ecosystem) == Some(group.language.as_str())
                })
            })
            .map(String::as_str)
            .collect();
        if present.len() < 2 {
            continue;
        }
        findings.push((
            group.name.clone(),
            format!(
                "{} packages from the {} group are all direct dependencies: {}",
                present.len(),
                group.name,
                present.join(", ")
            ),
        ));
    }

    // Unused: one part per scanner, and a finding per package a scanner named.
    let mut parts: Vec<(String, substance_core::model::Verdict)> = Vec::new();
    let mut unused_total = 0usize;
    for (tool, run, packages) in unused {
        match run {
            ScannerRun::Ran { .. } => {
                parts.push((tool.clone(), substance_core::model::Verdict::Pass));
                for package in packages {
                    unused_total += 1;
                    findings.push((
                        package.clone(),
                        format!("{package} is a direct dependency that {tool} found no use for"),
                    ));
                }
            }
            ScannerRun::Unavailable { reason } => parts.push((
                tool.clone(),
                substance_core::model::Verdict::NotMeasured {
                    reason: reason.clone(),
                },
            )),
        }
    }
    metrics.insert("unused".to_owned(), as_i64(unused_total as u64));

    if world.ecosystems.is_empty() {
        let mut judgment = Judgment::not_measured(
            "the repository has no manifest for any ecosystem, so it declares no dependencies",
            metrics,
        );
        judgment.notes = notes;
        return judgment;
    }

    findings.sort();
    let mut judgment = Judgment::measured(metrics);
    // Density is judged against a threshold when the file sets one, which it does not yet:
    // the plan gives no number and the corpora that would set it arrive later (ADR-0016).
    let floor = thresholds.history().p05_max_dependencies_per_1000_lines;
    let exceeded = match (floor, density) {
        (Some(floor), Some(density)) => density > u64::from(floor),
        _ => false,
    };
    judgment = judgment.judge(floor, exceeded);
    // Overlap and unused dependencies fail the check when present, whatever the density says:
    // two HTTP clients in one project is a defect, not a metric.
    if !findings.is_empty() {
        judgment.verdict = substance_core::model::Verdict::Fail;
    }
    judgment.evidence = findings
        .iter()
        .map(|(_, text)| substance_core::model::Evidence::Note(text.clone()))
        .collect();
    judgment.parts.extend(
        parts
            .into_iter()
            .collect::<BTreeMap<String, substance_core::model::Verdict>>(),
    );
    judgment.notes = notes;
    judgment
}

/// The language a rule file names for an ecosystem.
#[must_use]
fn language_of(ecosystem: Ecosystem) -> Option<&'static str> {
    match ecosystem {
        Ecosystem::Npm => Some("ts"),
        Ecosystem::PyPi => Some("python"),
        Ecosystem::Crates => Some("rust"),
    }
}

/// `u64` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
