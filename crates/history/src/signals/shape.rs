//! The shape every history signal returns (M1-T05).
//!
//! A signal is a pure function `fn(&HistoryInput, &Thresholds) -> Judgment`. The judgment
//! holds the verdict, the integer metrics and the evidence, so a check's module is one
//! readable function plus its formula.
//!
//! Two helpers encode the rule the task states as an acceptance check: **a missing input
//! never yields pass** (principle 8). [`Judgment::not_measured`] is the only way to express
//! "I could not look", and it always carries a reason; [`Judgment::judge`] takes an
//! `Option<u32>` threshold, so a check with no key reports and one below its threshold
//! passes, in one line and one place.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::{Evidence, Verdict};

use super::{EVIDENCE_COMMITS, HistoryInput};
use substance_core::thresholds::Thresholds;

/// One check's answer: what it decided, what it measured, what it can point at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judgment {
    /// The verdict.
    pub verdict: Verdict,
    /// Named integer measurements, sorted by key when the verdict is built.
    pub metrics: BTreeMap<String, i64>,
    /// The evidence, sorted by kind then reference.
    pub evidence: Vec<Evidence>,
    /// Sentences a reader needs to interpret the numbers, for example a cap the sample hit.
    pub notes: Vec<String>,
}

impl Judgment {
    /// A judgment resting on metrics only; the verdict comes later, from [`Self::judge`].
    #[must_use]
    pub fn measured(metrics: BTreeMap<String, i64>) -> Self {
        Self {
            verdict: Verdict::Reported,
            metrics,
            evidence: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A check that could not look, with the reason a reader needs.
    ///
    /// The metrics are kept: a check that computed half its numbers before discovering a
    /// missing input should still show them.
    #[must_use]
    pub fn not_measured(reason: impl Into<String>, metrics: BTreeMap<String, i64>) -> Self {
        Self {
            verdict: Verdict::NotMeasured {
                reason: reason.into(),
            },
            metrics,
            evidence: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A check whose evidence cannot decide, with the reason.
    #[must_use]
    pub fn unverifiable(reason: impl Into<String>, metrics: BTreeMap<String, i64>) -> Self {
        Self {
            verdict: Verdict::Unverifiable {
                reason: reason.into(),
            },
            metrics,
            evidence: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Decides pass or fail against a threshold, or reports when there is none.
    ///
    /// `value` is the measured quantity in the threshold's own units (a permille, a count, a
    /// rate) and `exceeded` says whether the value crosses it. A threshold that is present
    /// and not crossed is a `pass`; an absent one is [`Verdict::Reported`] — the metrics are
    /// shown and the check declines to judge (ADR-0013).
    #[must_use]
    pub fn judge(mut self, threshold: Option<u32>, exceeded: bool) -> Self {
        self.verdict = match threshold {
            Some(_) if exceeded => Verdict::Fail,
            Some(_) => Verdict::Pass,
            None => Verdict::Reported,
        };
        self
    }

    /// Adds evidence.
    #[must_use]
    pub fn with_evidence(mut self, evidence: Vec<Evidence>) -> Self {
        self.evidence = evidence;
        self
    }

    /// Adds one note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Adds one metric.
    #[must_use]
    pub fn with_metric(mut self, key: &str, value: i64) -> Self {
        self.metrics.insert(key.to_owned(), value);
        self
    }
}

/// `part` as a share of `whole`, in permille, or `None` when `whole` is zero.
///
/// Every ratio in these checks is a permille, and every one of them has a "no denominator"
/// case: no commits, no blamed lines, no added lines. Returning an [`Option`] forces the
/// caller to say what that case means — usually `not_measured` — instead of dividing by zero
/// or quietly reporting a zero that reads like a measurement. The arithmetic is integer and
/// saturating, so a huge numstat cannot overflow the multiply.
#[must_use]
pub fn permille(part: u64, whole: u64) -> Option<u64> {
    part.saturating_mul(1000).checked_div(whole)
}

/// One history signal: its check ID and the function that judges the input.
#[derive(Debug, Clone, Copy)]
pub struct Signal {
    /// The check this signal implements.
    pub check: CheckId,
    /// The judging function.
    pub judge: fn(&HistoryInput, &Thresholds) -> Judgment,
}

impl Signal {
    /// A signal for `check`.
    #[must_use]
    pub fn new(check: CheckId, judge: fn(&HistoryInput, &Thresholds) -> Judgment) -> Self {
        Self { check, judge }
    }
}

/// The commits a judgment lists as evidence, by contribution then SHA.
///
/// The plan asks each check for "the top 20 contributing commits". A contribution is a
/// number the check chooses — lines added, minutes saved, lines deleted again — and ties
/// break on the SHA, so two runs list one order. The evidence is one [`Evidence::Commit`]
/// per SHA: a commit is the artifact, and the metric map already says how much it
/// contributed.
#[must_use]
pub fn top_commits(mut contributions: Vec<(String, u64)>) -> Vec<Evidence> {
    contributions.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    contributions.truncate(EVIDENCE_COMMITS);
    contributions
        .into_iter()
        .map(|(sha, _)| Evidence::Commit(sha))
        .collect()
}
