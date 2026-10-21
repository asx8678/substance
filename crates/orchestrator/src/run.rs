//! One run, from intake to `report.json` (M0-T08).
//!
//! The skeleton does the parts that do not need an app: it pins the commit, fills the
//! manifest, derives `run_id`, creates the run directory and writes a report in which
//! every check that judges an app reports `not_measured` with the reason it did not run.
//! The stages between intake and report arrive with their own milestones.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use substance_core::config::ProjectConfig;
use substance_core::hash::CanonicalJsonError;
use substance_core::model::report::{REPORT_SCHEMA_VERSION, Report};
use substance_core::model::verdict::Target;
use substance_core::{CheckId, Timestamp, wall_now_utc};
use substance_report::{RunDir, RunDirError};

use crate::config::{ConfigSource, LoadError, load_config};
use crate::intake::{IntakeError, intake};
use crate::manifest::{ManifestInputs, build, default_master_seed};
use crate::stage::Stage;
use crate::tools::{manifest_tools, probe_versions, table};

/// The default directory a run is written to, under the caller's working directory.
pub const DEFAULT_RUNS_DIR: &str = "runs";

/// How to run.
///
/// This mirrors `substance run`'s flags, so the CLI passes them through unchanged.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The repository to evaluate: a local directory or a git URL.
    pub repo: PathBuf,
    /// The revision to evaluate; `None` means `HEAD`.
    pub rev: Option<String>,
    /// The output directory root; `None` means `./runs`.
    pub out: Option<PathBuf>,
    /// The instant the run pretends it is; `None` means now.
    pub as_of: Option<Timestamp>,
    /// The master seed; `None` means one derived from the commit.
    pub seed: Option<u64>,
    /// The config file to use instead of `<repo>/substance.toml`.
    pub config: Option<PathBuf>,
    /// Stop after triage.
    pub triage_only: bool,
    /// Whether repo-controlled code may run.
    pub allow_exec: bool,
    /// Replay an earlier run's external data instead of fetching it.
    pub snapshot_from: Option<PathBuf>,
}

/// What one run produced.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// The report, as written to `report.json`.
    pub report: Report,
    /// The run directory.
    pub run_dir: PathBuf,
    /// The gates that failed; every one of them means exit 1 under `--fail-on gate`.
    pub gates_hit: Vec<GateHit>,
}

/// A hard gate that failed (ADR-0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GateHit {
    /// The check whose failure gates the run.
    pub check_id: CheckId,
    /// What the check judged.
    pub target: Target,
}

/// Why a run stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Intake could not pin the code under test: a usage error, exit 2.
    #[error(transparent)]
    Intake(#[from] IntakeError),
    /// The config could not be read: a usage error, exit 2.
    #[error(transparent)]
    Config(#[from] LoadError),
    /// The run directory could not be written: an internal error, exit 3.
    #[error(transparent)]
    RunDir(#[from] RunDirError),
    /// The embedded `tools.toml` is malformed: an internal error, exit 3.
    #[error("tools.toml: {0}")]
    Tools(String),
    /// A probed tool's output could not be saved: an internal error, exit 3.
    #[error(transparent)]
    Blob(#[from] substance_runner::BlobError),
    /// A flag names something that cannot be used: a usage error, exit 2.
    #[error("{0}")]
    Usage(String),
    /// A document could not be serialized: an internal error, exit 3.
    #[error(transparent)]
    Serialize(#[from] CanonicalJsonError),
    /// The file system refused an operation: an internal error, exit 3.
    #[error("cannot prepare {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
}

/// Runs one evaluation.
///
/// The stages this build has are intake and report. Everything between them is absent,
/// so each check that judges an app reports `not_measured("not implemented (Mx)")`, and
/// the report says so in its notes.
///
/// # Errors
///
/// Returns [`RunError::Intake`] or [`RunError::Config`] for a usage, config or intake
/// problem (exit 2), and the other variants for an internal failure (exit 3).
pub async fn run(options: &RunOptions) -> Result<RunOutcome, RunError> {
    let out_root = options
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RUNS_DIR));
    ensure_dir(&out_root)?;
    if let Some(snapshot) = &options.snapshot_from {
        ensure_run_dir(snapshot)?;
    }
    let checkout = intake(
        &options.repo,
        options.rev.as_deref(),
        &out_root,
        options.allow_exec,
    )
    .await?;
    let (config, source) = load_config(options.config.as_deref(), &checkout.root)?;
    let as_of = options.as_of.unwrap_or_else(wall_now_utc);
    let master_seed = options
        .seed
        .unwrap_or_else(|| default_master_seed(&checkout.commit_sha));
    // The tool versions go into the manifest, so they are part of the run ID: two runs
    // with different gitleaks versions are different runs.
    let tools_file = table().map_err(|error| RunError::Tools(error.to_string()))?;
    let probes = probe_versions(&tools_file, &checkout.root).await;
    let manifest = build(&ManifestInputs {
        checkout: &checkout,
        as_of,
        master_seed,
        config_hash: hash_config(&config)?,
        allow_exec: options.allow_exec,
        tools: manifest_tools(&probes),
        thresholds_version: crate::thresholds::version(),
        rule_pack_hash: substance_statics::rule_pack_hash(),
        provenance_rules_hash: substance_core::provenance::DeclaredRules::hash(),
    })?;
    let run_dir = RunDir::create(&out_root, &manifest.run_id)?;
    run_dir.write_json("manifest.json", &manifest)?;
    run_dir.write_json(
        "invocation.json",
        &Invocation::of(options, run_dir.as_path(), source.kind()),
    )?;
    write_tool_blobs(run_dir.as_path(), &probes)?;

    // The checks that read the repository without running it. `--triage-only` is the only mode
    // this build has, so triage always runs; a later milestone adds the stages after it, and the
    // checks that need them already report the milestone that will deliver them.
    let thresholds =
        crate::thresholds::thresholds().map_err(|error| RunError::Usage(error.to_string()))?;
    let snapshot = match &options.snapshot_from {
        Some(snapshot) => Some(
            substance_history::snapshot::SnapshotHttp::replay(snapshot)
                .map_err(|error| RunError::Usage(error.to_string()))?,
        ),
        None => Some(
            substance_history::snapshot::SnapshotHttp::live(
                run_dir.as_path(),
                as_of,
                config.budgets.forge_max_requests,
            )
            .map_err(|error| RunError::Usage(error.to_string()))?,
        ),
    };
    let pending = crate::verdicts::pending(&config);
    let triage = crate::triage::run_triage(
        &crate::triage::TriageInput {
            run_id: manifest.run_id.as_str(),
            root: checkout.root.as_path(),
            rev: &checkout.commit_sha,
            origin: &checkout.origin,
            as_of,
            run_dir: run_dir.as_path(),
            config: &config,
            tools: &tools_file,
            thresholds: &thresholds,
            http: snapshot,
            permit: checkout.permit.as_ref(),
        },
        pending,
    )
    .await
    .map_err(RunError::Usage)?;

    // Findings are written one file each, so a report row's evidence can point at the file that
    // holds it rather than only at an id inside report.json.
    for finding in &triage.findings {
        let name = format!("findings/{}.json", finding.id.as_str());
        run_dir.write_json(&name, finding)?;
    }

    let mut gates_hit: Vec<GateHit> = Vec::new();
    for row in &triage.verdicts {
        // A hard gate fires on the check's own failure under `Always`, and on a *named* part's
        // failure under `SubChecks` (ADR-0002). The rule is asked once, here, so the exit code and
        // the report's gate list answer the same question.
        if matches!(row.verdict, substance_core::model::Verdict::Fail)
            && substance_core::meta(row.check_id).gate.gates(&row.parts)
        {
            gates_hit.push(GateHit {
                check_id: row.check_id,
                target: row.target.clone(),
            });
        }
    }
    gates_hit
        .sort_by(|left, right| (left.check_id, &left.target).cmp(&(right.check_id, &right.target)));

    let report = Report {
        schema_version: REPORT_SCHEMA_VERSION.to_owned(),
        manifest: manifest.clone(),
        verdicts: triage.verdicts,
        findings: triage.findings,
        coverage: triage.coverage,
        archetype: triage.archetype,
        provenance: triage.provenance,
        notes: {
            let mut notes = triage.notes_all;
            notes.extend(run_notes(&config, options.allow_exec, &source));
            notes.sort();
            notes.dedup();
            notes
        },
    };
    run_dir.write_json("report.json", &report)?;
    Ok(RunOutcome {
        report,
        run_dir: run_dir.as_path().to_path_buf(),
        gates_hit,
    })
}

/// The data `invocation.json` keeps: how the run was asked for, outside every hash.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Invocation {
    /// The stages this build ran, in order.
    stages: Vec<&'static str>,
    /// The directory the caller named, as they named it.
    input: String,
    /// The config source: `explicit`, `repository` or `defaults`.
    config_source: &'static str,
    /// Whether `--triage-only` was given.
    triage_only: bool,
    /// Whether `--snapshot-from` was given, and where it pointed.
    ///
    /// Kept here, never in the manifest: a replay changes no hash (§3 issue 29).
    snapshot_from: Option<String>,
    /// Whether external data was replayed rather than fetched.
    replay: bool,
    /// The run directory, for a caller reading the file on its own.
    run_dir: String,
}

impl Invocation {
    /// Records the options, keeping every value that no hash covers.
    ///
    /// No secrets can reach this file: the flags carry paths and numbers, and the origin
    /// URL is canonicalized to `local:<path>` or stripped of credentials before it is
    /// written to the manifest.
    fn of(options: &RunOptions, run_dir: &Path, config_source: &'static str) -> Self {
        Self {
            stages: vec![Stage::Intake.as_str(), Stage::Report.as_str()],
            input: options.repo.display().to_string(),
            config_source,
            triage_only: options.triage_only,
            snapshot_from: options
                .snapshot_from
                .as_ref()
                .map(|path| path.display().to_string()),
            replay: options.snapshot_from.is_some(),
            run_dir: run_dir.display().to_string(),
        }
    }
}

/// The notes about how this run was invoked, sorted so equal runs write equal bytes.
///
/// This milestone builds the triage stage only, so the note says which stages are absent rather
/// than leaving a reader to infer it from a list of `not_measured` verdicts. The `--allow-exec`
/// note and the disabled-check count are the other two facts a reader needs beside the numbers.
fn run_notes(config: &ProjectConfig, allow_exec: bool, source: &ConfigSource) -> Vec<String> {
    let mut notes = vec![
        "this build runs the triage stage only: the runtime, explore, spec, probe and judge \
         stages arrive in later milestones, so the checks that need them report not_measured \
         with their milestone"
            .to_owned(),
    ];
    notes.push(format!("config source: {}", source.kind()));
    if !allow_exec {
        notes.push("repo code was not executed; pass --allow-exec to allow it".to_owned());
    }
    let disabled = config.checks.disable.len();
    if disabled > 0 {
        notes.push(format!("{disabled} check(s) disabled by config"));
    }
    notes.sort();
    notes
}

/// Writes each probed tool's raw version output as a blob.
///
/// The manifest already points at these references; writing them here makes the
/// reference resolvable in the run directory, so a reader can see what each tool printed.
fn write_tool_blobs(
    run_dir: &Path,
    probes: &std::collections::BTreeMap<String, crate::tools::ToolProbe>,
) -> Result<(), RunError> {
    let store = substance_runner::BlobStore::new(run_dir);
    for probe in probes.values() {
        store.put(probe.raw.as_bytes())?;
    }
    Ok(())
}

/// The config's content hash, as `sha256:<hex>`.
fn hash_config(config: &ProjectConfig) -> Result<String, CanonicalJsonError> {
    config.config_hash()
}

/// Checks that `snapshot` is a run directory this build can replay from.
///
/// A missing directory is a usage error: the caller asked to replay data that is not
/// there, and continuing would silently fetch it instead, which is the one thing
/// `--snapshot-from` exists to prevent.
fn ensure_run_dir(snapshot: &Path) -> Result<(), RunError> {
    if !snapshot.is_dir() {
        return Err(RunError::Usage(format!(
            "{} is not a run directory; --snapshot-from needs a directory holding forge/, \
             registry/ and tools/",
            snapshot.display()
        )));
    }
    Ok(())
}

/// Creates `directory` if it does not exist.
fn ensure_dir(directory: &Path) -> Result<(), RunError> {
    std::fs::create_dir_all(directory).map_err(|source| RunError::Io {
        path: directory.to_path_buf(),
        source,
    })
}
