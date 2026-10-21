//! O05 Non-root, the static half (M2-T05).
//!
//! Formula (action plan, O05):
//!
//! > No container, so a static check (ADR-0020): fail when the final stage of a repo Dockerfile
//! > has no `USER`, or runs as `root` or `0`. No Dockerfile -> not_measured.
//!
//! ## Why a static check, and what it therefore cannot say
//!
//! The plan runs no containers (ADR-0003, ADR-0020), so nothing here can start an image and ask
//! which uid it ended up with. What a repository's Dockerfile *declares* is still a fact about the
//! repository, and it is the fact that decides the answer in the overwhelming majority of cases: an
//! image whose final stage never names a `USER` runs as root, because that is the engine's default.
//! So the check reads the final stage and judges the declaration.
//!
//! What it cannot see is a base image that sets a non-root user itself, or an entrypoint that drops
//! privileges. Both make the container non-root while the declaration says nothing -- so the
//! finding's message says "the final stage names no USER" rather than "the container runs as root",
//! which is the claim the evidence supports.
//!
//! ## Why only the final stage
//!
//! A multi-stage build's earlier stages compile; only the last one ships. A `USER builder` in stage
//! one says nothing about the image, and requiring one there would fail every ordinary multi-stage
//! frontend build for a reason that has no security meaning.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Verdict;

use crate::checks::s02::as_i64;
use crate::checks::verdict::StaticReport;
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// The Dockerfile names this build reads, at the repository root and at the conventional depth.
///
/// Ordered: the first name present is the one judged, because judging several would report the same
/// problem twice and a reader would think they had two.
pub const DOCKERFILES: &[&str] = &[
    "Dockerfile",
    "dockerfile",
    "Dockerfile.prod",
    "Dockerfile.production",
    "docker/Dockerfile",
    "docker/Dockerfile.prod",
];

/// The `USER` values that mean root.
///
/// `0` and `root` are the same account; `root:root` is the same account with its group spelled out.
/// A numeric uid that is not 0 is somebody else and passes.
pub const ROOT_USERS: &[&str] = &["root", "0"];

/// One `USER` instruction, and where it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDirective {
    /// The value as written, after the keyword and trimmed.
    pub value: String,
    /// The 1-based line it is on, so a finding can point at it.
    pub line: u32,
}

/// What the final stage's `USER` situation is.
///
/// A separate type from the report because it is the whole of the parsing, and a caller reading the
/// code should be able to see the parse and the judgement as two things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalStage {
    /// The final stage names a `USER` that is not root.
    NonRoot(UserDirective),
    /// The final stage names root explicitly.
    Root(UserDirective),
    /// The final stage names no `USER` at all, so the engine's default (root) applies.
    Absent {
        /// The stage's base image, for the message.
        base: String,
    },
    /// There is no `FROM` at all, so there is no image to speak of.
    NoStage,
}

/// Judges O05.
///
/// Static, so it runs in triage and needs no `--allow-exec`: a Dockerfile is a file in the commit.
#[must_use]
pub async fn non_root(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let Some(path) = DOCKERFILES
        .iter()
        .find(|name| tree.has(name))
        .map(|name| (*name).to_owned())
    else {
        return StaticReport::not_measured(
            CheckId::O05,
            "no Dockerfile at this commit, so there is no image user to check (ADR-0020: \
             substance runs no containers)",
        );
    };
    let Some(text) = tree.text(&path).await else {
        return StaticReport::not_measured(
            CheckId::O05,
            format!("{path} could not be read, so the image user is unknown"),
        );
    };

    let stages = stages(&text);
    let outcome = final_stage(&text);
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("stages".to_owned(), as_i64(stages.len()));

    let mut notes: Vec<String> = Vec::new();
    if stages.len() > 1 {
        notes.push(format!(
            "{path} has {} stages; only the final one ships, so only its USER is judged",
            stages.len()
        ));
    }

    let (verdict, findings) = match &outcome {
        FinalStage::NonRoot(user) => {
            notes.push(format!("{path}:{} sets USER {}", user.line, user.value));
            metrics.insert("root_user".to_owned(), 0);
            (Verdict::Pass, Vec::new())
        }
        FinalStage::Root(user) => {
            metrics.insert("root_user".to_owned(), 1);
            (
                Verdict::Fail,
                vec![file_finding::at(
                    run_id,
                    CheckId::O05,
                    "explicit-root-user",
                    &format!(
                        "{path} runs its final stage as {}, which is root",
                        user.value
                    ),
                    Location::lines(&path, user.line, user.line),
                )],
            )
        }
        FinalStage::Absent { base } => {
            metrics.insert("root_user".to_owned(), 1);
            (
                Verdict::Fail,
                vec![file_finding::at(
                    run_id,
                    CheckId::O05,
                    "no-user-directive",
                    &format!(
                        "{path} names no USER in its final stage (from {base}), so the image runs \
                         as the engine's default user, which is root"
                    ),
                    Location::file(&path),
                )],
            )
        }
        FinalStage::NoStage => (
            Verdict::NotMeasured {
                reason: format!("{path} has no FROM instruction, so there is no image to judge"),
            },
            Vec::new(),
        ),
    };

    let mut report = StaticReport::reported(CheckId::O05, metrics)
        .with_findings(findings)
        .with_notes(notes);
    report.verdict.verdict = verdict;
    report
}

/// The `FROM` instructions of a Dockerfile, in order, with comments and continuations folded.
///
/// Deliberately not a Dockerfile parser: only the instructions this check needs are read, and a `#`
/// comment or a trailing backslash continuation is handled because both appear in real Dockerfiles
/// between a `FROM` and the `USER` that follows it.
#[must_use]
pub fn stages(text: &str) -> Vec<String> {
    logical_lines(text)
        .into_iter()
        .filter_map(|(_, instruction)| {
            let lower = instruction.to_ascii_lowercase();
            let rest = lower
                .strip_prefix("from ")
                .map(|_| instruction[5..].trim())?;
            // Drop the AS clause and any platform flag: the base image is what the note names.
            let base = rest.split_whitespace().next().unwrap_or_default();
            Some(base.to_owned())
        })
        .collect()
}

/// The final stage's user situation.
#[must_use]
pub fn final_stage(text: &str) -> FinalStage {
    let mut stage: Option<usize> = None;
    let mut users: Vec<(usize, UserDirective)> = Vec::new();
    for (line, instruction) in logical_lines(text) {
        let lower = instruction.to_ascii_lowercase();
        if lower == "from" || lower.starts_with("from ") {
            stage = Some(stage.map_or(0, |index| index.saturating_add(1)));
            continue;
        }
        let Some(index) = stage else {
            continue;
        };
        let Some(rest) = lower
            .strip_prefix("user ")
            .map(|_| instruction[5..].trim().to_owned())
        else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        users.push((index, UserDirective { value: rest, line }));
    }

    let bases = stages(text);
    let Some(last) = bases.len().checked_sub(1) else {
        return FinalStage::NoStage;
    };
    // The last USER in the final stage wins: a Dockerfile may set one earlier and then override it,
    // and the instruction in effect when the image is built is the last one.
    let final_user = users
        .iter()
        .rev()
        .find(|(index, _)| *index == last)
        .map(|(_, directive)| directive.clone());
    match final_user {
        Some(directive) if is_root(&directive.value) => FinalStage::Root(directive),
        Some(directive) => FinalStage::NonRoot(directive),
        None => FinalStage::Absent {
            base: bases.last().cloned().unwrap_or_default(),
        },
    }
}

/// Whether a `USER` value names root.
///
/// Case-insensitive, because `USER Root` is the same account; the user half of `name:group` decides,
/// so `root:app` is still root; and a numeric uid is compared as a number, so `USER 00` is root too.
#[must_use]
pub fn is_root(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    let user = lowered.split(':').next().unwrap_or(&lowered).trim();
    if ROOT_USERS.contains(&user) {
        return true;
    }
    user.parse::<u32>().map(|uid| uid == 0).unwrap_or(false)
}

/// Every instruction in the file, as (1-based line, instruction text), with comments dropped and
/// continuations joined.
///
/// The line reported for a joined instruction is the line it *starts* on, which is where a reader
/// would put their cursor.
#[must_use]
pub fn logical_lines(text: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut pending: Option<(u32, String)> = None;
    for (index, raw) in text.lines().enumerate() {
        let number = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        let trimmed = raw.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let continues = trimmed.ends_with('\\');
        let body = if continues {
            trimmed.trim_end_matches('\\').trim_end()
        } else {
            trimmed
        };
        match pending.take() {
            Some((start, mut accumulated)) => {
                accumulated.push(' ');
                accumulated.push_str(body);
                if continues {
                    pending = Some((start, accumulated));
                } else {
                    out.push((start, accumulated));
                }
            }
            None => {
                if continues {
                    pending = Some((number, body.to_owned()));
                } else if !body.is_empty() {
                    out.push((number, body.to_owned()));
                }
            }
        }
    }
    // A file ending mid-continuation still has that instruction; dropping it would silently lose a
    // `USER` written on the last line with a trailing backslash.
    if let Some((start, accumulated)) = pending
        && !accumulated.trim().is_empty()
    {
        out.push((start, accumulated));
    }
    out
}
