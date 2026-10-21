//! V01 Declared provenance (M1-T11).
//!
//! The plan's rule:
//!
//! > Read what tools sign before guessing anything. […] All of these can be turned off or
//! > stripped, so their absence proves nothing. Map them onto today's lines with blame.
//!
//! That last sentence is the whole design. Counting matching commits would overstate a tool's
//! share: a commit that touched one line and a commit that wrote a thousand files count once
//! each. Counting **blamed lines** answers the question the panel asks — what share of the code
//! that exists today was written under a declaration — because blame already says which
//! commit owns each line.
//!
//! Four attributes are read off a commit, in this order, and the first that answers wins:
//!
//! 1. the author name (`Aider`'s `(aider)` tag, a bot account);
//! 2. a trailer, either a named key or any key;
//! 3. the whole message, for a signature that git does not treat as a trailer, such as
//!    Claude Code's `claude.ai/code/session_…` link in the body;
//! 4. the PR head-ref prefix, when the forge data carries the merge commit's branch.
//!
//! The model is a separate axis. Aider writes `aider (gpt-4o)`; Claude Code's trailer names
//! the session's model; Codex's trailer is static and names nothing, which is a fact the panel
//! shows by leaving the model map with fewer entries than the tool map.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::report::Ratio;
use substance_core::model::{CheckVerdict, Target, Verdict};
use substance_core::provenance::{DeclaredMarker, DeclaredRules, MarkerWhere};

use crate::git::{Commit, CommitIdentity, is_bot_commit};
use crate::signals::HistoryInput;
use crate::signals::common::is_test_path;

/// The `parts` key for the trailer-and-author half of V01.
pub const DECLARED_PART: &str = "declared";

/// The `parts` key for the PR-head-ref half.
///
/// Separate because it needs the forge: a run without a forge client still reads every commit
/// trailer, so the check measured something and the missing half is named rather than implied.
pub const BRANCH_PART: &str = "branches";

/// What one commit declares about how it was made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Declaration {
    /// The tools that signed it, sorted and deduplicated.
    pub tools: Vec<String>,
    /// The models that signed it, sorted and deduplicated.
    pub models: Vec<String>,
}

impl Declaration {
    /// Whether the commit carries any declaration at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.models.is_empty()
    }
}

/// Reads what one commit declares, from its author, its trailers and its message.
///
/// Pure: it takes the commit and the rules and returns the declaration, so the same commit
/// always produces the same answer. `head_ref` is the PR branch that introduced it, when the
/// forge said which one.
#[must_use]
pub fn declaration_of(
    commit: &Commit,
    rules: &DeclaredRules,
    head_ref: Option<&str>,
) -> Declaration {
    let mut tools: Vec<String> = Vec::new();
    let mut models: Vec<String> = Vec::new();
    let message = format!("{}\n{}", commit.subject, commit.body);
    for marker in &rules.markers {
        match marker.where_ {
            MarkerWhere::Author => {
                if author_matches(marker, &commit.author) {
                    tools.push(marker.tool.clone());
                    if marker.model_in_parens
                        && let Some(model) = model_in_parens(&commit.author.name)
                    {
                        models.push(model);
                    }
                }
            }
            MarkerWhere::Trailer => {
                for (key, value) in &commit.trailers {
                    if !marker.key.is_empty() && !key.eq_ignore_ascii_case(&marker.key) {
                        continue;
                    }
                    if !contains_any(marker, value) {
                        continue;
                    }
                    tools.push(marker.tool.clone());
                    if marker.model_in_parens
                        && let Some(model) = model_in_parens(value)
                    {
                        models.push(model);
                    }
                }
            }
            MarkerWhere::Message => {
                if contains_any(marker, &message) {
                    tools.push(marker.tool.clone());
                }
            }
        }
    }
    // A bot author is a declaration of its own, and the plan lists bots beside the agents.
    if is_bot_commit(&commit.author) {
        tools.push("bot".to_owned());
    }
    if let Some(reference) = head_ref {
        let lower = reference.to_ascii_lowercase();
        for branch in &rules.branches {
            if lower.starts_with(&branch.prefix) {
                tools.push(branch.tool.clone());
            }
        }
    }
    tools.sort();
    tools.dedup();
    models.sort();
    models.dedup();
    Declaration { tools, models }
}

/// Whether an author marker matches an identity.
fn author_matches(marker: &DeclaredMarker, author: &CommitIdentity) -> bool {
    if marker.contains.is_empty() {
        return false;
    }
    let name = author.name.to_ascii_lowercase();
    let email = author.email.to_ascii_lowercase();
    marker
        .contains
        .iter()
        .any(|needle| name.contains(needle.as_str()) || email.contains(needle.as_str()))
}

/// Whether any of the marker's substrings appears in `text`, case-insensitively.
///
/// An empty `contains` matches on presence alone, which is how Codex's static trailer is read:
/// the key is the signal, whatever the value says.
fn contains_any(marker: &DeclaredMarker, text: &str) -> bool {
    if marker.contains.is_empty() {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    marker
        .contains
        .iter()
        .any(|needle| lower.contains(needle.as_str()))
}

/// Reads a model from a parenthesized group: `aider (gpt-4o)` names `gpt-4o`.
///
/// The group does not have to end the string, because Aider's own trailer puts the address
/// after it: `aider (gpt-4o) <aider@example.com>`. The first balanced group wins, and a name
/// with no parentheses names no model — the honest answer for a tool whose convention this
/// build does not know.
///
/// A group that is empty or that looks like an email is skipped: `(noreply@example.com)` is an
/// address, not a model, and reporting it as one would put a person's mailbox where the panel
/// promises a model name.
fn model_in_parens(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let after = rest.get(open + 1..)?;
        let close = after.find(')')?;
        let inner = after.get(..close)?.trim();
        if !inner.is_empty() && !inner.contains('@') {
            return Some(inner.to_owned());
        }
        rest = after.get(close + 1..)?;
    }
    None
}

/// The line and commit shares one pass over the blame produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredShares {
    /// Blamed lines per tool.
    pub tool_lines: BTreeMap<String, u64>,
    /// Blamed lines per model.
    pub model_lines: BTreeMap<String, u64>,
    /// Blamed lines no declaration covers.
    pub undeclared_lines: u64,
    /// Every blamed line.
    pub total_lines: u64,
    /// Declared commits per tool.
    pub commits: BTreeMap<String, u64>,
    /// Declared commits merged with no review, per tool.
    pub merged_unreviewed: BTreeMap<String, u64>,
    /// Declared commits that changed no test file, per tool.
    pub without_test_change: BTreeMap<String, u64>,
}

impl DeclaredShares {
    /// A share in permille, rounding down, or zero for an empty denominator.
    ///
    /// `u64` to `Ratio` (`u32`): the value is 0–1000, so the cast cannot truncate; the
    /// `u32::try_from` is still used so a future change to the arithmetic would be caught by
    /// the compiler's own type rather than by a silent wrap.
    fn share(part: u64, whole: u64) -> Ratio {
        if whole == 0 {
            return 0;
        }
        let permille = part.saturating_mul(1000) / whole;
        u32::try_from(permille).unwrap_or(u32::MAX)
    }
}

/// Everything V01 produced: the verdict, its line shares, and the notes a reader needs.
#[derive(Debug, Clone)]
pub struct Declared {
    /// The verdict, its metrics and its per-part verdicts.
    pub verdict: CheckVerdict,
    /// The line and commit shares the panel is built from.
    pub shares: DeclaredShares,
    /// Sentences the panel shows beside the numbers, sorted.
    pub notes: Vec<String>,
}

/// Judges V01.
///
/// The verdict is always `Reported`: V01 has no threshold and, per ADR-0012, cannot have one.
/// The reason a reader can still trust it is that every input it could not get is a named
/// part or a note, never a silent zero.
#[must_use]
pub fn declared(input: &HistoryInput, rules: &DeclaredRules) -> Declared {
    let mut notes: Vec<String> = Vec::new();
    let by_sha: BTreeMap<&str, &Commit> = input
        .commits
        .iter()
        .map(|commit| (commit.sha.as_str(), commit))
        .collect();
    // The merge commit a PR produced, to its head ref. A declaration on a rewritten commit
    // survives here even when the trailer was stripped, because the branch still names the
    // agent.
    let mut head_refs: BTreeMap<&str, &str> = BTreeMap::new();
    if let Some(forge) = &input.forge {
        for pull in &forge.pulls {
            if let (Some(sha), Some(reference)) = (&pull.merge_commit_sha, &pull.head_ref) {
                head_refs.insert(sha.as_str(), reference.as_str());
            }
        }
    } else if let Some(reason) = &input.forge_missing {
        notes.push(format!(
            "no forge data, so PR head refs named no tool: {reason}"
        ));
    }

    // One declaration per commit, computed once: the blame visits commits in file order, so
    // recomputing per line would read the same trailer thousands of times.
    let mut declarations: BTreeMap<&str, Declaration> = BTreeMap::new();
    for commit in &input.commits {
        let declaration =
            declaration_of(commit, rules, head_refs.get(commit.sha.as_str()).copied());
        if !declaration.is_empty() {
            declarations.insert(commit.sha.as_str(), declaration);
        }
    }

    let mut shares = DeclaredShares::default();
    for (_, line) in input.blame.lines() {
        shares.total_lines += 1;
        let declaration = declarations.get(line.commit.as_str());
        match declaration {
            Some(declaration) if !declaration.tools.is_empty() => {
                for tool in &declaration.tools {
                    *shares.tool_lines.entry(tool.clone()).or_default() += 1;
                }
                for model in &declaration.models {
                    *shares.model_lines.entry(model.clone()).or_default() += 1;
                }
            }
            _ => shares.undeclared_lines += 1,
        }
    }

    // The per-commit counts the plan's "risk" line asks for: how many declared commits were
    // merged with no review, and how many changed no test.
    let mut test_rules_note = false;
    for (sha, declaration) in &declarations {
        let Some(commit) = by_sha.get(sha) else {
            continue;
        };
        for tool in &declaration.tools {
            *shares.commits.entry(tool.clone()).or_default() += 1;
        }
        let changed_test = commit
            .files
            .iter()
            .any(|file| is_test_path(&file.path, &input.test_paths));
        if !changed_test {
            for tool in &declaration.tools {
                *shares.without_test_change.entry(tool.clone()).or_default() += 1;
            }
            test_rules_note = true;
        }
        if !reviewed(input, commit) {
            for tool in &declaration.tools {
                *shares.merged_unreviewed.entry(tool.clone()).or_default() += 1;
            }
        }
    }

    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("lines_blamed".to_owned(), as_i64(shares.total_lines));
    metrics.insert(
        "lines_undeclared".to_owned(),
        as_i64(shares.undeclared_lines),
    );
    for (tool, lines) in &shares.tool_lines {
        metrics.insert(format!("lines_{tool}"), as_i64(*lines));
    }
    for (tool, commits) in &shares.commits {
        metrics.insert(format!("commits_{tool}"), as_i64(*commits));
    }

    if input.blame.coverage.truncated {
        notes.push(format!(
            "the blame covers {} of {} authored files, so the shares describe that sample",
            input.blame.coverage.files_blamed, input.blame.coverage.files_total
        ));
    }
    if shares.total_lines == 0 {
        notes.push(
            "no line could be blamed, so no share can be computed; that is not evidence of \
             undeclared code"
                .to_owned(),
        );
    }
    if test_rules_note && input.test_paths.is_empty() {
        notes.push(
            "rules/history/test-paths.txt is empty, so every commit counts as changing no test"
                .to_owned(),
        );
    }
    if head_refs.is_empty() {
        notes.push(
            "no merged pull request head ref was readable, so no tool was named by a branch"
                .to_owned(),
        );
    }

    let mut parts: BTreeMap<String, Verdict> = BTreeMap::new();
    parts.insert(DECLARED_PART.to_owned(), Verdict::Reported);
    parts.insert(
        BRANCH_PART.to_owned(),
        if input.forge.is_some() {
            Verdict::Reported
        } else {
            Verdict::NotMeasured {
                reason: input
                    .forge_missing
                    .clone()
                    .unwrap_or_else(|| "no forge client for this origin".to_owned()),
            }
        },
    );
    let verdict = CheckVerdict {
        check_id: CheckId::V01,
        target: Target::Repo,
        verdict: Verdict::Reported,
        finding_ids: Vec::new(),
        metrics,
        parts,
    };
    Declared {
        verdict,
        shares,
        notes,
    }
}

/// Whether a commit's changes were reviewed, as far as the forge data can say.
///
/// A commit with no matching pull request is not evidence of an unreviewed merge; nothing
/// recorded that it was merged through one. It is counted as unreviewed only when the forge
/// shows the merge and no review: that is the case the panel's risk line is about.
fn reviewed(input: &HistoryInput, commit: &Commit) -> bool {
    let Some(forge) = &input.forge else {
        // Without forge data every merge is unknown, and counting unknowns as unreviewed would
        // inflate the risk line. The note in the caller says the half was not read.
        return true;
    };
    let pull = forge.pulls.iter().find(|pull| {
        pull.merge_commit_sha.as_deref() == Some(commit.sha.as_str())
            || pull.head_sha.as_deref() == Some(commit.sha.as_str())
    });
    let Some(pull) = pull else {
        return true;
    };
    forge
        .reviews
        .iter()
        .find(|(number, _)| *number == pull.number)
        .is_some_and(|(_, reviews)| !reviews.is_empty())
}

/// `u64` as the metric type, saturating rather than wrapping.
fn as_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The shares as permille, for the panel.
#[must_use]
pub fn permille(
    shares: &DeclaredShares,
) -> (BTreeMap<String, Ratio>, BTreeMap<String, Ratio>, Ratio) {
    let mut tools: BTreeMap<String, Ratio> = BTreeMap::new();
    for (tool, lines) in &shares.tool_lines {
        tools.insert(
            tool.clone(),
            DeclaredShares::share(*lines, shares.total_lines),
        );
    }
    let mut models: BTreeMap<String, Ratio> = BTreeMap::new();
    for (model, lines) in &shares.model_lines {
        models.insert(
            model.clone(),
            DeclaredShares::share(*lines, shares.total_lines),
        );
    }
    (
        tools,
        models,
        DeclaredShares::share(shares.undeclared_lines, shares.total_lines),
    )
}
