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
/// instead of pass. The archetype cut-offs are grouped the same way: an archetype rule with
/// a missing number cannot match, and H22 says so rather than guessing.
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
    /// H12: below this permille of reviewed merges the check fails, when there are at
    /// least two maintainers.
    pub reviewed_prs_permille: Option<u32>,
    /// H12: how many maintainers it takes before the review threshold applies.
    pub reviewed_prs_min_maintainers: Option<u32>,
    /// H16: how many active weeks make a "production-ready" claim plausible rather than a
    /// claim over an empty history.
    pub claims_min_active_weeks: Option<u32>,
    /// H20: the median authored lines per person-day at which the check fails.
    pub person_day_median_lines: Option<u32>,
    /// H20: how many active days it takes before that median is worth reading.
    pub person_day_min_active_days: Option<u32>,
    /// H22: the archetype cut-offs.
    #[serde(default)]
    pub archetypes: ArchetypeThresholds,
}

impl HistoryThresholds {
    /// Whether the file sets any history key at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The numbers the archetype rules compare against (M1-T06, ADR-0016).
///
/// Every rule is a conjunction of comparisons, so a missing number is a rule that cannot
/// match — never a rule that matches something else. [`crate::thresholds::ArchetypeThresholds::is_complete`]
/// lets H22 say which number was missing instead of returning a silent `unclassified`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct ArchetypeThresholds {
    /// `one_shot`: the big-bang permille at or above which most code is from day one.
    pub one_shot_big_bang_permille: Option<u32>,
    /// `one_shot`: the most active weeks a one-shot dump may have in a year.
    pub one_shot_max_active_weeks: Option<u32>,
    /// `burst_abandoned`: how old a repository must be, in days.
    pub burst_min_age_days: Option<u32>,
    /// `young_iterating`: the most days old a young project may be.
    pub young_max_age_days: Option<u32>,
    /// `young_iterating`: the active days it needs.
    pub young_min_active_days: Option<u32>,
    /// `young_iterating`: the share of commits that change existing lines, in permille.
    pub young_min_edit_share_permille: Option<u32>,
    /// `young_iterating`: the share of commits whose tests change with the code, in
    /// permille.
    pub young_min_test_lines_change_permille: Option<u32>,
    /// `solo`: the active weeks in the last 90 days it needs.
    pub solo_min_active_weeks: Option<u32>,
    /// `solo`: how long a first response may take, in hours, for the project to count as
    /// answering its issues.
    pub solo_max_response_hours: Option<u32>,
    /// `team`: how many maintainers it takes.
    pub team_min_maintainers: Option<u32>,
    /// `team`: the reviewed-PR permille it needs.
    pub team_min_reviewed_permille: Option<u32>,
}

impl ArchetypeThresholds {
    /// Every key, so an incomplete file can be described rather than merely rejected.
    ///
    /// Returned sorted by key, which is the order a note lists them in.
    #[must_use]
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing: Vec<&'static str> = Vec::new();
        let entries: [(&'static str, Option<u32>); 11] = [
            (
                "one_shot_big_bang_permille",
                self.one_shot_big_bang_permille,
            ),
            ("one_shot_max_active_weeks", self.one_shot_max_active_weeks),
            ("burst_min_age_days", self.burst_min_age_days),
            ("young_max_age_days", self.young_max_age_days),
            ("young_min_active_days", self.young_min_active_days),
            (
                "young_min_edit_share_permille",
                self.young_min_edit_share_permille,
            ),
            (
                "young_min_test_lines_change_permille",
                self.young_min_test_lines_change_permille,
            ),
            ("solo_min_active_weeks", self.solo_min_active_weeks),
            ("solo_max_response_hours", self.solo_max_response_hours),
            ("team_min_maintainers", self.team_min_maintainers),
            (
                "team_min_reviewed_permille",
                self.team_min_reviewed_permille,
            ),
        ];
        for (key, value) in entries {
            if value.is_none() {
                missing.push(key);
            }
        }
        missing.sort_unstable();
        missing
    }

    /// Whether every archetype key is set.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing().is_empty()
    }
}

/// Why the threshold file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("thresholds.toml: {detail}")]
pub struct ThresholdsError {
    /// What was wrong, as the parser said it.
    pub detail: String,
}
