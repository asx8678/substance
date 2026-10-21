//! Substance core: content-derived IDs, the check registry, effect records, findings,
//! verdicts, evidence, the app model, run manifests, report types, the driver protocol,
//! the clock, canonical JSON and hashing.
//!
//! This crate depends on no other workspace crate, so every other crate can build on it.

pub mod check;
pub mod clock;
pub mod hash;
pub mod ids;
pub mod model;
pub mod registry;

pub use check::{
    Axis, CheckId, CheckStatus, Confidence, Family, GateRule, Level, Milestone, Need, Severity,
};
pub use clock::{Timestamp, wall_now_utc};
pub use hash::{CanonicalJsonError, canonical_json, canonical_json_value};
pub use ids::{BlobRef, FindingId, RecordId, RunId, SnapshotId};
pub use model::{
    AppModel, CheckVerdict, EffectRecord, Evidence, Finding, Measured, Report, RunManifest, Target,
    Verdict,
};
pub use registry::{CheckMeta, REGISTRY, meta};
