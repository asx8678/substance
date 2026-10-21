//! Assembling and storing one `EffectRecord` (M3-T03).
//!
//! The driver reports the browser's channels as a [`RecordPart`]; this module adds the channels
//! the recorder observes itself (rows, queries, server logs) and writes the result to
//! `records/<rec_id>.json` as canonical JSON.
//!
//! ## The id is derived, not assigned
//!
//! `RecordId` comes from the run, the check, the action's identity and the repetition. Two runs of
//! the same check on the same snapshot with the same seed therefore produce the same record id, and
//! a replayed record lands on the same file name -- which is what makes a diff of two runs a diff
//! of ids rather than a hunt through timestamps. The derive is over bytes the record itself does
//! not contain (the record contains the id), so it cannot be circular.
//!
//! ## A channel that could not be watched says so
//!
//! Every channel that can be missing is a [`Measured`]. The one place this module must be careful
//! is that it never turns "we did not watch" into "nothing happened": an empty `DbDelta` is a
//! claim, and it may only be made when the capture actually ran.

use std::path::{Path, PathBuf};

use substance_core::hash::canonical_json_pretty_value;
use substance_core::ids::{RecordId, RunId};
use substance_core::model::{DbDelta, EffectRecord, Logs, Measured, RecordPart, RowCounts, Timing};
use substance_runner::BlobStore;

use crate::logs::{LogChannel, LogMark, LogRules};

/// Why a record could not be assembled or written.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// The record could not be serialized.
    #[error("cannot serialize the record: {0}")]
    Serialize(String),
    /// The record could not be written.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The IO error.
        source: std::io::Error,
    },
    /// The record store's directory could not be created.
    #[error("cannot create {path}: {source}")]
    Directory {
        /// The directory.
        path: PathBuf,
        /// The IO error.
        source: std::io::Error,
    },
    /// A blob could not be stored.
    #[error(transparent)]
    Blob(#[from] substance_runner::BlobError),
    /// The log rules do not compile, which is a bug in the data file.
    #[error(transparent)]
    Rules(#[from] crate::logs::RuleError),
    /// A log window could not be read.
    #[error(transparent)]
    Logs(#[from] crate::logs::LogError),
}

/// The parts of a record the recorder knows before an action runs.
#[derive(Debug, Clone)]
pub struct RecordContext {
    /// The run.
    pub run_id: RunId,
    /// The check that asked for the action.
    pub check_id: substance_core::check::CheckId,
    /// What the record is about.
    pub target: substance_core::model::Target,
    /// The snapshot the action ran on.
    pub snapshot: substance_core::ids::SnapshotId,
    /// Which repetition this is.
    pub repetition: u8,
    /// The seed for this action's random inputs.
    pub seed: u64,
    /// The steps that ran before the action.
    pub setup: Vec<substance_core::model::Action>,
    /// The action under test.
    pub action: substance_core::model::Action,
}

impl RecordContext {
    /// The id this record will have.
    ///
    /// Derived from what makes two records the same observation: the run, the check, the target,
    /// the action's kind and target, and the repetition. The seed is deliberately *not* in it: a
    /// different seed is a different action, but the id is the record's identity for a replay, and
    /// a replay varies the seed on purpose.
    #[must_use]
    pub fn id(&self) -> RecordId {
        let repetition = [self.repetition];
        RecordId::derive(&[
            self.run_id.as_str().as_bytes(),
            self.check_id.as_str().as_bytes(),
            self.target.sort_key().0.as_bytes(),
            self.target.sort_key().1.as_bytes(),
            self.action.kind.as_bytes(),
            self.action.target.as_bytes(),
            &repetition,
        ])
    }
}

/// The channels the recorder observed around one action.
///
/// `Timing` has no derived default in `core` (a settle time of zero is a claim), so this struct
/// builds its zero explicitly in `Observed::new` rather than deriving one.
#[derive(Debug, Clone)]
pub struct Observed {
    /// The rows the action wrote, when the row channel was available.
    pub rows: Option<RowCapture>,
    /// Why the row channel was unavailable, when it was.
    pub rows_reason: Option<Measured<()>>,
    /// The statements the action ran, when the query channel was available.
    pub queries: Option<u32>,
    /// The reason the query channel was unavailable.
    pub queries_reason: Option<String>,
    /// How long the action took to settle.
    pub timing: Timing,
}

impl Default for Observed {
    fn default() -> Self {
        Self::new()
    }
}

impl Observed {
    /// No channel observed yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: None,
            rows_reason: None,
            queries: None,
            queries_reason: None,
            timing: Timing {
                settle_ms: 0,
                timed_out: false,
            },
        }
    }
}

/// What one capture window saw: the changes, and the lines it could not read.
///
/// The unparsed lines travel with the changes rather than being logged and dropped: a plugin whose
/// output format changed would otherwise make a write look like no write, and the count would be
/// wrong without anything in the record saying so.
#[derive(Debug, Clone, Default)]
pub struct RowCapture {
    /// Every change, in the order the engine reported it.
    pub changes: Vec<crate::changes::RowChange>,
    /// Lines that looked like changes and could not be parsed.
    pub unparsed: Vec<String>,
}

/// The counts type: operation, then table, then how many.
type CountsByOperation =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, u32>>;

impl RowCapture {
    /// Counts by operation, then table: `insert` → `notes` → 3.
    #[must_use]
    pub fn counts(&self) -> CountsByOperation {
        let mut counts = CountsByOperation::new();
        for change in &self.changes {
            let counter = counts
                .entry(change.operation.clone())
                .or_default()
                .entry(change.table.clone())
                .or_insert(0);
            *counter = counter.saturating_add(1);
        }
        counts
    }
}

/// Turns one capture into the record's `DbDelta`, with the rows as a blob.
///
/// The counts are per operation and table; the rows are the changes themselves -- column by column,
/// after and before -- sorted so two runs that did the same thing produce the same bytes.
///
/// The images hold values because the oracles judge values: B11 checks an after-image against the
/// injected key, B06/D03/B23 compare a stored value with what was typed, B25 looks for one user's
/// data in another user's rows. Nothing here logs them: the blob is a local, content-addressed
/// artifact under `blobs/`, and the record keeps counts plus a reference to it.
///
/// An empty capture still gets a delta: the capture ran and saw nothing, which is a claim the
/// channel is allowed to make because it watched.
///
/// # Errors
///
/// Returns [`RecordError::Serialize`] if a change cannot be serialized, and [`RecordError::Blob`]
/// if the blob cannot be written.
pub fn db_delta(blobs: &BlobStore, capture: &RowCapture) -> Result<DbDelta, RecordError> {
    let by_operation = capture.counts();
    let counts_for = |operation: &str| RowCounts {
        counts: by_operation.get(operation).cloned().unwrap_or_default(),
        rows: None,
    };
    let mut delta = DbDelta {
        insert: counts_for("insert"),
        update: counts_for("update"),
        delete: counts_for("delete"),
    };
    if capture.changes.is_empty() && capture.unparsed.is_empty() {
        return Ok(delta);
    }
    // Sorted by (operation, table, key, images): the engine's order is the WAL's order, which is
    // not deterministic across two runs of the same action, and two rows of one table can share a
    // key when the table has no primary key.
    let mut changes: Vec<&crate::changes::RowChange> = capture.changes.iter().collect();
    changes.sort();
    let rows = serde_json::json!({
        "changes": changes,
        "unparsed": &capture.unparsed,
    });
    let bytes = substance_core::hash::canonical_json_value(&rows);
    let reference = blobs.put(&bytes)?;
    // Only the operations that happened cite the blob. Pointing an empty bucket at rows that are
    // all of another kind would make `insert.rows` non-empty while `insert.counts` is empty, and a
    // reader would have to know which of the two to believe.
    if !delta.insert.counts.is_empty() {
        delta.insert.rows = Some(reference.clone());
    }
    if !delta.update.counts.is_empty() {
        delta.update.rows = Some(reference.clone());
    }
    if !delta.delete.counts.is_empty() {
        delta.delete.rows = Some(reference);
    }
    Ok(delta)
}

/// Writes records to `run_dir/records/`.
#[derive(Debug, Clone)]
pub struct RecordStore {
    dir: PathBuf,
    blobs: BlobStore,
}

impl RecordStore {
    /// A store under `run_dir/records/`, with blobs in `run_dir/blobs/`.
    #[must_use]
    pub fn new(run_dir: &Path) -> Self {
        Self {
            dir: run_dir.join("records"),
            blobs: BlobStore::new(run_dir),
        }
    }

    /// The directory records are written to.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The blob store this store writes row images and log slices to.
    ///
    /// Exposed so a reader -- a report, an oracle, or a probe -- can open the artifact a record
    /// cites without knowing the blob layout.
    #[must_use]
    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// The path a record id maps to.
    #[must_use]
    pub fn path_of(&self, id: &RecordId) -> PathBuf {
        self.dir.join(format!("{}.json", id.as_str()))
    }

    /// Writes `record` as canonical JSON, returning its path.
    ///
    /// Canonical rather than pretty: two runs that observed the same thing must produce the same
    /// bytes, and key order is part of that. The write is atomic (a temporary file renamed into
    /// place) so a reader never sees half a record: a run that is interrupted leaves either the
    /// old file or the new one.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when the record cannot be serialized or the file cannot be written.
    pub fn write(&self, record: &EffectRecord) -> Result<PathBuf, RecordError> {
        let value = serde_json::to_value(record)
            .map_err(|error| RecordError::Serialize(error.to_string()))?;
        let bytes = canonical_json_pretty_value(&value);
        std::fs::create_dir_all(&self.dir).map_err(|source| RecordError::Directory {
            path: self.dir.clone(),
            source,
        })?;
        let path = self.path_of(&record.id);
        let temporary = path.with_extension("partial");
        std::fs::write(&temporary, &bytes).map_err(|source| RecordError::Io {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, &path).map_err(|source| RecordError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    /// Merges the driver's part with the observed channels into a record.
    ///
    /// The parts the driver reports are taken from `part` unchanged. The channels the recorder
    /// owns are filled here, and a channel that was not watched becomes `not_measured` with its
    /// reason -- never an empty value.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] only if the platform's log rules cannot be compiled, which is a bug
    /// rather than a runtime condition.
    pub fn assemble(
        &self,
        context: &RecordContext,
        part: &RecordPart,
        observed: &Observed,
        log_window: Option<&str>,
    ) -> Result<EffectRecord, RecordError> {
        let db = match (&observed.rows, &observed.rows_reason) {
            (Some(capture), _) => Measured::observed(db_delta(&self.blobs, capture)?),
            (None, Some(Measured::NotMeasured { reason })) => {
                Measured::not_measured(reason.clone())
            }
            (None, _) => Measured::not_measured("the row channel did not report a reason"),
        };
        let queries = match (observed.queries, &observed.queries_reason) {
            (Some(count), _) => Measured::observed(count),
            (None, Some(reason)) => Measured::not_measured(reason.clone()),
            (None, None) => Measured::not_measured("the query channel did not report a reason"),
        };

        // The server's own log lines: counted by the rules, kept as a blob when there are any.
        let mut logs: Logs = part.logs.clone();
        match log_window {
            Some(window) => {
                let rules = LogRules::embedded()?;
                let slice = rules.slice(window);
                logs.server_errors = Measured::observed(slice.error_count());
                if slice.lines.is_empty() {
                    logs.server_lines = None;
                } else {
                    let mut text = slice.lines.join("\n");
                    text.push('\n');
                    logs.server_lines = Some(self.blobs.put(text.as_bytes())?);
                }
            }
            None => {
                logs.server_errors =
                    Measured::not_measured("no server log was attached to this run");
                logs.server_lines = None;
            }
        }

        Ok(EffectRecord {
            id: context.id(),
            run_id: context.run_id.clone(),
            check_id: context.check_id,
            target: context.target.clone(),
            snapshot: context.snapshot.clone(),
            repetition: context.repetition,
            seed: context.seed,
            setup: context.setup.clone(),
            action: context.action.clone(),
            // M5 injects failures; until then nothing was injected, which is a fact about this
            // record rather than a channel that could not be watched.
            injected: substance_core::model::Injected { failure: None },
            ui: part.ui.clone(),
            net: part.net.clone(),
            ws: part.ws.clone(),
            storage: part.storage.clone(),
            db,
            logs,
            queries,
            // M3-T04 fills these; saying so is the difference between "no third-party call" and
            // "nothing was watching for one".
            egress: Measured::not_measured("the egress proxy arrives in M3-T04"),
            email: Measured::not_measured("Mailpit arrives in M3-T04"),
            timing: observed.timing.clone(),
        })
    }
}

/// An empty delta for a channel that ran and saw nothing.
///
/// Separate from `DbDelta::default()` on purpose, so the one place that claims "no rows changed"
/// is a call with a name that says it was watched.
#[must_use]
pub fn empty_delta() -> DbDelta {
    DbDelta {
        insert: RowCounts {
            counts: std::collections::BTreeMap::new(),
            rows: None,
        },
        update: RowCounts {
            counts: std::collections::BTreeMap::new(),
            rows: None,
        },
        delete: RowCounts {
            counts: std::collections::BTreeMap::new(),
            rows: None,
        },
    }
}

/// Opens the app's log channel for a window, when the run has one.
#[must_use]
pub fn log_channel(logs_dir: &Path, name: &str) -> (LogChannel, LogMark) {
    let channel = LogChannel::new(logs_dir, name);
    let mark = channel.mark();
    (channel, mark)
}
