//! Substance core: content-derived IDs, the check registry, effect records, findings,
//! verdicts, evidence, the app model, run manifests, report types, the driver protocol,
//! the clock, canonical JSON and hashing.
//!
//! This crate depends on no other workspace crate, so every other crate can build on it.

pub mod check;
pub mod clock;
pub mod config;
pub mod deps;
pub mod hash;
pub mod ids;
pub mod model;
pub mod parts;
pub mod provenance;
pub mod registry;
pub mod scaffolds;
pub mod schema_export;
pub mod thresholds;
pub mod tools;

pub use check::{
    Axis, CheckId, CheckStatus, Confidence, Family, GateRule, Level, Milestone, Need, Severity,
};
pub use clock::{Timestamp, wall_now_utc, window_start};
pub use config::{
    ApiRequest, AppConfig, Budgets, ChecksConfig, ConfigError, DbConfig, DbEngine, EgressConfig,
    LoginConfig, LoginUi, MailConfig, NormalizeConfig, ProjectConfig, SignalsConfig, SignupConfig,
    UserConfig, UserCreate,
};
pub use hash::{CanonicalJsonError, canonical_json, canonical_json_value};
pub use ids::{BlobRef, FindingId, RecordId, RunId, SnapshotId};
pub use model::{
    AppModel, CheckVerdict, EffectRecord, Evidence, Finding, Measured, Report, RunManifest, Target,
    Verdict,
};
pub use parts::{CheckPart, parts_of};
pub use provenance::{
    Branch, DeclaredMarker, DeclaredRules, InstructionFile, MarkerWhere, ProvenanceRuleError,
    Vintage, VintageTable,
};
pub use registry::{CheckMeta, REGISTRY, meta};
pub use scaffolds::{Marker, SCAFFOLDS_TOML, ScaffoldFile, ScaffoldPack};
pub use schema_export::{SchemaFile, SchemaKind, TYPESCRIPT_DIR, TypeScriptFile};
pub use thresholds::{ArchetypeThresholds, HistoryThresholds, Thresholds, ThresholdsError};
pub use tools::{MISSING_VERSION, PIN_TBD, ToolSpec, ToolsError, ToolsFile, Version, meets_floor};
