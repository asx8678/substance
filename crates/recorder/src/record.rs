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
use substance_core::model::{
    DbDelta, EffectRecord, EgressEntry, EmailEntry, Logs, Measured, NetEntry, RecordPart,
    RowCounts, Timing,
};
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
    /// The normalization rules do not load, which is a bug in the data file.
    #[error("normalization rules: {0}")]
    Normalize(String),
    /// A log window could not be read.
    #[error(transparent)]
    Logs(#[from] crate::logs::LogError),
}

/// The parts of a record the recorder knows before an action runs.
#[derive(Debug, Clone)]
pub struct RecordContext {
    /// The run.
    pub run_id: RunId,
    /// The check that asked for the action, or `None` for a crawl record (M4-T01).
    pub check_id: Option<substance_core::check::CheckId>,
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
            // A crawl record has no check: the empty string is the id of "no check", and it cannot
            // collide with a real id because every real id has at least three characters.
            self.check_id.map_or("", |id| id.as_str()).as_bytes(),
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
    /// Third-party requests the proxy saw in the window, when the proxy was running.
    pub egress: Option<Vec<EgressEntry>>,
    /// Why the egress channel was unavailable, when it was.
    pub egress_reason: Option<String>,
    /// Emails the mailbox caught in the window, when Mailpit was running.
    pub email: Option<Vec<EmailEntry>>,
    /// Why the email channel was unavailable, when it was.
    pub email_reason: Option<String>,
    /// The idle baseline's patterns for this screen, as `METHOD /templated/path`.
    ///
    /// Requests matching one are left out of the record: they are what the page does with no action,
    /// and a record that named them would make two runs of the same action differ whenever a poll
    /// happened to fire. Empty when no baseline was taken, which excludes nothing (M3-T05).
    pub idle_patterns: Vec<String>,
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
            egress: None,
            egress_reason: None,
            email: None,
            email_reason: None,
            idle_patterns: Vec::new(),
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

        // The idle baseline subtracted: a poll the page does on its own is not an effect of this
        // action, and keeping it would make the record depend on how many polls fit in the window.
        let net = exclude_idle(&part.net, &observed.idle_patterns);
        let net = normalized_bodies(&self.blobs, net)?;

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
            net,
            ws: part.ws.clone(),
            storage: part.storage.clone(),
            db,
            logs,
            queries,
            // A proxy or mailbox that was not running is `not_measured`, never an empty list: that
            // is the difference between "no third-party call" and "nothing was watching" (ADR-0014).
            egress: channel(
                observed.egress.as_ref(),
                observed.egress_reason.as_deref(),
                "the egress proxy was not running for this action",
            ),
            email: channel(
                observed.email.as_ref(),
                observed.email_reason.as_deref(),
                "no mailbox was running for this action",
            ),
            timing: observed.timing.clone(),
        })
    }
}

/// The requests the record keeps: the driver's window minus what the idle baseline owns.
///
/// The match is on method plus a templated path, the same rule the settle counter uses. Sharing it
/// is the point: a request the settle rule ignored must not reappear in the record, or two runs that
/// settled identically would still differ in their bytes.
#[must_use]
pub fn exclude_idle(entries: &[NetEntry], idle_patterns: &[String]) -> Vec<NetEntry> {
    if idle_patterns.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|entry| {
            let templated = substance_core::normalize::template_of(&entry.path);
            let key = format!("{} {templated}", entry.method.to_uppercase());
            !substance_core::normalize::matches_idle(idle_patterns, &key)
        })
        .cloned()
        .collect()
}

/// Fills each entry's normalized hashes by reading its raw bodies back from the store.
///
/// Read back rather than carried in the entry: the body's bytes live in the blob store once, and a
/// second copy in the record would double the run's size for a value that is derived anyway. A body
/// whose blob is missing leaves its hash `None` rather than failing the record: the raw reference is
/// still evidence, and a record that refused to be written would lose every other channel with it.
fn normalized_bodies(
    blobs: &BlobStore,
    entries: Vec<NetEntry>,
) -> Result<Vec<NetEntry>, RecordError> {
    let normalizer = substance_core::normalize::Normalizer::shared()
        .map_err(|error| RecordError::Normalize(error.to_string()))?;
    let mut out = Vec::with_capacity(entries.len());
    for mut entry in entries {
        entry.req_normalized =
            text_of(blobs, entry.req_body.as_ref()).map(|text| normalizer.hash(&text));
        entry.res_normalized =
            text_of(blobs, entry.res_body.as_ref()).map(|text| normalizer.hash(&text));
        out.push(entry);
    }
    Ok(out)
}

/// A body blob's text, when it is one and it is there.
fn text_of(blobs: &BlobStore, reference: Option<&substance_core::BlobRef>) -> Option<String> {
    let reference = reference?;
    let path = blobs.path_of(reference).ok()?;
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The egress entries for a window, from what the proxy observed.
///
/// Only the fields a check reads are carried: the proxy's own index orders the entries here and
/// does not travel with them, because two runs of the same action must produce the same bytes.
#[must_use]
pub fn egress_entries(observed: &[substance_runner::egress::Observed]) -> Vec<EgressEntry> {
    observed
        .iter()
        .map(|entry| EgressEntry {
            host: entry.host.clone(),
            method: entry.method.clone(),
            path: entry.path.clone(),
            status: entry.status,
            denied: entry.denied,
            bytes: u32::try_from(entry.request_bytes).unwrap_or(u32::MAX),
            body: entry.request_body.clone(),
        })
        .collect()
}

/// One of the channels that arrives as a list, filled from what was observed.
///
/// A channel with a reason is `not_measured` with that reason, whatever else it has: a partial list
/// says less than the reason does, and a record must not claim a complete answer it does not have.
fn channel<T>(observed: Option<&Vec<T>>, reason: Option<&str>, fallback: &str) -> Measured<Vec<T>>
where
    T: Clone,
{
    match (observed, reason) {
        (_, Some(reason)) => Measured::not_measured(reason.to_owned()),
        (Some(entries), None) => Measured::observed(entries.clone()),
        (None, None) => Measured::not_measured(fallback.to_owned()),
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
