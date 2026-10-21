//! report.json v0: the whole judgment of one run.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::finding::Finding;
use crate::model::manifest::RunManifest;
use crate::model::verdict::CheckVerdict;

/// The report schema version this build writes.
pub const REPORT_SCHEMA_VERSION: &str = "0.1.0";

/// A share in permille, never a float (ADR-0009).
pub type Ratio = u32;

/// How much of the app the run actually reached.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Coverage {
    /// Registered checks.
    pub checks_total: u32,
    /// Checks whose verdict counts as measured (ADR-0013).
    pub checks_measured: u32,
    /// Routes reached, in permille, when the crawl ran.
    pub routes: Option<Ratio>,
    /// Controls exercised, in permille, when the crawl ran.
    pub controls: Option<Ratio>,
    /// Authored server lines executed, in permille, when coverage ran.
    pub server_code: Option<Ratio>,
}

/// The archetype the history checks assigned.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Archetype {
    /// Most of today's code is from day one.
    OneShotDump,
    /// Activity in the first weeks, then nothing for 90 days.
    BurstThenAbandoned,
    /// Young, many active days, mostly edits.
    YoungAndIterating,
    /// Recently active, bus factor one, answers issues.
    SoloMaintained,
    /// Two or more people merging, most pull requests reviewed.
    TeamMaintained,
    /// Commit dates predate the repository, so the timeline can't be trusted.
    UnverifiableHistory,
}

/// The provenance panel: a profile, never a score.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Provenance {
    /// Share of today's lines by declared tool, in permille.
    pub declared_tool_permille: BTreeMap<String, Ratio>,
    /// Share of today's lines with no declared tool, in permille.
    pub undeclared_permille: Ratio,
    /// The knowledge-cutoff estimate, when one could be made.
    pub knowledge_cutoff: Option<String>,
}

/// The report of one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Report {
    /// The report schema version, for readers that must refuse a newer shape.
    pub schema_version: String,
    /// Every input the run pinned.
    pub manifest: RunManifest,
    /// One verdict per check and target, sorted by (check ID, target).
    pub verdicts: Vec<CheckVerdict>,
    /// Findings, sorted by ID.
    pub findings: Vec<Finding>,
    /// How much was reached.
    pub coverage: Coverage,
    /// The history archetype, when history ran.
    pub archetype: Option<Archetype>,
    /// The provenance panel, when provenance ran.
    pub provenance: Option<Provenance>,
    /// Human-readable notes, for example a fired time cap.
    pub notes: Vec<String>,
}
