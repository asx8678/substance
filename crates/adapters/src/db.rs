//! Database adapters: per-run databases with snapshot and reset (M2-T03).
//!
//! One trait, three engines. Everything a check needs from a database goes through it, so a check
//! never learns which engine it is talking to and an engine that cannot answer a question says
//! `not_measured` instead of guessing.
//!
//! ## Reset is the load-bearing operation
//!
//! "Reset state per probe" is what makes a verdict reproducible: boot and seed once, snapshot, then
//! give every probe a fresh copy. A reset that runs while the app is writing is not a reset -- it is
//! a race that produces mixed records and, occasionally, a `flaky` verdict blamed on the app. So
//! [`DbAdapter::reset`] takes a [`StoppedApp`], a token that exists only after
//! [`crate::AppProcess::stop`](substance_runner::AppProcess) returned with the process group empty,
//! and every implementation re-checks [`StoppedApp::is_valid`].
//!
//! ## Credentials
//!
//! The admin URL is read from the environment by the adapter's constructor and kept only in memory.
//! Nothing here writes it to a file, a log, a manifest or an error message: [`DbHandle`] carries the
//! host and port through [`RedactedUrl`], and errors carry the same redacted form. The app's own URL
//! is built when the app is started and handed straight to its environment.

use std::path::PathBuf;

use async_trait::async_trait;
use substance_core::ids::SnapshotId;
use substance_core::model::{Capabilities, DbHandle, Measured};
use substance_runner::{ExecPermit, StoppedApp};

pub mod postgres;
pub mod sqlite;
pub mod supabase;

/// Why a database operation failed.
///
/// Every variant names the operation and the database, never a URL: an error is the most likely
/// thing to be logged, so it is the last place a credential may appear.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The adapter is not configured for this host.
    #[error("{engine} is not available: {reason}")]
    Unavailable {
        /// The engine.
        engine: String,
        /// Why it cannot be used, in one sentence.
        reason: String,
    },
    /// A statement failed.
    #[error("{engine}: {operation} failed on {database}: {detail}")]
    Statement {
        /// The engine.
        engine: String,
        /// What was being done, for example `create database`.
        operation: String,
        /// The database involved.
        database: String,
        /// The database's own message, with any URL in it redacted.
        detail: String,
    },
    /// The token supplied to a state-changing call does not prove the app is stopped.
    ///
    /// Raised when the token's latch is clear, which means `AppProcess::stop` never returned for
    /// the process it came from. The database is named so a reader knows what was not touched; the
    /// message deliberately does not guess whether an app is in fact running.
    #[error("refusing to touch {database}: no stop was observed, so the app may still be writing")]
    AppRunning {
        /// The database that was not touched.
        database: String,
    },
    /// The database file, directory or engine binary is missing.
    #[error("{path} is missing: {reason}")]
    Missing {
        /// The path that is missing.
        path: PathBuf,
        /// Why it was needed.
        reason: String,
    },
    /// An IO operation failed.
    #[error("cannot {operation} {path}: {source}")]
    Io {
        /// What was being done.
        operation: String,
        /// The path involved.
        path: PathBuf,
        /// The IO error.
        source: std::io::Error,
    },
    /// A SQLite operation failed.
    #[error("sqlite: {operation} failed for {path}: {detail}")]
    Sqlite {
        /// What was being done.
        operation: String,
        /// The database file.
        path: PathBuf,
        /// SQLite's message.
        detail: String,
    },
    /// A tool the adapter drives failed or is missing.
    #[error("{tool}: {detail}")]
    Tool {
        /// The tool.
        tool: String,
        /// What went wrong.
        detail: String,
    },
}

/// What one engine can do for one run.
///
/// `async_trait` because two of the three wait on a server or a process; the trait is object-safe so
/// the orchestrator can hold a `Box<dyn DbAdapter>` chosen at intake from the config.
#[async_trait]
pub trait DbAdapter: Send + Sync {
    /// The engine's name, as the report and the manifest spell it.
    fn engine(&self) -> &'static str;

    /// Asks the server what it can do, without creating anything.
    ///
    /// Never fails for a missing capability: a capability that is absent is `not_measured` with a
    /// reason. It fails only when the server cannot be reached at all, which is a run-level problem
    /// rather than a missing channel.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when the adapter was never configured, and
    /// [`DbError::Statement`] when the server rejects the capability queries themselves.
    async fn preflight(&self) -> Result<Capabilities, DbError>;

    /// Creates the run's databases: the one the app uses and the seeded template.
    ///
    /// Idempotent: a retry after a partial failure drops what it made and starts again, because a
    /// run directory can be reused and a half-created pair is worse than none.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Statement`] when the server refuses, and [`DbError::AppRunning`] is
    /// impossible here: nothing is running yet.
    async fn provision(&self) -> Result<DbHandle, DbError>;

    /// Freezes the current state of the app's database under `label`.
    ///
    /// Takes a [`StoppedApp`] because a snapshot of a database being written is not a snapshot of
    /// anything: the app must be stopped for the copy to be a point in time.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::AppRunning`] when the token does not prove a stop, and [`DbError::Statement`] when the
    /// engine refuses the copy.
    async fn snapshot(&self, label: &str, stopped: &StoppedApp) -> Result<SnapshotId, DbError>;

    /// The app's URL, for the app's environment.
    ///
    /// Returns the connection string with the admin credentials in it, so this is the one method
    /// whose result must not be logged. It exists because the app needs it; a caller that wants a
    /// printable form uses [`DbHandle::server`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when the adapter has no URL to build (SQLite's URL is a
    /// file path, which is not a secret, so it is returned normally).
    fn app_url(&self, handle: &DbHandle) -> Result<String, DbError>;

    /// Environment variables the app needs to reach the database.
    ///
    /// A map rather than one variable, because engines disagree: Postgres wants a URL, and a
    /// Supabase stack also wants its anon key under a second name.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Self::app_url`] returns.
    fn app_env(&self, handle: &DbHandle) -> Result<Vec<(String, String)>, DbError> {
        Ok(vec![(handle.url_env.clone(), self.app_url(handle)?)])
    }

    /// Puts the app's database back to `snapshot`.
    ///
    /// The one operation that must not run against a live app, so it takes the token. Implementations
    /// check [`StoppedApp::is_valid`] before the first statement, not after: an unproven token is refused
    /// without touching anything.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::AppRunning`] when the token is stale, and [`DbError::Statement`] when the
    /// engine refuses the reset.
    async fn reset(&self, snapshot: &SnapshotId, stopped: &StoppedApp) -> Result<(), DbError>;

    /// Starts capturing row changes, for the database channel.
    ///
    /// Filled in M3; until then every engine answers `not_measured("row capture arrives in M3")`,
    /// which is the truth and keeps a check from reading an empty diff as "no writes".
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when the engine has no capture channel at all.
    async fn begin_capture(&self) -> Result<(), DbError>;

    /// Stops capturing and returns the changes seen, as records.
    ///
    /// Filled in M3. The empty vector here is not a claim that nothing changed: a caller must read
    /// [`Capabilities::can_diff_rows`] first, which is the difference this whole design turns on.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when capture was never started.
    async fn end_capture(&self) -> Result<Vec<RowChange>, DbError>;

    /// How many statements the server has run since capture began.
    ///
    /// Filled in M3, from `pg_stat_statements`. `not_measured` is a variant of the return type
    /// rather than an error, because "this engine cannot count queries" is a fact about coverage,
    /// not a failure of the run.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when the adapter cannot answer at all.
    async fn query_count(&self) -> Result<Measured<u64>, DbError>;

    /// The database's schema, for M9's checks.
    ///
    /// Filled in M9; `not_measured` until then for the same reason as [`Self::query_count`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when the adapter cannot answer at all.
    async fn catalog(&self) -> Result<Measured<CatalogSummary>, DbError>;

    /// Removes everything the run created: databases, templates, snapshots and slots.
    ///
    /// Best effort and never fails the run: a teardown that returns an error would turn a
    /// successful evaluation into a failed one because a server was busy. A caller that needs to
    /// know reads the returned list.
    async fn teardown(&self) -> Vec<String>;

    /// Whether this adapter runs repo-controlled SQL, and so needs [`ExecPermit`].
    ///
    /// Postgres and SQLite run only substance's own statements against a database they created, so
    /// they do not; Supabase runs the repository's migrations through the CLI, so it does.
    fn needs_permit(&self) -> bool {
        false
    }
}

/// One row that changed, as the capture channel will report it in M3.
///
/// Defined now so the trait is complete and M3 fills implementations rather than reshaping the
/// API. There is deliberately no delta here: M3 decides whether a change is an insert, update or
/// delete from the WAL record, and guessing the shape before then would fix an interface the
/// implementer has not seen yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowChange {
    /// The table, schema-qualified when the engine reports a schema.
    pub table: String,
    /// The operation, as the plugin spelled it.
    pub operation: String,
}

/// What the catalog channel reports in M9.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSummary {
    /// The tables, sorted by name.
    pub tables: Vec<String>,
}

/// A permit that a database adapter may hold for repo-controlled SQL.
///
/// Not a bool: the point of [`ExecPermit`] is that possession is the proof, and an adapter that
/// stored `needs_permit: true` would have to be handed the permit at the call site anyway. This
/// alias exists so the Supabase adapter's constructor can say what it wants in one word.
pub type DbPermit<'a> = Option<&'a ExecPermit>;
