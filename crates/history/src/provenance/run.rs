//! Running V01 and V03 over one commit (M1-T11).
//!
//! The module the orchestrator calls, matching [`crate::deps::run`] and [`crate::signals`]: the
//! IO happens here, once, and the two judges are pure functions over what it produced.
//!
//! Two properties are structural rather than asserted:
//!
//! - **The verdicts cannot be pass or fail.** V01's and V03's verdicts are built by their own
//!   modules, which never call the threshold helper; this module only copies them and never
//!   rewrites a verdict. ADR-0012 says provenance is reported, and the way to keep that true is
//!   to have no path from a threshold to a `V` verdict at all.
//! - **The panel is built from the same numbers the verdicts carry.** `lines_*` metrics and
//!   `declared_tool_permille` are derived from one [`DeclaredShares`], so a reader comparing
//!   the panel with the verdict list cannot find two answers.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::check::CheckId;
use substance_core::deps::Ecosystem;
use substance_core::model::report::{KnowledgeCutoff, Provenance};
use substance_core::model::{CheckVerdict, Finding};
use substance_core::provenance::VintageTable;

use crate::deps::known::Known;
use crate::provenance::input::{
    InstructionFileHit, gather_declared, instruction_paths, knowledge_from_adoptions,
    load_vintages, vintages_from_matches,
};
use crate::provenance::v01::{self, DeclaredShares};
use crate::provenance::v03::{self, Window};
use crate::signals::HistoryInput;

/// What V03's rule scan needs: the vintage matches, as `(path, line, vintage id)`.
///
/// A plain tuple rather than the static engine's `RuleMatch`, because this crate must not
/// depend on `substance-statics`: the static engine already depends on this one. The caller
/// flattens its matches into this shape, which is also what keeps the judge testable against a
/// hand-written list.
pub type VintageMatches = Vec<(String, u32, String)>;

/// Everything one provenance pass produced.
#[derive(Debug)]
pub struct ProvenanceOutcome {
    /// The two verdicts, in ID order: V01 then V03.
    pub verdicts: Vec<CheckVerdict>,
    /// Notes per check, in the same order.
    pub notes: Vec<(CheckId, Vec<String>)>,
    /// The panel for `report.json`.
    pub panel: Provenance,
}

/// What the caller must supply.
#[derive(Debug, Clone, Copy)]
pub struct ProvenanceContext<'a> {
    /// The instant every window ends at.
    pub as_of: Timestamp,
    /// The dependency adoption dates P02 and P03 already computed.
    pub added_at: &'a BTreeMap<(Ecosystem, String), Timestamp>,
    /// The registry facts P02 and P03 already fetched.
    pub known: &'a Known,
    /// The vintage matches the static rule scan produced.
    pub vintage_matches: &'a VintageMatches,
}

/// Runs V01 and V03 over `input`.
#[must_use]
pub fn run_provenance(input: &HistoryInput, context: &ProvenanceContext<'_>) -> ProvenanceOutcome {
    let declared_input = gather_declared(&input.authored);
    let v03_input = load_vintages();

    let (v01_verdict, shares, mut v01_notes) = match declared_input.rules_error.clone() {
        Some(error) => {
            let failed = CheckVerdict {
                check_id: CheckId::V01,
                target: substance_core::model::Target::Repo,
                verdict: substance_core::model::Verdict::NotMeasured {
                    reason: format!("rules/provenance/declared.toml: {error}"),
                },
                finding_ids: Vec::new(),
                metrics: BTreeMap::new(),
                parts: BTreeMap::new(),
            };
            (failed, DeclaredShares::default(), Vec::new())
        }
        None => {
            let declared = v01::declared(input, &declared_input.rules);
            (declared.verdict, declared.shares, declared.notes)
        }
    };
    v01_notes.extend(shares_notes(&shares, &declared_input.instruction_files));

    let mut knowledge = knowledge_from_adoptions(context.added_at, context.known);
    let vintage_matches = vintages_from_matches(context.vintage_matches, &v03_input.vintages);
    knowledge.vintages = vintage_matches;
    if let Some(error) = &v03_input.table_error {
        knowledge
            .missing
            .push(format!("rules/provenance/api-vintage.toml: {error}"));
    }
    let window: Window = v03::knowledge_window(&knowledge, &v03_input.vintages, context.as_of);

    let panel = panel_of(
        &shares,
        &declared_input.instruction_files,
        &window,
        &v03_input.vintages,
        context.as_of,
    );

    ProvenanceOutcome {
        verdicts: vec![v01_verdict, window.verdict],
        notes: vec![
            (CheckId::V01, sorted(v01_notes)),
            (CheckId::V03, sorted(window.notes)),
        ],
        panel,
    }
}

/// Builds the panel the plan's "What the panel shows" list describes.
#[must_use]
fn panel_of(
    shares: &v01::DeclaredShares,
    instruction_files: &[InstructionFileHit],
    window: &Window,
    vintages: &VintageTable,
    as_of: Timestamp,
) -> Provenance {
    let (tools, models, undeclared) = v01::permille(shares);
    let cutoff = match (window.from, window.to) {
        (Some(from), Some(to)) => Some(KnowledgeCutoff {
            from,
            to,
            evidence: sorted(window.evidence.clone()),
        }),
        // One bound is not a window, and reporting it as `Some` would put a date where the
        // panel promised an interval. The reason goes to `knowledge_cutoff_missing` instead.
        (from, to) => {
            let mut missing = Vec::new();
            if from.is_none() {
                missing.push("the lower bound (dependency releases) was not readable".to_owned());
            }
            if to.is_none() {
                missing.push("the upper bound (outdated API vintages) was not readable".to_owned());
            }
            let _ = (vintages, as_of);
            let _ = missing;
            None
        }
    };
    let missing_reason = cutoff.is_none().then(|| {
        let mut parts: Vec<String> = Vec::new();
        if window.from.is_none() {
            parts.push("no dependency release date".to_owned());
        }
        if window.to.is_none() {
            parts.push("no dated API vintage still in use".to_owned());
        }
        if parts.is_empty() {
            parts.push("the evidence contradicted itself".to_owned());
        }
        parts.join("; ")
    });

    let mut notes: Vec<String> = window.notes.clone();
    notes.extend(shares_notes(shares, instruction_files));
    if shares.total_lines > 0 {
        notes.push(
            "shares are of today's blamed lines, so a declaration counts only where its code \
             still exists"
                .to_owned(),
        );
    }

    Provenance {
        declared_tool_permille: tools,
        declared_model_permille: models,
        undeclared_permille: undeclared,
        lines_blamed: shares.total_lines,
        instruction_files: instruction_paths(instruction_files),
        declared_merged_unreviewed: shares.merged_unreviewed.clone(),
        declared_without_test_change: shares.without_test_change.clone(),
        declared_commits: shares.commits.clone(),
        knowledge_cutoff: cutoff,
        knowledge_cutoff_missing: missing_reason,
        notes: sorted(notes),
    }
}

/// The sentences the panel shows beside the shares.
fn shares_notes(
    shares: &v01::DeclaredShares,
    instruction_files: &[InstructionFileHit],
) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    if shares.total_lines > 0 && shares.tool_lines.is_empty() {
        notes.push(
            "no commit among today's lines declares a tool; that is not evidence that a person \
             wrote them, because every declaration can be stripped"
                .to_owned(),
        );
    }
    if instruction_files.is_empty() {
        notes.push(
            "no instruction file the plan lists is tracked, which names no tool by itself"
                .to_owned(),
        );
    }
    notes
}

/// A sorted, deduplicated copy.
fn sorted(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines.dedup();
    lines
}

/// Every finding the provenance family produces: none.
///
/// V01 and V03 report shares and a window; a finding is a claim about a defect, and neither
/// check makes one. The function exists so the orchestrator can treat every family the same
/// way without a special case, and it says why it is empty rather than leaving a reader to
/// wonder.
#[must_use]
pub fn findings() -> Vec<Finding> {
    Vec::new()
}
