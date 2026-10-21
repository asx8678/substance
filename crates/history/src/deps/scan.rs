//! Running the external dependency scanners (M1-T10).
//!
//! P01 reads vulnerability reports, P05's third half finds unused dependencies and P06's second
//! half reads licence reports. All three are external programs over the same repo, so the
//! plumbing is here once: which binary, which arguments, which timeout, and what to say when
//! the binary is not installed.
//!
//! Three rules come from the plan and from `tools.toml`, and each has a reason:
//!
//! - **A missing binary is `tool missing: <name>`.** The plan spells the sentence, and it is the
//!   one [`substance_runner::tool::ToolError::Missing`] already produces. It is a
//!   `not_measured` reason, never an empty finding list: a scanner that did not run found
//!   nothing, which is not the same as finding nothing wrong.
//!
//! - **A tool that executes repo code needs a permit.** `tools.toml` marks `pip-audit`, `knip`
//!   and any other tool that loads the repository's own configuration with
//!   `exec_repo_code = true`; the runner refuses to spawn it without an
//!   [`substance_runner::permit::ExecPermit`], and this module reports that refusal as the
//!   reason it did not run. Only a run started with `--allow-exec` has such a permit.
//!
//! - **A `network = true` tool is replayed, not executed.** Under `--snapshot-from` the runner
//!   substitutes the saved output for the saved command, so a replayed run asks its registries
//!   nothing.
//!
//! A scanner's own exit code is not an error: `osv-scanner` exits 1 when it finds a
//! vulnerability, and that is exactly the run whose output matters. What counts is whether the
//! output parses into the shape this build reads; anything else is `not_measured` with the
//! reason, because a nonzero exit with unreadable output means the tool failed rather than
//! reported.

use std::ffi::OsStr;
use std::time::Duration;

use substance_core::tools::ToolSpec;
use substance_runner::permit::ExecPermit;
use substance_runner::tool::{ToolError, ToolOutput, ToolRunner};

/// How long a scanner may run before it is killed.
///
/// Two minutes: these tools read a lockfile and ask a service, or walk a tree. A longer run is
/// a hung network or a pathologically large repository, and a run that never returns would
/// hold the evaluation open.
pub const SCANNER_TIMEOUT_SECONDS: u64 = 120;

/// What one scanner produced, or why it produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScannerRun {
    /// The tool ran. `output` is its captured streams, whatever its exit code.
    ///
    /// Boxed: a captured run carries two strings and an index entry, and every scanner's
    /// absence would otherwise reserve that much space in each of the check's lists.
    Ran {
        /// The captured run.
        output: Box<ToolOutput>,
        /// The exact command line, for the report.
        command: String,
    },
    /// The tool could not be run.
    Unavailable {
        /// The sentence a `not_measured` verdict carries.
        reason: String,
    },
}

impl ScannerRun {
    /// The absence, with a reason.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    /// The run, when there was one.
    #[must_use]
    pub fn output(&self) -> Option<&ToolOutput> {
        match self {
            Self::Ran { output, .. } => Some(output),
            Self::Unavailable { .. } => None,
        }
    }

    /// Whether the tool ran successfully and printed something at all.
    #[must_use]
    pub fn printed(&self) -> bool {
        self.output()
            .is_some_and(|output| !output.stdout.trim().is_empty())
    }
}

/// What one scanner is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScannerTarget<'a> {
    /// The name in `tools.toml`.
    pub name: &'a str,
    /// The arguments, in order.
    pub args: &'a [&'a str],
}

/// Runs one scanner from `tools.toml` in `root`.
///
/// `permit` is the run's `ExecPermit` when `--allow-exec` was given. A tool that needs one and
/// does not get it answers [`ScannerRun::Unavailable`] with the runner's own sentence, which
/// names the permission rather than pretending the tool is absent.
pub async fn run_scanner(
    runner: &ToolRunner<'_>,
    spec: &ToolSpec,
    name: &str,
    args: &[&str],
    root: &std::path::Path,
    permit: Option<&ExecPermit>,
) -> ScannerRun {
    let os_args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let command = format!("{name} {}", args.join(" ")).trim_end().to_owned();
    match runner
        .run(
            name,
            spec,
            &os_args,
            root,
            Duration::from_secs(SCANNER_TIMEOUT_SECONDS),
            permit,
        )
        .await
    {
        Ok(output) => ScannerRun::Ran {
            output: Box::new(output),
            command,
        },
        Err(error) => ScannerRun::unavailable(reason_for(name, &error)),
    }
}

/// The sentence for one tool failure.
///
/// The plan spells `tool missing: <name>` for an absent binary; the other cases name what
/// happened, because "the scanner is missing" and "the run did not ask permission to run it"
/// call for different actions.
fn reason_for(name: &str, error: &ToolError) -> String {
    match error {
        ToolError::Missing { .. } => format!("tool missing: {name}"),
        ToolError::NeedsExec { .. } => {
            format!("tool not run: {name} executes repository code and needs --allow-exec")
        }
        ToolError::SnapshotMissing { .. } => {
            format!("snapshot missing: this run directory has no saved {name} output")
        }
        other => format!("tool failed: {name}: {other}"),
    }
}

/// The `args` a scanner needs, in the order `tools.toml` documents them.
///
/// Kept as data beside the check that uses it rather than inside the call, so a reader can see
/// the exact arguments a finding came from without following a function call.
pub mod args {
    /// `osv-scanner --format json --lockfile <path>`, once per lockfile.
    pub const OSV_SCANNER: &[&str] = &["--format", "json"];

    /// `osv-scanner --format json scan <dir>`: the recursive form, which reads every lockfile
    /// the repository has in one run. P01 prefers this shape because one process per repository
    /// is cheaper than one per lockfile and asks the same question.
    pub const OSV_SCANNER_RECURSIVE: &[&str] = &["--recursive", "--format", "json"];

    /// `npm audit --json`: the v2 report P01 parses.
    pub const NPM_AUDIT: &[&str] = &["audit", "--json"];

    /// `cargo audit --json`.
    pub const CARGO_AUDIT: &[&str] = &["--json"];

    /// `pip-audit --format json`.
    pub const PIP_AUDIT: &[&str] = &["--format", "json"];

    /// `knip --reporter json`: P05's unused dependencies.
    pub const KNIP: &[&str] = &["--reporter", "json"];

    /// `cargo machete --with-metadata`.
    pub const CARGO_MACHETE: &[&str] = &["--with-metadata"];

    /// `deptry --json-output`.
    pub const DEPTRY: &[&str] = &["--json-output"];

    /// `cargo deny --format json check licenses`.
    pub const CARGO_DENY: &[&str] = &["check", "licenses", "--format", "json"];
}
