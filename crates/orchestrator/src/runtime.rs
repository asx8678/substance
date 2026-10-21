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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use substance_adapters::accounts::{
    AccountError, AccountPlan, SignupRequest, password_for, render,
};
use substance_adapters::boot::BootEnv;
use substance_adapters::db::cleanup::DatabaseCleanup;
use substance_adapters::db::factory::{self, Adapter, FactoryError};
use substance_adapters::ladder::{self, BootedApp};
use substance_adapters::seed::{self, DetectedSeed};
use substance_adapters::stack::RepoView;
use substance_adapters::ts::package_manager;
use substance_core::config::{ProjectConfig, UserConfig, UserCreate};
use substance_core::ids::RunId;
use substance_core::model::seeding::{Canaries, Canary, seeds_for};
use substance_core::model::{BootOutcome, CanaryKind, CommandSpec, DbHandle};
use substance_core::tools::ToolSpec;
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

/// How long a signup request may take.
///
/// Short: the app is on loopback and already answering its ready path, so a signup that takes longer
/// than this is not slow, it is stuck.
const SIGNUP_CAP: Duration = Duration::from_secs(30);

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
    let env = match database.as_ref() {
        Some((adapter, handle)) => app_env(adapter, handle),
        None => BTreeMap::new(),
    };

    // Booted even without a database: a repository whose checks need none still gets its rung, its
    // friction and its boot outcome recorded.
    let Some(mut booted) = boot(input, permit, &env, &mut outcome).await else {
        return Ok(outcome);
    };

    let Some((adapter, handle)) = database else {
        // No database: no seed, no accounts, no snapshot. The app still stops here, because a
        // process this stage started and did not stop would outlive the run.
        let _ = stop(input, &mut booted, &mut outcome).await;
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

/// Boots the app, returning the running handle.
async fn boot(
    input: &RuntimeInput<'_>,
    permit: &ExecPermit,
    env: &BTreeMap<String, String>,
    outcome: &mut RuntimeOutcome,
) -> Option<BootedApp> {
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
            outcome.boot = Some(booted.outcome);
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
        Ok(token) => {
            input.registrar.forget_app(group);
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
