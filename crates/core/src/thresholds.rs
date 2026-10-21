//! The check threshold file (ADR-0016, M1-T05).
//!
//! A threshold is a number the benchmark corpora are supposed to set, not a guess. Until
//! they do, `config/thresholds.toml` carries `calibrated = false` and the starting values
//! from the action plan, and the report says so. This type is exactly that file: core parses
//! it and does no IO, so a caller reads the bytes and hands them here.
//!
//! Every value is optional, and that is the point: a check looks up its own key, and when the
//! key is absent it has no threshold at all. ADR-0013 turns that into
//! [`Verdict::Reported`](crate::model::Verdict::Reported): the metrics are measured and shown,
//! and the check neither passes nor fails. A missing key must never be read as a pass.
//!
//! M1-T05 defines only the history keys it consumes. M1-T06 extends this file with the
//! archetype cut-offs and the remaining history and slop thresholds; because every field is
//! optional and unknown members are refused, that task adds fields here rather than making
//! this one guess at them.

use serde::{Deserialize, Serialize};

/// The parsed `config/thresholds.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Thresholds {
    /// The file's format version. A reader that sees a newer version should not guess.
    pub version: u32,
    /// Whether the values came from the benchmark corpora. While false the report marks
    /// every thresholded verdict as resting on an uncalibrated number.
    pub calibrated: bool,
    /// The history thresholds, grouped under `[history]`.
    #[serde(default)]
    pub history: HistoryThresholds,
}

impl Thresholds {
    /// Parses the file. Core does no IO: the caller reads the bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ThresholdsError`] when the text is not TOML, or when it carries a member
    /// this build does not know — a misspelt key would otherwise silently remove a
    /// threshold and turn a possible failure into a reported metric.
    pub fn from_toml(text: &str) -> Result<Self, ThresholdsError> {
        toml::from_str(text).map_err(|error| ThresholdsError {
            detail: error.to_string(),
        })
    }

    /// The history thresholds.
    #[must_use]
    pub fn history(&self) -> &HistoryThresholds {
        &self.history
    }
}

/// The history thresholds: one key per check that has one (M1-T05, M1-T06).
///
/// A field is `None` when the file does not set it, which is what makes a check report
/// instead of pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct HistoryThresholds {
    /// H04: how many authored added lines make a commit "giant".
    pub giant_lines: Option<u32>,
    /// H04: the share of blamed HEAD lines from giant commits, in permille, at which the
    /// check fails.
    pub giant_share_permille: Option<u32>,
    /// H05: the 90th percentile of authored lines per minute, at which the check fails.
    pub machine_speed_p90_lines_per_minute: Option<u32>,
    /// H07: how many fix chains are too many.
    pub fix_chains: Option<u32>,
    /// H09: the share of short-lived lines, in permille, at which the check fails.
    pub short_lived_permille: Option<u32>,
    /// H10: the share of generic commit subjects, in permille, at which the check fails.
    pub generic_message_permille: Option<u32>,
}

impl HistoryThresholds {
    /// Whether the file sets any history key at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Why the threshold file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("thresholds.toml: {detail}")]
pub struct ThresholdsError {
    /// What was wrong, as the parser said it.
    pub detail: String,
}
