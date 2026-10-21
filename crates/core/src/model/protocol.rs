//! The JSON-lines driver protocol (M3-T01, M3-T02).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::clock::Timestamp;
use crate::ids::BlobRef;
use crate::model::Measured;
use crate::model::app_model::Control;
use crate::model::effect::{DbDelta, EgressEntry, EmailEntry, Logs, NetEntry, Storage, WsFrame};

/// One command Rust sends the driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverCommand {
    /// Open a new isolated browser context.
    OpenContext(OpenContext),
    /// Navigate, then settle.
    Goto {
        /// The URL to load.
        url: String,
    },
    /// Walk the accessibility tree for roles, names and selector candidates.
    ListControls {},
    /// Run one action, then settle.
    Act(ActCommand),
    /// Fail or delay matching requests inside the browser.
    Route(RouteCommand),
    /// Set or advance the browser clock.
    Clock(ClockCommand),
    /// Hash the normalized DOM and capture the visible-text map.
    SnapshotDom {},
    /// Run axe-core on the page.
    Axe {},
    /// Measure element boxes for overflow, overlap and tap-target checks.
    Layout {},
    /// Start or stop a Playwright trace.
    Trace(TraceCommand),
    /// Log in through the UI or the API.
    Login(LoginCommand),
    /// Dispose of the context.
    CloseContext {},
}

/// Everything a new context needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct OpenContext {
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
    /// The locale, for example `"en-US"`.
    pub locale: String,
    /// The timezone, for example `"UTC"`.
    pub tz: String,
    /// A saved storage state to restore, when a session already exists.
    pub storage_state: Option<String>,
    /// The normalization rules, as regex and replacement pairs.
    pub normalize: Vec<NormalizePattern>,
    /// Where bodies are written, as `sha256/<2>/<64>`.
    pub blob_dir: String,
    /// Extra success selectors from `[signals]`.
    pub success_selectors: Vec<String>,
    /// Extra error selectors from `[signals]`.
    pub error_selectors: Vec<String>,
    /// Request patterns that belong to the idle baseline.
    pub idle_patterns: Vec<String>,
    /// Toast containers from `rules/signals.toml`, with the attribute that names their kind.
    pub toast_containers: Vec<ToastContainer>,
    /// The settle quiet period in milliseconds.
    pub settle_quiet_ms: u32,
    /// The settle cap in milliseconds.
    pub settle_timeout_ms: u32,
}

/// One normalization rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NormalizePattern {
    /// The pattern to replace.
    pub regex: String,
    /// What to replace it with.
    pub replace: String,
}

/// One action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ActCommand {
    /// `click`, `fill`, `select`, `press` or `submit`.
    pub kind: String,
    /// The selector to address.
    pub target: String,
    /// The value for `fill`, `select` and `press`.
    pub value: Option<String>,
}

/// Run a burst of clicks, for the double-submit check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ClickBurst {
    /// The submit control.
    pub selector: String,
    /// How many clicks.
    pub count: u32,
    /// Milliseconds between clicks.
    pub interval_ms: u32,
}

/// Fail or delay matching requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RouteCommand {
    /// The URL pattern.
    pub pattern: String,
    /// `fail_500`, `abort` or `delay`.
    pub mode: String,
    /// The delay in milliseconds, for `delay`.
    pub ms: Option<u32>,
}

/// Set or advance the browser clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ClockCommand {
    /// `install` or `fast_forward`.
    pub op: String,
    /// The instant or the amount to advance by.
    pub value: String,
}

/// Start or stop a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TraceCommand {
    /// `start` or `stop`.
    pub op: String,
    /// Where the trace is written.
    pub path: String,
}

/// Log in through the UI or the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoginCommand {
    /// Fill and submit the login form.
    Ui {
        /// The login route.
        path: String,
        /// The identifier field's selector.
        identifier: String,
        /// The password field's selector.
        password: String,
        /// The submit control's selector.
        submit: String,
        /// The identifier value.
        email: String,
        /// The password value; never echoed in events or logs.
        password_value: String,
    },
    /// Send the login request through the page's request context.
    Api {
        /// The HTTP method.
        method: String,
        /// The path.
        path: String,
        /// The body, with `{email}` and `{password}` placeholders.
        body: String,
        /// The identifier value.
        email: String,
        /// The password value; never echoed in events or logs.
        password_value: String,
    },
}

/// One event the driver writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverEvent {
    /// The driver is ready for a command.
    Ready {},
    /// The browser channels of one action.
    ///
    /// Boxed, because a record part is much larger than the other events.
    RecordPart(Box<RecordPart>),
    /// The driver reports a problem with the command.
    Error(DriverError),
    /// What a command found, for the commands that return a result rather than a record part.
    ///
    /// A command that reports through `record_part` (an `act`, a `goto`) leaves this out. The
    /// two are separate events because a result is not an effect: `list_controls` describes what
    /// the page offers, and folding that into a record part would put a description of the page
    /// into a record of what an action changed.
    Result(DriverResult),
    /// The command finished.
    Done {},
}

/// What a command returns besides a record part.
///
/// Every variant carries the whole result: the driver never streams a partial answer, so a
/// reader either gets a complete observation or an `error` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverResult {
    /// The interactive controls `list_controls` found, on the page it was asked about.
    Controls {
        /// The path the walk ran on, without query or fragment.
        screen: String,
        /// The controls, in document order.
        controls: Vec<Control>,
    },
    /// The boxes `layout` measured.
    Layout {
        /// The path the measurement ran on, without query or fragment.
        screen: String,
        /// One box per element that was measured.
        boxes: Vec<LayoutBox>,
    },
    /// The normalized DOM hash and the visible-text map `snapshot_dom` produced.
    SnapshotDom {
        /// The path the snapshot was taken on, without query or fragment.
        screen: String,
        /// The content hash of the normalized DOM.
        dom_hash: String,
        /// Visible text by element, for unused-field detection.
        visible_text: BTreeMap<String, String>,
    },
    /// The axe-core violations `axe` found.
    Axe {
        /// The path axe ran on, without query or fragment.
        screen: String,
        /// One entry per violated rule, in the order axe reported them.
        violations: Vec<AxeViolation>,
    },
    /// What a login changed, for the session the recorder will reuse.
    Login {
        /// Browser storage after the login, in the same shape a record part reports.
        storage: crate::model::effect::Storage,
        /// The keys that appeared or changed, sorted: `localStorage:token`, `cookie:sid`.
        ///
        /// Values are not included. This is the answer to "did logging in do anything", and the
        /// answer does not need the secret that was just used to obtain it.
        changed: Vec<String>,
        /// Where the login request was sent, when the mode was `api`.
        request_path: Option<String>,
        /// The login request body, as a blob with the password replaced.
        request_body: Option<BlobRef>,
    },
}

/// A driver-side failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DriverError {
    /// The command that failed.
    pub cmd: String,
    /// The message, with session secrets replaced.
    pub message: String,
    /// Whether the driver kept the context usable.
    pub recoverable: bool,
}

/// The browser channels of one action; the recorder adds the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RecordPart {
    /// The browser's view of the change.
    pub ui: crate::model::effect::Ui,
    /// Requests in order.
    pub net: Vec<NetEntry>,
    /// WebSocket frames, or a reason they were not captured.
    pub ws: Measured<Vec<WsFrame>>,
    /// Browser storage after the action.
    pub storage: Storage,
    /// Console and page errors; the server part is filled in by the recorder.
    pub logs: Logs,
    /// When the action settled, on the frozen clock.
    pub settled_at: Timestamp,
    /// Channels the recorder captures itself, absent here.
    pub db: Option<DbDelta>,
    /// Channels the recorder captures itself, absent here.
    pub egress: Option<Vec<EgressEntry>>,
    /// Channels the recorder captures itself, absent here.
    pub email: Option<Vec<EmailEntry>>,
    /// Channels the recorder captures itself, absent here.
    pub queries: Option<u32>,
    /// Visible text by element, for unused-field detection.
    pub visible_text: BTreeMap<String, String>,
    /// Axe-core violations, when `axe` ran.
    pub axe_violations: Vec<AxeViolation>,
    /// Success and error signals the page showed, in discovery order.
    pub signals: Vec<SignalObservation>,
}

/// One axe-core violation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AxeViolation {
    /// The rule ID.
    pub id: String,
    /// The impact axe reported.
    pub impact: Option<String>,
    /// How many nodes matched.
    pub nodes: u32,
}

/// A toast container, and how to tell a success toast from an error one.
///
/// The container selectors and the attribute values are data from `rules/signals.toml`, not
/// code: a new toast library is a rule-pack change, and the driver never grows a list of
/// English words that would silently stop matching when a library renames `success`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ToastContainer {
    /// The container selector.
    pub selector: String,
    /// The attribute on the toast element whose value names the kind, for example `data-type`.
    pub kind_attribute: String,
    /// Values of that attribute that mean success.
    pub success_values: Vec<String>,
    /// Values of that attribute that mean error.
    pub error_values: Vec<String>,
}

/// One success or error signal the page showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SignalObservation {
    /// `success` or `error`.
    pub kind: String,
    /// What produced it: `role=alert`, `aria-live=assertive`, `toast:sonner`, `config`.
    pub source: String,
    /// The signal's visible text, normalized.
    pub text: String,
}

/// One element box, for layout checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LayoutBox {
    /// The selector that found the element.
    pub selector: String,
    /// Left edge in CSS pixels.
    pub x: u32,
    /// Top edge in CSS pixels.
    pub y: u32,
    /// Width in CSS pixels.
    pub width: u32,
    /// Height in CSS pixels.
    pub height: u32,
    /// Whether the element is clickable.
    pub clickable: bool,
}
