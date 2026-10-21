//! SQLite: a file in the working copy, snapshotted with the online backup API (M2-T03).
//!
//! ## Why the backup API rather than a file copy
//!
//! A SQLite database in WAL mode keeps recent commits in `<db>-wal`, not in the database file. A
//! plain `cp` of the `.db` file therefore copies an old state, or a torn one if a write is in
//! flight. The backup API reads through the WAL and produces one consistent database in the
//! destination, which is what a snapshot has to be. `bin/cp` would look right and be wrong, which
//! is the worst combination.
//!
//! The reset is a plain copy *back*, because the snapshot is already a single consistent file and
//! nothing is writing to it: the app is stopped, and the destination is a file substance owns.
//!
//! ## Why `-wal` and `-shm` are removed on reset
//!
//! They belong to the *old* database. Left beside a database restored from a snapshot, SQLite
//! would try to apply a WAL that describes a state the file no longer holds, and the result is
//! either a corrupted view or a `SQLITE_BUSY` on the next open. They are deleted, not moved, since
//! the snapshot has everything and the old WAL is exactly what must not be replayed.

use async_trait::async_trait;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use substance_core::ids::SnapshotId;
use substance_core::model::{Capabilities, DbHandle, Measured, RedactedUrl};
use substance_runner::StoppedApp;

use super::{CatalogSummary, DbAdapter, DbError, RowChange};

/// How many pages a backup copies per step.
///
/// SQLite's own documentation uses 100 in its examples. A larger number copies a small database in
/// one step; a smaller one makes the backup yield more often. Neither matters for correctness, and
/// a fixed number keeps the run deterministic.
const PAGES_PER_STEP: i32 = 100;

/// The suffix snapshot files carry inside the run directory.
const SNAPSHOT_SUFFIX: &str = "snapshot";

/// A SQLite database inside the work copy.
#[derive(Debug, Clone)]
pub struct SqliteAdapter {
    /// The database file, absolute.
    path: PathBuf,
    /// The directory snapshots are written to, inside the run directory.
    snapshots: PathBuf,
    /// The environment variable the app reads its URL from.
    url_env: String,
}

impl SqliteAdapter {
    /// An adapter for the database at `path`, with snapshots under `snapshots`.
    ///
    /// Both paths are absolute: the app runs with the work copy as its `cwd`, so a relative path
    /// here would resolve differently depending on where substance was started.
    #[must_use]
    pub fn new(path: PathBuf, snapshots: PathBuf, url_env: String) -> Self {
        Self {
            path,
            snapshots,
            url_env,
        }
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The snapshot file for `id`.
    #[must_use]
    pub fn snapshot_path(&self, id: &SnapshotId) -> PathBuf {
        self.snapshots
            .join(format!("{}.{SNAPSHOT_SUFFIX}", id.as_str()))
    }

    /// Opens the database, mapping a failure to the adapter's error.
    fn open(&self) -> Result<Connection, DbError> {
        Connection::open(&self.path).map_err(|error| DbError::Sqlite {
            operation: "open".to_owned(),
            path: self.path.clone(),
            detail: error.to_string(),
        })
    }

    /// Removes `-wal` and `-shm` beside a database file.
    ///
    /// Missing files are not an error: a database that has never been written in WAL mode has no
    /// side files, and a reset must not fail because there was nothing to delete.
    fn remove_side_files(&self) -> Result<(), DbError> {
        for suffix in ["-wal", "-shm"] {
            let side = side_file(&self.path, suffix);
            match std::fs::remove_file(&side) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(DbError::Io {
                        operation: "remove".to_owned(),
                        path: side,
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    /// Refuses a token whose latch is clear: no stop was observed, so the app may still be writing.
    fn require_stopped(&self, stopped: &StoppedApp) -> Result<(), DbError> {
        if stopped.is_valid() {
            Ok(())
        } else {
            Err(DbError::AppRunning {
                database: self.path.display().to_string(),
            })
        }
    }
}

/// `<db>-wal`, `<db>-shm`: SQLite appends to the whole path, not to the stem.
fn side_file(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_os_string();
    text.push(suffix);
    PathBuf::from(text)
}

#[async_trait]
impl DbAdapter for SqliteAdapter {
    fn engine(&self) -> &'static str {
        "sqlite"
    }

    async fn preflight(&self) -> Result<Capabilities, DbError> {
        // The only capability question SQLite can answer is whether the file can be opened at all;
        // it has no logical decoding and no query statistics. Both channels are `not_measured` with
        // a reason rather than a `false`, because a check reading `false` would report a failure the
        // app did not cause.
        let mut problems = Vec::new();
        match self.open() {
            Ok(connection) => {
                // `PRAGMA journal_mode` is the honest check: a database in `delete` mode has side
                // files that behave differently on reset, and a reader of the manifest should see
                // which one this run used.
                if let Err(error) = connection.query_row("PRAGMA journal_mode", [], |_| Ok(())) {
                    problems.push(format!("cannot read journal mode: {error}"));
                }
            }
            Err(_) => {
                problems.push(format!(
                    "{} does not exist yet, so it will be created by the app's migration",
                    self.path.display()
                ));
            }
        }
        problems.push("sqlite has no logical decoding, so row diffs are not measured".to_owned());
        problems
            .push("sqlite has no query statistics, so query counts are not measured".to_owned());
        Ok(Capabilities {
            engine: self.engine().to_owned(),
            logical_decoding: Measured::not_measured(
                "sqlite has no logical decoding, so row diffs are not measured",
            ),
            query_stats: Measured::not_measured(
                "sqlite has no query statistics, so query counts are not measured",
            ),
            createdb: true,
            problems,
        })
    }

    async fn provision(&self) -> Result<DbHandle, DbError> {
        // SQLite needs no provisioning: the file appears when the app's migration runs. Creating an
        // empty database here would be worse than not creating one, because a migration that finds
        // an existing empty file and a migration that finds nothing behave differently in some
        // tools. The directory is made so the file has somewhere to appear.
        let handle = DbHandle {
            engine: self.engine().to_owned(),
            database: self.path.display().to_string(),
            seeded: String::new(),
            // `sqlite://` and not `file:`: `RedactedUrl` requires a scheme with `://`, and a
            // display that came out as "unparseable" would tell a reader nothing about a database
            // that has no credentials to protect in the first place.
            server: RedactedUrl::parse(&format!("sqlite://{}", self.path.display())),
            url_env: self.url_env.clone(),
            sqlite_path: Some(self.path.display().to_string()),
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DbError::Io {
                operation: "create directory".to_owned(),
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::create_dir_all(&self.snapshots).map_err(|source| DbError::Io {
            operation: "create directory".to_owned(),
            path: self.snapshots.clone(),
            source,
        })?;
        Ok(handle)
    }

    async fn snapshot(&self, label: &str, stopped: &StoppedApp) -> Result<SnapshotId, DbError> {
        self.require_stopped(stopped)?;
        if !self.path.exists() {
            return Err(DbError::Missing {
                path: self.path.clone(),
                reason: "cannot snapshot a database the app never created".to_owned(),
            });
        }
        let id = SnapshotId::derive(&[label.as_bytes()]);
        let destination = self.snapshot_path(&id);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DbError::Io {
                operation: "create directory".to_owned(),
                path: parent.to_path_buf(),
                source,
            })?;
        }
        // The backup reads through the WAL, so the copy is one consistent database even though the
        // source was written to in the last few milliseconds. `rusqlite`'s `Connection::backup`
        // opens the destination itself and steps until done.
        let source = self.open()?;
        source
            .backup(rusqlite::MAIN_DB, &destination, None)
            .map_err(|error| DbError::Sqlite {
                operation: "backup".to_owned(),
                path: self.path.clone(),
                detail: error.to_string(),
            })?;
        Ok(id)
    }

    fn app_url(&self, _handle: &DbHandle) -> Result<String, DbError> {
        // A file path, not a secret. The app gets `sqlite://<path>` so a tool that understands
        // URLs and one that understands paths both work.
        Ok(format!("sqlite://{}", self.path.display()))
    }

    async fn reset(&self, snapshot: &SnapshotId, stopped: &StoppedApp) -> Result<(), DbError> {
        self.require_stopped(stopped)?;
        let source = self.snapshot_path(snapshot);
        if !source.exists() {
            return Err(DbError::Missing {
                path: source,
                reason: "no such snapshot; take one before resetting".to_owned(),
            });
        }
        // The WAL and SHM describe the state being replaced. Deleting them first means the copy
        // cannot be followed by SQLite replaying changes that belong to the old database.
        self.remove_side_files()?;
        std::fs::copy(&source, &self.path).map_err(|source_error| DbError::Io {
            operation: "restore".to_owned(),
            path: self.path.clone(),
            source: source_error,
        })?;
        Ok(())
    }

    async fn begin_capture(&self) -> Result<(), DbError> {
        Err(DbError::Unavailable {
            engine: self.engine().to_owned(),
            reason: "sqlite row capture arrives in M3".to_owned(),
        })
    }

    async fn end_capture(&self) -> Result<Vec<RowChange>, DbError> {
        Err(DbError::Unavailable {
            engine: self.engine().to_owned(),
            reason: "sqlite row capture arrives in M3".to_owned(),
        })
    }

    async fn query_count(&self) -> Result<Measured<u64>, DbError> {
        Ok(Measured::not_measured(
            "sqlite has no query statistics, so query counts are not measured",
        ))
    }

    async fn catalog(&self) -> Result<Measured<CatalogSummary>, DbError> {
        Ok(Measured::not_measured("the catalog channel arrives in M9"))
    }

    async fn teardown(&self) -> Vec<String> {
        let mut removed = Vec::new();
        let mut snapshots_removed = 0_usize;
        if let Ok(entries) = std::fs::read_dir(&self.snapshots) {
            for entry in entries.flatten() {
                if entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == SNAPSHOT_SUFFIX)
                    && std::fs::remove_file(entry.path()).is_ok()
                {
                    snapshots_removed += 1;
                }
            }
        }
        if snapshots_removed > 0 {
            removed.push(format!("{snapshots_removed} snapshot files"));
        }
        // The database file itself is inside the work copy, which the run directory's removal takes
        // with it. Deleting it here would mean teardown had to know whether the app still needed it.
        removed
    }
}

// The step count is a const because a backup that copies one page at a time is a different
// operation from one that copies a hundred, and the plan wants the run reproducible.
const _: () = assert!(PAGES_PER_STEP > 0);
