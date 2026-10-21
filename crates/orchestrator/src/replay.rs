//! `substance replay <id> --run <dir>`: rerun a recorded action and diff it (M3-T07).
//!
//! ## What a replay is for
//!
//! A finding links a record, and a record is only evidence if it can be reproduced. Replay restores
//! the record's snapshot, boots the app from the same commit, opens a context with the same seed and
//! instant, runs the record's `setup` and then its `action`, and compares the new normalized bytes
//! with the stored ones. Identical bytes are the claim "this run observed exactly this"; a diff is the
//! claim "it did then, and now it observes something else", which is a fact worth reporting rather
//! than a bug in the tool.
//!
//! ## The rules this module keeps
//!
//! - It never overwrites the record it replays: the new record goes to
//!   `replays/<rec_id>-<n>.json`. A replay that replaced its own evidence would destroy the thing it
//!   is checking.
//! - It needs `--allow-exec` in its own right. A replay boots the repository's code, and taking the
//!   permission from the run's `invocation.json` would mean one run made with `--allow-exec` grants
//!   permission to every replay of it, forever, on any machine. Section 4.1's rule is that the human
//!   at the keyboard grants that, per invocation.
//! - It cannot replay a static finding: there is no record to rerun, so the answer is exit 3 and a
//!   sentence naming M13-T04, never a silent success.
//! - It tears down on every path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use substance_adapters::boot::BootEnv;
use substance_adapters::db::factory::{Adapter, FactoryError};
use substance_adapters::stack::RepoView;
use substance_core::config::ProjectConfig;
use substance_core::model::protocol::{ActCommand, DriverCommand};
use substance_core::model::{Action, EffectRecord, Evidence, Finding, Measured, RunManifest};
use substance_core::normalize::normalized_record;
use substance_core::{RecordId, RunId};
use substance_recorder::context::{ContextPlan, open_context};
use substance_recorder::driver::{DriverConfig, DriverHost, DriverTimeouts};
use substance_recorder::record::{Observed, RecordContext, RecordStore};
use substance_runner::{ExecPermit, Limits, Registrar, StoppedApp};

use crate::config::load_config;
use crate::run::RunError;

/// The work copy a replay exports into. Named, so it cannot collide with the run's own `work/`.
const REPLAY_WORK_DIR: &str = "work-replay";

/// How long one boot, driver command or stop may take in a replay.
///
/// Generous on purpose: a replay is a person asking "does this still happen?", and a cap that fired
/// during a slow install would report a difference that is really a timeout.
const REPLAY_CAP_S: u64 = 300;

/// What the caller asked to replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// One record, by id.
    Record(RecordId),
    /// A finding, whose first record evidence is the thing to replay.
    Finding {
        /// The finding's id.
        id: String,
        /// The record to rerun.
        record: RecordId,
    },
}

impl Target {
    /// The record this target replays.
    #[must_use]
    pub fn record(&self) -> &RecordId {
        match self {
            Self::Record(id) => id,
            Self::Finding { record, .. } => record,
        }
    }
}

/// How a replay ended, which is also its exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The replay produced the same normalized bytes. Exit 0.
    Identical {
        /// The record that was replayed.
        record: RecordId,
        /// Where the new record was written.
        written: PathBuf,
    },
    /// The replay produced different bytes. Exit 1.
    Different {
        /// The record that was replayed.
        record: RecordId,
        /// Where the new record was written.
        written: PathBuf,
        /// The unified diff, `--- stored` / `+++ replayed`.
        diff: String,
    },
}

impl Ending {
    /// The process exit code this ending means: 0 identical, 1 different.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Identical { .. } => 0,
            Self::Different { .. } => 1,
        }
    }
}

/// The IDs a caller may name.
///
/// Split from the filesystem deliberately: a wrong prefix has to be a usage error (exit 2) whether or
/// not a run directory exists, and this part can be checked without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    /// `rec_…`
    Record(RecordId),
    /// `fnd_…`
    Finding(String),
    /// Something else, with the reason spelled out for the caller.
    Unknown(String),
}

/// Classifies the id the caller typed.
///
/// The shape is checked, not only the prefix: a `rec_` id is `rec_` and 26 base32 characters, and
/// accepting `rec_x` here would turn a typo into "no such record" -- a message about the run
/// directory for a mistake in the argument.
#[must_use]
pub fn classify(id: &str) -> Named {
    if let Some(rest) = id.strip_prefix(RecordId::PREFIX_WITH_SEPARATOR) {
        if valid_body(rest) {
            return Named::Record(RecordId::parse(id));
        }
        return Named::Unknown(format!(
            "{id} looks like a record id but its body is not 26 base32 characters"
        ));
    }
    if let Some(rest) = id.strip_prefix("fnd_") {
        if valid_body(rest) {
            return Named::Finding(id.to_owned());
        }
        return Named::Unknown(format!(
            "{id} looks like a finding id but its body is not 26 base32 characters"
        ));
    }
    Named::Unknown(format!(
        "{id} is neither a record id (rec_...) nor a finding id (fnd_...)"
    ))
}

/// Whether `body` is the 26 lowercase base32 characters an id carries.
#[must_use]
pub fn valid_body(body: &str) -> bool {
    body.len() == 26
        && body
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
}

/// The run directory a replay reads, and the documents in it.
#[derive(Debug, Clone)]
pub struct RecordedRun {
    /// The directory the caller named.
    pub dir: PathBuf,
    /// The manifest the run recorded.
    pub manifest: RunManifest,
}

/// Reads `manifest.json` from `dir`.
///
/// # Errors
///
/// Returns [`RunError::Usage`] when the directory or the manifest is missing or unreadable: every
/// one of those is something the caller can fix by naming a different directory, which is what exit 2
/// is for.
pub fn load_run(dir: &Path) -> Result<RecordedRun, RunError> {
    if !dir.is_dir() {
        return Err(RunError::Usage(format!(
            "{} is not a run directory",
            dir.display()
        )));
    }
    let path = dir.join("manifest.json");
    let bytes = std::fs::read(&path)
        .map_err(|source| RunError::Usage(format!("cannot read {}: {source}", path.display())))?;
    let manifest: RunManifest = serde_json::from_slice(&bytes).map_err(|source| {
        RunError::Usage(format!(
            "{} is not a run manifest: {source}",
            path.display()
        ))
    })?;
    Ok(RecordedRun {
        dir: dir.to_path_buf(),
        manifest,
    })
}

/// The record a [`Named`] target refers to, and the record itself.
///
/// # Errors
///
/// Returns [`RunError::Usage`] when the record does not exist in this run, and [`RunError::Replay`]
/// (exit 3) when the target is a finding with no record evidence.
pub fn load_record(run: &RecordedRun, named: &Named) -> Result<(Target, EffectRecord), RunError> {
    match named {
        Named::Unknown(reason) => Err(RunError::Usage(reason.clone())),
        Named::Record(id) => {
            let record = read_record(&run.dir, id)?;
            Ok((Target::Record(id.clone()), record))
        }
        Named::Finding(id) => {
            let path = run.dir.join("findings").join(format!("{id}.json"));
            let bytes = std::fs::read(&path).map_err(|source| {
                RunError::Usage(format!("cannot read {}: {source}", path.display()))
            })?;
            let finding: Finding = serde_json::from_slice(&bytes).map_err(|source| {
                RunError::Usage(format!("{} is not a finding: {source}", path.display()))
            })?;
            let record = finding.evidence.iter().find_map(|evidence| match evidence {
                Evidence::Record(id) => Some(id.clone()),
                _ => None,
            });
            let Some(record) = record else {
                return Err(RunError::Replay(format!(
                    "{id} cites no effect record, so there is nothing to rerun: replay for static findings arrives in M13-T04"
                )));
            };
            let stored = read_record(&run.dir, &record)?;
            Ok((
                Target::Finding {
                    id: id.clone(),
                    record,
                },
                stored,
            ))
        }
    }
}

/// Reads `records/<id>.json`.
fn read_record(dir: &Path, id: &RecordId) -> Result<EffectRecord, RunError> {
    let path = dir.join("records").join(format!("{}.json", id.as_str()));
    let bytes = std::fs::read(&path)
        .map_err(|source| RunError::Usage(format!("cannot read {}: {source}", path.display())))?;
    serde_json::from_slice(&bytes).map_err(|source| {
        RunError::Usage(format!(
            "{} is not an effect record: {source}",
            path.display()
        ))
    })
}

/// The run id a directory belongs to, from its manifest.
#[must_use]
pub fn run_id_of(run: &RecordedRun) -> &RunId {
    &run.manifest.run_id
}

/// Where a replay's own record goes: `replays/<rec_id>-<n>.json`, one past the highest there.
///
/// Never the record's own path: that file is the evidence being checked. The count is read from the
/// directory rather than kept in memory, so two replays in two processes cannot choose the same name.
#[must_use]
pub fn replay_path(dir: &Path, record: &RecordId) -> PathBuf {
    let replays = dir.join("replays");
    let prefix = format!("{}-", record.as_str());
    let mut highest = 0_u32;
    if let Ok(entries) = std::fs::read_dir(&replays) {
        // Collected and sorted, so the answer does not depend on the order the filesystem returned
        // the entries in.
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let Some(rest) = name.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let Some(digits) = rest.strip_suffix(".json") else {
                continue;
            };
            if let Ok(number) = digits.parse::<u32>() {
                highest = highest.max(number);
            }
        }
    }
    replays.join(format!("{}-{}.json", record.as_str(), highest + 1))
}

/// Compares two records and produces the ending a caller prints and exits with.
///
/// The comparison is on [`normalized_record`], not on the raw JSON: a replay of one action
/// legitimately differs in what normalization removes (a settle time, a fresh id), and comparing raw
/// bytes would report every replay as a difference.
///
/// # Errors
///
/// Returns [`RunError::Replay`] when either record cannot be normalized, which is an internal
/// failure rather than a finding about the app.
pub fn compare(
    stored: &EffectRecord,
    replayed: &EffectRecord,
    record: &RecordId,
    written: PathBuf,
) -> Result<Ending, RunError> {
    let left = normalized_record(stored).map_err(|error| RunError::Replay(error.to_string()))?;
    let right = normalized_record(replayed).map_err(|error| RunError::Replay(error.to_string()))?;
    if left == right {
        return Ok(Ending::Identical {
            record: record.clone(),
            written,
        });
    }
    // Indented rather than the canonical single line: a diff of one 40 KB line names nothing.
    let old_text = pretty(&left)?;
    let new_text = pretty(&right)?;
    Ok(Ending::Different {
        record: record.clone(),
        written,
        diff: substance_core::diff::unified(&old_text, &new_text, "stored", "replayed"),
    })
}

/// The normalized bytes as indented JSON, so a diff can point at a field.
fn pretty(bytes: &[u8]) -> Result<String, RunError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| RunError::Replay(format!("the normalized record is not JSON: {error}")))?;
    serde_json::to_string_pretty(&value)
        .map_err(|error| RunError::Replay(format!("cannot render the diff: {error}")))
}

/// Everything a replay needs from the caller, beyond the arguments.
#[derive(Debug)]
pub struct ReplayOptions {
    /// The run directory the record belongs to.
    pub run: PathBuf,
    /// The effect record or finding to replay.
    pub id: String,
    /// The pinned tool table, for an adapter that needs one (Supabase's CLI).
    pub tools: Vec<(String, substance_core::tools::ToolSpec)>,
    /// The permission to run repo-controlled code; the caller grants it after checking
    /// `allow_exec`.
    pub permit: Option<ExecPermit>,
    /// Whether `--allow-exec` was given.
    ///
    /// Required, and required *here*: replay boots the repository's code, and a permission read from
    /// the recorded run would be a grant made once and used forever (see the module docs).
    pub allow_exec: bool,
    /// Where a fresh work copy is exported from: the repository the run came from.
    ///
    /// Named by the caller rather than read back from the run, because a run directory does not
    /// record the path it was invoked from in the manifest -- `invocation.json` does, but it is the
    /// caller's own path and may not exist on this machine.
    pub source: Option<PathBuf>,
}

/// Runs a replay, or reports why it cannot.
///
/// This is the whole stage: load, restore, boot, drive, write, compare. Every exit from the body
/// stops what it started, which is why the work is in one function with a single `finally`-shaped
/// tail rather than spread across helpers that could each return early.
///
/// # Errors
///
/// Returns [`RunError::Usage`] for an id or a run directory that cannot be used (exit 2), and
/// [`RunError::Replay`] when the replay cannot run on this host (exit 3).
pub async fn replay(options: &ReplayOptions) -> Result<Ending, RunError> {
    let named = classify(&options.id);
    // Checked before the run is read, so a bad id is a usage error about the id rather than about a
    // missing directory.
    if let Named::Unknown(reason) = &named {
        return Err(RunError::Usage(reason.clone()));
    }
    if !options.allow_exec {
        return Err(RunError::Usage(
            "replay boots the repository's code, so it needs --allow-exec of its own: a run made with \
             it does not grant it to every later replay"
                .to_owned(),
        ));
    }
    let run = load_run(&options.run)?;
    let (target, stored) = load_record(&run, &named)?;
    let record = target.record().clone();
    let written = replay_path(&run.dir, &record);
    // The replay body proper. Kept in its own function so the teardown below cannot be skipped by an
    // early return in it.
    let replayed = drive(options, &run, &stored).await?;
    compare(&stored, &replayed, &record, written)
}

/// Restores the snapshot, boots, drives the record's setup and action, and returns the new record.
///
/// The shape is linear: a work copy, then a database reset, then the app, then the driver, each step
/// holding only what the next needs. A step that fails returns early and leaves the earlier handles to
/// their drops, which is why nothing here is spawned before the thing that needs it.
///
/// # Errors
///
/// Returns [`RunError::Usage`] when the caller has to name something else, and [`RunError::Replay`]
/// when a step of the replay itself cannot run.
async fn drive(
    options: &ReplayOptions,
    run: &RecordedRun,
    stored: &EffectRecord,
) -> Result<EffectRecord, RunError> {
    let source = options.source.clone().ok_or_else(|| {
        RunError::Usage(
            "replay needs --source <repo>: a run directory does not record the repository it was invoked from, and a work copy has to be exported from somewhere"
                .to_owned(),
        )
    })?;
    let (config, _) = load_config(None, source.as_path())?;
    // The config the run used, not the config the repository has now: a replay that picked up an
    // edited config would be measuring a different app.
    let now = config.config_hash().map_err(RunError::Serialize)?;
    if now != run.manifest.config_hash {
        return Err(RunError::Usage(format!(
            "the config at {} has changed since the run ({now} now, {} then), so a replay would measure a different app",
            source.display(),
            run.manifest.config_hash
        )));
    }
    let manifest = &run.manifest;

    // 1. A fresh work copy at the manifest's commit, named so it cannot collide with the run's own.
    let work = substance_runner::WorkCopy::create_named(
        &source,
        &manifest.repo.commit_sha,
        &run.dir,
        &manifest.run_id,
        REPLAY_WORK_DIR,
    )
    .await
    .map_err(|error| RunError::Replay(format!("cannot export the work copy: {error}")))?;

    // 2. The database, reset to the record's snapshot. Reset after the copy exists and before
    //    anything boots, so the app opens a database this replay owns.
    let adapter = Adapter::build(
        &config,
        &manifest.run_id,
        work.root(),
        &run.dir,
        &options.tools,
    )
    .map_err(|error| match error {
        FactoryError::Config(reason) => RunError::Usage(reason),
        FactoryError::Adapter(error) => {
            RunError::Replay(format!("cannot build the database adapter: {error}"))
        }
    })?;
    // The reset needs proof no app is running, and this is the only place in the process where that
    // proof can come from "none was started": no `AppProcess` is spawned above this line, and every
    // one below it is reached only through `rerun`, which is called after the reset returns. That
    // ordering is the whole argument, so this call stays immediately before the boot and must move if
    // anything above it ever spawns a process. The recorded run's own app cannot be the one writing:
    // it lives in another process this one cannot see, and it exited before the record was written.
    let stopped = StoppedApp::nothing_started();
    adapter
        .as_trait()
        .reset(&stored.snapshot, &stopped)
        .await
        .map_err(|error| RunError::Replay(format!("cannot restore the snapshot: {error}")))?;

    let permit = options.permit.as_ref().ok_or_else(|| {
        RunError::Usage("replay boots the app, so it needs a granted permit".to_owned())
    })?;
    // Everything this replay starts is registered before it exists, so a Ctrl-C during the boot
    // reaches the app and the database rather than leaving them behind.
    let registrar = Registrar::new();
    let record = rerun(run, &config, work.root(), stored, permit, &registrar).await?;
    // The app is stopped by `rerun` before it returns; the copy goes with this scope.
    drop(work);
    Ok(record)
}

/// Boots the app and drives the record's setup and action through a driver, returning the new record.
///
/// Every resource is started here and stopped before the return: the app by [`substance_runner::OwnedApp`]'s
/// own stop, the driver by [`DriverHost::stop`], and the interrupt registrar by the caller. The record
/// that comes back is written by the caller, so a failure to write cannot lose the evidence.
async fn rerun(
    run: &RecordedRun,
    config: &ProjectConfig,
    work: &Path,
    stored: &EffectRecord,
    permit: &ExecPermit,
    registrar: &Registrar,
) -> Result<EffectRecord, RunError> {
    let manifest = &run.manifest;
    // The frozen clock the manifest recorded, detected on *this* host: a replay on a host without
    // libfaketime cannot reproduce a run that had one, and says so rather than pretending.
    let libfaketime = substance_runner::host::detect_libfaketime();
    let here = substance_core::clock_controls::server_clock(&libfaketime);
    if here != manifest.server_clock {
        return Err(RunError::Replay(format!(
            "the run recorded server_clock {:?} and this host would give {here:?}, so the action would not run under the same clock",
            manifest.server_clock
        )));
    }
    let mut env = std::collections::BTreeMap::new();
    for (key, value) in substance_core::clock_controls::frozen_env(&libfaketime, manifest.as_of) {
        env.insert(key, value);
    }
    let logs = run.dir.join("logs");
    let home = run.dir.join(substance_runner::HOME_DIR);
    let repo = RepoView::new(work);
    if substance_adapters::ladder::preferred_rung(&repo, config).is_none() {
        return Err(RunError::Replay(
            "no boot rung is available for this repository, so the action cannot be rerun"
                .to_owned(),
        ));
    }

    // 3. The app.
    let boot_env = BootEnv {
        work,
        home: &home,
        logs: &logs,
        limits: Limits::detect(),
        fresh_copy: None,
        defer_seed: true,
        env: &env,
    };
    let booted = substance_adapters::ladder::boot(
        &repo,
        config,
        &boot_env,
        substance_runner::Budget::after("replay", Duration::from_secs(REPLAY_CAP_S)),
        permit,
    )
    .await
    .map_err(|error| RunError::Replay(format!("the app could not boot: {error}")))?;
    let Some(mut app) = booted.app else {
        return Err(RunError::Replay(
            "the app booted no process, so there is nothing to drive".to_owned(),
        ));
    };
    // Registered as soon as it exists, and forgotten after a clean stop, so a later Ctrl-C cannot
    // signal a pid the OS has given to something else.
    let pgid = app.pgid();
    registrar.register_app("app", pgid);
    let Some(port) = app.port() else {
        let _ = app.stop(Duration::from_secs(REPLAY_CAP_S)).await;
        return Err(RunError::Replay(
            "the app reported no port, so the action's URL cannot be built".to_owned(),
        ));
    };
    let origin = format!("http://127.0.0.1:{port}");

    // 4. The driver and the context.
    let mut driver = match DriverHost::spawn(
        &run.dir,
        &logs,
        &DriverConfig::new(DriverTimeouts::for_settle(
            config.budgets.settle_timeout_ms,
            u32::try_from(REPLAY_CAP_S).unwrap_or(u32::MAX),
        )),
    )
    .await
    {
        Ok(driver) => driver,
        Err(error) => {
            let _ = app.stop(Duration::from_secs(REPLAY_CAP_S)).await;
            return Err(RunError::Replay(format!(
                "the driver could not start: {error}"
            )));
        }
    };

    let outcome = drive_actions(
        &mut driver,
        config,
        stored,
        &origin,
        &run.dir,
        manifest.as_of,
    )
    .await;
    // Stopped in this order, whatever happened above: the driver first (it holds a browser attached
    // to the app), then the app. A driver left running would keep a browser alive against a dead
    // server, and its retries would show up as the *next* command's failures.
    if let Err(error) = driver.stop().await {
        tracing::warn!(%error, "replay: the driver did not stop cleanly");
    }
    let stopped = app.stop(Duration::from_secs(REPLAY_CAP_S)).await;
    if stopped.is_ok() {
        registrar.forget_app(pgid);
    }
    if let Err(error) = stopped {
        tracing::warn!(%error, "replay: the app did not stop cleanly");
    }
    outcome
}

/// Runs the record's setup and action through `driver`, and assembles the new record.
///
/// The setup steps are run in order and then the action, which is exactly what the recorded run did:
/// a replay that skipped setup would drive the action on a screen it was never recorded on.
async fn drive_actions(
    driver: &mut DriverHost,
    config: &ProjectConfig,
    stored: &EffectRecord,
    origin: &str,
    run_dir: &Path,
    as_of: substance_core::Timestamp,
) -> Result<EffectRecord, RunError> {
    let store = RecordStore::new(run_dir);
    let context = open_context(
        config,
        &ContextPlan {
            blob_dir: blob_dir_string(run_dir),
            storage_state: None,
            idle_patterns: Vec::new(),
            as_of,
        },
    )
    .map_err(|error| RunError::Replay(format!("cannot build open_context: {error}")))?;
    if let Err(error) = driver.send(&DriverCommand::OpenContext(context)).await {
        return Err(RunError::Replay(format!(
            "the context could not open: {error}"
        )));
    }

    for step in &stored.setup {
        run_step(driver, step, origin).await?;
    }
    // The action is the one that produces the record: everything before it was preparation, and its
    // own part is the observation being compared.
    let part = match stored.action.kind.as_str() {
        "goto" => {
            driver
                .record(&DriverCommand::Goto {
                    url: url_for(&stored.action.screen, origin),
                })
                .await
        }
        kind => {
            driver
                .record(&DriverCommand::Act(ActCommand {
                    kind: kind.to_owned(),
                    target: stored.action.target.clone(),
                    value: stored.action.input.get("value").cloned(),
                }))
                .await
        }
    };
    let part = match part {
        Measured::Measured { value } => value,
        Measured::NotMeasured { reason } => {
            return Err(RunError::Replay(format!(
                "the action produced no observation: {reason}"
            )));
        }
    };

    // The channels this replay cannot watch yet are not_measured with the reason, never empty: a
    // replay has no row capture attached and no proxy restart, and a record that claimed otherwise
    // would compare unequal to the stored one for a reason that is about the tool.
    let observed = Observed::new();
    let context = RecordContext {
        run_id: stored.run_id.clone(),
        check_id: stored.check_id,
        target: stored.target.clone(),
        snapshot: stored.snapshot.clone(),
        repetition: stored.repetition,
        seed: stored.seed,
        setup: stored.setup.clone(),
        action: stored.action.clone(),
    };
    store
        .assemble(&context, &part, &observed, None)
        .map_err(|error| RunError::Replay(format!("cannot assemble the record: {error}")))
}

/// Runs one setup step, ignoring its observation.
async fn run_step(driver: &mut DriverHost, step: &Action, origin: &str) -> Result<(), RunError> {
    let command = match step.kind.as_str() {
        "goto" => DriverCommand::Goto {
            url: url_for(&step.screen, origin),
        },
        kind => DriverCommand::Act(ActCommand {
            kind: kind.to_owned(),
            target: step.target.clone(),
            value: step.input.get("value").cloned(),
        }),
    };
    driver
        .send(&command)
        .await
        .map(|_| ())
        .map_err(|error| RunError::Replay(format!("a setup step could not run: {error}")))
}

/// The absolute URL for a recorded route: the app's origin plus the path the record names.
fn url_for(screen: &str, origin: &str) -> String {
    if screen.starts_with("http://") || screen.starts_with("https://") {
        return screen.to_owned();
    }
    if screen.starts_with('/') {
        return format!("{origin}{screen}");
    }
    format!("{origin}/{screen}")
}

/// The blob directory as the driver spells it.
fn blob_dir_string(run_dir: &Path) -> String {
    run_dir.join("blobs").display().to_string()
}
