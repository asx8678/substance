//! Pipeline stages, scheduling, budgets, the 3-of-3 rule and the run directory.
//! This is the crate that wires the rest of the workspace together.
//!
//! M0-T08 lands the skeleton: intake, the manifest, the run directory and `report.json`
//! v0, in which every check that cannot run yet reports `not_measured` with a reason.

pub mod config;
pub mod handlers;
pub mod intake;
pub mod manifest;
pub mod operability;
pub mod provenance;
pub mod replay;
pub mod run;
pub mod runtime;
pub mod stage;
pub mod thresholds;
pub mod tools;
pub mod triage;
pub mod verdicts;

pub use config::{CONFIG_FILE_NAME, ConfigSource, LoadError, load_config, read_config};
pub use handlers::{
    Family, Handler, TriageFn, TriageJudgement, family_of, handler_for, has_handler,
    implemented_without_handler, triage_check_count,
};
pub use intake::{Checkout, IntakeError, intake, permit_for};
pub use manifest::{
    ManifestInputs, NO_HASH, REPETITIONS, build, default_master_seed, derive_run_id,
};
pub use provenance::{PanelInputs, provenance};
pub use run::{GateHit, RunError, RunOptions, RunOutcome, run};
pub use runtime::{RuntimeInput, RuntimeOutcome, SEEDED, canaries_of};
pub use stage::Stage;
pub use thresholds::THRESHOLDS_TOML;
pub use tools::{TOOLS_TOML, ToolProbe, manifest_tools, probe_versions, tool_reason};
pub use triage::{BUDGET_EXHAUSTED, TIME_CAP_NOTE, TriageInput, TriageOutcome, run_triage};
pub use verdicts::{SkipReason, judging_checks, pending, skip_reason};
