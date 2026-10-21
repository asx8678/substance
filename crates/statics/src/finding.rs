//! Turning rule matches into findings (M1-T07).
//!
//! A static match is a **hint**: it points at code that usually means something, not at a
//! defect proven by running the app. Its confidence, severity and axes come from the catalog
//! registry ([`substance_core::registry::meta`]), so a rule cannot promote itself: the most a
//! rule chooses is which check it belongs to, and the check decides how much its finding
//! counts.
//!
//! Every finding carries its file range as evidence and nothing else. A hint that cites a
//! line is checkable; one that cites a whole file is not.

use substance_core::FindingId;
use substance_core::check::CheckId;
use substance_core::model::{Evidence, Finding, Target};
use substance_core::registry;

use crate::scan::RuleMatch;

/// Builds the finding one match produces.
///
/// The ID derives from the run, the check, the sub-check and the location, so the same match
/// in two runs of one commit has the same ID: a finding can be diffed across runs, and a
/// replayed run reproduces it exactly. The line range is part of the key — two `unwrap()`
/// calls in one file are two findings.
#[must_use]
pub fn to_finding(run_id: &str, found: &RuleMatch) -> Finding {
    let meta = registry::meta(found.check_id);
    let id = findings_id(run_id, found);
    Finding {
        id: id.clone(),
        check_id: found.check_id,
        sub_check: Some(found.sub_check.clone()),
        severity: meta.severity,
        confidence: meta.default_confidence,
        title: title(found),
        target: Target::File(found.path.clone()),
        evidence: vec![found.evidence()],
        replay: format!("substance replay {}", id.as_str()),
        cause: None,
        related: Vec::new(),
    }
}

/// The finding's one-line summary: the rule's message, with the location the report shows
/// beside it left to the report.
fn title(found: &RuleMatch) -> String {
    if found.message.trim().is_empty() {
        found.sub_check.clone()
    } else {
        found.message.clone()
    }
}

/// The content-derived finding ID.
///
/// `FindingId::derive` length-prefixes every part, so no separators are needed and no two
/// different part lists can hash the same. The line numbers go in as text for the same
/// reason the rest does: the derivation hashes bytes, and a number's spelling is its bytes.
fn findings_id(run_id: &str, found: &RuleMatch) -> FindingId {
    let check = found.check_id.as_str();
    let start = found.start_line.to_string();
    let end = found.end_line.to_string();
    FindingId::derive(&[
        run_id.as_bytes(),
        check.as_bytes(),
        found.sub_check.as_bytes(),
        found.path.as_bytes(),
        start.as_bytes(),
        end.as_bytes(),
    ])
}

/// The evidence list a match produces, exposed for callers that build evidence themselves.
#[must_use]
pub fn evidence(found: &RuleMatch) -> Evidence {
    found.evidence()
}

/// Whether a check ID is one the static engine may attribute rules to.
///
/// The registry is total, so this is a policy question, not a lookup: a rule naming `H04`
/// would produce a history finding from a parse of one file, which no reader would expect.
#[must_use]
pub fn is_static_check(id: CheckId) -> bool {
    matches!(
        id,
        CheckId::S01
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
    )
}
