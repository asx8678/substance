//! Verdicts, targets and per-check results (ADR-0013).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::check::CheckId;
use crate::ids::FindingId;

/// What one check decided about one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The check ran and found nothing.
    Pass,
    /// The check ran and reproduced the failure.
    Fail,
    /// The failure appeared in some repetitions but not all; the 3-of-3 rule failed.
    Flaky,
    /// The check ran and recorded metrics, but has no threshold yet (ADR-0013).
    Reported,
    /// The check could not run or lacked an input; always carries a reason.
    NotMeasured {
        /// Why the check did not run, for example `"needs --allow-exec"`.
        reason: String,
    },
    /// The check ran, but its evidence cannot decide.
    Unverifiable {
        /// Why the evidence cannot decide.
        reason: String,
    },
}

impl Verdict {
    /// Whether the check ran and produced a usable measurement.
    ///
    /// `pass`, `fail`, `flaky` and `reported` count; `not_measured` and
    /// `unverifiable` do not (ADR-0013).
    #[must_use]
    pub fn is_measured(&self) -> bool {
        matches!(self, Self::Pass | Self::Fail | Self::Flaky | Self::Reported)
    }

    /// Whether this verdict is a failure.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Fail)
    }
}

/// What a check or a record is about.
///
/// Serialized in serde's external form: `"repo"` or `{"file": "src/main.rs"}`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The repository as a whole.
    Repo,
    /// One file, by repo-relative path.
    File(String),
    /// One route template, for example `"/invoices/:id"`.
    Route(String),
    /// One screen reached by the crawl.
    Screen(String),
    /// One control, by role and accessible name.
    Control(String),
    /// One API endpoint, by method and path template.
    Endpoint(String),
    /// One database table.
    Table(String),
    /// One dependency.
    Dependency(String),
}

impl Target {
    /// A stable sort key for the target: kind first, then name.
    #[must_use]
    pub fn sort_key(&self) -> (String, String) {
        (self.kind().to_owned(), self.name().to_owned())
    }

    /// The target's kind as it appears in JSON.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::File(_) => "file",
            Self::Route(_) => "route",
            Self::Screen(_) => "screen",
            Self::Control(_) => "control",
            Self::Endpoint(_) => "endpoint",
            Self::Table(_) => "table",
            Self::Dependency(_) => "dependency",
        }
    }

    /// The target's name; empty for the repository.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Repo => "",
            Self::File(name)
            | Self::Route(name)
            | Self::Screen(name)
            | Self::Control(name)
            | Self::Endpoint(name)
            | Self::Table(name)
            | Self::Dependency(name) => name,
        }
    }
}

/// One check's result on one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CheckVerdict {
    /// The check.
    pub check_id: CheckId,
    /// What the check judged.
    pub target: Target,
    /// The verdict.
    pub verdict: Verdict,
    /// Findings this verdict rests on, sorted by ID.
    pub finding_ids: Vec<FindingId>,
    /// Named measurements, sorted by key.
    ///
    /// JSON Schema renders these as integers; TypeScript gets `number`, because
    /// `ts-rs` would otherwise emit the TS big integer type, which `JSON.parse`
    /// never produces.
    /// Every metric value stays within ±2^53.
    #[ts(type = "Record<string, number>")]
    pub metrics: BTreeMap<String, i64>,
    /// Per-part verdicts for checks built in stages (O01, S11, L05); empty otherwise.
    pub parts: BTreeMap<String, Verdict>,
}
