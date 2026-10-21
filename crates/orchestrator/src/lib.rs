//! Pipeline stages, scheduling, budgets, the 3-of-3 rule and the run directory.
//! This is the crate that wires the rest of the workspace together.

pub mod config;

pub use config::{CONFIG_FILE_NAME, ConfigSource, LoadError, load_config, read_config};
