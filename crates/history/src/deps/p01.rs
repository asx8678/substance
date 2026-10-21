//! P01 Known vulnerabilities (M1-T10).
//!
//! Formula (action plan, P01):
//!
//! > `osv-scanner` JSON over the lockfiles: one tool for every ecosystem, and it executes no
//! > repo code. Add `npm audit`, `cargo audit` and `pip-audit` when present; pip-audit only with
//! > `--allow-exec`, since it can build packages.
//!
//! OSV is the primary source because it is the only one that speaks all three ecosystems, and
//! because it reads the lockfile rather than the installed tree: it executes nothing. The three
//! ecosystem-native tools are additive — each finds advisories OSV may not carry yet, and each
//! reports only its own ecosystem — and each contributes a part, so a reader can see which
//! sources actually answered.
//!
//! ## Deduplication
//!
//! Two tools routinely report one vulnerability: OSV mirrors the GitHub Advisory Database and
//! RustSec, and `cargo audit` reads RustSec directly. Reporting both would inflate a count and
//! suggest a project has two problems where it has one. So findings are deduplicated on
//! `(ecosystem, package, version, advisory-id-set)`:
//!
//! - the advisory-id set is the union of every identifier a tool gives for one vulnerability
//!   (`GHSA-…`, `CVE-…`, `RUSTSEC-…`, `PYSEC-…`) with aliases resolved, and it is compared as a
//!   sorted set;
//! - two reports that share **any** identifier are the same vulnerability;
//! - a vulnerability with no identifier at all is its own key, because there is nothing to
//!   match it on.
//!
//! Without the alias resolution the same advisory would appear once as `GHSA-x` and once as
//! `CVE-y`, which is the common case rather than the exception.
//!
//! ## What a missing tool means
//!
//! Every tool is a part. When `osv-scanner` is absent the check reports `not_measured` with
//! `tool missing: osv-scanner` — the plan's wording — and no findings, because a scanner that
//! did not run has not cleared the repository. When the native tools are absent but OSV ran,
//! their parts say `tool missing: …` while the check's own verdict rests on OSV, which is a
//! complete answer on its own.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::check::CheckId;
use substance_core::deps::Ecosystem;
use substance_core::model::Verdict;
use substance_core::thresholds::Thresholds;

use crate::deps::scan::ScannerRun;
use crate::deps::world::World;
use crate::signals::shape::Judgment;

/// One vulnerability report, from whichever tool found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vulnerability {
    /// The ecosystem the package belongs to.
    pub ecosystem: Ecosystem,
    /// The package name.
    pub package: String,
    /// The installed version.
    pub version: String,
    /// Every identifier the report carried, sorted.
    pub ids: Vec<String>,
    /// The summary, from whichever tool supplied one.
    pub summary: String,
    /// Which tool reported it, sorted afterwards.
    pub tools: Vec<String>,
}

impl Vulnerability {
    /// The set the report deduplicates on: the package, the version and every id.
    #[must_use]
    pub fn key(&self) -> (Ecosystem, String, String, Vec<String>) {
        (
            self.ecosystem,
            self.package.clone(),
            self.version.clone(),
            self.ids.clone(),
        )
    }

    /// The identifier a reader quotes: the first one in sorted order, or a placeholder.
    #[must_use]
    pub fn lead_id(&self) -> &str {
        self.ids
            .first()
            .map_or("an unidentified advisory", String::as_str)
    }
}

/// What the scanners said.
#[derive(Debug, Clone, Default)]
pub struct Vulnerabilities {
    /// The reports, deduplicated and sorted.
    pub found: Vec<Vulnerability>,
    /// One entry per scanner, in a fixed order, with its outcome.
    pub scanners: Vec<(String, ScannerRun)>,
    /// Reports whose advisory matched a different identifier and were merged away.
    pub merged: usize,
    /// Output that did not parse, sorted, with the reason.
    pub unreadable: Vec<(String, String)>,
}

/// Judges P01.
///
/// `scanners` is what [`crate::deps::scan`] ran, in the order the check consults them:
/// `osv-scanner`, then `npm audit`, `cargo audit`, `pip-audit`. A tool that did not run
/// contributes its part and no findings.
#[must_use]
pub fn known_vulnerabilities(
    world: &World,
    gathered: &Vulnerabilities,
    _thresholds: &Thresholds,
) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut parts: Vec<(&str, Verdict)> = Vec::new();

    let mut ran = 0usize;
    for (name, run) in &gathered.scanners {
        match run {
            ScannerRun::Ran { .. } if run.printed() => {
                ran += 1;
                parts.push((name.as_str(), Verdict::Pass));
            }
            ScannerRun::Ran { .. } => {
                // It ran and printed nothing. For osv-scanner that is a scan that found no
                // lockfile it understands; for the native tools it is the "no advisories"
                // output. Either way the part is measured, not failed.
                ran += 1;
                parts.push((name.as_str(), Verdict::Pass));
                notes.push(format!("{name} printed no report"));
            }
            ScannerRun::Unavailable { reason } => {
                parts.push((
                    name.as_str(),
                    Verdict::NotMeasured {
                        reason: reason.clone(),
                    },
                ));
            }
        }
    }

    for (tool, reason) in &gathered.unreadable {
        notes.push(format!("{tool}: {reason}"));
    }
    metrics.insert("vulnerabilities".to_owned(), as_i64(gathered.found.len()));
    metrics.insert("reports_merged".to_owned(), as_i64(gathered.merged));
    metrics.insert("scanners_ran".to_owned(), as_i64(ran));
    metrics.insert("direct_dependencies".to_owned(), as_i64(world.direct.len()));

    if ran == 0 {
        // Every scanner was missing or refused. The plan's sentence for the primary one is the
        // one a reader needs; the others are in their parts.
        let reason = gathered
            .scanners
            .iter()
            .find_map(|(_, run)| match run {
                ScannerRun::Unavailable { reason } => Some(reason.clone()),
                ScannerRun::Ran { .. } => None,
            })
            .unwrap_or_else(|| "no vulnerability scanner could be run".to_owned());
        let mut judgment = Judgment::not_measured(reason, metrics);
        judgment.parts = parts
            .into_iter()
            .map(|(name, verdict)| (name.to_owned(), verdict))
            .collect();
        judgment.notes = notes;
        return judgment;
    }

    let mut judgment = Judgment::measured(metrics);
    judgment.verdict = if gathered.found.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    judgment.evidence = gathered
        .found
        .iter()
        .map(|found| {
            substance_core::model::Evidence::RegistryObject(substance_core::model::RegistryObject {
                registry: found.ecosystem.as_str().to_owned(),
                package: found.package.clone(),
                version: format!("{} ({})", found.version, found.lead_id()),
            })
        })
        .collect();
    if gathered.merged > 0 {
        notes.push(format!(
            "{} reports named a vulnerability another scanner also named; they were merged on \
             their advisory identifiers",
            gathered.merged
        ));
    }
    judgment.parts = parts
        .into_iter()
        .map(|(name, verdict)| (name.to_owned(), verdict))
        .collect();
    judgment.notes = notes;
    judgment
}

/// Merges one tool's reports into the accumulator, deduplicating on identifiers.
///
/// Two reports are the same vulnerability when their id sets intersect: a `GHSA-` advisory and
/// its `CVE-` alias must not appear twice, and tools disagree about which one they print. The
/// merged entry keeps the union of both id sets, so a third tool reporting either alias still
/// matches.
pub fn merge_report(found: &mut Vec<Vulnerability>, merged: &mut usize, report: Vulnerability) {
    let ids: BTreeSet<&String> = report.ids.iter().collect();
    if let Some(existing) = found.iter_mut().find(|existing| {
        existing.ecosystem == report.ecosystem
            && existing.package == report.package
            && existing.version == report.version
            && existing.ids.iter().any(|id| ids.contains(id))
    }) {
        let mut combined: BTreeSet<String> = existing.ids.iter().cloned().collect();
        combined.extend(report.ids.iter().cloned());
        existing.ids = combined.into_iter().collect();
        existing.tools.extend(report.tools.iter().cloned());
        existing.tools.sort();
        existing.tools.dedup();
        if existing.summary.is_empty() {
            existing.summary = report.summary;
        }
        *merged += 1;
        return;
    }
    found.push(report);
}

/// Sorts the accumulator into report order: by ecosystem, package, version, then first id.
///
/// Two runs of one commit must produce the same report, and the order the tools answered in is
/// a detail of the network.
pub fn finish(found: &mut Vec<Vulnerability>) {
    for vulnerability in found.iter_mut() {
        vulnerability.ids.sort();
        vulnerability.ids.dedup();
        vulnerability.tools.sort();
        vulnerability.tools.dedup();
    }
    found.sort_by(|left, right| {
        left.ecosystem
            .cmp(&right.ecosystem)
            .then_with(|| left.package.cmp(&right.package))
            .then_with(|| left.version.cmp(&right.version))
            .then_with(|| left.ids.cmp(&right.ids))
    });
    found.dedup();
}

/// `usize` as the `i64` a metric uses, saturating rather than wrapping.
fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The ecosystems a tool speaks for. A native auditor is asked only when the project uses its
/// ecosystem; asking `cargo audit` about a Node project is a wasted run and a confusing part.
#[must_use]
pub fn speaks_for(tool: &str) -> Option<Ecosystem> {
    match tool {
        "npm" => Some(Ecosystem::Npm),
        "cargo-audit" => Some(Ecosystem::Crates),
        "pip-audit" => Some(Ecosystem::PyPi),
        _ => None,
    }
}

/// The check id, for a caller that needs to name the check a report belongs to.
#[must_use]
pub const fn check_id() -> CheckId {
    CheckId::P01
}

/// The scanners P01 consults, in the order their parts are listed.
///
/// `osv-scanner` is first because it is the one that covers every ecosystem and executes no
/// repository code. The native auditors follow, each additive.
pub const SCANNERS: &[&str] = &["osv-scanner", "npm", "cargo-audit", "pip-audit"];
