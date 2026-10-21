//! Supabase: the local stack, driven through its CLI (M2-T03, ADR-0018).
//!
//! ## Why this is the slow path
//!
//! The other two engines reset by copying a database or a file. Supabase resets by running
//! `supabase db reset`, which stops the local containers, recreates them from the repository's
//! migrations and then reseeds. On a laptop that is tens of seconds per reset rather than
//! milliseconds, which changes how many probes a run can afford: ADR-0018 records that a Supabase
//! run is not the configuration to pick when reset count matters.
//!
//! ## Why the CLI and not the connection
//!
//! A Supabase stack is Postgres plus GoTrue, PostgREST, Storage and a gateway, all with their own
//! ports and keys. Creating a database with SQL would give the app a database no other service
//! knows about. The CLI owns the stack's lifecycle, so it is what provisions and resets.
//!
//! ## Missing CLI is not a failure
//!
//! Without the CLI on `PATH` -- or without Docker, which the CLI needs -- Supabase checks report
//! `not_measured("tool missing: supabase")`. Reporting a failure would blame the app for the host.
//! This is the same rule as every other external tool in `tools.toml`.

use async_trait::async_trait;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use substance_core::ids::{RunId, SnapshotId};
use substance_core::model::{Capabilities, DbHandle, Measured, RedactedUrl, run_tag};
use substance_core::tools::ToolSpec;
use substance_runner::{Captured, ExecPermit, StoppedApp, run_tool_capture_any};

use super::postgres::PostgresAdapter;
use super::{CatalogSummary, DbAdapter, DbError, RowChange};

/// The tool name, as `tools.toml` spells it.
pub const TOOL: &str = "supabase";

/// How long a stack operation may take.
///
/// `supabase start` pulls container images on a cold host, which is minutes rather than seconds.
/// This is a safety cap (§4.3), not a target: a reset that finishes in 20 s is normal and a cap that
/// fired at 60 would fail honest runs.
const STACK_CAP: Duration = Duration::from_secs(900);

/// A local Supabase stack for one run.
// `Clone` by hand rather than derived: `torn_down` has to be shared between clones, or a clone
// that tore the stack down would leave the original thinking there was still something to stop.
#[derive(Debug)]
pub struct SupabaseAdapter {
    /// The project directory: the work copy, which holds `supabase/`.
    project: PathBuf,
    /// The tool spec, so the binary and its version arguments come from `tools.toml`.
    spec: ToolSpec,
    /// The run's stack name, so two runs on one host do not share containers.
    project_id: String,
    /// The environment variable the app reads its URL from.
    url_env: String,
    /// Set once teardown has stopped the stack, so a second call is a no-op. Shared by clones.
    torn_down: Arc<AtomicBool>,
    /// The stack's own Postgres URL, recorded by `start()`.
    ///
    /// The capture channels have no permit and must not run the CLI, so the URL is kept from the
    /// one call that legitimately ran it. Holds the credentials, so it is never formatted.
    url: Arc<Mutex<Option<String>>>,
}

impl Clone for SupabaseAdapter {
    fn clone(&self) -> Self {
        Self {
            project: self.project.clone(),
            spec: self.spec.clone(),
            project_id: self.project_id.clone(),
            url_env: self.url_env.clone(),
            torn_down: Arc::clone(&self.torn_down),
            url: Arc::clone(&self.url),
        }
    }
}

impl SupabaseAdapter {
    /// An adapter for the stack in `project`.
    #[must_use]
    pub fn new(project: PathBuf, spec: ToolSpec, run_id: &RunId, url_env: String) -> Self {
        Self {
            project,
            spec,
            // Supabase's CLI names a project's containers after the directory plus this id, so a
            // run-scoped id is what keeps two concurrent runs apart.
            project_id: format!("substance_{}", run_tag(run_id)),
            url_env,
            torn_down: Arc::new(AtomicBool::new(false)),
            url: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether the repository carries a Supabase config, so there is a stack to start.
    #[must_use]
    pub fn has_config(project: &Path) -> bool {
        project.join("supabase/config.toml").exists()
    }

    /// A Postgres adapter for the running stack's server.
    ///
    /// The URL is the one `start()` recorded from `supabase status -o json`: the capture channels
    /// run with no permit, and re-running the CLI from a channel would be a stack operation at a
    /// moment that has nothing to do with the stack. A stack that was never started is a missing
    /// channel, and the reason says so.
    fn postgres(&self) -> Result<PostgresAdapter, DbError> {
        let Some(url) = self.url.lock().ok().and_then(|slot| slot.clone()) else {
            return Err(DbError::Unavailable {
                engine: self.engine().to_owned(),
                reason: "the supabase stack has not been started, so there is no server to read"
                    .to_owned(),
            });
        };
        let adapter = PostgresAdapter::new(
            &url,
            &RunId::derive(&[self.project_id.as_bytes()]),
            self.url_env.clone(),
        )?;
        // The stack's database exists and is already migrated, so the run's own database pair
        // would be a detour: the adapter reads the database the app actually writes to, and names
        // its replication slot after the run so two runs on one host do not share one.
        Ok(adapter.use_existing_database("postgres".to_owned()))
    }

    /// Runs the CLI and returns its captured output whatever the exit code.
    ///
    /// `permit` is required because `supabase db reset` applies the repository's own migrations,
    /// which are repo-controlled SQL: this is the one database adapter that runs code the evaluated
    /// repository wrote.
    async fn cli(&self, args: &[&str], permit: &ExecPermit) -> Result<Captured, DbError> {
        let _ = permit;
        let os_args: Vec<&OsStr> = args.iter().map(|arg| OsStr::new(*arg)).collect();
        run_tool_capture_any(&self.spec.bin, &os_args, &self.project, STACK_CAP)
            .await
            .map_err(|error| DbError::Tool {
                tool: self.spec.bin.clone(),
                detail: error.to_string(),
            })
    }

    /// The CLI's status, as JSON, when the stack is up.
    async fn status(&self, permit: &ExecPermit) -> Result<Captured, DbError> {
        self.cli(&["status", "-o", "json"], permit).await
    }
}

#[async_trait]
impl DbAdapter for SupabaseAdapter {
    fn engine(&self) -> &'static str {
        "supabase"
    }

    fn needs_permit(&self) -> bool {
        // `db reset` runs the repository's migrations.
        true
    }

    async fn preflight(&self) -> Result<Capabilities, DbError> {
        // Preflight takes no permit, so it may not run the CLI: every Supabase command that talks
        // to a stack can apply migrations. What it can do without one is check the two facts that
        // decide whether the stack is usable at all, and report both as reasons.
        let mut problems = Vec::new();
        if !Self::has_config(&self.project) {
            problems.push("no supabase/config.toml, so there is no stack to start".to_owned());
        }
        problems.push(
            "supabase's row-diff channel is the Postgres adapter on the local stack, and preflight \
             cannot start it without --allow-exec"
                .to_owned(),
        );
        Ok(Capabilities {
            engine: self.engine().to_owned(),
            logical_decoding: Measured::not_measured(
                "supabase reaches Postgres through its CLI, which preflight does not run",
            ),
            query_stats: Measured::not_measured(
                "supabase reaches Postgres through its CLI, which preflight does not run",
            ),
            createdb: false,
            problems,
        })
    }

    async fn provision(&self) -> Result<DbHandle, DbError> {
        // Provisioning is `supabase start`: it creates the containers, applies the repository's
        // migrations and leaves a Postgres the app can reach. It needs a permit, and the trait's
        // `provision` has no place for one, so a caller that has no permit gets a clear error
        // rather than a silent partial start.
        Err(DbError::Unavailable {
            engine: self.engine().to_owned(),
            reason: "supabase provisioning needs --allow-exec and runs through start(); use \
                     SupabaseAdapter::start"
                .to_owned(),
        })
    }

    async fn snapshot(&self, _label: &str, _stopped: &StoppedApp) -> Result<SnapshotId, DbError> {
        Err(DbError::Unavailable {
            engine: self.engine().to_owned(),
            reason: "supabase has no cheap snapshot; every reset replays the migrations (ADR-0018)"
                .to_owned(),
        })
    }

    fn app_url(&self, handle: &DbHandle) -> Result<String, DbError> {
        // The CLI serves Postgres on 54322 by default; `status -o json` reports the real port, and
        // a caller that has run `start()` has already put the URL in the handle's environment.
        Ok(format!(
            "postgres://postgres:postgres@127.0.0.1:54322/{}?sslmode=disable",
            handle.database
        ))
    }

    async fn reset(&self, _snapshot: &SnapshotId, stopped: &StoppedApp) -> Result<(), DbError> {
        if !stopped.is_valid() {
            return Err(DbError::AppRunning {
                database: self.project_id.clone(),
            });
        }
        Err(DbError::Unavailable {
            engine: self.engine().to_owned(),
            reason: "supabase resets need --allow-exec and run through reset_with(); use \
                     SupabaseAdapter::reset_stack"
                .to_owned(),
        })
    }

    async fn begin_capture(&self) -> Result<(), DbError> {
        // The stack *is* Postgres, so the channels are the Postgres adapter's: one implementation
        // of the WAL parsing and the `dbid`-filtered delta, not a second that could disagree with
        // it. What Supabase cannot do cheaply is reset (ADR-0018), which is a fact about resets,
        // not about reading what an action wrote.
        let adapter = self.postgres()?;
        adapter.begin_capture().await
    }

    async fn end_capture(&self) -> Result<Vec<RowChange>, DbError> {
        let adapter = self.postgres()?;
        adapter.end_capture().await
    }

    async fn query_count(&self) -> Result<Measured<u64>, DbError> {
        // A stack that is not running has no counts to report, and that is a missing channel
        // rather than a failed run: the boot stage is what starts it.
        match self.postgres() {
            Ok(adapter) => adapter.query_count().await,
            Err(error) => Ok(Measured::not_measured(error.to_string())),
        }
    }

    async fn catalog(&self) -> Result<Measured<CatalogSummary>, DbError> {
        Ok(Measured::not_measured("the catalog channel arrives in M9"))
    }

    async fn teardown(&self) -> Vec<String> {
        // Idempotent: a second teardown, or a teardown after an interrupt already stopped the
        // stack, must not stop it again and report a second time.
        if self.torn_down.swap(true, Ordering::SeqCst) {
            return Vec::new();
        }
        // Best effort with no permit: `supabase stop` only stops containers substance started under
        // this project id, and a run that cannot stop them must say so rather than hang.
        let mut removed = Vec::new();
        let args = [
            "stop",
            "--no-backup",
            "--project-id",
            self.project_id.as_str(),
        ];
        let os_args: Vec<&OsStr> = args.iter().map(|arg| OsStr::new(*arg)).collect();
        match run_tool_capture_any(&self.spec.bin, &os_args, &self.project, STACK_CAP).await {
            Ok(captured) if captured.success => removed.push(format!("stack {}", self.project_id)),
            Ok(captured) => removed.push(format!(
                "could not stop stack {}: {}",
                self.project_id,
                captured.stderr_trimmed()
            )),
            Err(error) => {
                removed.push(format!("could not stop stack {}: {error}", self.project_id))
            }
        }
        removed
    }

    fn app_env(&self, handle: &DbHandle) -> Result<Vec<(String, String)>, DbError> {
        // The stack exposes a URL and an anon key; an app built against Supabase reads both. The
        // URL comes from the handle, the key from the environment the CLI reported when it started.
        let mut env = vec![(handle.url_env.clone(), self.app_url(handle)?)];
        if let Ok(key) = std::env::var("SUBSTANCE_SUPABASE_ANON_KEY") {
            env.push(("SUPABASE_ANON_KEY".to_owned(), key));
        }
        Ok(env)
    }
}

impl SupabaseAdapter {
    /// Starts the stack, which provisions the run's database.
    ///
    /// Separate from the trait's `provision` because it needs a permit: the CLI applies the
    /// repository's migrations, which is repo-controlled SQL.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Tool`] when the CLI is missing, and [`DbError::Statement`] when it fails.
    pub async fn start(&self, permit: &ExecPermit) -> Result<DbHandle, DbError> {
        let captured = self
            .cli(&["start", "--project-id", self.project_id.as_str()], permit)
            .await?;
        if !captured.success {
            return Err(DbError::Tool {
                tool: self.spec.bin.clone(),
                detail: format!(
                    "supabase start exited {}: {}",
                    captured.code.unwrap_or(-1),
                    captured.stderr_trimmed()
                ),
            });
        }
        let status = self.status(permit).await?;
        // The capture channels read this server, and they have no permit to ask for it again.
        if let Some(text) = status_url_text(&status.stdout)
            && let Ok(mut slot) = self.url.lock()
        {
            *slot = Some(text);
        }
        let server = status_url(&status.stdout)
            .unwrap_or_else(|| RedactedUrl::parse("postgres://127.0.0.1:54322"));
        Ok(DbHandle {
            engine: self.engine().to_owned(),
            database: "postgres".to_owned(),
            seeded: String::new(),
            server,
            url_env: self.url_env.clone(),
            sqlite_path: None,
        })
    }

    /// Resets the stack: `supabase db reset`, then the reseed M2-T04 adds.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::AppRunning`] for a stale token, and [`DbError::Tool`] when the CLI fails.
    pub async fn reset_stack(
        &self,
        stopped: &StoppedApp,
        permit: &ExecPermit,
    ) -> Result<(), DbError> {
        if !stopped.is_valid() {
            return Err(DbError::AppRunning {
                database: self.project_id.clone(),
            });
        }
        let captured = self
            .cli(
                &["db", "reset", "--project-id", self.project_id.as_str()],
                permit,
            )
            .await?;
        if captured.success {
            Ok(())
        } else {
            Err(DbError::Tool {
                tool: self.spec.bin.clone(),
                detail: format!(
                    "supabase db reset exited {}: {}",
                    captured.code.unwrap_or(-1),
                    captured.stderr_trimmed()
                ),
            })
        }
    }

    /// Whether the CLI is on `PATH` and answers `--version`.
    #[must_use]
    pub fn available(&self) -> bool {
        probe_binary(&self.spec.bin)
    }
}

/// Whether `bin` exists on `PATH`, without running it.
///
/// Checked by walking `PATH` rather than by spawning, because a capability probe must not execute
/// anything: an incorrectly installed CLI that crashes would look like a missing tool.
fn probe_binary(bin: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':').any(|dir| {
        let candidate = Path::new(dir).join(bin);
        candidate.is_file()
    })
}

/// The `DB URL` out of `supabase status -o json`, with its credentials.
///
/// Only the capture channels call this, and only to build a connection config: the value must not
/// reach a message, a log or a record, which is why the redacted [`status_url`] is what the rest of
/// this module uses.
fn status_url_text(stdout: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let object = value.as_object()?;
    for key in ["DB_URL", "db_url", "DATABASE_URL"] {
        if let Some(text) = object.get(key).and_then(serde_json::Value::as_str) {
            return Some(text.to_owned());
        }
    }
    None
}

/// The `DB URL` out of `supabase status -o json`, redacted.
///
/// The JSON shape is `{"DB_URL": "postgresql://...", ...}`. Parsed with `serde_json` rather than by
/// searching for `://`, so a field that merely mentions a URL cannot be mistaken for the real one.
fn status_url(stdout: &str) -> Option<RedactedUrl> {
    let value: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let object = value.as_object()?;
    for key in ["DB_URL", "db_url", "DATABASE_URL"] {
        if let Some(text) = object.get(key).and_then(serde_json::Value::as_str) {
            return Some(RedactedUrl::parse(text));
        }
    }
    None
}

/// The argument list a caller can print in an error without leaking a key.
///
/// Two rules, because a secret can arrive in two shapes: as a flag that names one (`--anon-key`,
/// `--token`) and as the *value* that follows it. Checking only the argument's own text misses
/// `--anon-key sk_live_...` entirely, since the value says nothing about being a secret -- which is
/// exactly the case a probe caught here before this function was written this way.
#[must_use]
pub fn redacted_args(args: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(args.len());
    // Set while walking, so the argument after a secret-bearing flag is redacted whatever it says.
    let mut previous_was_secret_flag = false;
    for arg in args {
        let names_a_secret = arg.contains("key") || arg.contains("token") || arg.contains("secret");
        if previous_was_secret_flag || names_a_secret {
            out.push("<redacted>".to_owned());
        } else {
            out.push((*arg).to_owned());
        }
        // `--flag=value` carries its value in the same argument, so the next one is ordinary.
        previous_was_secret_flag = names_a_secret && !arg.contains('=');
    }
    out
}
