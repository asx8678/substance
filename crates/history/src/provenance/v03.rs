//! V03 Knowledge-cutoff estimate (M1-T11).
//!
//! The plan's rule:
//!
//! > From P03's dependency vintage and outdated API styles (for example the old OpenAI Python
//! > SDK's `openai.ChatCompletion.create`), estimate when the generating model's knowledge
//! > ends.
//!
//! Two independent bounds, and the estimate is the interval they leave:
//!
//! - **`from`, from adoptions.** A dependency version could not have been known to a model
//!   before the registry published it. The newest release among the versions the project
//!   adopted is therefore a lower bound: whoever wrote this knew about a release from that
//!   date. Adopting an old version says nothing, so the bound takes the **maximum**.
//! - **`to`, from vintages.** A call to an API that a library retired on a known date was
//!   written before that date, so a tree that still calls it was authored before then. Each
//!   match brackets the code; the **earliest** retirement among the matches is the tightest
//!   upper bound.
//!
//! Both are bounds on *knowledge*, not on authorship: a project written today against the old
//! SDK would report an early `to`, because its author chose the old API rather than having
//! learned the new one. The plan says "estimate", the verdict says `reported`, and the note
//! says the same thing in one sentence — V03 never accuses anyone of anything, and ADR-0012
//! keeps it out of every score.
//!
//! When only one bound exists the window is one-sided, and the verdict says which side is
//! missing rather than inventing the other. When neither exists it is `not_measured`.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::check::CheckId;
use substance_core::model::report::Ratio;
use substance_core::model::{CheckVerdict, Target, Verdict};
use substance_core::provenance::VintageTable;

/// One API-vintage match in the tree, with the date range it implies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VintageHit {
    /// The vintage id the rule named.
    pub id: String,
    /// The file the call is in, repo-relative.
    pub path: String,
    /// The first line of the match.
    pub line: u32,
    /// When the API was current, from the vintage table.
    pub current_to: Timestamp,
}

/// What V03 reads: the adoption dates and the vintage matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnowledgeInput {
    /// The newest release date among the dependency versions the project adopted, with the
    /// package it came from, for the evidence line.
    pub newest_adoption: Option<(Timestamp, String)>,
    /// Every dependency whose adoption bound was read, sorted.
    pub adoptions_considered: Vec<String>,
    /// The ancient-looking vintages still in use, sorted by `(current_to, id, path, line)`.
    pub vintages: Vec<VintageHit>,
    /// Why one side of the bracket is missing, when it is, sorted.
    pub missing: Vec<String>,
}

/// The window V03 estimated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// The verdict: `reported`, or `not_measured` with the reason only one side was readable.
    pub verdict: CheckVerdict,
    /// The earliest instant the evidence permits.
    pub from: Option<Timestamp>,
    /// The latest instant the evidence permits.
    pub to: Option<Timestamp>,
    /// The sentences the bracket rests on, sorted.
    pub evidence: Vec<String>,
    /// Notes a reader needs, sorted.
    pub notes: Vec<String>,
}

/// Judges V03.
#[must_use]
pub fn knowledge_window(
    input: &KnowledgeInput,
    vintages: &VintageTable,
    as_of: Timestamp,
) -> Window {
    let mut notes: Vec<String> = input.missing.clone();
    let mut evidence: Vec<String> = Vec::new();

    let from = input.newest_adoption.as_ref().map(|(at, package)| {
        evidence.push(format!(
            "dependency release: {} was published on {}, before any code using it was written",
            package,
            at.to_rfc3339()
        ));
        *at
    });

    // The tightest upper bound is the earliest retirement among the vintages still called.
    // A hit whose vintage the table does not hold is counted and skipped: its date is unknown,
    // and inventing one would move the window.
    let mut unknown_vintages: Vec<String> = Vec::new();
    let mut matched: Vec<&VintageHit> = Vec::new();
    for hit in &input.vintages {
        if vintages.get(&hit.id).is_some() {
            matched.push(hit);
        } else if !unknown_vintages.contains(&hit.id) {
            unknown_vintages.push(hit.id.clone());
        }
    }
    matched.sort_by(|left, right| {
        left.current_to
            .cmp(&right.current_to)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
    let to = matched.first().map(|hit| {
        let count = matched
            .iter()
            .filter(|other| other.current_to == hit.current_to)
            .count();
        evidence.push(format!(
            "outdated API: {} at {}:{} was retired on {} ({} match{} of that vintage)",
            hit.id,
            hit.path,
            hit.line,
            hit.current_to.to_rfc3339(),
            count,
            if count == 1 { "" } else { "es" }
        ));
        hit.current_to
    });
    for id in &unknown_vintages {
        notes.push(format!(
            "a rule reports vintage `{id}`, which rules/provenance/api-vintage.toml does not \
             date, so it bounded nothing"
        ));
    }

    // A `from` after `to` is not an error in the code, it is a contradiction in the evidence:
    // the project adopted a release published after the API it calls was retired. Saying so is
    // the honest report; picking one bound would hide it.
    if let (Some(from_at), Some(to_at)) = (from, to)
        && from_at > to_at
    {
        notes.push(format!(
            "the evidence contradicts itself: an adopted release ({}) postdates the \
                 retirement of an API still in use ({}), so no window is reported",
            from_at.to_rfc3339(),
            to_at.to_rfc3339()
        ));
        evidence.sort();
        notes.sort();
        return Window {
            verdict: not_measured(
                "the evidence brackets an empty interval: an adopted release postdates an \
                     API the tree still calls",
                &input.vintages,
                input.adoptions_considered.len(),
            ),
            from: None,
            to: None,
            evidence,
            notes,
        };
    }

    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert(
        "adoptions_considered".to_owned(),
        i64::try_from(input.adoptions_considered.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "vintage_matches".to_owned(),
        i64::try_from(matched.len()).unwrap_or(i64::MAX),
    );
    if let Some(at) = from {
        metrics.insert("days_before_as_of".to_owned(), days_before(at, as_of));
    }
    if let Some(at) = to {
        metrics.insert(
            "api_vintage_days_before_as_of".to_owned(),
            days_before(at, as_of),
        );
    }
    evidence.sort();
    notes.sort();

    let verdict = match (from, to) {
        // One bound is still a bound: the window is open on the other side and the reports
        // says which, so the check measured what it could.
        (Some(_), Some(_)) | (Some(_), None) | (None, Some(_)) => CheckVerdict {
            check_id: CheckId::V03,
            target: Target::Repo,
            verdict: Verdict::Reported,
            finding_ids: Vec::new(),
            metrics,
            parts: BTreeMap::new(),
        },
        (None, None) => not_measured(
            "no dependency release date and no dated API vintage could be read, so neither \
             bound of the knowledge window exists",
            &input.vintages,
            input.adoptions_considered.len(),
        ),
    };
    Window {
        verdict,
        from,
        to,
        evidence,
        notes,
    }
}

/// A `not_measured` verdict for V03, with whatever was counted.
fn not_measured(reason: &str, hits: &[VintageHit], adoptions: usize) -> CheckVerdict {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert(
        "adoptions_considered".to_owned(),
        i64::try_from(adoptions).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "vintage_matches".to_owned(),
        i64::try_from(hits.len()).unwrap_or(i64::MAX),
    );
    CheckVerdict {
        check_id: CheckId::V03,
        target: Target::Repo,
        verdict: Verdict::NotMeasured {
            reason: reason.to_owned(),
        },
        finding_ids: Vec::new(),
        metrics,
        parts: BTreeMap::new(),
    }
}

/// Whole days from `instant` to `as_of`, negative when `instant` is later.
///
/// Integer arithmetic throughout (ADR-0009): a day is 86,400 seconds and the difference is
/// truncated toward zero, so the metric never depends on a float's rounding.
fn days_before(instant: Timestamp, as_of: Timestamp) -> i64 {
    let seconds = as_of.as_offset_date_time().unix_timestamp()
        - instant.as_offset_date_time().unix_timestamp();
    seconds / 86_400
}

/// The window as the panel shows it: the two bounds and their evidence.
#[must_use]
pub fn panel(window: &Window) -> (Option<Timestamp>, Option<Timestamp>, Vec<String>) {
    (window.from, window.to, window.evidence.clone())
}

/// The share of a whole, in permille, for a caller that wants one.
///
/// V03 has no share of its own; the function is here so a panel that later divides the
/// matched files by the scanned ones uses the same integer rule as V01.
#[must_use]
pub fn share(part: u64, whole: u64) -> Ratio {
    if whole == 0 {
        return 0;
    }
    u32::try_from(part.saturating_mul(1000) / whole).unwrap_or(u32::MAX)
}
