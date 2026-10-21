//! Findings for checks that read names rather than syntax (M1-T08).
//!
//! The rule engine's findings come from a parsed match and carry a line range
//! ([`crate::finding`]). The file-backed checks know a path, sometimes a line, and nothing
//! about a parse, so they need the same [`Finding`] built from less.
//!
//! Two shapes cover every use: [`Location::file`] for a whole file, and
//! [`Location::lines`] for a range inside one. Severity and confidence still come from the
//! catalog registry, so a check cannot promote its own finding.

use substance_core::FindingId;
use substance_core::check::CheckId;
use substance_core::model::{Evidence, FileRange, Finding, Target};
use substance_core::registry;

/// Where a file-backed finding points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// The whole file.
    File(String),
    /// A line range inside the file, 1-based and inclusive.
    Lines {
        /// Repo-relative path.
        path: String,
        /// First line, 1-based.
        start: u32,
        /// Last line, 1-based and inclusive.
        end: u32,
    },
}

impl Location {
    /// A whole file.
    #[must_use]
    pub fn file(path: &str) -> Self {
        Self::File(path.to_owned())
    }

    /// A line range inside a file.
    #[must_use]
    pub fn lines(path: &str, start: u32, end: u32) -> Self {
        Self::Lines {
            path: path.to_owned(),
            start,
            end,
        }
    }

    /// The path.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::File(path) | Self::Lines { path, .. } => path,
        }
    }

    /// The evidence this location produces.
    ///
    /// A file-range evidence always needs two line numbers, so a whole-file finding cites line
    /// 1 as both ends. That is the model's shape, not a claim about the file's extent: a
    /// reader who wants the extent opens the file, which is exactly what the range is for.
    #[must_use]
    pub fn evidence(&self) -> Evidence {
        match self {
            Self::File(path) => Evidence::FileRange(FileRange {
                path: path.clone(),
                start_line: 1,
                end_line: 1,
            }),
            Self::Lines { path, start, end } => Evidence::FileRange(FileRange {
                path: path.clone(),
                start_line: *start,
                end_line: *end,
            }),
        }
    }
}

/// Builds a finding at `location`, named by `sub_check` and summarised by `title`.
///
/// The ID derives from the run, the check, the sub-check and the location, so the same
/// problem in two runs of one commit has the same ID and can be diffed across runs.
#[must_use]
pub fn at(
    run_id: &str,
    check: CheckId,
    sub_check: &str,
    title: &str,
    location: Location,
) -> Finding {
    let meta = registry::meta(check);
    let id = derive_id(run_id, check, sub_check, &location);
    Finding {
        id: id.clone(),
        check_id: check,
        sub_check: Some(sub_check.to_owned()),
        severity: meta.severity,
        confidence: meta.default_confidence,
        title: title.to_owned(),
        target: Target::File(location.path().to_owned()),
        evidence: vec![location.evidence()],
        replay: format!("substance replay {}", id.as_str()),
        cause: None,
        related: Vec::new(),
    }
}

/// The content-derived finding ID for a file-backed finding.
fn derive_id(run_id: &str, check: CheckId, sub_check: &str, location: &Location) -> FindingId {
    let (start, end) = match location {
        Location::File(_) => ("1".to_owned(), "1".to_owned()),
        Location::Lines { start, end, .. } => (start.to_string(), end.to_string()),
    };
    FindingId::derive(&[
        run_id.as_bytes(),
        check.as_str().as_bytes(),
        sub_check.as_bytes(),
        location.path().as_bytes(),
        start.as_bytes(),
        end.as_bytes(),
    ])
}
