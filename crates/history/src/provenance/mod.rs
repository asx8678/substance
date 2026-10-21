//! Provenance V01 and V03 (M1-T11).
//!
//! Two checks that describe how a repository was built and never judge it. ADR-0012 is
//! explicit: provenance is reported, never scored, and a `V` verdict is `reported` or
//! `not_measured` — never `pass` and never `fail`. The type system is not able to enforce that,
//! so [`v01::declared`] and [`v03::knowledge_window`] construct their verdicts directly rather
//! than through [`crate::signals::shape::Judgment::judge`], which is the only function in the
//! workspace that can produce a pass or a fail from a threshold.
//!
//! The split is the same one every other family keeps:
//!
//! - [`input`] gathers what git and the forge already recorded: the commits, the blame of
//!   HEAD's authored lines, each direct dependency's adoption date and its release dates, and
//!   which instruction files exist. It is IO.
//! - [`v01`] and [`v03`] judge it. They are pure functions over the gathered data.
//!
//! What neither check does is guess. V01 reads signatures a tool writes; a repository whose
//! signatures were stripped looks exactly like one written by hand, and the note says so. V03
//! brackets a window from dates; it does not name a model, and it does not run the statistical
//! attribution V02 will bring.

pub mod input;
pub mod run;
pub mod v01;
pub mod v03;

pub use input::{
    InstructionFileHit, ProvenanceInput, V03Input, gather_declared, instruction_paths,
    knowledge_from_adoptions, load_vintages, vintages_from_matches,
};
pub use run::{ProvenanceContext, ProvenanceOutcome, VintageMatches, run_provenance};
pub use v01::{Declared, DeclaredShares, declaration_of, declared, permille};
pub use v03::{KnowledgeInput, VintageHit, Window, knowledge_window};
