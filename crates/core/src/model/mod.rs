//! The core data model: records, findings, verdicts, the app model, the manifest and
//! the report. Every type carries JSON Schema and TypeScript derives, and there are no
//! floats anywhere (ADR-0009).

pub mod app_model;
pub mod db;
pub mod effect;
pub mod finding;
pub mod manifest;
pub mod measured;
pub mod normalize;
pub mod protocol;
pub mod report;
pub mod seed;
pub mod seeding;
pub mod stack;
pub mod verdict;

pub use app_model::{
    AppModel, Control, ControlPurpose, Entity, Field, Form, Labels, RequestTrigger, Screen,
    TestUser,
};
pub use db::{
    Capabilities, DbHandle, DecodingPlugin, RedactedUrl, TAG_CHARS, run_tag, snapshot_database,
};
pub use effect::{
    Action, DbDelta, EffectRecord, EgressEntry, EmailEntry, Injected, Logs, NetEntry, RowCounts,
    Signal, SignalKind, SignalSource, Storage, Timing, Ui, WsFrame,
};
pub use finding::{Cause, Evidence, FileRange, Finding, ForgeObject, RegistryObject, ToolOutput};
pub use manifest::{AccessMode, Host, RepoRef, RunManifest, ServerClock, ToolVersion};
pub use measured::Measured;
pub use normalize::normalized_bytes;
pub use protocol::{
    ActCommand, AxeViolation, ClickBurst, ClockCommand, DriverCommand, DriverError, DriverEvent,
    DriverResult, LayoutBox, LoginCommand, NormalizePattern, OpenContext, RecordPart, RouteCommand,
    SignalObservation, ToastContainer, TraceCommand,
};
pub use report::{
    Archetype, Coverage, KnowledgeCutoff, Provenance, REPORT_SCHEMA_VERSION, Ratio, Report,
};
pub use seeding::{Canaries, Canary, CanaryKind, canary_value, seed_of, seeds_for};
pub use stack::{BootOutcome, BootRung, CommandSpec, Friction, RouteEntry, RouteTable};
pub use verdict::{CheckVerdict, Target, Verdict};
