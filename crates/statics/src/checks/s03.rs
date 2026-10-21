//! S03 Swallowed writes (M1-T08).
//!
//! Formula (action plan, S03):
//!
//! > Rules for: TS DB-client calls (insert, update, delete, create, upsert, save, execute,
//! > query) that aren't under `await`, `return` or `.then`; Rust `let _ = $Q.execute($$$)`;
//! > Python `except: pass` in a `try` that calls `commit()`.
//!
//! The rules match the *shapes*; this check decides which of them are actually unhandled.
//! The distinction matters: `await db.insert(x)` and `db.insert(x).then(…)` are the same call
//! as the one S03 is about, and reporting them would make the check wrong on well-written
//! code.
//!
//! - **TypeScript**: the rule matches an expression statement holding a client-method call.
//!   A call under `await`, under `return` or followed by `.then` is not the whole body of a
//!   statement, so it never matches; nothing further to filter.
//! - **Rust**: the rule matches `let _ = $Q.execute(…)` exactly.
//! - **Python**: the rule matches a `try` that commits and swallows its own `except`.
//!
//! S03 is a hint for B02 and B03, so it reports rather than judges: an unawaited write is a
//! defect in a request handler and a deliberate choice in a fire-and-forget job, and a parse
//! of one file cannot tell which it is looking at.

use std::collections::BTreeMap;

use substance_core::check::CheckId;

use crate::checks::s02::{as_i64, findings, matching};
use crate::checks::verdict::{StaticReport, cap};
use crate::scan::ScanReport;

/// Judges S03 from the scan's matches.
#[must_use]
pub fn swallowed_writes(run_id: &str, scan: &ScanReport) -> StaticReport {
    let matches = matching(scan, CheckId::S03);
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("occurrences".to_owned(), as_i64(matches.len()));
    for found in &matches {
        let key = format!("count.{}", found.sub_check);
        *metrics.entry(key).or_insert(0) += 1;
    }
    let (findings, cut) = cap(findings(run_id, &matches));
    StaticReport::reported(CheckId::S03, metrics)
        .with_findings(findings)
        .with_note(
            "these are hints for B02 and B03: whether an unawaited write is a bug depends on \
             what the caller does with the failure, which a parse cannot see",
        )
        .with_notes(cut.into_iter().collect())
}
