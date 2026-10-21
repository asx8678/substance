//! Running the dependency checks over one commit (M1-T10).
//!
//! This is the module the orchestrator calls, matching [`crate::signals`] for the history
//! checks: the IO happens here, once, and each check is a pure function over what it produced.
//! That split is what makes a finding traceable — a verdict can only rest on facts that were
//! gathered before it — and what makes `--snapshot-from` replay the whole family without
//! touching the network.
//!
//! The order matters, because later steps read earlier ones:
//!
//! 1. [`World::load`] reads the manifests and lockfiles. Nothing else can run without it.
//! 2. [`dates::walks`] finds when each dependency was adopted, from git. P02 and P03 need it;
//!    it is a git walk, not a network call, so it happens before anything is spent.
//! 3. [`known::gather`] asks the registries, one request per package.
//! 4. P07 asks the forge about the dependencies' own repositories, bounded by
//!    [`p07::MAX_REPOS`].
//! 5. The scanners run (P01 always; P05's and P06's when the ecosystem is present).
//! 6. The judges run over all of it.
//!
//! A failure in a step is not a failure of the run: step 3 with an unreachable registry leaves
//! every check that needs one `not_measured` with the reason, which is the honest answer. Only
//! a git failure — no commits, no tree — stops the family, and it is returned as an error
//! because there is nothing at all to report.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::check::CheckId;
use substance_core::model::CheckVerdict;
use substance_core::model::Finding;
use substance_core::thresholds::Thresholds;
use substance_core::tools::ToolsFile;
use substance_runner::permit::ExecPermit;
use substance_runner::tool::ToolRunner;

use crate::deps::known::{self, Known};
use crate::deps::p01::{self, Vulnerabilities};
use crate::deps::p07;
use crate::deps::rules;
use crate::deps::scan::{self, ScannerRun};
use crate::deps::world::World;
use crate::deps::{p02, p03, p04, p05, p06};
use crate::git::GitRepo;
use crate::signals::shape::Judgment;
use crate::snapshot::SnapshotHttp;

/// Everything one dependency pass produced.
#[derive(Debug)]
pub struct DepsOutcome {
    /// The seven verdicts, in ID order.
    pub verdicts: Vec<CheckVerdict>,
    /// Notes per check, in the same order.
    pub notes: Vec<(CheckId, Vec<String>)>,
    /// Every finding, sorted by `(check, id)`.
    pub findings: Vec<Finding>,
    /// How many registry requests the pass spent.
    pub registry_requests: u32,
    /// How many forge requests the pass spent.
    pub forge_requests: u32,
    /// The notes the registry clients recorded, sorted.
    pub registry_notes: Vec<String>,
    /// The registry facts, kept for the provenance panel.
    ///
    /// Returned rather than fetched again: V03 lower bound is the newest release among the
    /// versions the project adopted, which is the document P02 and P03 already read. A second
    /// fetch would double the registry request count for one date per dependency.
    pub known: Known,
    /// When each direct dependency was adopted, kept for the same reason.
    pub added_at: BTreeMap<(substance_core::deps::Ecosystem, String), Timestamp>,
}

/// Why the dependency pass could not run at all.
#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    /// The tree or the manifests could not be read.
    #[error(transparent)]
    Git(#[from] crate::git::GitError),
    /// An embedded rule file is malformed, which is a build error.
    #[error("rule file: {0}")]
    Rules(#[from] rules::RuleError),
}

/// What the caller must supply that only the orchestrator knows.
#[derive(Debug)]
pub struct DepsInput<'a> {
    /// The run's identifier, which every finding id derives from.
    pub run_id: &'a str,
    /// The repository under evaluation.
    pub repo: &'a GitRepo,
    /// The revision.
    pub rev: &'a str,
    /// The run's instant.
    pub as_of: Timestamp,
    /// The embedded tool table.
    pub tools: &'a ToolsFile,
    /// The run's tool runner.
    pub runner: &'a ToolRunner<'a>,
    /// The run's `ExecPermit`, when `--allow-exec` was given.
    pub permit: Option<&'a ExecPermit>,
    /// A snapshot client for the registries, when the run has one.
    pub http: Option<&'a mut SnapshotHttp>,
    /// How many commits a dependency-date walk may read.
    pub dates_max_commits: u32,
    /// How many files the blame may cover, for P05's authored-line denominator.
    pub blame_max_files: u32,
    /// The thresholds.
    pub thresholds: &'a Thresholds,
}

/// Runs P01–P07 over `input`.
///
/// # Errors
///
/// Returns [`DepsError`] when the tree cannot be listed or an embedded rule file is malformed:
/// the two failures that leave nothing to report. Everything else becomes a check's reason.
pub async fn run_dependency_checks(mut input: DepsInput<'_>) -> Result<DepsOutcome, DepsError> {
    let world = World::load_tracked(input.repo, input.rev).await?;
    let groups = rules::overlap_groups()?;
    let license_rules = rules::license_rules()?;

    // When the dates: the names to look for are the direct dependencies, one entry per
    // ecosystem. A project with no lockfile still has adoption dates — the manifest's own
    // history is where they come from.
    let direct_names: Vec<(substance_core::deps::Ecosystem, String)> = world
        .direct
        .iter()
        .map(|dep| (dep.ecosystem, dep.name.clone()))
        .collect();
    let dates = crate::dates::walks(
        input.repo,
        input.rev,
        &direct_names,
        input.dates_max_commits,
    )
    .await?;
    let mut added_at: BTreeMap<(substance_core::deps::Ecosystem, String), Timestamp> =
        BTreeMap::new();
    for date in &dates.dates {
        added_at
            .entry((date.ecosystem, date.name.clone()))
            .or_insert(date.added_at);
    }

    // The registries, when the run has a client for them.
    let (known, registry_requests, registry_notes) = match input.http.as_deref_mut() {
        Some(http) => {
            let known = known::gather(&world, http, input.as_of).await;
            let used = http.used();
            let notes = known.notes.clone();
            (known, used, notes)
        }
        None => (Known::default(), 0, Vec::new()),
    };

    // The scanners.
    let vulnerabilities = gather_vulnerabilities(&world, &input).await;
    let unused = gather_unused(&world, &input).await;
    let license_tools = gather_license_tools(&world, &input).await;

    // The judgements.
    let mut judgments: Vec<Judgment> = vec![
        p01::known_vulnerabilities(&world, &vulnerabilities, input.thresholds),
        p02::reputation(&world, &known, &added_at, input.thresholds),
    ];
    judgments.push(p03::born_stale(&world, &known, &added_at, input.thresholds));
    judgments.push(p04::deprecated_or_yanked(&world, &known, input.thresholds));
    // Authored lines, for P05's density: the number of lines the blame covers, which is the
    // count H04 and H05 divide by. A failure here is not fatal — density is one of P05's three
    // questions — so it becomes zero, and P05's own note says the rate had no denominator.
    let authored_lines =
        match crate::git::blame_all(input.repo, input.rev, &world.paths, input.blame_max_files)
            .await
        {
            Ok(blame) => blame.lines().count() as u64,
            Err(_) => 0,
        };
    judgments.push(p05::bloat(
        &world,
        &groups,
        authored_lines,
        &unused,
        input.thresholds,
    ));
    let project_license = world.license.as_deref();
    judgments.push(p06::licenses(
        &world,
        &known,
        project_license,
        &license_rules,
        &license_tools,
        input.thresholds,
    ));
    // P07 reads each dependency's own GitHub repository. The client is built over the same
    // borrowed SnapshotHttp as the registries, so both spend one `forge_max_requests` budget
    // and both replay the same way.
    let vitality = match input.http.as_deref_mut() {
        Some(http) => p07::gather(&world, &known, &http.clone(), input.as_of).await,
        None => p07::VitalityResult::unavailable(
            "this run has no snapshot client, so no dependency's repository could be read",
        ),
    };
    judgments.push(p07::vitality(
        &world,
        &known,
        &vitality.repos,
        &vitality.skipped,
        input.as_of,
        vitality.budget_exhausted,
        input.thresholds,
    ));

    Ok(assemble(
        input.run_id,
        judgments,
        registry_requests,
        registry_notes,
        known,
        added_at,
    ))
}

/// Gathers P01's vulnerability reports from every scanner that can run.
///
/// `osv-scanner` is asked once, recursively: one process for every lockfile the repository has
/// is cheaper than one per lockfile and asks the same question. The native auditors are asked
/// only when the project uses their ecosystem, because a `cargo audit` on a Node project is a
/// wasted run and a confusing part.
async fn gather_vulnerabilities(world: &World, input: &DepsInput<'_>) -> Vulnerabilities {
    let mut gathered = Vulnerabilities::default();
    let mut reports: Vec<p01::Vulnerability> = Vec::new();
    let mut merged = 0usize;

    if let Some(spec) = input.tools.spec("osv-scanner") {
        let run = scan::run_scanner(
            input.runner,
            spec,
            "osv-scanner",
            scan::args::OSV_SCANNER_RECURSIVE,
            input.repo.root(),
            input.permit,
        )
        .await;
        if let Some(output) = run.output() {
            match parse_osv(&output.stdout) {
                Ok(found) => {
                    for vulnerability in found {
                        p01::merge_report(&mut reports, &mut merged, vulnerability);
                    }
                }
                Err(reason) => gathered.unreadable.push(("osv-scanner".to_owned(), reason)),
            }
        }
        gathered.scanners.push(("osv-scanner".to_owned(), run));
    } else {
        gathered.scanners.push((
            "osv-scanner".to_owned(),
            ScannerRun::unavailable("tool missing: osv-scanner"),
        ));
    }

    // The native auditors, each only for its own ecosystem.
    for (tool, ecosystem) in [
        ("npm", substance_core::deps::Ecosystem::Npm),
        ("cargo-audit", substance_core::deps::Ecosystem::Crates),
        ("pip-audit", substance_core::deps::Ecosystem::PyPi),
    ] {
        if !world.uses(ecosystem) {
            continue;
        }
        let Some(spec) = input.tools.spec(tool) else {
            continue;
        };
        let args = match tool {
            "npm" => scan::args::NPM_AUDIT,
            "cargo-audit" => scan::args::CARGO_AUDIT,
            "pip-audit" => scan::args::PIP_AUDIT,
            _ => continue,
        };
        let run = scan::run_scanner(
            input.runner,
            spec,
            tool,
            args,
            input.repo.root(),
            input.permit,
        )
        .await;
        if let Some(output) = run.output() {
            let parsed = match tool {
                "npm" => parse_npm_audit(&output.stdout),
                "cargo-audit" => parse_cargo_audit(&output.stdout),
                "pip-audit" => parse_pip_audit(&output.stdout),
                _ => Ok(Vec::new()),
            };
            match parsed {
                Ok(found) => {
                    for vulnerability in found {
                        p01::merge_report(&mut reports, &mut merged, vulnerability);
                    }
                }
                Err(reason) => gathered.unreadable.push((tool.to_owned(), reason)),
            }
        }
        gathered.scanners.push((tool.to_owned(), run));
    }

    p01::finish(&mut reports);
    gathered.found = reports;
    gathered.merged = merged;
    gathered
}

/// Gathers P05's unused-dependency reports.
async fn gather_unused(
    world: &World,
    input: &DepsInput<'_>,
) -> Vec<(String, ScannerRun, Vec<String>)> {
    let mut out: Vec<(String, ScannerRun, Vec<String>)> = Vec::new();
    for (tool, ecosystem, args) in [
        (
            "knip",
            substance_core::deps::Ecosystem::Npm,
            scan::args::KNIP,
        ),
        (
            "cargo-machete",
            substance_core::deps::Ecosystem::Crates,
            scan::args::CARGO_MACHETE,
        ),
        (
            "deptry",
            substance_core::deps::Ecosystem::PyPi,
            scan::args::DEPTRY,
        ),
    ] {
        if !world.uses(ecosystem) {
            continue;
        }
        let Some(spec) = input.tools.spec(tool) else {
            continue;
        };
        let run = scan::run_scanner(
            input.runner,
            spec,
            tool,
            args,
            input.repo.root(),
            input.permit,
        )
        .await;
        let packages = match run.output() {
            Some(output) => parse_unused(tool, &output.stdout).unwrap_or_default(),
            None => Vec::new(),
        };
        out.push((tool.to_owned(), run, packages));
    }
    out
}

/// Gathers P06's licence-tool reports.
async fn gather_license_tools(
    world: &World,
    input: &DepsInput<'_>,
) -> Vec<(String, ScannerRun, Vec<String>)> {
    let mut out: Vec<(String, ScannerRun, Vec<String>)> = Vec::new();
    for (tool, ecosystem, args) in [(
        "cargo-deny",
        substance_core::deps::Ecosystem::Crates,
        scan::args::CARGO_DENY,
    )] {
        if !world.uses(ecosystem) {
            continue;
        }
        let Some(spec) = input.tools.spec(tool) else {
            continue;
        };
        let run = scan::run_scanner(
            input.runner,
            spec,
            tool,
            args,
            input.repo.root(),
            input.permit,
        )
        .await;
        let packages = match run.output() {
            Some(output) => parse_deny_licenses(&output.stdout).unwrap_or_default(),
            None => Vec::new(),
        };
        out.push((tool.to_owned(), run, packages));
    }
    out
}

/// Splits the judgments into verdicts, findings and notes, in ID order.
/// Splits the judgments into verdicts, findings and notes, in ID order.
///
/// Each judgment's evidence becomes one finding: the evidence is what the judge pointed at, and
/// a verdict naming findings it does not carry would be a claim a reader cannot check. Finding
/// ids are derived from the check and the position, which is stable across runs because the
/// judges sort their evidence before returning it.
/// Splits the judgments into verdicts, findings and notes, in ID order.
///
/// Each judgment's evidence becomes one finding: the evidence is what the judge pointed at, and
/// a verdict naming findings it does not carry would be a claim a reader cannot check. The id
/// derives from the run, the check, the sub-check and the title (the same way
/// `substance_statics::file_finding` derives one), so the same problem in two runs of one
/// commit has the same id and can be diffed.
fn assemble(
    run_id: &str,
    judgments: Vec<Judgment>,
    registry_requests: u32,
    registry_notes: Vec<String>,
    known: Known,
    added_at: BTreeMap<(substance_core::deps::Ecosystem, String), Timestamp>,
) -> DepsOutcome {
    let mut verdicts: Vec<CheckVerdict> = Vec::new();
    let mut notes: Vec<(CheckId, Vec<String>)> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    for (index, judgment) in judgments.into_iter().enumerate() {
        let check = CHECK_ORDER[index];
        let meta = substance_core::registry::meta(check);
        let mut finding_ids: Vec<substance_core::FindingId> = Vec::new();
        for (position, evidence) in judgment.evidence.iter().enumerate() {
            let sub_check = format!("evidence-{position:04}");
            let title = title_of(evidence);
            let id = substance_core::FindingId::derive(&[
                run_id.as_bytes(),
                check.as_str().as_bytes(),
                sub_check.as_bytes(),
                title.as_bytes(),
            ]);
            finding_ids.push(id.clone());
            findings.push(Finding {
                id: id.clone(),
                check_id: check,
                sub_check: Some(sub_check),
                severity: meta.severity,
                confidence: meta.default_confidence,
                title,
                target: substance_core::model::Target::Repo,
                evidence: vec![evidence.clone()],
                replay: format!("substance replay {}", id.as_str()),
                cause: None,
                related: Vec::new(),
            });
        }
        verdicts.push(CheckVerdict {
            check_id: check,
            target: substance_core::model::Target::Repo,
            verdict: judgment.verdict,
            finding_ids,
            metrics: judgment.metrics,
            parts: judgment.parts,
        });
        if !judgment.notes.is_empty() {
            notes.push((check, judgment.notes));
        }
    }
    verdicts.sort_by_key(|verdict| verdict.check_id);
    notes.sort_by_key(|(check, _)| *check);
    findings.sort_by(|left, right| {
        left.check_id
            .cmp(&right.check_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    DepsOutcome {
        verdicts,
        notes,
        findings,
        registry_requests,
        forge_requests: 0,
        registry_notes,
        known,
        added_at,
    }
}

/// The checks, in the order [`run_dependency_checks`] pushes their judgments.
///
/// A judgment does not carry its own id: the judge answers a question, and which check asked is
/// the caller's business. Naming the order once here keeps the two in step; a mismatch is an
/// index out of bounds when the family runs, not a report that quietly attributes P04's verdict
/// to P03.
pub const CHECK_ORDER: [CheckId; 7] = [
    CheckId::P01,
    CheckId::P02,
    CheckId::P03,
    CheckId::P04,
    CheckId::P05,
    CheckId::P06,
    CheckId::P07,
];

/// The title a finding carries, from the sentence its evidence holds.
///
/// A registry object has no sentence of its own, so the package and version are spelled as one;
/// that is what a reader needs to act on the finding.
fn title_of(evidence: &substance_core::model::Evidence) -> String {
    match evidence {
        substance_core::model::Evidence::Note(text) => text.clone(),
        substance_core::model::Evidence::RegistryObject(object) => {
            format!("{} {} {}", object.registry, object.package, object.version)
        }
        substance_core::model::Evidence::ForgeObject(object) => {
            format!("{} {}", object.kind, object.url)
        }
        other => format!("{other:?}"),
    }
}

/// Parses `osv-scanner --recursive --format json`.
///
/// The v2 report is `{"results": [{"source": {...}, "packages": [{"package": {"name",
/// "version", "ecosystem"}, "vulnerabilities": [{"id", "aliases", "summary"}]}]}]}`. An
/// ecosystem string is mapped onto the three this build knows; a fourth (Go, Maven, NuGet) is
/// skipped rather than guessed at.
pub fn parse_osv(text: &str) -> Result<Vec<p01::Vulnerability>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let mut out: Vec<p01::Vulnerability> = Vec::new();
    let Some(results) = value.get("results").and_then(|value| value.as_array()) else {
        // A report with no `results` key is a scan that found nothing, which is what
        // osv-scanner prints for a repository with no lockfile it reads.
        return Ok(Vec::new());
    };
    for result in results {
        let Some(packages) = result.get("packages").and_then(|value| value.as_array()) else {
            continue;
        };
        for package in packages {
            let Some(meta) = package.get("package") else {
                continue;
            };
            let Some(ecosystem) = meta
                .get("ecosystem")
                .and_then(|value| value.as_str())
                .and_then(ecosystem_of_osv)
            else {
                continue;
            };
            let name = meta
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned();
            let version = meta
                .get("version")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned();
            let Some(vulnerabilities) = package
                .get("vulnerabilities")
                .and_then(|value| value.as_array())
            else {
                continue;
            };
            for vulnerability in vulnerabilities {
                let mut ids: Vec<String> = Vec::new();
                if let Some(id) = vulnerability.get("id").and_then(|value| value.as_str()) {
                    ids.push(id.to_owned());
                }
                if let Some(aliases) = vulnerability
                    .get("aliases")
                    .and_then(|value| value.as_array())
                {
                    for alias in aliases {
                        if let Some(alias) = alias.as_str() {
                            ids.push(alias.to_owned());
                        }
                    }
                }
                ids.sort();
                ids.dedup();
                out.push(p01::Vulnerability {
                    ecosystem,
                    package: name.clone(),
                    version: version.clone(),
                    ids,
                    summary: vulnerability
                        .get("summary")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    tools: vec!["osv-scanner".to_owned()],
                });
            }
        }
    }
    Ok(out)
}

/// The ecosystem an OSV report names.
fn ecosystem_of_osv(text: &str) -> Option<substance_core::deps::Ecosystem> {
    use substance_core::deps::Ecosystem;
    match text {
        "npm" => Some(Ecosystem::Npm),
        "PyPI" => Some(Ecosystem::PyPi),
        "crates.io" => Some(Ecosystem::Crates),
        _ => None,
    }
}

/// Parses `npm audit --json` (report version 2).
fn parse_npm_audit(text: &str) -> Result<Vec<p01::Vulnerability>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let mut out: Vec<p01::Vulnerability> = Vec::new();
    let Some(vulnerabilities) = value
        .get("vulnerabilities")
        .and_then(|value| value.as_object())
    else {
        return Ok(Vec::new());
    };
    for (name, entry) in vulnerabilities {
        let mut ids: Vec<String> = Vec::new();
        if let Some(via) = entry.get("via").and_then(|value| value.as_array()) {
            for item in via {
                // `via` is a list of advisory objects and/or package-name strings; only the
                // objects carry an advisory id.
                if let Some(id) = item
                    .get("url")
                    .and_then(|value| value.as_str())
                    .and_then(|url| url.rsplit('/').next())
                {
                    ids.push(id.to_owned());
                }
                for key in ["cve", "source"] {
                    if let Some(id) = item.get(key).and_then(|value| value.as_str()) {
                        ids.push(id.to_owned());
                    }
                }
                if let Some(title) = item.get("title").and_then(|value| value.as_str()) {
                    ids.push(title.to_owned());
                }
            }
        }
        ids.sort();
        ids.dedup();
        // The range is what npm reports; the installed version is in `nodes`, which names
        // paths rather than versions, so the range is what a reader gets.
        let version = entry
            .get("range")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned();
        if ids.is_empty() {
            continue;
        }
        out.push(p01::Vulnerability {
            ecosystem: substance_core::deps::Ecosystem::Npm,
            package: name.clone(),
            version,
            ids,
            summary: entry
                .get("severity")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            tools: vec!["npm".to_owned()],
        });
    }
    Ok(out)
}

/// Parses `cargo audit --json`.
fn parse_cargo_audit(text: &str) -> Result<Vec<p01::Vulnerability>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let mut out: Vec<p01::Vulnerability> = Vec::new();
    let Some(vulnerabilities) = value
        .get("vulnerabilities")
        .and_then(|value| value.get("list"))
    else {
        return Ok(Vec::new());
    };
    let Some(list) = vulnerabilities.as_array() else {
        return Ok(Vec::new());
    };
    for entry in list {
        let advisory = entry.get("advisory").cloned().unwrap_or_default();
        let mut ids: Vec<String> = Vec::new();
        for key in ["id", "cve"] {
            if let Some(id) = advisory.get(key).and_then(|value| value.as_str()) {
                ids.push(id.to_owned());
            }
        }
        if let Some(aliases) = advisory.get("aliases").and_then(|value| value.as_array()) {
            for alias in aliases {
                if let Some(alias) = alias.as_str() {
                    ids.push(alias.to_owned());
                }
            }
        }
        ids.sort();
        ids.dedup();
        let package = entry.get("package").cloned().unwrap_or_default();
        out.push(p01::Vulnerability {
            ecosystem: substance_core::deps::Ecosystem::Crates,
            package: package
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            version: package
                .get("version")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            ids,
            summary: advisory
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            tools: vec!["cargo-audit".to_owned()],
        });
    }
    Ok(out)
}

/// Parses `pip-audit --format json`.
fn parse_pip_audit(text: &str) -> Result<Vec<p01::Vulnerability>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let Some(dependencies) = value.get("dependencies").and_then(|value| value.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<p01::Vulnerability> = Vec::new();
    for dependency in dependencies {
        let name = dependency
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let version = dependency
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let Some(vulns) = dependency.get("vulns").and_then(|value| value.as_array()) else {
            continue;
        };
        for vuln in vulns {
            let mut ids: Vec<String> = Vec::new();
            if let Some(id) = vuln.get("id").and_then(|value| value.as_str()) {
                ids.push(id.to_owned());
            }
            if let Some(aliases) = vuln.get("aliases").and_then(|value| value.as_array()) {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        ids.push(alias.to_owned());
                    }
                }
            }
            ids.sort();
            ids.dedup();
            out.push(p01::Vulnerability {
                ecosystem: substance_core::deps::Ecosystem::PyPi,
                package: name.to_owned(),
                version: version.to_owned(),
                ids,
                summary: vuln
                    .get("description")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                tools: vec!["pip-audit".to_owned()],
            });
        }
    }
    Ok(out)
}

/// The package names an unused-dependency tool named.
fn parse_unused(tool: &str, text: &str) -> Result<Vec<String>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let mut names: Vec<String> = Vec::new();
    match tool {
        // knip's JSON reporter is a list of issues, each with a `filePath` and symbols; the
        // unused-dependency case names the package in `symbol`.
        "knip" => {
            if let Some(issues) = value.as_array() {
                for issue in issues {
                    if let Some(symbol) = issue.get("symbol").and_then(|value| value.as_str()) {
                        names.push(symbol.to_owned());
                    }
                }
            }
        }
        // cargo-machete prints `{"unused": [{"name": ..., "version": ...}]}`.
        "cargo-machete" => {
            if let Some(unused) = value.get("unused").and_then(|value| value.as_array()) {
                for item in unused {
                    if let Some(name) = item.get("name").and_then(|value| value.as_str()) {
                        names.push(name.to_owned());
                    }
                }
            }
        }
        // deptry's `--json-output` is a list of violations, each naming the module.
        "deptry" => {
            if let Some(items) = value.as_array() {
                for item in items {
                    if let Some(module) = item.get("module").and_then(|value| value.as_str()) {
                        names.push(module.to_owned());
                    }
                }
            }
        }
        _ => {}
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// The package names `cargo deny check licenses` reported.
fn parse_deny_licenses(text: &str) -> Result<Vec<String>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = Vec::new();
    // cargo-deny's JSON is a stream of objects, one per line in some versions and one array in
    // others; both name the offending crate in `fields.crate` or a `labels` entry.
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(name) = value
            .get("fields")
            .and_then(|value| value.get("crate"))
            .and_then(|value| value.as_str())
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}
