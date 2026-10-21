//! EffectRecord: everything one action changed, across every channel.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::check::CheckId;
use crate::ids::{BlobRef, RecordId, RunId, SnapshotId};
use crate::model::Measured;
use crate::model::seed;
use crate::model::verdict::Target;

/// One action the driver performed, and what it proved the app was in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Action {
    /// `click`, `fill`, `select`, `press`, `submit`, `goto`, `login` or `logout`.
    pub kind: String,
    /// The selector the action addressed, empty for a navigation.
    pub target: String,
    /// The route the action ran on.
    pub screen: String,
    /// Free-form input, for example `{"value": "42"}`.
    pub input: BTreeMap<String, String>,
}

/// What was injected before the action ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Injected {
    /// The failure forced on the browser or the server, when one was.
    pub failure: Option<String>,
}

/// The browser's view of what the action changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Ui {
    /// The URL before the action.
    pub url_before: String,
    /// The URL after the action settled.
    pub url_after: String,
    /// Whether the normalized DOM changed at all.
    pub dom_changed: bool,
    /// Content-derived hash of the normalized DOM before the action.
    pub dom_hash_before: String,
    /// Content-derived hash of the normalized DOM after the action.
    pub dom_hash_after: String,
    /// A download the action started, as a blob.
    pub download: Option<BlobRef>,
    /// A dialog the action opened, as text.
    pub dialog: Option<String>,
}

/// A success or error signal the UI showed, judged by role or a configured selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Signal {
    /// Whether the signal claims success or an error.
    pub kind: SignalKind,
    /// How the signal was recognized; never an English word list.
    pub source: SignalSource,
    /// The selector that matched.
    pub selector: String,
}

/// What a signal claims.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// A success signal.
    Success,
    /// An error signal.
    Error,
}

/// How a signal was recognized.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum SignalSource {
    /// `role=status`.
    RoleStatus,
    /// `role=alert`.
    RoleAlert,
    /// An `aria-live` region.
    AriaLive,
    /// A toast container.
    Toast,
    /// A selector named in `[signals]`.
    ConfigSelector,
}

/// One HTTP request the page made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NetEntry {
    /// The method.
    pub method: String,
    /// The path, with query parameters.
    pub path: String,
    /// The status code, absent when the request never completed.
    pub status: Option<u16>,
    /// The request body, as a blob.
    pub req_body: Option<BlobRef>,
    /// The lookup key of the request body's normalized form, next to the raw blob.
    ///
    /// The raw bytes are what was sent; this is what they hash to once the volatile values are
    /// replaced. Two runs whose responses differ only in a timestamp refer to different raw blobs but
    /// the same normalized hash, which is what makes them comparable without re-reading the blob
    /// (M3-T05).
    pub req_normalized: Option<String>,
    /// The response body, as a blob.
    pub res_body: Option<BlobRef>,
    /// The lookup key of the response body's normalized form, next to the raw blob.
    pub res_normalized: Option<String>,
    /// Why the request failed, when it was aborted or blocked.
    pub failed: Option<String>,
}

/// One WebSocket frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct WsFrame {
    /// The socket URL.
    pub url: String,
    /// `sent` or `received`.
    pub direction: String,
    /// The frame payload, as a blob when large.
    pub payload: Option<BlobRef>,
    /// The payload size in bytes.
    pub bytes: u32,
}

/// Browser storage after the action.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Storage {
    /// `localStorage` entries, sorted by key.
    pub local: BTreeMap<String, String>,
    /// `sessionStorage` entries, sorted by key.
    pub session: BTreeMap<String, String>,
    /// IndexedDB entries, sorted by key.
    pub indexeddb: BTreeMap<String, String>,
    /// Cookie names and values, sorted by name.
    pub cookies: BTreeMap<String, String>,
}

/// Row counts per table for one kind of write.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RowCounts {
    /// Table name to row count, sorted by table.
    pub counts: BTreeMap<String, u32>,
    /// The changed rows themselves, as a blob.
    pub rows: Option<BlobRef>,
}

/// What the DB channel saw.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DbDelta {
    /// Rows inserted.
    pub insert: RowCounts,
    /// Rows updated.
    pub update: RowCounts,
    /// Rows deleted.
    pub delete: RowCounts,
}

/// Counts the driver and the recorder collect from the browser and the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Logs {
    /// Uncaught console errors.
    pub console_errors: u32,
    /// Uncaught page errors.
    pub page_errors: u32,
    /// Server error lines between action start and settle.
    pub server_errors: Measured<u32>,
    /// The matching server lines, as a blob.
    pub server_lines: Option<BlobRef>,
}

/// One third-party request the egress proxy saw.
///
/// The path and the status are here as well as the host because the checks need them: a canary
/// smuggled in a query string is only visible in the path (B13), and a request that reached a paid
/// provider and got a 401 is a different finding from one that never left (B26).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EgressEntry {
    /// The host contacted.
    pub host: String,
    /// The method.
    pub method: String,
    /// The path and query, as the proxy saw them.
    pub path: String,
    /// The status the client received, or 0 when there was no response at all.
    pub status: u16,
    /// Whether the run's allow list refused it.
    pub denied: bool,
    /// The request size in bytes.
    pub bytes: u32,
    /// The request body, as a blob.
    pub body: Option<BlobRef>,
}

/// One email the app sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EmailEntry {
    /// The recipient.
    pub to: Vec<String>,
    /// The subject.
    pub subject: String,
    /// The body, as a blob.
    pub body: Option<BlobRef>,
}

/// How long the action took to settle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Timing {
    /// Milliseconds until the settle rule was satisfied. Left out of the normalized form.
    pub settle_ms: u32,
    /// Whether the settle cap fired instead of the settle rule.
    pub timed_out: bool,
}

/// Everything one action changed, across every channel (ADR-0014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EffectRecord {
    /// Content-derived record ID.
    pub id: RecordId,
    /// The run this record belongs to.
    pub run_id: RunId,
    /// The check that asked for the action.
    pub check_id: CheckId,
    /// What the record is about.
    pub target: Target,
    /// The snapshot the action ran on.
    pub snapshot: SnapshotId,
    /// Which repetition of the action this is; the 3-of-3 rule uses 1 to 3.
    pub repetition: u8,
    /// The seed of the random-input generator for this action.
    #[serde(with = "seed")]
    #[ts(type = "string")]
    #[schemars(with = "String")]
    pub seed: u64,
    /// Steps run on a fresh snapshot before `action`, for example login and navigation.
    pub setup: Vec<Action>,
    /// The action under test.
    pub action: Action,
    /// What was injected before the action ran.
    pub injected: Injected,
    /// The browser's view of the change.
    pub ui: Ui,
    /// Requests in order.
    pub net: Vec<NetEntry>,
    /// WebSocket frames in order.
    pub ws: Measured<Vec<WsFrame>>,
    /// Browser storage after the action.
    pub storage: Storage,
    /// Database writes.
    pub db: Measured<DbDelta>,
    /// Console, page and server errors.
    pub logs: Logs,
    /// Queries the action ran, when the engine can count them.
    pub queries: Measured<u32>,
    /// Third-party requests in order.
    pub egress: Measured<Vec<EgressEntry>>,
    /// Emails sent, in order.
    pub email: Measured<Vec<EmailEntry>>,
    /// Timing for the action.
    pub timing: Timing,
}
