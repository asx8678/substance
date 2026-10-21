//! T06 Late test batches: tests written long after the code they test.
//!
//! Formula (action plan, T06):
//!
//! > Fail when at least 70% of today's test lines (by blame) arrived after the point in history
//! > where 80% of today's non-test lines existed. Both thresholds go in `thresholds.toml`
//!
//! The question is temporal and it needs no diff: take today's surviving lines, ask git when
//! each arrived, and find the instant at which 80% of the *non-test* lines existed. If at least
//! 70% of the *test* lines arrived after that instant, the tests were written against a codebase
//! that already existed — which is what a retrofitted test suite looks like, and what an honest
//! test-after-the-fact suite does not: a suite written alongside its code has test lines spread
//! over the same period as the code.
//!
//! Two details make the measurement honest:
//!
//! - **The two populations are disjoint** ([`is_test_path`] decides once), because a test line
//!   counted in both would pull the cut-off towards itself.
//! - **The cut-off is the 80th percentile instant of the non-test lines, not the commit where
//!   the fraction first reaches 80%**: lines are dated individually by blame, so the percentile
//!   is the honest reading of "the point where 80% existed". Interpolating between two instants
//!   would invent a time nothing happened at, so the upper of the two neighbours is used.
//!
//! Both numbers are thresholds (ADR-0016, `[history] t06_late_tests_permille` and
//! `t06_code_share_permille`), because unlike most of this family the plan does not say 70/80 in
//! prose and leave it there — it says they belong in the file, and a corpus can move them.

use std::collections::BTreeMap;

use substance_core::clock::Timestamp;
use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::common::is_test_path;
use super::shape::{Judgment, permille};

/// Judges T06.
#[must_use]
pub fn late_test_batches(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut test_lines: Vec<Timestamp> = Vec::new();
    let mut code_lines: Vec<Timestamp> = Vec::new();
    for (path, line) in input.blame.lines() {
        if is_test_path(path, &input.test_paths) {
            test_lines.push(line.at);
        } else {
            code_lines.push(line.at);
        }
    }
    metrics.insert(
        "test_lines".to_owned(),
        i64::try_from(test_lines.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "code_lines".to_owned(),
        i64::try_from(code_lines.len()).unwrap_or(i64::MAX),
    );

    if test_lines.is_empty() {
        return Judgment::not_measured(
            "no test line is blamed at HEAD, so there is no test suite to date",
            metrics,
        );
    }
    if code_lines.is_empty() {
        return Judgment::not_measured(
            "no non-test line is blamed at HEAD, so there is no code to compare the tests \
             against",
            metrics,
        );
    }
    let Some(share) = thresholds.history.t06_code_share_permille else {
        return Judgment::measured(metrics).with_note(
            "config/thresholds.toml sets no t06_code_share_permille, so the cut-off instant \
             cannot be chosen",
        );
    };
    let Some(cut_off_permille) = thresholds.history.t06_late_tests_permille else {
        return Judgment::measured(metrics).with_note(
            "config/thresholds.toml sets no t06_late_tests_permille, so the check reports the \
             numbers without judging them",
        );
    };

    test_lines.sort_unstable();
    code_lines.sort_unstable();
    let Some(cut_off) = percentile_instant(&code_lines, share) else {
        return Judgment::measured(metrics).with_note("the code lines could not be ordered");
    };
    let late = test_lines.iter().filter(|at| **at > cut_off).count();
    let Some(late_permille) = permille(
        u64::try_from(late).unwrap_or(u64::MAX),
        u64::try_from(test_lines.len()).unwrap_or(u64::MAX),
    ) else {
        return Judgment::measured(metrics);
    };
    metrics.insert(
        "late_test_lines".to_owned(),
        i64::try_from(late).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "late_test_permille".to_owned(),
        i64::try_from(late_permille).unwrap_or(i64::MAX),
    );
    // The cut-off as a Unix second: a report reader can turn it into a date, and nothing here
    // depends on a clock, only on the commits' own timestamps.
    metrics.insert(
        "cut_off_unix".to_owned(),
        cut_off.as_offset_date_time().unix_timestamp(),
    );
    metrics.insert("code_share_permille".to_owned(), i64::from(share));

    let mut judgment = Judgment::measured(metrics).judge(
        Some(cut_off_permille),
        late_permille >= u64::from(cut_off_permille),
    );
    if input.blame.coverage.truncated {
        judgment = judgment
            .with_note("the blame budget was reached, so these counts cover the files it read");
    }
    judgment.with_note(format!(
        "{} of {} test lines arrived after {}% of today's code existed",
        late,
        test_lines.len(),
        share / 10
    ))
}

/// The instant at or below which `share_permille` of the sorted lines arrived.
///
/// The upper neighbour is chosen rather than an interpolation: a line either existed by an
/// instant or did not, and a synthetic instant between two commits would be a time at which
/// nothing happened.
#[must_use]
pub fn percentile_instant(sorted: &[Timestamp], share_permille: u32) -> Option<Timestamp> {
    if sorted.is_empty() {
        return None;
    }
    let wanted = u64::from(share_permille);
    let total = u64::try_from(sorted.len()).ok()?;
    let index = (total.saturating_mul(wanted))
        .div_ceil(1000)
        .saturating_sub(1);
    let index = usize::try_from(index).ok()?;
    sorted.get(index.min(sorted.len() - 1)).copied()
}
