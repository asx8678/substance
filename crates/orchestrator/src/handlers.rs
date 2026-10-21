//! Which stage runs which check (M1-T12).
//!
//! One table, written as a single exhaustive `match` over [`CheckId`], because the table's job is
//! to turn a missing entry into a **compile error** rather than a check that silently never runs.
//! Every variant of the enum appears exactly once below, and every arm ends in `Some` or `None`.
//! Adding a variant breaks this file until someone says which stage answers for it.
//!
//! The orchestrator dispatches **only** through [`handler_for`]. Nothing else in the pipeline may
//! decide that a check runs. If `handler_for` returns `None`, the check reports the milestone that
//! will deliver it, and `checks list --json` shows `"handler": false`, which is what `C-IMPL`
//! reads to catch a check marked `implemented` with nothing to run it.
//!
//! The two questions this module answers are deliberately separate:
//!
//! - [`handler_for`] is whether a stage in *this build* runs the check, which is the answer
//!   `checks list` and the scheduler need.
//! - [`family_of`] is *which family pass* computes it, which is the answer the report and the
//!   coverage line need. It is a separate match because the registry's `axes` field says which
//!   axis a check feeds, not which pass computes it, and a check may move between passes without
//!   its catalog entry changing.

use substance_core::REGISTRY;
use substance_core::check::{CheckId, CheckStatus};

/// The stage that runs one check.
///
/// Deliberately not `PartialEq`: it holds a function pointer, and comparing two function
/// pointers is not a meaningful question in Rust, because the compiler does not guarantee one
/// address per function. A caller comparing stages compares [`Family`], whose variants are plain.
#[derive(Debug, Clone, Copy)]
pub enum Handler {
    /// A triage check: judged from the repository, the forge and the registries, without
    /// executing any of the repository's code.
    Triage(TriageFn),
}

/// One check's judging function.
///
/// A plain function pointer, not a closure: a function pointer is `Copy`, comparable in a match
/// and usable in a `const`-shaped table, and nothing here needs to capture state. The argument is
/// the check itself, because one family pass judges several checks and needs to know which verdict
/// it is being asked for.
pub type TriageFn = fn(CheckId) -> TriageJudgement;

/// What a triage handler answers for one check.
///
/// The verdict itself is built by the family pass, which is the only code holding the gathered
/// input. The handler's answer is the *claim* that this stage runs the check. Keeping the two
/// apart is what lets `handler_for` be a pure table with no IO: a caller reads it to schedule work
/// and to answer `checks list`, and reads the family's output to write a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageJudgement {
    /// This stage runs the check; the verdict comes from the family pass named here.
    Handled(Family),
    /// This stage does not run it, with the reason a `not_measured` verdict carries.
    NotRun(&'static str),
}

/// The family pass that judges a check, in the order triage runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    /// The commit-history signals H01–H22, T05 and T06, in `substance-history`.
    History,
    /// The tree checks S01–S12, D04, O01, O10, T04 and L05, in `substance-statics`.
    Statics,
    /// The dependency checks P01–P07, in `substance-history`.
    Deps,
    /// The provenance checks V01 and V03, in `substance-history`.
    Provenance,
}

impl Family {
    /// The family as a lowercase word, for logs and notes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Statics => "statics",
            Self::Deps => "deps",
            Self::Provenance => "provenance",
        }
    }

    /// Every family, in the order triage runs them.
    pub const ALL: &'static [Self] = &[Self::History, Self::Statics, Self::Deps, Self::Provenance];

    /// The checks this family judges, in ID order.
    #[must_use]
    pub fn checks(self) -> Vec<CheckId> {
        REGISTRY
            .iter()
            .map(|meta| meta.id)
            .filter(|id| family_of(*id) == Some(self))
            .collect()
    }
}

/// The triage handler: a check is judged by its family's pass.
fn triage(check: CheckId) -> TriageJudgement {
    match family_of(check) {
        Some(family) => TriageJudgement::Handled(family),
        // `handler_for` only returns `Triage` for a check that has a family, so this arm is
        // unreachable through the table. Saying so beats a panic, which the workspace forbids.
        None => TriageJudgement::NotRun("no triage family judges this check"),
    }
}

/// The stage that runs `id`, or `None` when no stage in this build does.
///
/// One arm per check, in ID order, so a reader can find any check by its ID and a new ID breaks
/// the match.
#[must_use]
pub fn handler_for(id: CheckId) -> Option<Handler> {
    match id {
        // ── Implemented: the triage stage judges these (M1). ─────────────────────────────────
        CheckId::D04
        | CheckId::O01
        | CheckId::O10
        | CheckId::L05
        | CheckId::S01
        | CheckId::S02
        | CheckId::S03
        | CheckId::S04
        | CheckId::S05
        | CheckId::S06
        | CheckId::S07
        | CheckId::S08
        | CheckId::S09
        | CheckId::S10
        | CheckId::S11
        | CheckId::S12
        | CheckId::T04
        | CheckId::T05
        | CheckId::T06
        | CheckId::P01
        | CheckId::P02
        | CheckId::P03
        | CheckId::P04
        | CheckId::P05
        | CheckId::P06
        | CheckId::P07
        | CheckId::H01
        | CheckId::H02
        | CheckId::H03
        | CheckId::H04
        | CheckId::H05
        | CheckId::H06
        | CheckId::H07
        | CheckId::H08
        | CheckId::H09
        | CheckId::H10
        | CheckId::H11
        | CheckId::H12
        | CheckId::H13
        | CheckId::H14
        | CheckId::H15
        | CheckId::H16
        | CheckId::H17
        | CheckId::H18
        | CheckId::H19
        | CheckId::H20
        | CheckId::H21
        | CheckId::H22
        | CheckId::V01
        | CheckId::V03 => Some(Handler::Triage(triage)),

        // ── Reserved: the eight IDs the catalog holds back; they never run. ────────────────
        CheckId::V02
        | CheckId::A01
        | CheckId::A02
        | CheckId::A03
        | CheckId::A04
        | CheckId::A05
        | CheckId::G01
        | CheckId::G02 => None,

        // ── Registered for M2–M14: their stage has not landed. ─────────────────────────────
        CheckId::B01
        | CheckId::B02
        | CheckId::B03
        | CheckId::B04
        | CheckId::B05
        | CheckId::B06
        | CheckId::B07
        | CheckId::B08
        | CheckId::B09
        | CheckId::B10
        | CheckId::B11
        | CheckId::B12
        | CheckId::B13
        | CheckId::B14
        | CheckId::B15
        | CheckId::B16
        | CheckId::B17
        | CheckId::B18
        | CheckId::B19
        | CheckId::B20
        | CheckId::B21
        | CheckId::B22
        | CheckId::B23
        | CheckId::B24
        | CheckId::B25
        | CheckId::B26
        | CheckId::B27
        | CheckId::B28
        | CheckId::B29
        | CheckId::B30
        | CheckId::B31
        | CheckId::B32
        | CheckId::B33
        | CheckId::I01
        | CheckId::I02
        | CheckId::I03
        | CheckId::I04
        | CheckId::D01
        | CheckId::D02
        | CheckId::D03
        | CheckId::O02
        | CheckId::O03
        | CheckId::O04
        | CheckId::O05
        | CheckId::O06
        | CheckId::O07
        | CheckId::O08
        | CheckId::O09
        | CheckId::O11
        | CheckId::L01
        | CheckId::L02
        | CheckId::L03
        | CheckId::L04
        | CheckId::T01
        | CheckId::T02
        | CheckId::T03 => None,
    }
}

/// The family that judges `id`, when one does.
#[must_use]
pub fn family_of(id: CheckId) -> Option<Family> {
    match id {
        CheckId::H01
        | CheckId::H02
        | CheckId::H03
        | CheckId::H04
        | CheckId::H05
        | CheckId::H06
        | CheckId::H07
        | CheckId::H08
        | CheckId::H09
        | CheckId::H10
        | CheckId::H11
        | CheckId::H12
        | CheckId::H13
        | CheckId::H14
        | CheckId::H15
        | CheckId::H16
        | CheckId::H17
        | CheckId::H18
        | CheckId::H19
        | CheckId::H20
        | CheckId::H21
        | CheckId::H22
        | CheckId::T05
        | CheckId::T06 => Some(Family::History),
        CheckId::D04
        | CheckId::L05
        | CheckId::O01
        | CheckId::O10
        | CheckId::S01
        | CheckId::S02
        | CheckId::S03
        | CheckId::S04
        | CheckId::S05
        | CheckId::S06
        | CheckId::S07
        | CheckId::S08
        | CheckId::S09
        | CheckId::S10
        | CheckId::S11
        | CheckId::S12
        | CheckId::T04 => Some(Family::Statics),
        CheckId::P01
        | CheckId::P02
        | CheckId::P03
        | CheckId::P04
        | CheckId::P05
        | CheckId::P06
        | CheckId::P07 => Some(Family::Deps),
        CheckId::V01 | CheckId::V03 => Some(Family::Provenance),
        _ => None,
    }
}

/// Whether `id` has a handler in this build.
#[must_use]
pub fn has_handler(id: CheckId) -> bool {
    handler_for(id).is_some()
}

/// How many checks the triage stage runs, for a coverage line.
#[must_use]
pub fn triage_check_count() -> usize {
    REGISTRY
        .iter()
        .filter(|meta| matches!(handler_for(meta.id), Some(Handler::Triage(_))))
        .count()
}

/// Every check marked `implemented` that has no handler.
///
/// This is the invariant `C-IMPL` states, in code: a check this build claims to have delivered
/// must have something that runs it. An empty result is the pass condition, and the triage stage
/// refuses to write a report when it is not empty — a report that says "implemented" while
/// nothing ran would be the exact misreport principle 8 forbids.
#[must_use]
pub fn implemented_without_handler() -> Vec<CheckId> {
    REGISTRY
        .iter()
        .filter(|meta| meta.status == CheckStatus::Implemented)
        .map(|meta| meta.id)
        .filter(|id| !has_handler(*id))
        .collect()
}
