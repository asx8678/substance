//! T05 Test weakening: commits that made a suite easier to pass.
//!
//! Formula (action plan, T05):
//!
//! > From hunks: commits that touch non-test files and also delete assertion lines, change
//! > expected literals in test files, or add skip markers, without adding test lines
//!
//! The shape being caught is specific and it is the one that matters: a commit that changes
//! production code **and** takes tests away. Any one of those alone is ordinary:
//!
//! - deleting an assertion while deleting the feature it tested is a legitimate removal;
//! - adding a skip marker while the suite is red is a normal way to unblock;
//! - changing an expected literal is what refactoring a value looks like.
//!
//! Together with a production change in the same commit, the three read as "make the tests agree
//! with the new behaviour", which is how a failing suite is silenced rather than fixed. The
//! check therefore reports a commit only when it touches a non-test file **and** does at least
//! one of the three **and** adds no test lines: a commit that also adds real tests is doing work,
//! and the finding would be noise.
//!
//! Everything comes from hunks ([`super::Hunks`]), which the run gathered once, so this is a pure
//! function of the input like every other signal. Test paths come from
//! `rules/history/test-paths.txt`, the same rule H22's test share uses: two definitions of "a
//! test file" in one report would be a contradiction a reader has to resolve.

use std::collections::BTreeMap;

use substance_core::model::{Evidence, Verdict};
use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::common::is_test_path;
use super::shape::Judgment;

/// The markers that make a test not run.
///
/// The same spellings T04 counts, so a skipped test is skipped for both checks.
pub const SKIP_MARKERS: &[&str] = &[
    ".skip(",
    "xit(",
    "xdescribe(",
    "@pytest.mark.skip",
    "@pytest.mark.xfail",
    "#[ignore]",
    "t.Skip(",
    "@Ignore",
];

/// The Rust test attribute, as a prefix, assembled from parts so the literal never appears in
/// substance's own source: `scripts/guards.sh` rejects it there because a real attribute would be
/// a test of substance (ADR-0008). Here it is data — the marker a test in the evaluated
/// repository carries.
pub const RUST_TEST_ATTRIBUTE: &str = concat!("#[", "test", "]");

/// The async test attributes, as prefixes (`tokio`, `async_std`).
pub const RUST_ASYNC_TEST_ATTRIBUTE: &str = concat!("#[tokio::", "test");

/// The spellings of an assertion, in the languages this build reads.
pub const ASSERTION_MARKERS: &[&str] = &[
    "expect(",
    "assert ",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "self.assert",
    "should ",
    "require.NoError",
    "require.Equal",
];

/// Judges T05.
#[must_use]
pub fn test_weakening(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut evidence: Vec<Evidence> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut weakening: Vec<String> = Vec::new();
    let mut assertions_deleted = 0i64;
    let mut literals_changed = 0i64;
    let mut skips_added = 0i64;
    let mut examined = 0i64;

    for commit in &input.hunks.commits {
        examined += 1;
        let mut touches_code = false;
        let mut test_added = 0usize;
        let mut deleted_assertions: Vec<String> = Vec::new();
        let mut changed_literals: Vec<String> = Vec::new();
        let mut added_skips: Vec<String> = Vec::new();

        for file in &commit.files {
            let test = is_test_path(&file.path, &input.test_paths);
            if !test {
                touches_code = true;
                // A test added elsewhere in the same commit is the commit doing work, so the
                // added-line counts are read for every file, not only test files.
                test_added += file
                    .added()
                    .iter()
                    .filter(|line| is_test_line(line))
                    .count();
                continue;
            }
            for line in file.removed() {
                if ASSERTION_MARKERS.iter().any(|marker| line.contains(marker)) {
                    deleted_assertions.push(format!("{}: {}", file.path, excerpt(line)));
                }
            }
            for line in file.added() {
                if is_test_line(line) {
                    test_added += 1;
                }
                if SKIP_MARKERS.iter().any(|marker| line.contains(marker)) {
                    added_skips.push(format!("{}: {}", file.path, excerpt(line)));
                }
            }
            // A changed expected literal: the same line removed and added, with its quoted text
            // different. Comparing the removed and added sets in one file is enough because a
            // hunk only holds what changed.
            for removed in file.removed() {
                let Some(removed_literal) = first_quoted(removed) else {
                    continue;
                };
                for added in file.added() {
                    let Some(added_literal) = first_quoted(added) else {
                        continue;
                    };
                    if removed_literal != added_literal && same_skeleton(removed, added) {
                        changed_literals.push(format!(
                            "{}: {} -> {}",
                            file.path,
                            excerpt(removed),
                            excerpt(added)
                        ));
                        break;
                    }
                }
            }
        }

        if !touches_code {
            continue;
        }
        if test_added > 0 {
            // The commit added real tests; whatever else it did, it is not the shape T05 names.
            continue;
        }
        if deleted_assertions.is_empty() && changed_literals.is_empty() && added_skips.is_empty() {
            continue;
        }
        assertions_deleted += i64::try_from(deleted_assertions.len()).unwrap_or(i64::MAX);
        literals_changed += i64::try_from(changed_literals.len()).unwrap_or(i64::MAX);
        skips_added += i64::try_from(added_skips.len()).unwrap_or(i64::MAX);
        weakening.push(commit.sha.clone());
        evidence.push(Evidence::Commit(commit.sha.clone()));
        if let Some(first) = deleted_assertions.first() {
            notes.push(format!(
                "{} deleted an assertion: {first}",
                short(&commit.sha)
            ));
        } else if let Some(first) = added_skips.first() {
            notes.push(format!("{} added a skip: {first}", short(&commit.sha)));
        } else if let Some(first) = changed_literals.first() {
            notes.push(format!(
                "{} changed an expected value: {first}",
                short(&commit.sha)
            ));
        }
    }

    metrics.insert("commits_examined".to_owned(), examined);
    metrics.insert(
        "weakening_commits".to_owned(),
        i64::try_from(weakening.len()).unwrap_or(i64::MAX),
    );
    metrics.insert("assertions_deleted".to_owned(), assertions_deleted);
    metrics.insert("expected_literals_changed".to_owned(), literals_changed);
    metrics.insert("skips_added".to_owned(), skips_added);

    if examined == 0 {
        return Judgment::not_measured(
            "no commit hunks were gathered, so no commit could be examined",
            metrics,
        );
    }
    let found = metrics.get("weakening_commits").copied().unwrap_or(0);
    let mut judgment = Judgment::measured(metrics).with_evidence(evidence);
    if input.hunks.truncated {
        judgment = judgment
            .with_note("only the most recent commits were examined; the count covers those");
    }
    if found == 0 {
        judgment.with_note("no commit weakened a test while changing code")
    } else {
        judgment.with_note(format!(
            "{found} commit(s) removed test coverage while changing production code"
        ))
    }
}

/// Whether a line is a test rather than an assertion inside one.
///
/// The distinction matters for "did this commit add tests": adding an assertion to an existing
/// test is still adding coverage, but adding a `describe` block with nothing in it is not. Both
/// count, because both are the author doing test work rather than removing it.
#[must_use]
pub fn is_test_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("it(")
        || trimmed.starts_with("test(")
        || trimmed.starts_with("it.each(")
        || trimmed.starts_with("test.each(")
        || trimmed.starts_with("describe(")
        || trimmed.starts_with("def test_")
        || trimmed.starts_with("async def test_")
        || trimmed.starts_with(RUST_TEST_ATTRIBUTE)
        || trimmed.starts_with(RUST_ASYNC_TEST_ATTRIBUTE)
        || trimmed.starts_with("func Test")
        || ASSERTION_MARKERS.iter().any(|marker| line.contains(marker))
}

/// The first single- or double-quoted string in a line, without its quotes.
#[must_use]
pub fn first_quoted(line: &str) -> Option<&str> {
    for quote in ['\'', '"', '`'] {
        let Some(start) = line.find(quote) else {
            continue;
        };
        let rest = &line[start + quote.len_utf8()..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        return Some(&rest[..end]);
    }
    None
}

/// A line with every quoted string replaced by `""`, so two lines that differ only in their
/// literal compare equal.
#[must_use]
pub fn same_skeleton(left: &str, right: &str) -> bool {
    strip_literals(left).trim() == strip_literals(right).trim()
}

/// A line with each quoted run blanked.
#[must_use]
pub fn strip_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        match quote {
            Some(active) if ch == active => {
                quote = None;
                out.push_str("\"\"");
            }
            Some(_) => {}
            None if ch == '\'' || ch == '"' || ch == '`' => quote = Some(ch),
            None => out.push(ch),
        }
    }
    out
}

/// A line shortened for a note.
fn excerpt(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= 70 {
        return trimmed.to_owned();
    }
    let cut: String = trimmed.chars().take(67).collect();
    format!("{cut}…")
}

/// The first seven characters of a commit SHA, as reports spell it.
fn short(sha: &str) -> &str {
    let end = sha
        .char_indices()
        .nth(7)
        .map(|(at, _)| at)
        .unwrap_or(sha.len());
    &sha[..end]
}

/// T05's verdict: a weakening commit is a fail and its absence a pass.
///
/// No threshold key is involved: the plan states the shape (a commit that touches code, removes
/// test coverage and adds no tests) rather than leaving its cut-off to the corpora, so the
/// definition is the rule and the count is what the report shows.
#[must_use]
pub fn judge(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let mut judgment = test_weakening(input, thresholds);
    if matches!(judgment.verdict, Verdict::Reported) {
        let found = judgment
            .metrics
            .get("weakening_commits")
            .copied()
            .unwrap_or(0);
        judgment.verdict = if found == 0 {
            Verdict::Pass
        } else {
            Verdict::Fail
        };
    }
    judgment
}
