//! The static checks S01-S12 (M1-T08).
//!
//! Two kinds of check live here, and the difference is deliberate:
//!
//! - **Rule-backed** checks (S02, S03, and S09's forwarding half) read the matches the engine
//!   in [`crate::scan`] produced. The rule decided what to look for; the check decides what
//!   the matches mean, because a metric is a question about the whole tree (a count per
//!   thousand authored lines, say) and a rule only ever sees one file.
//! - **File-backed** checks (S04-S08, S10-S12) ask about names, configs and text rather than
//!   syntax: which paths git tracks, which configs disagree, which command a README promises.
//!
//! Every check returns [`StaticReport`], which is a [`CheckVerdict`](substance_core::model::CheckVerdict)
//! plus the findings it rests on plus any notes. The shape is the history signals' own
//! (`SignalReport` in `substance_history`), so the report stage treats both families alike.
//!
//! A check that cannot look reports `not_measured` with a reason, never a pass: a missing
//! scanner is not a clean repository (principle 8). A check whose metric has no threshold
//! reports `Reported` (ADR-0013): the numbers are measured and shown, and the check neither
//! passes nor fails until the benchmark corpora set a cut-off.

pub mod d04;
pub mod l05;
pub mod o01;
pub mod o10;
pub mod s01;
pub mod s02;
pub mod s03;
pub mod s04;
pub mod s05;
pub mod s06;
pub mod s07;
pub mod s08;
pub mod s09;
pub mod s10;
pub mod s11;
pub mod s12;
pub mod t04;
pub mod verdict;

pub use d04::startup_schema_traps;
pub use l05::llm_keys_in_client;
pub use o01::locked_install;
pub use o10::fake_ci;
pub use s01::slop_scan;
pub use s02::escape_hatches;
pub use s03::swallowed_writes;
pub use s04::scaffold_leftovers;
pub use s05::agent_leftovers;
pub use s06::committed_junk;
pub use s07::config_contradictions;
pub use s08::docs_honesty;
pub use s09::over_abstraction;
pub use s10::data_access_sprawl;
pub use s11::secrets;
pub use s12::evaluator_manipulation;
pub use t04::skipped_and_weak_tests;
pub use verdict::StaticReport;
