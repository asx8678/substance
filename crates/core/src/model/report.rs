//! report.json v0: the whole judgment of one run.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Timestamp;
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
///
/// Every field here is **reported**, never scored (ADR-0012): no field feeds a gate, a grade
/// or the slop label. The panel answers "how was this built", and a reader weighs it; the
/// checks that judge the app never read it.
///
/// The plan's "What the panel shows" list is the shape: the declared share per tool and per
/// model, then what could be said about review and tests, then the knowledge-cutoff bracket.
/// A field whose input the run did not have stays absent or zero with
/// `knowledge_cutoff_missing` or `notes` saying why — an absent share means nobody looked,
/// and a share of zero would say nobody declared.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Provenance {
    /// Share of today's lines by declared tool, in permille. Sorted by tool name.
    pub declared_tool_permille: BTreeMap<String, Ratio>,
    /// Share of today's lines by declared model, where a tool named one, in permille.
    ///
    /// A tool that names no model (Codex writes a static trailer) contributes to the tool
    /// share and not to this one, so the difference between the two maps is visible.
    pub declared_model_permille: BTreeMap<String, Ratio>,
    /// Share of today's lines with no declared tool, in permille.
    pub undeclared_permille: Ratio,
    /// How many of today's lines the blame covers, the denominator of every share.
    pub lines_blamed: u64,
    /// The instruction files present at HEAD, sorted by path. Each names a tool, not a model.
    pub instruction_files: Vec<String>,
    /// Declared commits merged with no review, per tool, sorted by tool.
    pub declared_merged_unreviewed: BTreeMap<String, u64>,
    /// Declared commits that changed no test file, per tool, sorted by tool.
    pub declared_without_test_change: BTreeMap<String, u64>,
    /// Declared commits in total, per tool, sorted by tool.
    pub declared_commits: BTreeMap<String, u64>,
    /// The knowledge-cutoff bracket, when one could be made.
    pub knowledge_cutoff: Option<KnowledgeCutoff>,
    /// Why the knowledge cutoff is absent, when it is. A missing input, said out loud.
    pub knowledge_cutoff_missing: Option<String>,
    /// Every note the panel needs read beside it, sorted.
    pub notes: Vec<String>,
}

/// The knowledge-cutoff estimate: the window V03's two inputs leave open.
///
/// Both bounds are lower bounds and both are deliberately loose: a dependency adopted in
/// 2022 could not have been known before it was published, and a call to an API retired in
/// 2023 was written before it was retired. The window is what those constraints leave, not a
/// claim about a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct KnowledgeCutoff {
    /// The earliest instant the evidence permits, from adopted dependency releases.
    pub from: Timestamp,
    /// The latest instant the evidence permits, from API vintages still in use.
    pub to: Timestamp,
    /// The sentences the bracket rests on, sorted: which adoption set `from` and which
    /// vintage set `to`.
    pub evidence: Vec<String>,
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
