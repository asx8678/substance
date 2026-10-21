//! The dependency rule files, embedded (M1-T10).
//!
//! `rules/overlap.toml` and `rules/licenses.toml` are data, not code: adding an HTTP client to
//! an overlap group or a licence to the compatibility matrix is a one-line change to a file a
//! reviewer can read, not a rebuild of a match arm. They are embedded with `include_str!`
//! because a report has to be reproducible from the binary alone: a run must not change its
//! findings because the working directory holds a different copy of a rule file.
//!
//! That embedding creates one hazard the existing rule pack already guards against, and the same
//! guard covers these files: a rule file that exists but is not listed here is a rule nobody
//! runs. `scripts/guards.sh` checks the pack; the test that a file parses happens on load, so a
//! malformed file is a build error at run start rather than a silently empty group list.

use serde::Deserialize;

use crate::deps::p05::OverlapGroup;
use crate::deps::p06::{LicenseRules, Obligation};

/// The overlap rule file.
pub const OVERLAP_TOML: &str = include_str!("../../../../rules/overlap.toml");

/// The licence rule file.
pub const LICENSES_TOML: &str = include_str!("../../../../rules/licenses.toml");

/// Why a rule file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {detail}")]
pub struct RuleError {
    /// The file's path, as a reader would open it.
    pub file: String,
    /// What was wrong.
    pub detail: String,
}

/// The wire shape of `rules/overlap.toml`.
#[derive(Debug, Deserialize)]
struct OverlapFile {
    #[serde(default)]
    group: Vec<OverlapGroupWire>,
}

#[derive(Debug, Deserialize)]
struct OverlapGroupWire {
    language: String,
    name: String,
    #[serde(default)]
    packages: Vec<String>,
}

/// Loads the overlap groups, sorted by `(language, name)` and with each group's packages sorted.
///
/// Sorting here rather than at the call site means two runs report the same groups in the same
/// order whatever order the file happens to list them in.
///
/// # Errors
///
/// Returns [`RuleError`] when the file is not TOML of the documented shape.
pub fn overlap_groups() -> Result<Vec<OverlapGroup>, RuleError> {
    let file: OverlapFile = toml::from_str(OVERLAP_TOML).map_err(|error| RuleError {
        file: "rules/overlap.toml".to_owned(),
        detail: error.to_string(),
    })?;
    let mut groups: Vec<OverlapGroup> = file
        .group
        .into_iter()
        .map(|group| {
            let mut packages = group.packages;
            packages.sort();
            packages.dedup();
            OverlapGroup {
                name: group.name,
                language: group.language,
                packages,
            }
        })
        .collect();
    groups.sort_by(|left, right| {
        left.language
            .cmp(&right.language)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(groups)
}

/// The wire shape of `rules/licenses.toml`.
#[derive(Debug, Deserialize)]
struct LicensesFile {
    #[serde(default)]
    obligations: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    incompatible: Vec<IncompatibleWire>,
}

#[derive(Debug, Deserialize)]
struct IncompatibleWire {
    project: String,
    dependency: String,
}

/// Loads the licence rules.
///
/// An obligation class this build does not know is a rule error rather than a silent
/// [`Obligation::Unknown`]: a misspelt `strong_copyleft` would otherwise turn the one rule that
/// matters into a rule that never fires.
///
/// # Errors
///
/// Returns [`RuleError`] when the file is not TOML of the documented shape, or names an
/// obligation class this build does not read.
pub fn license_rules() -> Result<LicenseRules, RuleError> {
    let file: LicensesFile = toml::from_str(LICENSES_TOML).map_err(|error| RuleError {
        file: "rules/licenses.toml".to_owned(),
        detail: error.to_string(),
    })?;
    let mut obligations = std::collections::BTreeMap::new();
    for (identifier, class) in file.obligations {
        let obligation = obligation_of(class.as_str()).ok_or_else(|| RuleError {
            file: "rules/licenses.toml".to_owned(),
            detail: format!(
                "{identifier} names the obligation class \"{class}\", which this build does not \
                 know (permissive, weak_copyleft, strong_copyleft)"
            ),
        })?;
        obligations.insert(identifier, obligation);
    }
    let mut incompatible: Vec<(Obligation, Obligation)> = Vec::new();
    for pair in file.incompatible {
        let project = obligation_of(pair.project.as_str()).ok_or_else(|| RuleError {
            file: "rules/licenses.toml".to_owned(),
            detail: format!(
                "[[incompatible]] names an unknown project class \"{}\"",
                pair.project
            ),
        })?;
        let dependency = obligation_of(pair.dependency.as_str()).ok_or_else(|| RuleError {
            file: "rules/licenses.toml".to_owned(),
            detail: format!(
                "[[incompatible]] names an unknown dependency class \"{}\"",
                pair.dependency
            ),
        })?;
        incompatible.push((project, dependency));
    }
    // The matrix is compared by containment, so it is deduplicated and sorted to keep the
    // report and the debug output of two runs identical.
    incompatible.sort();
    incompatible.dedup();
    Ok(LicenseRules {
        obligations,
        incompatible,
    })
}

/// The obligation class a rule file spells.
fn obligation_of(text: &str) -> Option<Obligation> {
    match text {
        "permissive" => Some(Obligation::Permissive),
        "weak_copyleft" => Some(Obligation::WeakCopyleft),
        "strong_copyleft" => Some(Obligation::StrongCopyleft),
        "unknown" => Some(Obligation::Unknown),
        _ => None,
    }
}
