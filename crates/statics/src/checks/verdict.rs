//! The shape every static check returns (M1-T08).
//!
//! A static check answers with the same three things a history signal does: a verdict with
//! its metrics, the findings the verdict rests on, and notes a reader needs to interpret the
//! numbers. Keeping one shape means the report stage does not care which family produced a
//! verdict.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::{CheckVerdict, Finding, Target, Verdict};

/// One static check's answer.
///
/// Not `Default`: a report always names its check, and an empty verdict would be one no reader
/// could place.
#[derive(Debug, Clone)]
pub struct StaticReport {
    /// The verdict, its metrics and its per-part verdicts.
    pub verdict: CheckVerdict,
    /// The findings behind it, sorted by id.
    pub findings: Vec<Finding>,
    /// Sentences a reader needs to interpret the numbers.
    pub notes: Vec<String>,
}

impl StaticReport {
    /// A check that could not look, with the reason a reader needs.
    ///
    /// The metrics are kept when there are any: a check that measured half of something
    /// before discovering a missing tool should still show the half.
    #[must_use]
    pub fn not_measured(check: CheckId, reason: impl Into<String>) -> Self {
        Self {
            verdict: CheckVerdict {
                check_id: check,
                target: Target::Repo,
                verdict: Verdict::NotMeasured {
                    reason: reason.into(),
                },
                finding_ids: Vec::new(),
                metrics: BTreeMap::new(),
                parts: BTreeMap::new(),
            },
            findings: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A check that measured its metrics but has no threshold for them (ADR-0013).
    #[must_use]
    pub fn reported(check: CheckId, metrics: BTreeMap<String, i64>) -> Self {
        Self {
            verdict: CheckVerdict {
                check_id: check,
                target: Target::Repo,
                verdict: Verdict::Reported,
                finding_ids: Vec::new(),
                metrics,
                parts: BTreeMap::new(),
            },
            findings: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Attaches the findings, filling in `finding_ids` from them so the two cannot disagree.
    ///
    /// The findings are sorted by id first: two runs of one commit must produce the same
    /// report, and the scan order is a detail of the walk.
    #[must_use]
    pub fn with_findings(mut self, mut findings: Vec<Finding>) -> Self {
        findings.sort_by(|left, right| left.id.cmp(&right.id));
        self.verdict.finding_ids = findings.iter().map(|finding| finding.id.clone()).collect();
        self.findings = findings;
        self
    }

    /// Adds one note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Adds notes, keeping their order.
    #[must_use]
    pub fn with_notes(mut self, notes: Vec<String>) -> Self {
        self.notes.extend(notes);
        self
    }

    /// Sets one metric.
    #[must_use]
    pub fn with_metric(mut self, key: &str, value: i64) -> Self {
        self.verdict.metrics.insert(key.to_owned(), value);
        self
    }

    /// Records one part's verdict (S04's hash half, S11's bundle half).
    ///
    /// A part is what makes a staged check honest: S04 can see the text markers and cannot
    /// see the scaffold hashes yet, so it says so per part instead of pretending the whole
    /// check ran.
    #[must_use]
    pub fn with_part(mut self, part: &str, verdict: Verdict) -> Self {
        self.verdict.parts.insert(part.to_owned(), verdict);
        self
    }
}

/// A bound on how many findings one check keeps.
///
/// A repository can have thousands of `any` casts. The metrics count all of them; the
/// findings list the first [`MAX_FINDINGS`] in report order, so a report stays a report rather
/// than a dump. The note each check adds says when the list was cut.
pub const MAX_FINDINGS: usize = 200;

/// Cuts `findings` to [`MAX_FINDINGS`] and says whether anything was dropped.
///
/// The input is already sorted by the scan, so the cut keeps the first matches in report
/// order instead of an arbitrary subset.
#[must_use]
pub fn cap(mut findings: Vec<Finding>) -> (Vec<Finding>, Option<String>) {
    if findings.len() <= MAX_FINDINGS {
        return (findings, None);
    }
    let total = findings.len();
    findings.truncate(MAX_FINDINGS);
    (
        findings,
        Some(format!(
            "listing the first {MAX_FINDINGS} of {total} findings; the metrics count all of them"
        )),
    )
}
