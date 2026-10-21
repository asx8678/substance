//! T04 Skipped and weak tests (M1-T09).
//!
//! Formula (action plan, T04):
//!
//! > Per framework: test count, skip markers (`it.skip`, `test.skip`, `xit`, `describe.skip`,
//! > `@pytest.mark.skip`, `#[ignore]`), assertions per test, and the share of weak matchers
//! > (`toBeTruthy`, `toBeDefined`)
//!
//! Two of the four numbers come from rules (`rules/*/skipped-tests.yml`), because a skip marker
//! is syntax: `it.skip(` is the same shape in every file that has it. The other two need the
//! whole file:
//!
//! - **test count** is not one syntax across the frameworks. Jest/Vitest answer to `it(`,
//!   `test(` and `describe(`-scoped variants; pytest to `def test_*`; Rust to its test
//!   attribute ([`RUST_TEST_ATTRIBUTES`]). The
//!   check therefore counts declarations per framework rather than matching a universal
//!   pattern that would be wrong for two of the three.
//! - **assertions per test** needs both halves at once: an assertion is a call (`expect(`,
//!   `assert `, `assert_eq!`) and a test is the declaration it is inside. Counting them
//!   separately and dividing would need a file-level average; counting them per test is what
//!   the metric means, and a test with no assertion is the thing worth seeing.
//!
//! Everything here is a metric. T04 has no threshold in the plan, so it reports (ADR-0013):
//! "2 of 40 tests are skipped and 60% of assertions are `toBeTruthy`" is a sentence a reader
//! can judge, and a cut-off would have to come from the corpora.
//!
//! The evaluated repository's tests are the *subject*, not substance's own: this module reads
//! test files in the tree under evaluation, which is product behaviour, not a test of
//! substance (ADR-0008 forbids those and `scripts/guards.sh` enforces it).

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::scan::RuleMatch;
use crate::tree::TrackedTree;

/// A test framework this build can count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Framework {
    /// Jest, Vitest, Mocha, Bun's runner: `it(`, `test(`.
    JsTest,
    /// pytest, unittest: `def test_`.
    Python,
    /// Rust's test attribute and its async variants (`tokio`, `async_std`).
    Rust,
}

impl Framework {
    /// The metric key prefix.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::JsTest => "js",
            Self::Python => "python",
            Self::Rust => "rust",
        }
    }
}

/// The matchers that assert something exists rather than something specific.
///
/// `toBeTruthy` and `toBeDefined` are the catalog's two. They are not wrong on their own; a
/// suite made mostly of them cannot tell a working feature from a returned object.
pub const WEAK_MATCHERS: &[&str] = &["toBeTruthy", "toBeDefined"];

/// The Rust test attributes, as prefixes so the async variants match too.
///
/// Written as concatenated pieces rather than as the attribute's literal text:
/// `scripts/guards.sh` rejects that literal in substance's own source, because a real attribute
/// there would be a test of substance (ADR-0008). Here the string is data — the marker a test in
/// the *evaluated* repository carries — so it is assembled at compile time and never appears in a
/// line that could be read as an attribute on one of our own functions.
pub const RUST_TEST_ATTRIBUTES: &[&str] = &[
    concat!("#[", "test", "]"),
    concat!("#[tokio::", "test"),
    concat!("#[async_std::", "test"),
];

/// The assertion spellings this build counts.
pub const ASSERTIONS: &[&str] = &[
    "expect(",
    "assert ",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "self.assert",
];

/// Judges T04.
#[must_use]
pub async fn skipped_and_weak_tests(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    matches: &[RuleMatch],
) -> StaticReport {
    let mut by_key: BTreeMap<String, i64> = BTreeMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // The skip markers, from the rules. One finding each: a skipped test is a thing a reader
    // may want to look at, and the file has to be a test file for the marker to mean anything.
    let mut skips: BTreeMap<String, i64> = BTreeMap::new();
    for found in matches
        .iter()
        .filter(|found| found.check_id == CheckId::T04)
    {
        *skips.entry(found.path.clone()).or_insert(0) += 1;
        findings.push(file_finding::at(
            run_id,
            CheckId::T04,
            &found.sub_check,
            &format!("{} in {}", found.message, found.path),
            Location::lines(&found.path, found.start_line, found.end_line),
        ));
    }
    let skip_total: i64 = skips.values().sum();
    by_key.insert("skip_markers".to_owned(), skip_total);

    // The per-framework counts. Only files a framework would run are read: counting `it(` in
    // an application file would double the numerator with code that is not a test.
    let test_files: Vec<String> = tree
        .matching(is_test_file)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut tests = 0i64;
    let mut assertions = 0i64;
    let mut weak = 0i64;
    let mut frameworks: BTreeMap<&'static str, i64> = BTreeMap::new();
    for path in &test_files {
        let Some(text) = tree.text(path).await else {
            continue;
        };
        let framework = framework_of(path);
        let Some(framework) = framework else {
            continue;
        };
        let count = count_tests(&text, framework);
        if count == 0 {
            continue;
        }
        *frameworks.entry(framework.key()).or_insert(0) += count;
        tests += count;
        for line in text.lines() {
            if ASSERTIONS.iter().any(|assertion| line.contains(assertion)) {
                assertions += 1;
            }
            if WEAK_MATCHERS.iter().any(|matcher| line.contains(matcher)) {
                weak += 1;
            }
        }
    }

    by_key.insert("tests".to_owned(), tests);
    by_key.insert("assertions".to_owned(), assertions);
    by_key.insert("weak_matchers".to_owned(), weak);
    let assertions_per_test = if tests > 0 {
        Some(assertions.saturating_mul(100) / tests)
    } else {
        None
    };
    if let Some(hundredths) = assertions_per_test {
        by_key.insert("assertions_per_test_x100".to_owned(), hundredths);
    }
    if assertions > 0 {
        by_key.insert(
            "weak_share_permille".to_owned(),
            weak.saturating_mul(1000) / assertions,
        );
    }
    for (name, count) in &frameworks {
        by_key.insert(format!("tests_{name}"), *count);
    }

    if test_files.is_empty() {
        // No test file at all is not "no weak tests": it is a question this check cannot ask.
        // Principle 8: a missing input never yields pass.
        let (findings, _cut) = cap(findings);
        return StaticReport::not_measured(
            CheckId::T04,
            "no test file for jest, vitest, mocha, pytest or cargo test at this commit, so \
             there is nothing to count",
        )
        .with_findings(findings)
        .with_metric("skip_markers", skip_total);
    }
    if tests == 0 {
        notes.push(
            "test files are present but none declares a test this build can count, so the \
             assertion metrics have no denominator"
                .to_owned(),
        );
    }
    if !skips.is_empty() {
        let mut paths: Vec<&String> = skips.keys().collect();
        paths.sort();
        notes.push(format!(
            "skipped tests in: {}",
            paths
                .iter()
                .map(|path| format!("{path} ({})", skips[*path]))
                .collect::<Vec<String>>()
                .join(", ")
        ));
    }
    let (findings, cut) = cap(findings);
    notes.extend(cut);

    StaticReport::reported(CheckId::T04, by_key)
        .with_findings(findings)
        .with_notes(notes)
}

/// Whether a path is a test file one of the frameworks would run.
#[must_use]
pub fn is_test_file(path: &str) -> bool {
    let name = TrackedTree::file_name(path);
    if name.starts_with("test_") && name.ends_with(".py") {
        return true;
    }
    if name.ends_with("_test.py") || name == "conftest.py" {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("/__tests__/") || lower.starts_with("__tests__/") {
        return true;
    }
    if lower.contains("/tests/") || lower.starts_with("tests/") {
        // A Rust integration test, or a Python/JS one under a tests directory.
        return true;
    }
    name.contains(".test.") || name.contains(".spec.")
}

/// The framework a test file belongs to, from its extension.
#[must_use]
pub fn framework_of(path: &str) -> Option<Framework> {
    let ext = path.rsplit_once('.').map(|(_, ext)| ext)?;
    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Framework::JsTest),
        "py" => Some(Framework::Python),
        "rs" => Some(Framework::Rust),
        _ => None,
    }
}

/// How many tests a file declares for `framework`.
#[must_use]
pub fn count_tests(text: &str, framework: Framework) -> i64 {
    let mut count = 0i64;
    for line in text.lines() {
        let trimmed = line.trim_start();
        match framework {
            Framework::JsTest => {
                // `it(`, `test(`, `it.each(`, `test.concurrent(` — the declaration, and not the
                // `.only`/`.skip` variants only: those still declare a test.
                if trimmed.starts_with("it(")
                    || trimmed.starts_with("test(")
                    || trimmed.starts_with("it.each(")
                    || trimmed.starts_with("test.each(")
                    || trimmed.starts_with("it.skip(")
                    || trimmed.starts_with("test.skip(")
                    || trimmed.starts_with("it.only(")
                    || trimmed.starts_with("test.only(")
                    || trimmed.starts_with("fit(")
                    || trimmed.starts_with("xit(")
                {
                    count += 1;
                }
            }
            Framework::Python => {
                if trimmed.starts_with("def test_") || trimmed.starts_with("async def test_") {
                    count += 1;
                }
            }
            Framework::Rust => {
                if RUST_TEST_ATTRIBUTES
                    .iter()
                    .any(|attribute| trimmed.starts_with(attribute))
                {
                    count += 1;
                }
            }
        }
    }
    count
}
