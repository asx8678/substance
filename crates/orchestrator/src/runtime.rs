//! The runtime stage: database, app, seed, accounts, `seeded` snapshot (M2-T04).
//!
//! ## The order, and why it is that order
//!
//! ```text
//! provision DB -> boot app -> seed -> accounts -> stop -> snapshot "seeded"
//! ```
//!
//! - **Provision before boot** because the app reads its connection string from the environment, and
//!   a server started before its database exists caches a failed connection.
//! - **Seed before accounts** because the repository's own seed may create the accounts an
//!   `existing` account refers to; creating them first would collide with it.
//! - **Stop before snapshot** because a copy of a database being written is not a copy of anything.
//!   The stop also produces the `StoppedApp` token the snapshot requires, which is why the boot hands
//!   back a running `BootedApp` rather than a port: a function that stopped its own app could never
//!   produce that token.
//!
//! ## Registration, so Ctrl-C reaches what a cancelled future leaves behind
//!
//! The app's process group and the database adapter are registered with the run's `Registrar` as
//! soon as they exist. When a run is interrupted, the future this stage is inside is dropped at an
//! await point, so nothing here runs its own cleanup: the interrupt task signals the group and awaits
//! the adapter's teardown instead. The group is forgotten after a clean stop, so a stale pid is never
//! signalled.
//!
//! ## What a missing piece means
//!
//! Every early return produces a *note*, not an error: a repository with no database, no boot plan or
//! no `--allow-exec` still has a report to write, and the checks that need this stage then report
//! `not_measured` with the note's reason.
//!
//! ## Signup is loopback, so it is not egress
//!
//! The `api` method posts to the app's own origin (`http://127.0.0.1:<port>`). §4.1's egress proxy
//! governs what reaches the network, and the app's own loopback listener is not the network: routing
//! this through a proxy would make creating an account depend on the proxy being up. Every other
//! outbound request in the plan goes through the proxy in M3.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use substance_adapters::accounts::{
    AccountError, AccountPlan, SignupRequest, password_for, render,
};
use substance_adapters::boot::BootEnv;
use substance_adapters::boot::StepFact;
use substance_adapters::db::cleanup::DatabaseCleanup;
use substance_adapters::db::factory::{self, Adapter, FactoryError};
use substance_adapters::ladder::{self, BootedApp};
use substance_adapters::seed::{self, DetectedSeed};
use substance_adapters::stack::RepoView;
use substance_adapters::ts::package_manager;
use substance_core::config::{ProjectConfig, UserConfig, UserCreate};
use substance_core::ids::RunId;
use substance_core::model::seeding::{Canaries, Canary, seeds_for};
use substance_core::model::{BootOutcome, BootRung, CanaryKind, CommandSpec, DbHandle};
use substance_core::tools::ToolSpec;
use substance_runner::ExitInfo;
use substance_runner::{Budget, ExecPermit, Limits, Registrar};

/// The label the seed snapshot carries.
///
/// A constant because it is derived into a snapshot id: renaming it would make every later reset look
/// for a snapshot that does not exist.
pub const SEEDED: &str = "seeded";

/// The generators this build derives seeds for.
///
/// Named here rather than computed, because each name is part of its seed's derivation: a generator
/// the run never instantiates would be a pinned input that pins nothing, and renaming one changes
/// every seed it ever produced.
pub const GENERATORS: &[&str] = &["bulk", "form", "invalid_payload"];

/// How long one repo-controlled command in this stage may take.
///
/// A safety cap (§4.3), not a target. One number for install, build, migrate and seed together,
/// because a reader comparing an `npm ci` on two machines wants one number they can point at.
const STEP_CAP: Duration = Duration::from_secs(1800);

/// How long a stop waits for the app's process group to empty.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// How long the whole stage may take, in seconds.
const STAGE_CAP_S: u64 = 3600;

/// How long the README trial may take, separately from the stage (M2-T05).
///
/// Its own cap because it is a second boot inside one stage: a README that starts a dev server and
/// never answers would otherwise eat the whole stage's budget and leave the checks after it
/// unmeasured. A quarter of the stage's cap, which is enough for an install and a start.
const README_CAP_S: u64 = 900;

/// How long a signup request may take.
///
/// Short: the app is on loopback and already answering its ready path, so a signup that takes longer
/// than this is not slow, it is stuck.
const SIGNUP_CAP: Duration = Duration::from_secs(30);

/// How long Mailpit is given to start. It is a single binary that binds two ports, so a wait
/// longer than seconds means it did not start at all.
const HELPER_CAP_S: u64 = 30;

/// What the runtime stage needs from the run it is part of.
pub struct RuntimeInput<'a> {
    /// The config, for commands, accounts and the database.
    pub config: &'a ProjectConfig,
    /// The work copy the app runs in.
    pub work: &'a Path,
    /// The run directory: home, logs, snapshots, `canaries.json`.
    pub run_dir: &'a Path,
    /// The run's id, which names its databases.
    pub run_id: &'a RunId,
    /// The master seed every generator seed derives from.
    pub master_seed: u64,
    /// The tool table, for Supabase's pinned CLI.
    pub tools: &'a [(String, ToolSpec)],
    /// The permission to run repo-controlled code, when `--allow-exec` was given.
    pub permit: Option<&'a ExecPermit>,
    /// The run's interrupt registrar, so a Ctrl-C reaches the app and the database.
    pub registrar: &'a Registrar,
    /// Where egress bodies and email bodies go.
    pub blobs: substance_runner::BlobStore,
}

/// What the runtime stage produced.
#[derive(Debug, Default)]
pub struct RuntimeOutcome {
    /// How the boot ended: the rung, its friction and whether it booted.
    pub boot: Option<BootOutcome>,
    /// The accounts that exist, by handle, sorted.
    pub accounts: Vec<String>,
    /// The canaries written to `canaries.json`.
    pub canaries: Canaries,
    /// The seed snapshot's id, when one was taken.
    pub snapshot: Option<String>,
    /// The database engine, when one was provisioned.
    pub engine: Option<String>,
    /// Every fact a reader needs that is not a verdict, sorted.
    pub notes: Vec<String>,
    /// The generator seeds this stage used, for the manifest.
    pub seeds: BTreeMap<String, u64>,
    /// Every one-shot step the boot ran: install, build, migrate and seed.
    ///
    /// O01's exec part reads the install step from here, and O04 the migrate step. Recorded even
    /// when the boot failed, because a failed install is exactly what O01 has to see.
    pub steps: Vec<StepFact>,
    /// What the stop observed when the app's group was signalled, for O03.
    ///
    /// `None` when the app was never stopped by this stage, which is `not_measured`: a token this
    /// run did not produce is not evidence about this run's shutdown.
    pub shutdown: Option<ExitInfo>,
    /// What the README rung did when it was tried on its own, for O02.
    ///
    /// `None` means there was no README rung to try, which is `not_measured`. `Some(Ok(()))` means
    /// the documented setup booted the app; `Some(Err(reason))` means it did not, and the reason is
    /// the evidence.
    pub readme: Option<Result<(), String>>,
    /// Whether the migrate command survived a second run, for O04.
    ///
    /// `None` when no migrate command ran successfully the first time: there is then nothing to
    /// re-run, and the check says so rather than passing.
    pub second_migrate: Option<MigrateRun>,
    /// What the egress proxy saw, for the recorder's egress channel.
    ///
    /// `None` when the proxy never started, which is `not_measured` on every record rather than an
    /// empty list: "no third-party call" and "nothing was watching" are different claims.
    pub egress: Option<substance_runner::egress::EgressLog>,
    /// The mailbox's API port, for the recorder's email channel.
    ///
    /// `None` when Mailpit is not on PATH. The port rather than the client, because the messages
    /// have to be read per action window, after the app has sent them.
    pub mail_http_port: Option<u16>,
    /// The SMTP port the app was told to send to, recorded so a reader can see it was wired up.
    pub mail_smtp_port: Option<u16>,
    /// What each screen does with no action, keyed by route (M3-T05).
    ///
    /// Written to `idle_patterns.json` so a record can be assembled by excluding exactly the traffic
    /// the baseline saw. Empty in this build: the baseline is taken by a driver command
    /// (`observe_idle`), and the driver is spawned by the crawler in M4-T01, not by this stage. An
    /// empty map excludes nothing, which is the honest default rather than a claim of quiet.
    pub idle: BTreeMap<String, IdleBaseline>,
}

/// One screen's idle baseline: what the page does on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdleBaseline {
    /// Request patterns, as `METHOD /templated/path`, sorted and deduplicated.
    pub patterns: Vec<String>,
    /// Socket pings, as `WS /templated/path`, sorted.
    pub pings: Vec<String>,
}

/// One re-run of the migrate command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateRun {
    /// Whether it exited 0.
    pub success: bool,
    /// Its exit code, absent when a signal ended it.
    pub code: Option<i32>,
    /// The command, for the message.
    pub command: String,
}

impl RuntimeOutcome {
    /// The steps this outcome got through, in pipeline order.
    ///
    /// Not the pipeline's stage list: the run has one runtime stage (see [`Stage`]), and provision,
    /// boot, seed, accounts and snapshot are steps inside it. Recorded separately because a reader
    /// comparing two runs wants to know how far this one got, and which step stopped it.
    #[must_use]
    pub fn steps(&self) -> Vec<&'static str> {
        let mut steps = Vec::new();
        if self.engine.is_some() {
            steps.push("provision");
        }
        if self.boot.as_ref().is_some_and(|boot| boot.booted) {
            steps.push("boot");
        }
        if !self.accounts.is_empty() {
            steps.push("accounts");
        }
        if self.snapshot.is_some() {
            steps.push("snapshot");
        }
        steps
    }
}

/// Runs the runtime stage.
///
/// # Errors
///
/// Returns a note describing why the stage could not run at all. A repository without a boot plan is
/// not an error: the note says so and the checks that need an app report `not_measured`.
pub async fn run(input: &RuntimeInput<'_>) -> Result<RuntimeOutcome, String> {
    let mut outcome = RuntimeOutcome {
        seeds: seeds_for(input.master_seed, GENERATORS),
        canaries: canaries_of(input),
        ..RuntimeOutcome::default()
    };
    // Derived before anything runs: the manifest records the seeds, and a seed derived after a
    // partial run would be a seed the report cannot be compared against. The canaries are written
    // even when nothing runs, so the file's absence never stands in for "nothing to report".

    let Some(permit) = input.permit else {
        outcome.notes.push(
            "runtime: without --allow-exec nothing is installed, booted, migrated, seeded or \
             snapshotted"
                .to_owned(),
        );
        return Ok(outcome);
    };

    let database = provision(input, &mut outcome).await;
    let mut env = match database.as_ref() {
        Some((adapter, handle)) => app_env(adapter, handle),
        None => BTreeMap::new(),
    };

    // The outside world is closed *before* the app starts, because the app reads its proxy
    // variables at boot: a client that captures them once would otherwise keep talking to the
    // network for the whole run, unseen.
    let egress = start_egress(input, &env, &mut outcome).await;
    if let Some(egress) = &egress {
        let bypass: Vec<String> = db_hosts(&env).into_iter().collect();
        for (key, value) in egress.env(&bypass) {
            env.insert(key, value);
        }
    }
    let mail = start_mailpit(input, input.config, &mut env, &mut outcome).await;

    // Booted even without a database: a repository whose checks need none still gets its rung, its
    // friction and its boot outcome recorded.
    let Some(mut booted) = boot(input, permit, &env, &mut outcome).await else {
        stop_helpers(egress, mail, &mut outcome).await;
        return Ok(outcome);
    };

    // O04 before anything writes to the database: the migrate command's second run is judged on
    // the state the first one left, and a seed would be a second writer in between.
    outcome.second_migrate = migrate_twice(input, permit, &env, &outcome.steps).await;

    let Some((adapter, handle)) = database else {
        // No database: no seed, no accounts, no snapshot. The app still stops here, because a
        // process this stage started and did not stop would outlive the run.
        let _ = stop(input, &mut booted, &mut outcome).await;
        stop_helpers(egress, mail, &mut outcome).await;
        return Ok(outcome);
    };

    // Seed, then accounts; the app is running throughout, because the repository's seed may need it
    // and `api` accounts are created through it.
    let seeded = seed_repository(input, &adapter, &handle, permit, &mut outcome).await;
    let created = create_accounts(input, &booted, permit, &mut outcome).await;

    // Stop, then snapshot. Stopped whatever happened: a failed seed still leaves a process running,
    // and a snapshot is only worth taking when both steps succeeded.
    let token = stop(input, &mut booted, &mut outcome).await;
    match (token, seeded && created) {
        (Some(stopped), true) => match adapter.as_trait().snapshot(SEEDED, &stopped).await {
            Ok(id) => {
                outcome
                    .notes
                    .push(format!("runtime: the {SEEDED} snapshot is {}", id.as_str()));
                outcome.snapshot = Some(id.as_str().to_owned());
            }
            Err(error) => outcome
                .notes
                .push(format!("runtime: the {SEEDED} snapshot failed: {error}")),
        },
        (Some(_), false) => outcome.notes.push(format!(
            "runtime: no {SEEDED} snapshot, because {} did not complete",
            if seeded {
                "account creation"
            } else {
                "seeding"
            }
        )),
        // No token means the stop could not confirm the process group was empty, and a copy of a
        // database an unconfirmed process may still be writing is not a point in time.
        (None, _) => outcome.notes.push(
            "runtime: no seeded snapshot, because the app could not be stopped cleanly".to_owned(),
        ),
    }
    stop_helpers(egress, mail, &mut outcome).await;
    outcome.notes.sort();
    outcome.notes.dedup();
    Ok(outcome)
}

/// The canaries one run generates, from its master seed and its accounts.
///
/// Derived even when nothing ran: `canaries.json` is what a later check reads, and a file that
/// appeared only on a full run would make B13's absence indistinguishable from a missing file.
#[must_use]
pub fn canaries_of(input: &RuntimeInput<'_>) -> Canaries {
    let mut canaries = Canaries {
        canaries: input
            .config
            .users
            .iter()
            .map(|user| Canary {
                value: substance_core::model::canary_value(input.master_seed, &user.handle),
                owner: user.handle.clone(),
                role: user.role.clone(),
                kind: CanaryKind::UserData,
            })
            .collect(),
    };
    canaries.sort();
    canaries
}

/// The environment the app needs to reach its database.
///
/// An adapter that cannot build its own URL is a configuration problem this stage cannot fix; the
/// empty map makes the app fail to connect rather than fail to start, which is the failure a reader
/// can act on.
fn app_env(adapter: &Adapter, handle: &DbHandle) -> BTreeMap<String, String> {
    adapter
        .as_trait()
        .app_env(handle)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Provisions the database, or records the note explaining why there is none.
async fn provision(
    input: &RuntimeInput<'_>,
    outcome: &mut RuntimeOutcome,
) -> Option<(Adapter, DbHandle)> {
    if let Err(reason) = factory::configured(input.config) {
        outcome.notes.push(format!("runtime: {reason}"));
        return None;
    }
    let adapter = match Adapter::build(
        input.config,
        input.run_id,
        input.work,
        input.run_dir,
        input.tools,
    ) {
        Ok(adapter) => adapter,
        Err(FactoryError::Config(reason)) => {
            outcome.notes.push(format!("runtime: {reason}"));
            return None;
        }
        Err(FactoryError::Adapter(error)) => {
            outcome.notes.push(format!("runtime: no database: {error}"));
            return None;
        }
    };
    outcome.engine = Some(adapter.engine().to_owned());
    // Registered before provisioning, so an interrupt during the create still tears down what the
    // create made. The adapter's own guard is armed by `provision`; this is the path that runs when
    // the future holding the adapter is cancelled.
    input
        .registrar
        .register_cleanup(Arc::new(DatabaseCleanup::of(adapter.engine())));

    let handle = match &adapter {
        // Supabase cannot provision through the trait: `start` applies the repository's migrations
        // and so needs the permit, which is why the factory keeps the concrete type.
        Adapter::Supabase(stack) => match input.permit {
            Some(permit) => stack.start(permit).await,
            None => {
                outcome.notes.push(
                    "runtime: supabase needs --allow-exec to start, because it applies the \
                     repository's migrations"
                        .to_owned(),
                );
                return None;
            }
        },
        other => other.as_trait().provision().await,
    };
    match handle {
        Ok(handle) => Some((adapter, handle)),
        Err(error) => {
            outcome.notes.push(format!(
                "runtime: the database could not be provisioned: {error}"
            ));
            None
        }
    }
}

/// Starts the egress proxy, or records why it could not start.
///
/// A proxy that cannot start is a note, not a failed run: the checks that need the egress channel
/// then report `not_measured` with this reason, which is worse evidence but not a broken run.
async fn start_egress(
    input: &RuntimeInput<'_>,
    env: &BTreeMap<String, String>,
    outcome: &mut RuntimeOutcome,
) -> Option<substance_runner::egress::Egress> {
    let allow = &input.config.egress.allow;
    match substance_runner::egress::Egress::start(input.run_dir, allow, input.blobs.clone()).await {
        Ok(egress) => {
            outcome.notes.push(format!(
                "runtime: the egress proxy listens on {} and allows {} host(s)",
                egress.address(),
                allow.len()
            ));
            outcome.egress = Some(egress.log().clone());
            Some(egress)
        }
        Err(error) => {
            outcome.notes.push(format!(
                "runtime: the egress proxy was not started: {error}"
            ));
            let _ = env;
            None
        }
    }
}

/// The hosts a proxied client must reach directly: the database, and the app's own loopback.
///
/// `NO_PROXY` entries, not allow-list entries: these are reached without the proxy, so they are
/// invisible to the egress channel. The database is here because a proxied SQL connection would be
/// a protocol the proxy cannot speak, and loopback because the app talking to itself is not egress.
fn db_hosts(env: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut hosts = BTreeSet::new();
    for name in ["DATABASE_URL", "PGHOST", "DATABASE_HOST"] {
        let Some(value) = env.get(name) else {
            continue;
        };
        // A URL carries its host between `@` and the next `:` or `/`; a bare host is itself.
        let after_scheme = value
            .split_once("://")
            .map_or(value.as_str(), |(_, rest)| rest);
        let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
        let host_part = authority.rsplit('@').next().unwrap_or(authority);
        let host = host_part.split(':').next().unwrap_or(host_part);
        if !host.is_empty() && !host.contains('[') {
            hosts.insert(host.to_owned());
        }
    }
    hosts
}

/// Starts Mailpit and applies `[mail] env` to the app, or records why the channel is unavailable.
async fn start_mailpit(
    input: &RuntimeInput<'_>,
    config: &ProjectConfig,
    env: &mut BTreeMap<String, String>,
    outcome: &mut RuntimeOutcome,
) -> Option<substance_runner::mail::Mailpit> {
    if config.mail.env.is_empty() {
        outcome.notes.push(
            "runtime: [mail] env is empty, so the app was not told where to send mail".to_owned(),
        );
        return None;
    }
    let logs = input.run_dir.join("logs");
    match substance_runner::mail::Mailpit::start(
        input.run_dir,
        &logs,
        substance_runner::Budget::after("mailpit", Duration::from_secs(HELPER_CAP_S)),
    )
    .await
    {
        Ok(mailpit) => {
            let smtp_host = "127.0.0.1";
            for (key, value) in substance_runner::mail::environment(
                &config.mail.env,
                smtp_host,
                mailpit.smtp_port(),
            ) {
                env.insert(key, value);
            }
            outcome.mail_http_port = Some(mailpit.http_port());
            outcome.mail_smtp_port = Some(mailpit.smtp_port());
            outcome.notes.push(format!(
                "runtime: Mailpit catches mail on port {} and serves its API on {}",
                mailpit.smtp_port(),
                mailpit.http_port()
            ));
            Some(mailpit)
        }
        Err(error) => {
            outcome.notes.push(format!(
                "runtime: the email channel is not measured: {error}"
            ));
            None
        }
    }
}

/// Stops the helpers this stage started.
///
/// In one place rather than at each exit: the proxy and the mailbox hold ports and child processes,
/// and a stage that returned early without stopping them would leave both for the run's teardown to
/// find by pid.
async fn stop_helpers(
    egress: Option<substance_runner::egress::Egress>,
    mail: Option<substance_runner::mail::Mailpit>,
    outcome: &mut RuntimeOutcome,
) {
    if let Some(mut egress) = egress {
        egress.stop().await;
    }
    if let Some(mut mail) = mail
        && let Err(error) = mail.stop(Duration::from_secs(5)).await
    {
        outcome
            .notes
            .push(format!("runtime: Mailpit did not stop cleanly: {error}"));
    }
}

/// Boots the app, returning the running handle.
async fn boot(
    input: &RuntimeInput<'_>,
    permit: &ExecPermit,
    env: &BTreeMap<String, String>,
    outcome: &mut RuntimeOutcome,
) -> Option<BootedApp> {
    // `env` already carries the proxy and SMTP variables; the boot's own spec applies the map below.
    let repo = RepoView::new(input.work);
    let home = input.run_dir.join(substance_runner::HOME_DIR);
    let logs = input.run_dir.join("logs");
    let boot_env = BootEnv {
        work: input.work,
        home: &home,
        logs: &logs,
        limits: Limits::detect(),
        fresh_copy: None,
        // Seeded below, once the app is up: this stage owns the seed step, and the plan's own record
        // of the command is what it uses to find it.
        defer_seed: true,
        env,
    };
    match ladder::boot(
        &repo,
        input.config,
        &boot_env,
        Budget::after("runtime", Duration::from_secs(STAGE_CAP_S)),
        permit,
    )
    .await
    {
        Ok(booted) => {
            let readme_booted = booted.outcome.rung == Some(BootRung::Readme);
            outcome.boot = Some(booted.outcome);
            // Recorded before the app is unpacked: a boot that got as far as its steps and then
            // failed to start still has an install outcome, and O01 must see it.
            outcome.steps = booted.steps;
            // O02 asks the README on its own, because the ladder stops at the first rung that works:
            // a repository with a good config never reaches rung 1, and the check would be
            // unmeasurable in exactly the case it exists for.
            if !readme_booted {
                outcome.readme = ladder::try_readme(
                    &repo,
                    &boot_env,
                    Budget::after("readme", Duration::from_secs(README_CAP_S)),
                    permit,
                )
                .await;
            }
            let app = booted.app?;
            // Registered as soon as it exists. `forget_app` after a clean stop is what keeps a later
            // Ctrl-C from signalling a pid the OS may have given to something else.
            input
                .registrar
                .register_app(start_program(input), app.pgid());
            Some(app)
        }
        Err(error) => {
            outcome
                .notes
                .push(format!("runtime: no rung could boot it: {error}"));
            None
        }
    }
}

/// The start command's program, for the registrar's message.
fn start_program<'a>(input: &'a RuntimeInput<'_>) -> &'a str {
    input
        .config
        .app
        .start
        .as_deref()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("app")
}

/// Stops the app and returns the token that proves it is gone.
async fn stop(
    input: &RuntimeInput<'_>,
    booted: &mut BootedApp,
    outcome: &mut RuntimeOutcome,
) -> Option<substance_runner::StoppedApp> {
    let group = booted.pgid();
    match booted.stop(STOP_GRACE).await {
        Ok((info, token)) => {
            input.registrar.forget_app(group);
            // O03 is judged from this: the group's emptiness, the grace period and the leader's
            // code, all observed here because only here is the stop of *this* run's app.
            outcome.shutdown = Some(info);
            Some(token)
        }
        Err(error) => {
            outcome
                .notes
                .push(format!("runtime: the app did not stop cleanly: {error}"));
            None
        }
    }
}

/// Runs the migrate command a second time, for O04.
///
/// Only when the first run succeeded: a migration that failed the first time has already failed, and
/// reporting its second failure as an idempotency problem would blame the tool for the repository's
/// own error. The command comes from the boot's step list rather than being re-derived, so the second
/// run is provably the same command as the first.
async fn migrate_twice(
    input: &RuntimeInput<'_>,
    permit: &ExecPermit,
    env: &BTreeMap<String, String>,
    steps: &[StepFact],
) -> Option<MigrateRun> {
    let first = steps
        .iter()
        .find(|step| step.name == "migrate" && step.success)?;
    let command = CommandSpec {
        program: first.program.clone(),
        args: first.args.clone(),
        env: BTreeMap::new(),
    };
    let step = step_for(&command, input.work, "migrate-again", env);
    match substance_runner::oneshot::run(&step, &input.run_dir.join("logs"), STEP_CAP, permit).await
    {
        Ok(output) => Some(MigrateRun {
            success: output.success,
            code: output.code,
            command: first.display(),
        }),
        // Could not be run at all is not a failure *of the migration*: the check reports the reason
        // rather than a fail, which is the difference between "it is not idempotent" and "we could
        // not ask".
        Err(_) => None,
    }
}

/// Runs the repository's seed command.
///
/// Returns whether the database is seeded: a repository with no seed convention has nothing to seed
/// and returns `true`, because "there is nothing to do" and "the seed failed" lead to different
/// snapshots.
async fn seed_repository(
    input: &RuntimeInput<'_>,
    adapter: &Adapter,
    handle: &DbHandle,
    permit: &ExecPermit,
    outcome: &mut RuntimeOutcome,
) -> bool {
    let repo = RepoView::new(input.work);
    let manager = package_manager(&repo)
        .manager()
        .map_or("npm", |m| m.as_str());
    let Some(DetectedSeed { command, evidence }) = seed::seed_command(&repo, input.config, manager)
    else {
        outcome.notes.push(
            "runtime: no seed command in the config and none detected, so the database holds only \
             what the migrations created"
                .to_owned(),
        );
        return true;
    };
    let env = app_env(adapter, handle);
    outcome.notes.push(format!(
        "runtime: seeding with {} (from {evidence})",
        command.display()
    ));
    let step = step_for(&command, input.work, "seed", &env);
    match substance_runner::oneshot::run(&step, &input.run_dir.join("logs"), STEP_CAP, permit).await
    {
        Ok(output) if output.success => true,
        Ok(output) => {
            outcome.notes.push(format!(
                "runtime: the seed command exited {} (from {evidence})",
                output.code.unwrap_or(-1)
            ));
            false
        }
        Err(error) => {
            outcome
                .notes
                .push(format!("runtime: the seed command could not run: {error}"));
            false
        }
    }
}

/// One one-shot step for a command.
fn step_for(
    command: &CommandSpec,
    cwd: &Path,
    name: &str,
    env: &BTreeMap<String, String>,
) -> substance_runner::OneShot {
    let mut step = substance_runner::OneShot::new(&command.program, &command.args, cwd, name)
        .with_limits(Limits::detect());
    for (key, value) in env {
        step = step.with_env(key, value);
    }
    for (key, value) in &command.env {
        step = step.with_env(key, value);
    }
    step
}

/// Creates the accounts the config asks for.
async fn create_accounts(
    input: &RuntimeInput<'_>,
    booted: &BootedApp,
    permit: &ExecPermit,
    outcome: &mut RuntimeOutcome,
) -> bool {
    let plan = AccountPlan::of(input.config, input.master_seed);
    outcome.notes.extend(plan.notes.iter().cloned());
    outcome.notes.extend(plan.problems.iter().cloned());
    if plan.accounts.is_empty() {
        outcome
            .notes
            .push("runtime: no accounts to create".to_owned());
        return false;
    }
    let origin = match booted.port() {
        Some(port) => format!("http://127.0.0.1:{port}"),
        None => {
            outcome.notes.push(
                "runtime: the app reported no port, so no account can be created through it"
                    .to_owned(),
            );
            return false;
        }
    };
    let mut all_created = true;
    for account in &plan.accounts {
        let user = &account.user;
        match user.create {
            UserCreate::Existing => outcome.notes.push(format!(
                "runtime: {} must already exist after seeding; M4-T01 verifies it by logging in",
                user.handle
            )),
            UserCreate::Ui => outcome.notes.push(format!(
                "runtime: {} uses the ui method, which arrives with the driver in M4",
                user.handle
            )),
            UserCreate::Sql => {
                all_created = false;
                outcome.notes.push(format!(
                    "runtime: {} uses the sql method, which needs a schema template this build \
                     does not read",
                    user.handle
                ));
            }
            UserCreate::Supabase => {
                all_created = false;
                outcome.notes.push(format!(
                    "runtime: {} uses the supabase method, which arrives with the GoTrue admin \
                     client in M3",
                    user.handle
                ));
            }
            // A plan problem means this build cannot create the account at all (no `[signup]`
            // template), and the problem is already in the notes; attempting it anyway would add a
            // second, less specific line.
            UserCreate::Api if plan.problems.iter().any(|p| p.contains(&user.handle)) => {
                all_created = false;
            }
            UserCreate::Api => match signup(input, user, &origin, permit).await {
                Ok(()) => {
                    outcome.accounts.push(user.handle.clone());
                    outcome.notes.push(format!(
                        "runtime: created {} through the signup API",
                        user.handle
                    ));
                }
                Err(reason) => {
                    all_created = false;
                    outcome.notes.push(reason);
                }
            },
        }
    }
    outcome.accounts.sort();
    all_created
}

/// Creates one account through the app's signup request.
async fn signup(
    input: &RuntimeInput<'_>,
    user: &UserConfig,
    origin: &str,
    permit: &ExecPermit,
) -> Result<(), String> {
    let Some(request) = &input.config.signup.api else {
        return Err(format!(
            "runtime: {} asks for the api method but [signup] api is not set",
            user.handle
        ));
    };
    let password = password_for(user).map_err(|error: AccountError| format!("runtime: {error}"))?;
    let body = match &request.body {
        Some(template) => Some(
            render(template, "signup", &user.email, &password, &user.handle)
                .map_err(|error| format!("runtime: {error}"))?,
        ),
        None => None,
    };
    let built = SignupRequest {
        handle: user.handle.clone(),
        method: request.method.clone(),
        path: request.path.clone(),
        body,
    };
    let _ = (permit, &built.handle);

    // The body is moved in here and never stored or logged; the error path names the handle and the
    // status, which is everything a reader needs and nothing a password appears in.
    let url = format!("{origin}{}", built.path);
    let client = reqwest::Client::builder()
        .timeout(SIGNUP_CAP)
        .build()
        .map_err(|error| format!("runtime: cannot build the signup client: {error}"))?;
    let method = reqwest::Method::from_bytes(built.method.as_bytes()).map_err(|error| {
        format!(
            "runtime: the signup method {:?} is not an HTTP method: {error}",
            built.method
        )
    })?;
    let mut outgoing = client.request(method, &url);
    if let Some(body) = built.body {
        outgoing = outgoing
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    let response = outgoing.send().await.map_err(|error| {
        format!(
            "runtime: {} signup POST {url} failed: {error}",
            built.handle
        )
    })?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    // The body may quote the request, so only the status is reported. A 409 means the account already
    // exists, which the repository's own seed may have created, and is worth saying separately.
    Err(format!(
        "runtime: {} signup returned {}{}",
        built.handle,
        status.as_u16(),
        if status.as_u16() == 409 {
            ", so it already exists"
        } else {
            ""
        }
    ))
}
