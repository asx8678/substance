//! The provenance panel's entry point (M1-T11).
//!
//! [`provenance`] is what M1-T12's triage calls: one function that runs V01 and V03 and returns
//! the panel for `report.json`. It exists now, at the end of M1-T11, so that the panel's shape
//! and its inputs are settled before the stage that wires it — changing a panel field after a
//! stage depends on it would mean touching the stage, the schema and the TypeScript types at
//! once.
//!
//! The work itself is in `substance_history`: this module is a thin boundary, and deliberately
//! so. The orchestrator may not read git, the forge or the static engine itself; it hands the
//! passes their inputs and collects their answers.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::deps::Ecosystem;
use substance_core::model::report::Provenance;
use substance_history::deps::known::Known;
use substance_history::provenance::{ProvenanceContext, run_provenance};
use substance_history::signals::HistoryInput;

/// What the panel needs from the rest of triage.
#[derive(Debug, Clone, Copy)]
pub struct PanelInputs<'a> {
    /// The history pass's own input: the commits, the blame and the forge data V01 reads.
    pub history: &'a HistoryInput,
    /// The dependency adoption dates P02 and P03 computed.
    pub added_at: &'a BTreeMap<(Ecosystem, String), Timestamp>,
    /// The registry facts P02 and P03 fetched.
    pub known: &'a Known,
    /// The vintage matches the static pass produced.
    pub vintage_matches: &'a [(String, u32, String)],
    /// The instant every window ends at.
    pub as_of: Timestamp,
}

/// Runs V01 and V03 and builds the report's provenance panel.
///
/// The panel is reported and never scored (ADR-0012): nothing here returns a verdict that can
/// feed a gate, and the callers that score read [`substance_core::model::Report::verdicts`]
/// and never this.
#[must_use]
pub fn provenance(inputs: &PanelInputs<'_>) -> Provenance {
    let context = ProvenanceContext {
        as_of: inputs.as_of,
        added_at: inputs.added_at,
        known: inputs.known,
        vintage_matches: &inputs.vintage_matches.to_vec(),
    };
    run_provenance(inputs.history, &context).panel
}
