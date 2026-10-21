//! The check threshold file this build ships (M1-T06, ADR-0016).
//!
//! `config/thresholds.toml` is embedded at compile time, for the same reason `tools.toml` is:
//! it describes substance's own judgement, not the evaluated repository's, and a repo under
//! test must not be able to raise its own thresholds. Embedding also means the manifest's
//! `thresholds_version` names the file the binary was built from, which is what makes a run
//! reproducible: two runs of one binary compare against one file.
//!
//! The version is the file's sha256, so any edit — including a comment — changes it. That is
//! deliberate: a reader diffing two reports wants to know the thresholds were byte-identical,
//! not merely numerically equal.

use substance_core::Thresholds;
use substance_core::hash::{hex, sha256};
use substance_core::thresholds::ThresholdsError;

/// The `config/thresholds.toml` this binary was built from.
pub const THRESHOLDS_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../config/thresholds.toml"
));

/// The embedded file's sha256, in lowercase hex.
///
/// This is the value the manifest carries as `thresholds_version`.
#[must_use]
pub fn version() -> String {
    hex(&sha256(THRESHOLDS_TOML.as_bytes()))
}

/// The embedded thresholds, parsed.
///
/// # Errors
///
/// Returns [`ThresholdsError`] when the embedded file is malformed, which is a build error
/// rather than a runtime one: the file is part of the binary.
pub fn thresholds() -> Result<Thresholds, ThresholdsError> {
    Thresholds::from_toml(THRESHOLDS_TOML)
}
