//! Pipeline stages, scheduling, budgets, the 3-of-3 rule and the run directory.
//! This is the crate that wires the rest of the workspace together.
//!
//! M0-T08 lands the skeleton: intake, the manifest, the run directory and `report.json`
//! v0, in which every check that cannot run yet reports `not_measured` with a reason.

pub mod config;
pub mod intake;
pub mod manifest;
pub mod run;
pub mod stage;
pub mod verdicts;

pub use config::{CONFIG_FILE_NAME, ConfigSource, LoadError, load_config, read_config};
pub use intake::{Checkout, IntakeError, intake};
pub use manifest::{
    ManifestInputs, NO_HASH, REPETITIONS, build, default_master_seed, derive_run_id,
};
pub use run::{GateHit, RunError, RunOptions, RunOutcome, run};
pub use stage::Stage;
pub use verdicts::{SkipReason, judging_checks, pending, skip_reason};
