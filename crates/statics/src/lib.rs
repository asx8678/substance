//! The tree-sitter and ast-grep rule runner plus the other file-based checks (M1-T07).
//!
//! One shape, so every static check reuses it:
//!
//! 1. [`RulePack::load`] reads the embedded ast-grep YAML from `rules/<lang>/*.yml`. The pack
//!    is compiled into the binary, so a repository under test cannot change the rules that
//!    judge it.
//! 2. [`scan`] walks the run's commit — not the working tree, so uncommitted edits never
//!    leak in — keeps only authored paths, skips files over a megabyte, and matches the
//!    pack's rules per language.
//! 3. [`findings`] turns the matches into hint-confidence [`Finding`]s with file-range
//!    evidence, sorted by `(path, line, rule id)`, whose severity and confidence come from
//!    the catalog registry rather than from the rule.
//!
//! Nothing here decides pass or fail: a static match is a hint that explains a verdict, and
//! the checks in M1-T08 turn groups of hints into their metrics.
//!
//! ```text
//! let pack = RulePack::load()?;               // embedded rules, hashed
//! let report = scan(&repo, "HEAD", &pack).await?;   // matches + what was skipped
//! let findings = findings(run_id, &report.matches);  // hint findings, sorted
//! let hash = pack.hash();                     // manifest rule_pack_hash
//! ```

pub mod finding;
pub mod lang;
pub mod rulepack;
pub mod scan;

pub use finding::{is_static_check, to_finding};
pub use lang::{LANGS, Lang};
pub use rulepack::{FILES, Rule, RulePack, RulePackError, hash_files};
pub use scan::{MAX_FILE_BYTES, RuleMatch, ScanReport, ScanStats, scan};

use substance_core::model::Finding;

/// Turns every match into a finding, in match order.
///
/// The input is already sorted by `(path, line, rule id)` ([`scan`] sorts it), and the
/// output keeps that order: sorting here again would hide a caller that reordered matches.
#[must_use]
pub fn findings(run_id: &str, matches: &[RuleMatch]) -> Vec<Finding> {
    matches
        .iter()
        .map(|found| to_finding(run_id, found))
        .collect()
}

/// The rule pack's hash, for a manifest that has not loaded the pack.
///
/// The manifest is built before any check runs, so it cannot borrow the loaded pack; it
/// calls this instead. Both go through [`hash_files`], so the value is the same.
#[must_use]
pub fn rule_pack_hash() -> String {
    hash_files(FILES)
}
