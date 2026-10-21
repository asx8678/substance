//! O10 Fake CI (M1-T09).
//!
//! Formula (action plan, O10):
//!
//! > Parse `.github/workflows/*.yml` with a maintained YAML crate. Flag test steps with
//! > `|| true`, `continue-on-error: true`, `--passWithNoTests`, or a trailing `exit 0`, and a
//! > package.json `test` script that exits 0 without running tests
//!
//! Five ways a workflow can look like it tests something and cannot fail:
//!
//! | Shape | What it does |
//! | --- | --- |
//! | `run: npm test \|\| true` | swallows the exit status |
//! | `continue-on-error: true` | tells the runner to ignore it |
//! | `--passWithNoTests` | passes when the suite is empty |
//! | a trailing `exit 0` | overwrites the status with success |
//! | `"test": "echo ok"` | a test script that never runs a runner |
//!
//! The last one is the subtle one and the plan is explicit about it: npm's own default
//! `"test": "echo \"Error: no test specified\" && exit 1"` is **not** fake — it fails, which is
//! honest. What is fake is a script that exits 0 without running anything: `echo ok`, `exit 0`,
//! `true`, or a script with no runner in it at all. So the test is not "does it look like a
//! stub" but "can this command fail?".
//!
//! The YAML is parsed with the crate `substance-adapters` already depends on (`serde_yaml_ng`),
//! rather than by matching text: a workflow is a nested structure, and a line-oriented scan
//! would miss `run: |` blocks, flow mappings and quoted scalars — the three places a workflow
//! actually hides its commands.

use std::collections::BTreeMap;

use serde_yaml_ng::Value;
use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// The workflow directory GitHub reads.
pub const WORKFLOW_DIR: &str = ".github/workflows/";

/// Runners that mean "this script runs tests".
///
/// A script containing one of these *and* exiting zero may be fine (a runner that reports zero
/// tests as success is still a runner), so this list is used to answer `--passWithNoTests` and
/// the empty-script question, not to decide the whole check.
pub const TEST_RUNNERS: &[&str] = &[
    "jest",
    "vitest",
    "mocha",
    "ava",
    "tap",
    "uvu",
    "node --test",
    "pytest",
    "cargo test",
    "go test",
    "bun test",
    "deno test",
    "playwright",
    "cypress",
];

/// Judges O10.
#[must_use]
pub async fn fake_ci(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let workflows: Vec<String> = tree
        .matching(|path| path.starts_with(WORKFLOW_DIR))
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut findings: Vec<Finding> = Vec::new();
    let mut by_shape: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();

    for path in &workflows {
        let Some(text) = tree.text(path).await else {
            continue;
        };
        let parsed: Value = match serde_yaml_ng::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                notes.push(format!("{path} could not be parsed as YAML: {error}"));
                continue;
            }
        };
        for (job, step, line) in steps(&parsed) {
            if let Some(shape) = swallowed_shape(&step) {
                *by_shape.entry(shape.to_owned()).or_insert(0) += 1;
                findings.push(file_finding::at(
                    run_id,
                    CheckId::O10,
                    shape,
                    &format!("workflow step in {path} ({job}) cannot fail: {shape}"),
                    Location::file(path),
                ));
                let _ = line;
            }
        }
    }

    findings.extend(package_test_script(run_id, tree, &mut by_shape).await);

    let mut metrics: BTreeMap<String, i64> = by_shape.clone();
    metrics.insert("workflows".to_owned(), as_i64(workflows.len()));
    metrics.insert("files".to_owned(), as_i64(findings.len()));
    let shapes: i64 = by_shape.values().sum();
    metrics.insert("shapes".to_owned(), shapes);

    if workflows.is_empty() {
        // No workflow at all is not "no fake CI": it is a question this check cannot ask. A
        // package.json test script may still be fake, so the check continues, and the note says
        // the workflow half had nothing to look at.
        notes.push(
            "no .github/workflows file at this commit, so only the package.json test \
                    script was read"
                .to_owned(),
        );
    }
    let (findings, cut) = cap(findings);
    notes.extend(cut);

    let mut report = StaticReport::reported(CheckId::O10, metrics)
        .with_findings(findings)
        .with_notes(notes);
    if shapes == 0 {
        report.verdict.verdict = substance_core::model::Verdict::Pass;
    } else {
        report.verdict.verdict = substance_core::model::Verdict::Fail;
    }
    report
}

/// The shapes that make a step unable to fail.
///
/// Ordered: the first one that explains the step is the one reported, so a step with both
/// `continue-on-error` and `|| true` is reported once, under the more explicit flag.
#[must_use]
pub fn swallowed_shape(step: &Value) -> Option<&'static str> {
    if step
        .get("continue-on-error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("continue-on-error");
    }
    let run = step.get("run").and_then(Value::as_str).unwrap_or("");
    if run.contains("|| true") || run.contains("||true") || run.contains("|| :") {
        return Some("or-true");
    }
    if run.contains("--passWithNoTests") {
        return Some("pass-with-no-tests");
    }
    if trailing_exit_zero(run) {
        return Some("trailing-exit-0");
    }
    None
}

/// Whether a shell script ends by forcing success.
///
/// Only the last non-empty line counts: `npm test; exit 0` cannot fail, while `exit 0` in the
/// middle of a script that then runs the tests does not affect the status.
#[must_use]
pub fn trailing_exit_zero(script: &str) -> bool {
    let last = script
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with('#'));
    matches!(last, Some("exit 0") | Some("exit 0;"))
}

/// Every step in a workflow, with its job name.
fn steps(workflow: &Value) -> Vec<(String, Value, u32)> {
    let mut out: Vec<(String, Value, u32)> = Vec::new();
    let Some(jobs) = workflow.get("jobs").and_then(Value::as_mapping) else {
        return out;
    };
    for (job, body) in jobs {
        let job_name = job.as_str().unwrap_or("job").to_owned();
        let Some(list) = body.get("steps").and_then(Value::as_sequence) else {
            continue;
        };
        for step in list {
            out.push((job_name.clone(), step.clone(), 0));
        }
    }
    out
}

/// A package.json `test` script that exits 0 without running anything.
async fn package_test_script(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    by_shape: &mut BTreeMap<String, i64>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let paths: Vec<String> = tree
        .matching(|path| path == "package.json")
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in paths {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        let parsed: Value = match serde_yaml_ng::from_str(&text) {
            Ok(value) => value,
            Err(_) => match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) => continue,
            },
        };
        let Some(script) = parsed
            .get("scripts")
            .and_then(|scripts| scripts.get("test"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !fake_test_script(script) {
            continue;
        }
        *by_shape.entry("fake-test-script".to_owned()).or_insert(0) += 1;
        findings.push(file_finding::at(
            run_id,
            CheckId::O10,
            "fake-test-script",
            &format!("the test script in {path} exits 0 without running a test: {script}"),
            Location::file(&path),
        ));
    }
    findings
}

/// Whether a `test` script succeeds without running anything.
///
/// The plan's distinction, spelled out: `echo "Error: no test specified" && exit 1` is npm's
/// default and it fails, so it is honest. A script is fake when it contains no test runner and
/// its outcome is success — `echo ok`, `true`, `exit 0`, or an empty string.
#[must_use]
pub fn fake_test_script(script: &str) -> bool {
    let trimmed = script.trim();
    if trimmed.is_empty() {
        return true;
    }
    if TEST_RUNNERS.iter().any(|runner| trimmed.contains(runner)) {
        return false;
    }
    // A script that fails is not fake, however little it does.
    if trimmed.contains("exit 1") || trimmed.contains("&& false") {
        return false;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    matches!(
        words.first(),
        Some(&"echo") | Some(&"true") | Some(&"exit") | Some(&":")
    ) || trimmed == "exit 0"
        || trimmed.starts_with("echo ")
}
