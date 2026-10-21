//! The dependency checks P01–P07 (M1-T10).
//!
//! These checks ask about the packages a project depends on rather than about its code. They
//! live in `substance-history` because that is the crate that already speaks to registries,
//! the forge and the git dates, and because putting them in `substance-statics` would make the
//! two crates depend on each other's subject matter as well as on each other.
//!
//! The split every module keeps:
//!
//! - [`world`] reads the project's own manifests and lockfiles: what it depends on, at which
//!   versions. Pure IO over the tracked tree.
//! - [`known`] asks the registries about those packages, once each, through
//!   [`crate::snapshot::SnapshotHttp`] so a run replays offline and every request spends one
//!   unit of the request budget.
//! - [`scan`] runs the external scanners (P01, P05's unused-dependency tools, P06's licence
//!   tools) through the run's [`substance_runner::tool::ToolRunner`], which is what makes a
//!   missing binary a `not_measured` reason and a `network = true` tool replayable.
//! - [`p01`]–[`p07`] are the judges: pure functions over those three inputs, returning a
//!   [`crate::signals::shape::Judgment`].
//!
//! The rule from principle 8 holds here as everywhere: a check that could not look answers
//! `not_measured` with the reason. A registry that was unreachable is not a clean dependency
//! list, and a missing scanner is not an empty finding set.

pub mod known;
pub mod p01;
pub mod p02;
pub mod p03;
pub mod p04;
pub mod p05;
pub mod p06;
pub mod p07;
pub mod rules;
pub mod run;
pub mod scan;
pub mod version;
pub mod world;

pub use known::{Known, KnownError, KnownPackage, KnownVersion, gather as gather_known};
pub use p01::known_vulnerabilities;
pub use p02::reputation;
pub use p03::born_stale;
pub use p04::deprecated_or_yanked;
pub use p05::bloat;
pub use p06::licenses;
pub use p07::vitality;
pub use rules::{license_rules, overlap_groups};
pub use run::{DepsError, DepsInput, DepsOutcome, run_dependency_checks};
pub use scan::{ScannerRun, ScannerTarget};
pub use world::{Direct, World};
