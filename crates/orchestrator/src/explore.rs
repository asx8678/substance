//! The explore stage: crawl the running app and build the AppModel (M4-T01).
//!
//! ## What this stage owns
//!
//! Everything the pure crawler crate deliberately does not: booting the app, spawning the driver,
//! opening a context, walking the frontier, running the login, and writing the three outputs
//! (`app_model.json`, `crawl.har`, and the crawl's records). The order of decisions is the crawler's
//! (`substance_crawler`); this module is the loop that asks it.
//!
//! ## Registration and teardown
//!
//! The app's process group and the driver are registered with the run's `Registrar` as soon as they
//! exist, so a Ctrl-C reaches a browser and a server that a dropped future would otherwise leave
//! behind. Every path stops the driver before the app: the driver holds a browser attached to the app,
//! and a browser left running against a dead server turns into the next command's failures.
//!
//! ## A missing prerequisite is a note, not a failure
//!
//! No `--allow-exec`, no boot rung, no port, no driver: each is a note and a stage that reports it,
//! because a run without an app still has a report to write and the checks that need the model then
//! say `not_measured` with this stage's reason.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use substance_adapters::accounts::password_for;
use substance_adapters::boot::BootEnv;
use substance_adapters::boot::FreshCopy;
use substance_adapters::ladder;
use substance_adapters::stack::RepoView;
use substance_core::config::ProjectConfig;
use substance_core::ids::RunId;
use substance_core::model::app_model::TestUser;
use substance_core::model::{
    ActCommand, Action, Control, ControlPurpose, DriverCommand, LoginCommand, Measured, Storage,
    Target,
};
use substance_core::tools::ToolSpec;
use substance_crawler::cache::{CacheInputs, driver_version_of, playwright_version_of};
use substance_crawler::controls::{detect_login, selector_of, sort_controls};
use substance_crawler::effects::{cleared_session, is_auth_key, login_failure, mark_logout};
use substance_crawler::forms::{Purpose, field_seed, value_for};
use substance_crawler::frontier::Frontier;
use substance_crawler::model::Visited;
use substance_crawler::route::{is_own_origin, template_path, url_for};
use substance_recorder::context::{ContextPlan, open_context};
use substance_recorder::driver::{DriverConfig, DriverHost, DriverTimeouts};
use substance_recorder::record::{Observed, RecordContext, RecordStore};
use substance_runner::{Budget, ExecPermit, Limits, Registrar};

/// How long one explore stage may take, in seconds.
const EXPLORE_CAP_S: u64 = 900;

/// The route the crawl starts from, per role.
const ENTRY: &str = "/";

/// What the explore stage needs.
pub struct ExploreInput<'a> {
    /// The config: login, budgets and the users.
    pub config: &'a ProjectConfig,
    /// The work copy the app runs in: `runs/<run_id>/work/`, never the user's checkout (§4.1).
    pub work: &'a Path,
    /// How to export the commit again, for the README rung's own copy.
    pub fresh_copy: Option<&'a FreshCopy<'a>>,
    /// The run directory: logs, records, blobs and the outputs.
    pub run_dir: &'a Path,
    /// The run's id, which every crawl record carries.
    pub run_id: &'a RunId,
    /// The master seed form values derive from.
    pub master_seed: u64,
    /// The manifest's frozen instant, for every clock the driver installs.
    pub as_of: substance_core::Timestamp,
    /// The frozen environment the app was booted with, reused so the driver's app sees one clock.
    pub env: &'a BTreeMap<String, String>,
    /// The tool table, for the adapters that need one.
    pub tools: &'a [(String, ToolSpec)],
    /// The permission to run repo-controlled code.
    pub permit: Option<&'a ExecPermit>,
    /// The commit under test, for the cache key.
    pub commit_sha: &'a str,
    /// The config's own hash, for the cache key.
    pub config_hash: &'a str,
    /// The rule pack's hash, for the cache key.
    pub rule_pack_hash: &'a str,
    /// The run's interrupt registrar.
    pub registrar: &'a Registrar,
}

/// What the explore stage produced.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ExploreOutcome {
    /// The model's cache key, which names the file it is stored under.
    pub cache_key: String,
    /// How many screens were visited.
    pub screens: u32,
    /// How many actions the crawl ran.
    pub actions: u32,
    /// The route templates visited, in crawl order.
    pub routes: Vec<String>,
    /// Whether a login works, from config or detected.
    pub login: Option<String>,
    /// Why there is no working login, when there is none.
    pub login_failure: Option<String>,
    /// Every note a reader needs that is not a verdict, sorted.
    pub notes: Vec<String>,
}

/// Runs the crawl.
///
/// # Errors
///
/// Returns a note rather than an error for every missing prerequisite, so the caller can record it.
/// The only `Err` is a panic-free invariant the caller must not ignore: the stage never returns one
/// today, and its signature keeps the door open for a future step that genuinely cannot continue.
pub async fn run(input: &ExploreInput<'_>) -> Result<ExploreOutcome, String> {
    let mut outcome = ExploreOutcome::default();
    let Some(permit) = input.permit else {
        outcome.notes.push(
            "explore: without --allow-exec there is no app to crawl, so there is no app model"
                .to_owned(),
        );
        return Ok(outcome);
    };
    let repo = RepoView::new(input.work);
    if ladder::preferred_rung(&repo, input.config).is_none() {
        outcome
            .notes
            .push("explore: no boot rung is available, so the app cannot be crawled".to_owned());
        return Ok(outcome);
    }

    let logs = input.run_dir.join("logs");
    let home = input.run_dir.join(substance_runner::HOME_DIR);
    let boot_env = BootEnv {
        work: input.work,
        home: &home,
        logs: &logs,
        limits: Limits::detect(),
        fresh_copy: input.fresh_copy,
        defer_seed: true,
        env: input.env,
    };
    let booted = ladder::boot(
        &repo,
        input.config,
        &boot_env,
        Budget::after("explore", Duration::from_secs(EXPLORE_CAP_S)),
        permit,
    )
    .await
    .map_err(|error| error.to_string())?;
    let Some(mut app) = booted.app else {
        outcome
            .notes
            .push("explore: the app booted no process, so there is nothing to crawl".to_owned());
        return Ok(outcome);
    };
    let pgid = app.pgid();
    input.registrar.register_app("app", pgid);
    let Some(port) = app.port() else {
        let _ = app.stop(Duration::from_secs(EXPLORE_CAP_S)).await;
        outcome
            .notes
            .push("explore: the app reported no port, so the crawl has no URL".to_owned());
        return Ok(outcome);
    };
    let origin = format!("http://127.0.0.1:{port}");

    let mut driver = match DriverHost::spawn(
        input.run_dir,
        &logs,
        &DriverConfig::new(DriverTimeouts::for_settle(
            input.config.budgets.settle_timeout_ms,
            u32::try_from(EXPLORE_CAP_S).unwrap_or(u32::MAX),
        )),
    )
    .await
    {
        Ok(driver) => driver,
        Err(error) => {
            let _ = app.stop(Duration::from_secs(EXPLORE_CAP_S)).await;
            outcome
                .notes
                .push(format!("explore: the driver did not start: {error}"));
            return Ok(outcome);
        }
    };

    let result = crawl(input, &mut driver, &origin, &mut outcome).await;
    // The driver first, then the app: a browser attached to a server that already died is the next
    // command's noise, and a browser is the thing holding the actual connection.
    if let Err(error) = driver.stop().await {
        tracing::warn!(%error, "explore: the driver did not stop cleanly");
    }
    let stopped = app.stop(Duration::from_secs(EXPLORE_CAP_S)).await;
    if stopped.is_ok() {
        input.registrar.forget_app(pgid);
    }
    outcome.notes.sort();
    outcome.notes.dedup();
    result.map(|()| outcome)
}

/// The crawl itself, with the app and driver alive.
async fn crawl(
    input: &ExploreInput<'_>,
    driver: &mut DriverHost,
    origin: &str,
    outcome: &mut ExploreOutcome,
) -> Result<(), String> {
    let store = RecordStore::new(input.run_dir);
    let context = open_context(
        input.config,
        &ContextPlan {
            blob_dir: input.run_dir.join("blobs").display().to_string(),
            storage_state: None,
            idle_patterns: Vec::new(),
            as_of: input.as_of,
        },
    )
    .map_err(|error| error.to_string())?;
    if let Err(error) = driver.send(&DriverCommand::OpenContext(context)).await {
        outcome
            .notes
            .push(format!("explore: the context did not open: {error}"));
        return Ok(());
    }

    // The login comes first, before any crawling: a session-gated app whose landing page is a login
    // form would otherwise have its form counted as the app's only screen.
    let login = login_path(input, driver, origin, outcome).await;
    let mut frontier = Frontier::new();
    frontier.push(ENTRY, 0, 0, false);
    if let Some(login) = &login {
        // Each role's post-login landing page is an entry point of its own (the plan's second half).
        for route in &login.landings {
            frontier.push(route, 1, 0, true);
        }
    }

    let mut visited: Vec<Visited> = Vec::new();
    let mut records = Vec::new();
    let mut logout_controls: Vec<String> = Vec::new();
    let max_screens = usize::try_from(input.config.budgets.crawl_max_screens).unwrap_or(0);
    let max_actions = usize::try_from(input.config.budgets.crawl_max_actions).unwrap_or(0);
    let max_depth = input.config.budgets.crawl_max_depth;
    let mut actions = 0usize;

    while let Some(pending) = frontier.pop() {
        if visited.len() >= max_screens {
            outcome.notes.push(format!(
                "explore: the screen budget ({} ) stopped the crawl with {:?} still queued",
                max_screens,
                frontier.routes()
            ));
            break;
        }
        if pending.depth > max_depth {
            outcome.notes.push(format!(
                "explore: the depth budget ({max_depth}) skipped {}",
                pending.route
            ));
            continue;
        }
        let url = url_for(origin, &pending.path);
        if let Err(error) = driver.send(&DriverCommand::Goto { url }).await {
            outcome
                .notes
                .push(format!("explore: {} did not load: {error}", pending.route));
            continue;
        }
        // The idle baseline, before any action on the screen: what the page does on its own is only
        // measurable while nothing has been touched.
        let idle = driver
            .send(&DriverCommand::ObserveIdle {
                ms: input.config.budgets.idle_baseline_ms,
            })
            .await
            .ok()
            .and_then(|response| response.result)
            .and_then(|result| match *result {
                substance_core::model::DriverResult::Idle {
                    patterns, pings, ..
                } => Some((patterns, pings)),
                _ => None,
            });
        let (idle_patterns, idle_pings) = idle.unwrap_or_default();
        let mut controls = match list_controls(driver, &pending.path).await {
            Some(controls) => controls,
            None => {
                outcome
                    .notes
                    .push(format!("explore: no controls on {}", pending.route));
                Vec::new()
            }
        };
        sort_controls(&mut controls);
        let depth = pending.depth;
        visited.push(Visited {
            route: pending.route.clone(),
            depth,
            requires_auth: pending.requires_auth,
            controls: controls.clone(),
            idle_patterns,
            idle_pings,
            bodies: Vec::new(),
        });
        outcome.routes.push(pending.route.clone());

        // Iterated over a copy: a control that turns out to be the logout control is marked in the
        // stored list below, and Rust cannot borrow that list both ways at once.
        let listed = controls.clone();
        for (index, control) in listed.iter().enumerate() {
            if actions >= max_actions {
                outcome.notes.push(format!(
                    "explore: the action budget ({max_actions}) stopped the crawl on {}",
                    pending.route
                ));
                break;
            }
            let Some(selector) = selector_of(control) else {
                continue;
            };
            if matches!(control.purpose, ControlPurpose::Login) {
                continue;
            }
            if logout_controls.iter().any(|known| known == selector) {
                continue;
            }
            actions += 1;
            let part = driver
                .record(&DriverCommand::Act(ActCommand {
                    kind: "click".to_owned(),
                    target: selector.to_owned(),
                    value: None,
                }))
                .await;
            let Measured::Measured { value: part } = part else {
                continue;
            };
            // A control that cleared the session is the logout control: mark it, stop using it, and
            // log in again, because every later action on a logged-out page would measure a login
            // screen. Judged by the shared effect rule against the storage the login produced, so a
            // cart token expiring is not mistaken for a logout.
            if let Some(session) = &login
                && cleared_session(&session.storage, &part.storage)
            {
                if mark_logout(&mut controls[index]) {
                    logout_controls.push(selector.to_owned());
                    outcome.notes.push(format!(
                        "explore: {selector} logs the session out by effect, so the crawl stops using it"
                    ));
                }
                // Logged in again straight away: the crawl continues as the same role, and the next
                // action is measured on the screen it was found on rather than on a login form.
                if log_in_again(input, driver, session, outcome)
                    .await
                    .is_none()
                {
                    outcome.notes.push(
                        "explore: the crawl could not log in again after the logout control, so the rest of this crawl runs logged out"
                            .to_owned(),
                    );
                }
                continue;
            }
            let context = RecordContext {
                run_id: input.run_id.clone(),
                check_id: None,
                target: Target::Screen(pending.route.clone()),
                snapshot: substance_core::ids::SnapshotId::parse("crawl"),
                repetition: 1,
                seed: forms_seed(input.master_seed, &pending.route, selector),
                setup: Vec::new(),
                action: Action {
                    kind: "click".to_owned(),
                    target: selector.to_owned(),
                    screen: pending.route.clone(),
                    input: BTreeMap::new(),
                },
            };
            let observed = Observed {
                idle_patterns: visited
                    .last()
                    .map(|screen| screen.idle_patterns.clone())
                    .unwrap_or_default(),
                ..Observed::new()
            };
            match store.assemble(&context, &part, &observed, None) {
                Ok(record) => {
                    let _ = store.write(&record);
                    records.push(record);
                }
                Err(error) => outcome
                    .notes
                    .push(format!("explore: a record was not assembled: {error}")),
            }
            // Links the click revealed are new screens.
            if let Some(found) = list_controls(driver, &pending.path).await {
                for (at, link) in found.iter().enumerate() {
                    let Some(href) = link.selectors.first() else {
                        continue;
                    };
                    let Some(path) = path_of_selector(href) else {
                        continue;
                    };
                    if !is_own_origin(origin, &url_for(origin, &path)) {
                        continue;
                    }
                    frontier.push(&path, depth + 1, u32::try_from(at).unwrap_or(0), true);
                }
            }
        }
    }

    outcome.screens = u32::try_from(visited.len()).unwrap_or(u32::MAX);
    outcome.actions = u32::try_from(actions).unwrap_or(u32::MAX);

    // The model, keyed by everything it depends on (review fix 40).
    let package = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../driver/package.json"),
    )
    .unwrap_or_default();
    let driver_version = driver_version_of(&package).unwrap_or_else(|| "unknown".to_owned());
    let playwright_version =
        playwright_version_of(&package).unwrap_or_else(|| "unknown".to_owned());
    let cache = CacheInputs {
        commit_sha: input.commit_sha,
        config_hash: input.config_hash,
        master_seed: input.master_seed,
        rule_pack_hash: input.rule_pack_hash,
        driver_version: &driver_version,
        playwright_version: &playwright_version,
    };
    let users: Vec<TestUser> = input
        .config
        .users
        .iter()
        .map(|user| TestUser {
            role: user.role.clone(),
            handle: user.handle.clone(),
        })
        .collect();
    let model = substance_crawler::model::build(&visited, &records, &cache, users);
    outcome.cache_key.clone_from(&model.cache_key);
    if let Err(error) = write_outputs(input.run_dir, &model, &records, origin, input.as_of) {
        outcome
            .notes
            .push(format!("explore: the outputs were not written: {error}"));
    }
    Ok(())
}

/// The login path, if one works, from config first and then by detection.
async fn login_path(
    input: &ExploreInput<'_>,
    driver: &mut DriverHost,
    origin: &str,
    outcome: &mut ExploreOutcome,
) -> Option<LoginSession> {
    let account = account_of(input, outcome)?;
    if let Some(ui) = &input.config.login.ui {
        let command = LoginCommand::Ui {
            path: ui.path.clone(),
            identifier: ui.identifier.clone(),
            password: ui.password.clone(),
            submit: ui.submit.clone(),
            email: account.handle.clone(),
            // Never a placeholder: a login filled with the wrong password would be recorded as "the
            // app has no working login", which is a false finding about the app. The secret comes
            // from the environment the run created the account with, exactly as the runtime stage
            // read it.
            password_value: account.password,
        };
        return finish_login(driver, command, outcome).await;
    }
    // Detection: the first screen in frontier order with exactly one password form.
    let _ = driver
        .send(&DriverCommand::Goto {
            url: url_for(origin, ENTRY),
        })
        .await;
    let controls = list_controls(driver, ENTRY).await.unwrap_or_default();
    let fields = match detect_login(&controls) {
        Some(fields) => fields,
        None => {
            outcome.login_failure = Some("no login form was configured or detected".to_owned());
            outcome.notes.push(
                "explore: no working login, so the checks that need a session report not_measured"
                    .to_owned(),
            );
            return None;
        }
    };
    let command = LoginCommand::Ui {
        path: ENTRY.to_owned(),
        identifier: fields.identifier,
        password: fields.password,
        submit: fields.submit,
        email: account.handle.clone(),
        password_value: account.password,
    };
    finish_login(driver, command, outcome).await
}

/// Runs a login command and judges it by effect.
async fn finish_login(
    driver: &mut DriverHost,
    command: LoginCommand,
    outcome: &mut ExploreOutcome,
) -> Option<LoginSession> {
    let response = driver
        .send(&DriverCommand::Login(command.clone()))
        .await
        .ok()?;
    let result = response.result?;
    let substance_core::model::DriverResult::Login {
        storage, changed, ..
    } = *result
    else {
        return None;
    };
    let landed = list_controls(driver, ENTRY).await.unwrap_or_default();
    // No error signal is read here: the driver reports success and error signals on a record part,
    // and a login answers with a login result rather than a part. The third condition -- a landing
    // page that still asks for a password -- is the one that catches a form that silently failed.
    let error_signal = false;
    match login_failure(&changed, error_signal, &landed) {
        Some(reason) => {
            outcome.login_failure = Some(reason.clone());
            outcome.notes.push(format!(
                "explore: the login did not work by effect ({reason}), so session checks report not_measured"
            ));
            None
        }
        None => {
            let auth_keys = changed.iter().filter(|key| is_auth_key(key)).count();
            outcome.login = Some(format!("{auth_keys} auth keys"));
            Some(LoginSession {
                command: Some(command),
                storage,
                landings: vec![ENTRY.to_owned()],
            })
        }
    }
}

/// A working login, as the crawl needs it.
struct LoginSession {
    /// The command that logged in, so a re-login after a logout repeats it rather than re-detecting.
    command: Option<LoginCommand>,
    /// The storage the login left behind, which every later action is compared against.
    ///
    /// The comparison is against the *session*, not against "any auth key": an app can set an
    /// anonymous cart token before login, and a logout rule that only asked "did any auth-like key
    /// disappear" would call the cart expiring a logout.
    storage: Storage,
    /// The routes to crawl as a logged-in entry point.
    landings: Vec<String>,
}

/// Asks the driver for the controls on `screen`.
async fn list_controls(driver: &mut DriverHost, screen: &str) -> Option<Vec<Control>> {
    let response = driver.send(&DriverCommand::ListControls {}).await.ok()?;
    let result = response.result?;
    match *result {
        substance_core::model::DriverResult::Controls { controls, .. } => {
            if controls.is_empty() && screen.is_empty() {
                None
            } else {
                Some(controls)
            }
        }
        _ => None,
    }
}

/// The action seed for one control on one screen.
fn forms_seed(master_seed: u64, route: &str, selector: &str) -> u64 {
    let mut input = Vec::new();
    input.extend_from_slice(&master_seed.to_be_bytes());
    input.extend_from_slice(route.as_bytes());
    input.extend_from_slice(selector.as_bytes());
    let digest = substance_core::hash::sha256(&input);
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

/// The path a selector names, when it names one.
///
/// Only an `a[href=...]`-shaped selector carries a route the crawl can follow; a button's selector
/// does not, and inferring a route from it would be a guess. The driver reports a link's target as
/// part of its role and name, so this reads the one form that can appear there.
fn path_of_selector(selector: &str) -> Option<String> {
    let (_, rest) = selector.split_once("href=")?;
    let rest = rest.trim_start_matches(['\'', '"']);
    let end = rest.find(['\'', '"'])?;
    let href = &rest[..end];
    if href.starts_with('/') {
        Some(href.to_owned())
    } else {
        None
    }
}

/// Writes `app_model.json` and `crawl.har` into the run directory.
fn write_outputs(
    run_dir: &Path,
    model: &substance_core::model::app_model::AppModel,
    records: &[substance_core::model::EffectRecord],
    origin: &str,
    as_of: substance_core::Timestamp,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(model).map_err(|error| error.to_string())?;
    std::fs::write(run_dir.join("app_model.json"), json).map_err(|error| error.to_string())?;
    let har = substance_crawler::har::from_records(records, origin, &as_of.to_rfc3339());
    let bytes = serde_json::to_string_pretty(&har).map_err(|error| error.to_string())?;
    std::fs::write(run_dir.join("crawl.har"), bytes).map_err(|error| error.to_string())?;
    Ok(())
}

/// The form values a login needs, from the account the run created.
///
/// Kept separate from the login command so the password never travels through a log line: the caller
/// reads it from the config's own secret and passes it once.
#[must_use]
pub fn login_values(handle: &str, password: &str) -> (String, String) {
    (handle.to_owned(), password.to_owned())
}

/// The helpers a caller that fills a signup form needs (M2-T04's `ui` method).
///
/// Re-exported so the runtime stage can fill the app's own signup form without depending on the
/// crawler's internals: the field-value rule is the crawl's, and duplicating it would let the two
/// disagree about what a valid value is.
#[must_use]
pub fn signup_value(field: &substance_core::model::Field, seed: u64) -> String {
    value_for(field, seed, Purpose::Value, "", "")
}

/// The seed for one signup field, derived the same way the crawl derives one.
#[must_use]
pub fn signup_seed(action_seed: u64, field: &substance_core::model::Field, index: usize) -> u64 {
    field_seed(action_seed, field, index)
}

/// The account the crawl logs in as: the first configured handle and its secret.
///
/// The first, not a role's first: the crawl explores as one identity, and the checks that need a
/// second one log in themselves once they have the model. A config with no users, or a user whose
/// password variable is unset, is a note -- the crawl still runs, logged out, and the checks that
/// need a session say so.
fn account_of(input: &ExploreInput<'_>, outcome: &mut ExploreOutcome) -> Option<Account> {
    let user = input.config.users.first()?;
    match password_for(user) {
        Ok(secret) => Some(Account {
            handle: user.handle.clone(),
            password: secret.expose().to_owned(),
        }),
        Err(error) => {
            outcome.notes.push(format!(
                "explore: no account to log in as ({error}), so the crawl runs logged out"
            ));
            None
        }
    }
}

/// One account, with the secret read once.
struct Account {
    /// The handle, which is also the identifier a login form wants.
    handle: String,
    /// The password, from the environment the run created the account with.
    password: String,
}

/// Logs in again with the command that worked before.
///
/// The same command, not a re-detected one: the crawl already knows the form, and re-detecting it on
/// a screen the crawl happens to be on would find the logout page's form or nothing at all.
async fn log_in_again(
    input: &ExploreInput<'_>,
    driver: &mut DriverHost,
    session: &LoginSession,
    outcome: &mut ExploreOutcome,
) -> Option<()> {
    let command = session.command.clone()?;
    let response = driver.send(&DriverCommand::Login(command)).await.ok()?;
    let result = response.result?;
    let substance_core::model::DriverResult::Login { changed, .. } = *result else {
        return None;
    };
    if login_failure(&changed, false, &[]).is_some() {
        outcome
            .notes
            .push("explore: logging in again did not restore the session".to_owned());
        return None;
    }
    let _ = input;
    Some(())
}

/// The route template of a path, for a caller that has already normalized it.
#[must_use]
pub fn route_of(path: &str) -> String {
    template_path(path)
}
