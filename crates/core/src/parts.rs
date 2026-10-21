//! Checks that are built in parts, and the milestone that delivers each part (M0-T08).
//!
//! O01, S11 and L05 report per-part verdicts in `CheckVerdict.parts`, because parts of them
//! land in different milestones: O01's exec half arrives with the runner (M2-T05), and the
//! bundle half of S11 and L05 needs a built app to scan (M4-T04). A part that has not been
//! built reports `not_measured("not implemented (Mx)")` rather than staying silent.

use crate::check::{CheckId, Milestone};

/// One part of a check, and the milestone that delivers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckPart {
    /// The part name as it appears in `CheckVerdict.parts`.
    pub name: &'static str,
    /// The milestone that delivers the part.
    pub milestone: Milestone,
}

/// O01's parts: the static lockfile analysis (M1) and the exec half (M2).
const O01_PARTS: &[CheckPart] = &[
    CheckPart {
        name: "static",
        milestone: Milestone::M1,
    },
    CheckPart {
        name: "exec",
        milestone: Milestone::M2,
    },
];

/// S11's parts: the repository scan (M1) and the built-bundle scan (M4).
const S11_PARTS: &[CheckPart] = &[
    CheckPart {
        name: "repo",
        milestone: Milestone::M1,
    },
    CheckPart {
        name: "bundle",
        milestone: Milestone::M4,
    },
];

/// L05's parts: the repository scan (M1) and the built-bundle scan (M4).
const L05_PARTS: &[CheckPart] = &[
    CheckPart {
        name: "repo",
        milestone: Milestone::M1,
    },
    CheckPart {
        name: "bundle",
        milestone: Milestone::M4,
    },
];

/// The parts `id` is built from, in the order they are reported; empty for most checks.
#[must_use]
pub fn parts_of(id: CheckId) -> &'static [CheckPart] {
    match id {
        CheckId::O01 => O01_PARTS,
        CheckId::S11 => S11_PARTS,
        CheckId::L05 => L05_PARTS,
        _ => &[],
    }
}
