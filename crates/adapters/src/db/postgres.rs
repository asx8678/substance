//! Postgres: one database pair per run, reset from a template (M2-T03, ADR-0018).
//!
//! ## The reset sequence
//!
//! Resetting from a template is not one statement, and the order is forced by the server:
//!
//! 1. **Drop the logical slot.** A slot holds WAL the server may not recycle. Dropping the database
//!    a slot belongs to does not remove the slot, so a run that skipped this would leave behind a
//!    slot per reset and slowly fill the server's disk -- and the next run's decode would start
//!    from the wrong place.
//! 2. **`DROP DATABASE ... WITH (FORCE)`.** `FORCE` disconnects remaining clients; without it a
//!    pooled connection the app left behind blocks the drop and the reset fails on a lock it cannot
//!    see. The app is stopped (the caller holds a [`StoppedApp`]), so any remaining connection is a
//!    leak, and closing it is what the plan's "app is disconnected" requirement means in practice.
//! 3. **`CREATE DATABASE ... TEMPLATE ..._seeded`.** A template copy is fast because Postgres
//!    copies files rather than replaying statements.
//! 4. **Recreate the slot.** Last, so its start position is the fresh database's.
//!
//! Any failure mid-sequence leaves the database missing or empty, which is why [`PostgresAdapter::reset`]
//! reports what it was doing: a run that fails at step 3 needs to know it is not at step 1.
//!
//! ## Cleanup when nobody calls teardown
//!
//! A logical slot is the one artefact here that outlives its run badly: it pins WAL on the server
//! until something drops it, so a process that exits without teardown -- an error path, a Ctrl-C,
//! a panic on its way out -- leaves the server filling its disk. `PostgresAdapter` therefore owns a
//! [`PgCleanupGuard`]: a plain struct whose `Drop` drops the slot and the run's databases on a
//! short-lived thread with its own runtime and a timeout. `Drop` is synchronous and cannot await, so
//! the alternative -- blocking a runtime thread -- can deadlock a current-thread runtime; a new
//! one-purpose thread cannot, and it is bounded so a hung server cannot hang the exit.
//!
//! What none of this covers: `SIGKILL`. A process killed outright runs no `Drop` and no handler, so
//! a leftover `substance_%` database or slot is possible and `substance doctor` (M2-T06) lists
//! them. The guard is the best effort, not a guarantee.
//!
//! ## Credentials
//!
//! The admin URL comes from `SUBSTANCE_PG_ADMIN_URL` and is kept in the `Config` only. `Config`'s own
//! `Debug` redacts the password, and nothing here formats it: errors carry [`RedactedUrl`], and a
//! server message that somehow contains a URL is passed through [`redact_text`] before it is stored.

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use substance_core::ids::{RunId, SnapshotId};
use substance_core::model::{
    Capabilities, DbHandle, DecodingPlugin, Measured, RedactedUrl, run_tag, snapshot_database,
};
use substance_runner::StoppedApp;
use tokio_postgres::{Client, Config, NoTls};

use super::{CatalogSummary, DbAdapter, DbError, RowChange};

/// The environment variable the admin URL is read from.
pub const ADMIN_URL_ENV: &str = "SUBSTANCE_PG_ADMIN_URL";

/// The slot every run creates, so a later run can tell its own from a leaked one.
const SLOT_SUFFIX: &str = "_slot";

/// The temporary slot used to test a decoding plugin.
///
/// Not named after a run id because it is created and dropped inside one preflight; if a run is
/// killed between the two, `substance doctor` reports it and teardown removes anything matching
/// the prefix.
const PROBE_SLOT: &str = "substance_probe_slot";

/// A Postgres server, and the databases this run owns on it.
#[derive(Debug)]
pub struct PostgresAdapter {
    /// The admin connection settings. Contains the password; never formatted.
    config: Config,
    /// The redacted form of the same settings, for every message.
    server: RedactedUrl,
    /// The base name for this run's databases: `substance_<12 chars>`.
    base: String,
    /// The environment variable the app reads its URL from.
    url_env: String,
    /// What preflight found, so a channel's reason is decided once.
    ///
    /// Behind a mutex because `preflight` takes `&self` and the capture channels read it later:
    /// without the store, `begin_capture` would consult the "preflight has not run yet" value that
    /// every adapter starts with and report every row diff as unmeasured even on a server that
    /// decodes perfectly.
    capabilities: Mutex<Capabilities>,
    /// The snapshot databases this adapter made, so a guard can drop them too.
    ///
    /// A snapshot's database name is derived from its id, which the adapter sees when it makes it;
    /// nothing else records the list, so the guard would otherwise have to sweep names it cannot
    /// know. Appended under a mutex because `snapshot` takes `&self`.
    snapshots: Mutex<Vec<String>>,
    /// Cleans up on drop unless teardown ran. Does nothing until `provision` arms it.
    guard: Mutex<Option<PgCleanupGuard>>,
    /// `pg_stat_statements` as it stood when capture began.
    ///
    /// A field rather than a parameter because the trait's capture is two calls, and the baseline
    /// has to survive between them. `None` before the first `begin_capture`, which is what makes
    /// `query_count` outside a capture say so instead of reporting a number that means nothing.
    query_baseline: Mutex<Option<QueryStatsSnapshot>>,
}

/// `sum(calls)` and the eviction counter, as one read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueryStatsSnapshot {
    /// Total calls against the app's `dbid` at the moment of the read.
    calls: u64,
    /// `pg_stat_statements_info.dealloc`: how many entries have been evicted server-wide.
    dealloc: u64,
}

impl PostgresAdapter {
    /// An adapter from the environment's admin URL, for the run `run_id`.
    ///
    /// The URL is read once, here, so a later change to the environment cannot point a reset at a
    /// different server than the one provisioned against.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when `SUBSTANCE_PG_ADMIN_URL` is unset or unparseable. The
    /// message names the variable, never its value.
    pub fn from_env(run_id: &RunId, url_env: String) -> Result<Self, DbError> {
        let url = std::env::var(ADMIN_URL_ENV).map_err(|_| DbError::Unavailable {
            engine: "postgres".to_owned(),
            reason: format!("{ADMIN_URL_ENV} is not set, so no database can be created"),
        })?;
        Self::new(&url, run_id, url_env)
    }

    /// An adapter for a known admin URL.
    ///
    /// `new` rather than `from_env` so a caller that already holds the URL -- a test harness, or a
    /// future `--pg-admin-url` flag -- does not have to set an environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Unavailable`] when the URL is not a Postgres URL tokio-postgres accepts.
    pub fn new(url: &str, run_id: &RunId, url_env: String) -> Result<Self, DbError> {
        let config: Config =
            url.parse()
                .map_err(|error: tokio_postgres::Error| DbError::Unavailable {
                    engine: "postgres".to_owned(),
                    reason: format!("{ADMIN_URL_ENV} is not a usable Postgres URL: {error}"),
                })?;
        // Derived from the run id, so two runs never collide and a retry of the same run finds its
        // own databases instead of making new ones.
        let base = format!("substance_{}", run_tag(run_id));
        Ok(Self {
            config,
            server: RedactedUrl::parse(url),
            base,
            url_env,
            capabilities: Mutex::new(unprobed_capabilities("preflight has not run yet")),
            snapshots: Mutex::new(Vec::new()),
            // Armed by `provision`, not here: a guard that existed before the databases did would
            // have nothing to drop and would fire on an adapter that never created anything.
            guard: Mutex::new(None),
            query_baseline: Mutex::new(None),
        })
    }

    /// The capabilities preflight found, or the "not run yet" placeholder.
    ///
    /// A clone rather than a guard: every caller reads two fields and drops it, and a caller that
    /// held the mutex across an await would block `preflight` from storing its result.
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| unprobed_capabilities("the capability store was poisoned"))
    }

    /// Points the adapter at a database that already exists, instead of the run's own pair.
    ///
    /// Supabase is the case this exists for: its stack creates and migrates one database, and a
    /// second pair made by this adapter would be a database nothing else in the stack knows about.
    /// The name becomes the app's database for every channel -- the WAL read, the `dbid` filter,
    /// the slot -- so the adapter reports on what the app actually writes to.
    #[must_use]
    pub fn use_existing_database(mut self, name: String) -> Self {
        self.base = name;
        self
    }

    /// The app's database name: `substance_<12 chars>`.
    #[must_use]
    pub fn database(&self) -> String {
        self.base.clone()
    }

    /// The template database name: `substance_<12 chars>_seeded`.
    #[must_use]
    pub fn seeded(&self) -> String {
        format!("{}_seeded", self.base)
    }

    /// The replication slot's name.
    #[must_use]
    pub fn slot(&self) -> String {
        format!("{}{SLOT_SUFFIX}", self.base)
    }

    /// The server's address, credentials removed.
    #[must_use]
    pub fn server(&self) -> &RedactedUrl {
        &self.server
    }

    /// Runs the same preflight a run does, for a caller that is not a run (M2-T06).
    ///
    /// `substance doctor` has to answer exactly the questions a run would answer, or it would be
    /// describing a different host than the one the next run gets. Delegating is the only way to
    /// guarantee that: a second implementation would drift, and the drift would only show up as a
    /// run failing after doctor said the server was fine.
    ///
    /// # Errors
    ///
    /// As [`DbAdapter::preflight`]: a server that cannot be reached, or a statement that fails.
    pub async fn preflight_public(&self) -> Result<Capabilities, DbError> {
        DbAdapter::preflight(self).await
    }

    /// The server's `wal_level`, as it reports it.
    ///
    /// Doctor wants the value itself, where a run only needs to know whether decoding is available.
    /// Asked directly rather than read back out of [`Capabilities::problems`]: that list is prose
    /// written for a report, and parsing prose to recover a fact is how a message improvement turns
    /// into a wrong answer.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] when the server cannot be reached or the setting cannot be read.
    pub async fn wal_level(&self) -> Result<Option<String>, DbError> {
        let client = self.connect().await?;
        Self::scalar(&client, "read wal_level", "SHOW wal_level").await
    }

    /// The `substance_%` databases left on the server, names only, sorted.
    ///
    /// These are the residue a `SIGKILL` leaves, and each may also hold a replication slot, which
    /// pins WAL until something drops it. Doctor lists them; it does not remove them, because a
    /// database named `substance_` may belong to a run that is still going.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] when the catalog cannot be read.
    pub async fn leftover_databases(&self) -> Result<Vec<String>, DbError> {
        let client = self.connect().await?;
        let messages = client
            .simple_query(
                "SELECT datname FROM pg_database WHERE datname LIKE 'substance\\_%' \
                 AND datname <> 'substance' ORDER BY datname",
            )
            .await
            .map_err(|error| DbError::Statement {
                engine: "postgres".to_owned(),
                operation: "list leftover databases".to_owned(),
                database: self.server.to_string(),
                detail: redact_text(&error.to_string()),
            })?;
        let mut names: Vec<String> = Vec::new();
        for message in messages {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message
                && let Some(name) = row.get(0)
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// The replication slots left on the server, names only, sorted.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] when the catalog cannot be read. A server where this fails is one
    /// where the row-diff channel is unavailable, and the caller reports that rather than a leak.
    pub async fn leftover_slots(&self) -> Result<Vec<String>, DbError> {
        let client = self.connect().await?;
        let messages = client
            .simple_query("SELECT slot_name FROM pg_replication_slots ORDER BY slot_name")
            .await
            .map_err(|error| DbError::Statement {
                engine: "postgres".to_owned(),
                operation: "list replication slots".to_owned(),
                database: self.server.to_string(),
                detail: redact_text(&error.to_string()),
            })?;
        let mut names: Vec<String> = Vec::new();
        for message in messages {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message
                && let Some(name) = row.get(0)
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Every database this run owns: the pair `provision` makes plus the snapshots taken since.
    ///
    /// Sorted and deduplicated, so two calls with the same state produce the same list and a
    /// caller reading teardown's output sees a stable order.
    fn cleanup_databases(&self) -> Vec<String> {
        let mut databases = vec![self.database(), self.seeded()];
        if let Ok(snapshots) = self.snapshots.lock() {
            databases.extend(snapshots.iter().cloned());
        }
        databases.sort();
        databases.dedup();
        databases
    }

    /// Connects, returning a client with its connection task running.
    ///
    /// `NoTls` because the admin URL in CI is a local service container and a Unix socket or a
    /// loopback TCP connection needs no TLS. A server that requires TLS fails here with the server's
    /// own message, which is a setup problem a reader should see rather than one to paper over with
    /// a TLS stack the plan does not ask for.
    async fn connect(&self) -> Result<Client, DbError> {
        let (client, connection) =
            self.config
                .connect(NoTls)
                .await
                .map_err(|error| DbError::Statement {
                    engine: "postgres".to_owned(),
                    operation: "connect".to_owned(),
                    database: self.server.to_string(),
                    detail: redact_text(&error.to_string()),
                })?;
        // The connection future drives the protocol; without a task polling it, every query hangs.
        tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(client)
    }

    /// Runs one statement, mapping a failure to the adapter's error.
    async fn execute(
        client: &Client,
        operation: &str,
        database: &str,
        sql: &str,
    ) -> Result<(), DbError> {
        client
            .batch_execute(sql)
            .await
            .map_err(|error| DbError::Statement {
                engine: "postgres".to_owned(),
                operation: operation.to_owned(),
                database: database.to_owned(),
                detail: redact_text(&error.to_string()),
            })
    }

    /// Reads one text column, mapping a failure to the adapter's error.
    async fn scalar(
        client: &Client,
        operation: &str,
        sql: &str,
    ) -> Result<Option<String>, DbError> {
        let messages = client
            .simple_query(sql)
            .await
            .map_err(|error| DbError::Statement {
                engine: "postgres".to_owned(),
                operation: operation.to_owned(),
                database: "admin".to_owned(),
                detail: redact_text(&error.to_string()),
            })?;
        for message in messages {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                return Ok(row.get(0).map(str::to_owned));
            }
        }
        Ok(None)
    }

    /// `sum(calls)` for one database, plus the server's eviction counter.
    ///
    /// Filtered by `dbid` so another run's queries on the same server are not counted, and read
    /// over the admin connection to `postgres`, which is not the app's database: substance's own
    /// statements therefore never appear in the file being read.
    ///
    /// Returns `Ok(None)` when the view is not readable at all (the extension is installed in
    /// another database), which a caller reports as `not_measured` rather than as zero.
    async fn query_stats_snapshot(
        client: &Client,
        database: &str,
    ) -> Result<Option<QueryStatsSnapshot>, DbError> {
        let sql = format!(
            "SELECT \
                COALESCE((SELECT SUM(calls)::bigint FROM pg_stat_statements s \
                  WHERE s.dbid = (SELECT oid FROM pg_database WHERE datname = '{database}')), 0), \
                (SELECT dealloc FROM pg_stat_statements_info)"
        );
        let rows = client
            .simple_query(&sql)
            .await
            .map_err(|error| DbError::Statement {
                engine: "postgres".to_owned(),
                operation: "read pg_stat_statements".to_owned(),
                database: database.to_owned(),
                detail: error.to_string(),
            });
        let rows = match rows {
            Ok(rows) => rows,
            // `query_stats` being preloaded does not mean the view exists in `postgres`. That is
            // a missing channel, not a failed run.
            Err(_) => return Ok(None),
        };
        for message in rows {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                let calls = row
                    .get(0)
                    .and_then(|text| text.parse::<u64>().ok())
                    .unwrap_or(0);
                let dealloc = row
                    .get(1)
                    .and_then(|text| text.parse::<u64>().ok())
                    .unwrap_or(0);
                return Ok(Some(QueryStatsSnapshot { calls, dealloc }));
            }
        }
        Ok(None)
    }

    /// Creates the run's replication slot, failing if the plugin cannot be used.
    async fn create_slot(
        client: &Client,
        slot: &str,
        plugin: DecodingPlugin,
    ) -> Result<(), DbError> {
        let sql = format!(
            "SELECT pg_create_logical_replication_slot('{slot}', '{}')",
            plugin.as_str()
        );
        Self::execute(client, "create replication slot", slot, &sql).await
    }
}

/// How long the drop-path cleanup may take before the thread is abandoned.
///
/// `Drop` runs on the way out of a run that has already failed, often with a server that is slow or
/// gone. Five seconds is enough for three statements against a local server and short enough that
/// a person pressing Ctrl-C does not wait on a database to find out the run is over.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// What a cleanup has to reach: the server, and the names to remove on it.
///
/// Cloned out of the adapter so the cleanup thread owns everything it needs and holds no reference
/// to the adapter that spawned it. `Debug` is not derived from `Config`'s fields: `Config`'s own
/// `Debug` redacts the password, and nothing here formats it at all.
#[derive(Debug)]
struct CleanupTarget {
    /// The admin connection settings. Contains the password; never formatted.
    config: Config,
    /// The redacted form, for the one message a failure produces.
    server: RedactedUrl,
    /// The run's databases: the app's, the seeded template, and any snapshot databases.
    databases: Vec<String>,
    /// The run's replication slot.
    slot: String,
}

impl CleanupTarget {
    /// Drops the slot and then the databases, returning what it removed.
    ///
    /// Slot first, for the same reason the reset does it first: the slot pins WAL for a database
    /// the drop cannot reclaim, so dropping the database alone would leave the slot behind -- which
    /// is the leak this guard exists to prevent.
    async fn run(self) -> Vec<String> {
        let mut removed = Vec::new();
        let Ok((client, connection)) = self.config.connect(NoTls).await else {
            return vec![format!("postgres at {}: could not connect", self.server)];
        };
        // The connection future drives the protocol. Aborted when this scope ends, which is what
        // keeps a cleanup thread from outliving its timeout with a live socket.
        let handle = tokio::spawn(async move {
            let _ = connection.await;
        });
        let slot = self.slot;
        let exists = client
            .simple_query(&format!(
                "SELECT slot_name FROM pg_replication_slots WHERE slot_name = '{slot}'"
            ))
            .await;
        if exists.is_ok()
            && client
                .batch_execute(&format!("SELECT pg_drop_replication_slot('{slot}')"))
                .await
                .is_ok()
        {
            removed.push(format!("slot {slot}"));
        }
        for name in self.databases {
            let sql = format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)");
            if client.batch_execute(&sql).await.is_ok() {
                removed.push(format!("database {name}"));
            }
        }
        // Dropped before the runtime goes away, so the cleanup does not leave a task that a
        // current-thread runtime would then have to unwind.
        handle.abort();
        removed
    }
}

/// Drops a run's slot and databases if the adapter is dropped without a teardown.
///
/// A field of [`PostgresAdapter`] rather than an `impl Drop` on the adapter itself: this way the
/// adapter stays destructurable and `Drop` on the guard can reach the names it needs without
/// borrowing the adapter back out of itself.
#[derive(Debug)]
pub struct PgCleanupGuard {
    /// What to remove, taken when the guard fires.
    target: Option<CleanupTarget>,
    /// Set by `PostgresAdapter::teardown`, so a teardown followed by a drop does nothing.
    defused: Arc<AtomicBool>,
}

impl PgCleanupGuard {
    /// A guard that will clean up `target` unless it is defused first.
    fn new(target: CleanupTarget) -> Self {
        Self {
            target: Some(target),
            defused: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Marks the run as already cleaned up.
    ///
    /// Called by [`PostgresAdapter::teardown`] and by a successful `Drop` path, so a second
    /// teardown and a teardown followed by a drop both do nothing.
    fn defuse(&self) {
        self.defused.store(true, Ordering::SeqCst);
    }
}

impl Drop for PgCleanupGuard {
    /// Best effort, bounded, and never panics.
    ///
    /// `Drop` cannot await and must not block a runtime thread: `block_on` inside a current-thread
    /// runtime would panic ("cannot block the current thread from within a runtime"), and the
    /// adapters are used from both kinds of runtime. A dedicated thread with its own runtime cannot
    /// deadlock either way. The thread is joined with a timeout, so a server that accepts the
    /// connection and then stops answering cannot hold the process open.
    fn drop(&mut self) {
        if self.defused.load(Ordering::SeqCst) {
            return;
        }
        let Some(target) = self.target.take() else {
            return;
        };
        // The thread reports completion on a channel, so the wait is bounded: `join` has no
        // timeout in the standard library, and an unbounded join on a server that stopped answering
        // would hang the exit this guard exists to make clean. A thread left running past the
        // deadline is abandoned, and the process exit ends it.
        let (done, finished) = std::sync::mpsc::channel::<()>();
        // A thread that cannot be spawned is not a panic: the cleanup is best effort, and the
        // alternative -- doing nothing at all -- is what happens anyway on SIGKILL.
        let Ok(_handle) = std::thread::Builder::new()
            .name("substance-pg-cleanup".to_owned())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let _removed = runtime.block_on(target.run());
                let _ = done.send(());
            })
        else {
            return;
        };
        let _ = finished.recv_timeout(CLEANUP_TIMEOUT);
    }
}

/// Capabilities before preflight has run: every channel `not_measured`, with `reason`.
fn unprobed_capabilities(reason: &str) -> Capabilities {
    Capabilities {
        engine: "postgres".to_owned(),
        logical_decoding: Measured::not_measured(reason),
        query_stats: Measured::not_measured(reason),
        createdb: false,
        problems: vec![reason.to_owned()],
    }
}

/// Removes anything that looks like a connection URL from a server message.
///
/// A Postgres error can echo the connection string in a `DETAIL` line when a `dblink` or a misuse
/// of `current_setting('...')` fails. Passing it through unchanged would put a password in a log.
/// The rule is deliberately blunt: any `scheme://...@...` token is replaced whole, because a
/// partially redacted URL is a leak with a fig leaf.
#[must_use]
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        // Copy everything up to the scheme's start, then the scheme itself.
        let scheme_start = rest[..index]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'))
            .map_or(0, |position| position + 1);
        out.push_str(&rest[..scheme_start]);
        let scheme = &rest[scheme_start..index];
        // The URL runs to the next whitespace or quote; anything further is other text.
        let after = &rest[index..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ')')
            .unwrap_or(after.len());
        out.push_str(&RedactedUrl::parse(&format!("{scheme}{}", &after[..end])).to_string());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[async_trait]
impl DbAdapter for PostgresAdapter {
    fn engine(&self) -> &'static str {
        "postgres"
    }

    async fn preflight(&self) -> Result<Capabilities, DbError> {
        let client = self.connect().await?;
        let mut problems = Vec::new();

        // 1. wal_level must be logical, or no slot can decode anything.
        let wal_level = Self::scalar(&client, "read wal_level", "SHOW wal_level").await?;
        let logical_ok = wal_level.as_deref() == Some("logical");
        if !logical_ok {
            problems.push(format!(
                "wal_level is {}, not logical, so row diffs cannot be captured",
                wal_level.unwrap_or_else(|| "unknown".to_owned())
            ));
        }

        // 2. A decoding plugin, tried for real: `wal2json` first because its output carries column
        //    names, then `test_decoding`, which ships with Postgres. Trying is the only honest
        //    check: a plugin name in a catalog entry does not mean the shared library loads.
        let mut decoding =
            Measured::not_measured("wal_level is not logical, so no plugin could be tested");
        if logical_ok {
            let mut found: Option<DecodingPlugin> = None;
            let mut failures: Vec<String> = Vec::new();
            for (plugin, label) in [
                (DecodingPlugin::Wal2json, "wal2json"),
                (DecodingPlugin::TestDecoding, "test_decoding"),
            ] {
                // The slot attempt *is* the test, so there is no pre-check that could skip it:
                // `wal2json` builds that predate the extension packaging are not listed in
                // `pg_available_extensions` yet work perfectly, and a pre-check would have
                // reported them missing and fallen back for no reason. A plugin whose library is
                // absent fails the attempt with the server's own message, which is a better
                // reason to report than "not listed".
                match Self::create_slot(&client, PROBE_SLOT, plugin).await {
                    Ok(()) => {
                        let drop_sql = format!("SELECT pg_drop_replication_slot('{PROBE_SLOT}')");
                        let _ =
                            Self::execute(&client, "drop probe slot", PROBE_SLOT, &drop_sql).await;
                        found = Some(plugin);
                        break;
                    }
                    Err(error) => {
                        failures.push(format!("{label}: {error}"));
                        // A half-created slot would block the next attempt.
                        let drop_sql = format!("SELECT pg_drop_replication_slot('{PROBE_SLOT}')");
                        let _ =
                            Self::execute(&client, "drop probe slot", PROBE_SLOT, &drop_sql).await;
                    }
                }
            }
            decoding = match found {
                Some(plugin) => Measured::observed(plugin),
                None => {
                    let reason = format!(
                        "no logical decoding plugin is usable: {}",
                        failures.join("; ")
                    );
                    problems.push(reason.clone());
                    Measured::not_measured(reason)
                }
            };
        }

        // 3. pg_stat_statements must be preloaded: it cannot be loaded at runtime, so the setting
        //    is the answer, not the extension's presence in a database.
        let shared = Self::scalar(
            &client,
            "read shared_preload_libraries",
            "SHOW shared_preload_libraries",
        )
        .await?;
        let preloaded = shared.as_deref().is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim() == "pg_stat_statements")
        });
        let query_stats = if preloaded {
            Measured::observed(())
        } else {
            let reason = format!(
                "pg_stat_statements is not in shared_preload_libraries (it is {:?}), so query counts are not measured",
                shared.unwrap_or_default()
            );
            problems.push(reason.clone());
            Measured::not_measured(reason)
        };

        // 4. CREATEDB. Checked by asking for the privilege rather than by trying to create a
        //    database: a failed create would be a side effect of a capability probe.
        let createdb = Self::scalar(
            &client,
            "read createdb privilege",
            "SELECT rolcreatedb::text FROM pg_roles WHERE rolname = current_user",
        )
        .await?
        .as_deref()
            == Some("true");
        if !createdb {
            problems.push(
                "the admin role lacks CREATEDB, so no run database can be created".to_owned(),
            );
        }

        let capabilities = Capabilities {
            engine: self.engine().to_owned(),
            logical_decoding: decoding,
            query_stats,
            createdb,
            problems,
        };
        // Stored before returning: the capture channels run after preflight and must see the same
        // answer that the run recorded, not the placeholder the adapter was built with.
        if let Ok(mut slot) = self.capabilities.lock() {
            *slot = capabilities.clone();
        }
        Ok(capabilities)
    }

    async fn provision(&self) -> Result<DbHandle, DbError> {
        let client = self.connect().await?;
        let database = self.database();
        let seeded = self.seeded();
        // Armed before the first DROP, so a provision that fails between the two creates still
        // leaves the guard responsible for what it made. Arming after the creates would leak exactly
        // the half-done state a retry is most likely to hit.
        let target = CleanupTarget {
            config: self.config.clone(),
            server: self.server.clone(),
            databases: self.cleanup_databases(),
            slot: self.slot(),
        };
        if let Ok(mut guard) = self.guard.lock()
            && let Some(previous) = guard.replace(PgCleanupGuard::new(target))
        {
            // The replaced guard must not drop the databases the new one is responsible for, so
            // it is defused before it goes out of scope.
            previous.defuse();
        }
        // Idempotent: a retry after a partial failure must not fail on the database the failed
        // attempt left behind.
        for name in [&database, &seeded] {
            let drop_sql = format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)");
            Self::execute(&client, "drop database", name, &drop_sql).await?;
        }
        for name in [&database, &seeded] {
            let create_sql = format!("CREATE DATABASE {name}");
            Self::execute(&client, "create database", name, &create_sql).await?;
        }
        Ok(DbHandle {
            engine: self.engine().to_owned(),
            database,
            seeded,
            server: self.server.clone(),
            url_env: self.url_env.clone(),
            sqlite_path: None,
        })
    }

    async fn snapshot(&self, label: &str, stopped: &StoppedApp) -> Result<SnapshotId, DbError> {
        if !stopped.is_valid() {
            return Err(DbError::AppRunning {
                database: self.database(),
            });
        }
        let client = self.connect().await?;
        let id = SnapshotId::derive(&[label.as_bytes()]);
        let name = snapshot_database(&id);
        // A snapshot in Postgres *is* a database: `CREATE DATABASE x TEMPLATE y` is the copy, so
        // making the snapshot is creating the template a reset will use.
        let drop_sql = format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)");
        Self::execute(&client, "drop snapshot", &name, &drop_sql).await?;
        let create_sql = format!("CREATE DATABASE {name} TEMPLATE {}", self.database());
        Self::execute(&client, "create snapshot", &name, &create_sql).await?;
        // Recorded before returning: a snapshot the guard does not know about is a database only
        // `substance doctor` can find.
        if let Ok(mut snapshots) = self.snapshots.lock()
            && !snapshots.contains(&name)
        {
            snapshots.push(name);
        }
        Ok(id)
    }

    fn app_url(&self, handle: &DbHandle) -> Result<String, DbError> {
        // Built from the admin config's host, port and user, with the admin password, and the run's
        // database name. The password has to be in it, because the app connects to the server.
        let host = match self.config.get_hosts().first() {
            Some(tokio_postgres::config::Host::Tcp(host)) => host.clone(),
            #[cfg(unix)]
            Some(tokio_postgres::config::Host::Unix(path)) => path.display().to_string(),
            None => "127.0.0.1".to_owned(),
        };
        let port = self.config.get_ports().first().copied().unwrap_or(5432);
        let user = self.config.get_user().unwrap_or("postgres");
        let password = self
            .config
            .get_password()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default();
        Ok(format!(
            "postgres://{user}:{password}@{host}:{port}/{}",
            handle.database
        ))
    }

    async fn reset(&self, snapshot: &SnapshotId, stopped: &StoppedApp) -> Result<(), DbError> {
        // Checked before the first statement: a stale token must touch nothing at all.
        if !stopped.is_valid() {
            return Err(DbError::AppRunning {
                database: self.database(),
            });
        }
        let client = self.connect().await?;
        let database = self.database();
        let template = snapshot_database(snapshot);
        let slot = self.slot();

        // 1. The slot first: it pins WAL and belongs to the database about to be dropped.
        let slot_exists = Self::scalar(
            &client,
            "read replication slots",
            &format!("SELECT slot_name FROM pg_replication_slots WHERE slot_name = '{slot}'"),
        )
        .await?;
        if slot_exists.is_some() {
            let drop_sql = format!("SELECT pg_drop_replication_slot('{slot}')");
            Self::execute(&client, "drop replication slot", &slot, &drop_sql).await?;
        }
        // 2. Drop, forcing leftover connections out. A pooled client the app abandoned is a leak,
        //    not a client still using the database, because the app is stopped.
        let drop_sql = format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)");
        Self::execute(&client, "drop database", &database, &drop_sql).await?;
        // 3. Copy the template. This is the reset.
        let create_sql = format!("CREATE DATABASE {database} TEMPLATE {template}");
        Self::execute(
            &client,
            "create database from template",
            &database,
            &create_sql,
        )
        .await?;
        // 4. Recreate the slot, so M3's capture starts from the fresh database's position.
        if let Some(plugin) = self.capabilities().logical_decoding.value().copied() {
            Self::create_slot(&client, &slot, plugin).await?;
        }
        Ok(())
    }

    async fn begin_capture(&self) -> Result<(), DbError> {
        // The slot is created here rather than in `provision` because it is a per-capture
        // resource: a slot created at provision time and never read would pin WAL for the whole
        // run. It is dropped at teardown either way, and `Drop` drops it too.
        let capabilities = self.capabilities();
        let Some(plugin) = capabilities.logical_decoding.value().copied() else {
            return Err(DbError::Unavailable {
                engine: self.engine().to_owned(),
                reason: capabilities
                    .logical_decoding
                    .reason()
                    .unwrap_or("no logical decoding plugin")
                    .to_owned(),
            });
        };
        let client = self.connect().await?;
        // A slot left by an earlier capture in the same run would start from the old position, so
        // it is dropped first. Only this run's slot has this name.
        let slot = self.slot();
        let exists = Self::scalar(
            &client,
            "read replication slots",
            &format!("SELECT slot_name FROM pg_replication_slots WHERE slot_name = '{slot}'"),
        )
        .await?;
        if exists.is_some() {
            let drop_sql = format!("SELECT pg_drop_replication_slot('{slot}')");
            Self::execute(&client, "drop replication slot", &slot, &drop_sql).await?;
        }
        Self::create_slot(&client, &slot, plugin).await?;
        // The query baseline is taken here, before the action, so the delta has something to be a
        // delta from. It is a best-effort read: a server without pg_stat_statements must not make
        // `begin_capture` fail, because the row channel is independent of the query channel.
        if let Ok(Some(snapshot)) = Self::query_stats_snapshot(&client, &self.database()).await
            && let Ok(mut baseline) = self.query_baseline.lock()
        {
            *baseline = Some(snapshot);
        }
        Ok(())
    }

    async fn end_capture(&self) -> Result<Vec<RowChange>, DbError> {
        let capabilities = self.capabilities();
        let Some(plugin) = capabilities.logical_decoding.value().copied() else {
            return Err(DbError::Unavailable {
                engine: self.engine().to_owned(),
                reason: capabilities
                    .logical_decoding
                    .reason()
                    .unwrap_or("no logical decoding plugin")
                    .to_owned(),
            });
        };
        let client = self.connect().await?;
        let slot = self.slot();
        // `pg_logical_slot_get_changes` both reads and consumes: a second call returns nothing
        // more, which is what makes one capture window one call.
        let sql = format!("SELECT data FROM pg_logical_slot_get_changes('{slot}', NULL, NULL)");
        let rows = client
            .simple_query(&sql)
            .await
            .map_err(|error| DbError::Statement {
                engine: self.engine().to_owned(),
                operation: "read logical changes".to_owned(),
                database: self.server.to_string(),
                detail: error.to_string(),
            })?;
        let mut output = String::new();
        for message in rows {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message
                && let Some(data) = row.get(0)
            {
                output.push_str(data);
                output.push('\n');
            }
        }
        let set = substance_recorder::changes::parse_capture(&output, plugin.as_str());
        // Lines the parser did not understand are kept in the record rather than dropped: a plugin
        // whose output changed shape would otherwise make a write look like no write.
        Ok(set.changes)
    }

    async fn query_count(&self) -> Result<Measured<u64>, DbError> {
        let capabilities = self.capabilities();
        if capabilities.query_stats.reason().is_some() {
            return Ok(Measured::not_measured(
                capabilities
                    .query_stats
                    .reason()
                    .unwrap_or("pg_stat_statements is unavailable")
                    .to_owned(),
            ));
        }
        let baseline = self.query_baseline.lock().ok().and_then(|guard| *guard);
        let Some(baseline) = baseline else {
            return Ok(Measured::not_measured(
                "no query baseline: capture did not start",
            ));
        };
        let client = self.connect().await?;
        let Some(after) = Self::query_stats_snapshot(&client, &self.database()).await? else {
            return Ok(Measured::not_measured(
                "pg_stat_statements is not readable on this server",
            ));
        };
        // Eviction check: entries removed between the two reads mean some statements were never
        // counted, so the difference is a lower bound that cannot be reported as a count. This is
        // the plan's §3 issue 38, and it is why no reset is ever sent: a reset is server-wide and
        // would corrupt a run happening beside this one.
        if after.dealloc != baseline.dealloc {
            return Ok(Measured::not_measured("pg_stat_statements evicted"));
        }
        let delta = after.calls.saturating_sub(baseline.calls);
        Ok(Measured::observed(delta))
    }

    async fn catalog(&self) -> Result<Measured<CatalogSummary>, DbError> {
        Ok(Measured::not_measured("the catalog channel arrives in M9"))
    }

    async fn teardown(&self) -> Vec<String> {
        // Idempotent twice over: the guard is defused first, so nothing fires when the adapter is
        // dropped, and taking the guard out means a second `teardown` finds nothing armed and
        // returns an empty list rather than repeating the statements.
        let armed = match self.guard.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => None,
        };
        let Some(mut guard) = armed else {
            return Vec::new();
        };
        guard.defuse();
        let target = guard.target.take();
        let Some(target) = target else {
            return Vec::new();
        };
        // The async path, not the guard's thread: teardown is always called from a task, so it can
        // await and report what failed instead of being bounded by the drop path's timeout.
        let mut removed = Vec::new();
        let Ok((client, connection)) = target.config.connect(NoTls).await else {
            return vec![format!(
                "postgres at {}: could not connect, nothing torn down",
                target.server
            )];
        };
        let connection_task = tokio::spawn(async move {
            let _ = connection.await;
        });
        if let Ok(Some(_)) = Self::scalar(
            &client,
            "read replication slots",
            &format!(
                "SELECT slot_name FROM pg_replication_slots WHERE slot_name = '{}'",
                target.slot
            ),
        )
        .await
        {
            let sql = format!("SELECT pg_drop_replication_slot('{}')", target.slot);
            if Self::execute(&client, "drop replication slot", &target.slot, &sql)
                .await
                .is_ok()
            {
                removed.push(format!("slot {}", target.slot));
            }
        }
        for name in target.databases {
            let sql = format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)");
            if Self::execute(&client, "drop database", &name, &sql)
                .await
                .is_ok()
            {
                removed.push(format!("database {name}"));
            }
        }
        connection_task.abort();
        removed
    }
}

/// Drops a run's slot and databases, reading the admin URL from the environment again.
///
/// The interrupt path's equivalent of [`PostgresAdapter::teardown`], for a caller that holds the
/// *names* rather than the adapter: `DatabaseCleanup` in [`super::cleanup`]. It reads
/// `SUBSTANCE_PG_ADMIN_URL` itself and never keeps it, for the same reason the adapter does not put
/// it in a message -- the registrar is long-lived and freely cloned.
///
/// Idempotent, so it is safe beside the adapter's own teardown and the drop guard: a slot is dropped
/// only when it is listed, and every database is dropped with `IF EXISTS`.
///
/// Every failure is a line in the returned list rather than an error, because the caller is an
/// interrupt handler with no one left to hand an error to.
pub async fn drop_names(databases: &[String], slot: &str) -> Vec<String> {
    let mut removed = Vec::new();
    let Ok(url) = std::env::var(ADMIN_URL_ENV) else {
        return vec![format!(
            "postgres: {ADMIN_URL_ENV} is not set, so nothing could be torn down"
        )];
    };
    let Ok(config) = url.parse::<Config>() else {
        return vec!["postgres: the admin URL is not usable, so nothing was torn down".to_owned()];
    };
    let Ok((client, connection)) = config.connect(NoTls).await else {
        return vec!["postgres: could not connect, so nothing was torn down".to_owned()];
    };
    let task = tokio::spawn(async move {
        let _ = connection.await;
    });
    let listed = client
        .simple_query(&format!(
            "SELECT slot_name FROM pg_replication_slots WHERE slot_name = '{slot}'"
        ))
        .await;
    if listed.is_ok()
        && client
            .batch_execute(&format!("SELECT pg_drop_replication_slot('{slot}')"))
            .await
            .is_ok()
    {
        removed.push(format!("slot {slot}"));
    }
    for name in databases {
        if client
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
            .await
            .is_ok()
        {
            removed.push(format!("database {name}"));
        }
    }
    task.abort();
    removed.sort();
    removed
}

/// Implementation of the runner's interrupt cleanup, so a Ctrl-C tears the databases down.
///
/// The runner cannot name this type (`substance-adapters` depends on `substance-runner`, not the
/// other way round), so the bridge is this trait implementation: the orchestrator registers the
/// adapter as an `Arc<dyn Cleanup>` and the interrupt handler calls `cleanup` on it.
impl substance_runner::Cleanup for PostgresAdapter {
    fn cleanup(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send + '_>> {
        Box::pin(async move { self.teardown().await })
    }
}
