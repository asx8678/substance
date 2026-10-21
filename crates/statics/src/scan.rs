//! Scanning one commit's files with the rule pack (M1-T07).
//!
//! The walk is over `git ls-tree` at the run's commit, filtered by the same authored rule
//! the history signals use: a lockfile, a generated file or a vendored tree is not code the
//! project wrote, and a static hint about one is noise. Reading blobs with `git cat-file`
//! rather than from the working tree is what keeps uncommitted edits out of a run: the
//! checks describe the commit, and the commit alone.
//!
//! Three limits bound the work, all of them reported rather than silent:
//!
//! - a file over [`MAX_FILE_BYTES`] is skipped and counted, because a megabyte of source is
//!   a bundle or a generated file whatever its extension says;
//! - a file git cannot produce as a blob (a submodule, a broken object) is skipped and
//!   counted, because one unreadable file must not lose the others;
//! - a file that fails to parse is counted per language and never fails the run: a hint is
//!   not worth stopping a report for.
//!
//! Matches come out sorted by `(path, line, rule id)`, so two runs of one commit produce
//! byte-identical output.

use std::collections::BTreeMap;
use std::path::Path;

use ast_grep_core::AstGrep;
use ast_grep_core::tree_sitter::StrDoc;
use substance_core::check::CheckId;
use substance_core::model::{Evidence, FileRange};
use substance_history::git::{BlobEntry, GitError, GitRepo};

use crate::lang::Lang;
use crate::rulepack::{Rule, RulePack};

/// The largest file the scanner parses, in bytes.
///
/// One megabyte is roughly 25,000 lines of source. Above that a "source file" is a bundle,
/// a data table or a generated client, and parsing it would cost seconds to produce hints
/// about code nobody wrote by hand.
pub const MAX_FILE_BYTES: u64 = 1_000_000;

/// One rule match, before it is turned into a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    /// Repo-relative path of the file.
    pub path: String,
    /// First line of the match, 1-based.
    pub start_line: u32,
    /// Last line of the match, 1-based and inclusive.
    pub end_line: u32,
    /// First column (UTF-8 bytes are irrelevant here: this is a character column, 1-based).
    pub start_column: u32,
    /// Last column, 1-based.
    pub end_column: u32,
    /// The rule's own id.
    pub rule_id: String,
    /// The rule file it came from.
    pub rule_path: String,
    /// The catalog check the rule's metadata names.
    pub check_id: CheckId,
    /// The catalog sub-check the rule's metadata names.
    pub sub_check: String,
    /// The rule's message.
    pub message: String,
}

impl RuleMatch {
    /// The evidence a finding carries for this match: the file and the line range.
    #[must_use]
    pub fn evidence(&self) -> Evidence {
        Evidence::FileRange(FileRange {
            path: self.path.clone(),
            start_line: self.start_line,
            end_line: self.end_line,
        })
    }

    /// The sort key: `(path, start line, rule id)`.
    #[must_use]
    pub fn sort_key(&self) -> (&str, u32, &str) {
        (self.path.as_str(), self.start_line, self.rule_id.as_str())
    }
}

/// What a scan covered, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Files whose language a rule pack reads.
    pub considered: u32,
    /// Files parsed.
    pub scanned: u32,
    /// Files skipped for being over [`MAX_FILE_BYTES`].
    pub skipped_large: u32,
    /// Files skipped because git could not produce them as text.
    ///
    /// The runner decodes a child's stdout lossily, so this is not a UTF-8 test: it means
    /// `git cat-file` failed for the blob — a submodule entry, a broken object — and one
    /// such file must not lose the scan's other findings.
    pub skipped_binary: u32,
    /// Files whose parse failed, by language directory (`ts`, `python`, …).
    pub parse_failures: BTreeMap<String, u32>,
    /// Authored lines across every authored path at the commit, whatever its language.
    ///
    /// The denominator for S02's "per 1,000 authored lines". It counts the whole authored
    /// tree, not just the five parsed languages, because the question is how much escape
    /// hatch a project has per thousand lines *it wrote* — a Rust service with one
    /// `unwrap()` in a handler is a different thing at 200 lines and at 20,000.
    pub authored_lines: u64,
}

/// The result of one scan.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    /// Every match, sorted by `(path, line, rule id)`.
    pub matches: Vec<RuleMatch>,
    /// What was covered.
    pub stats: ScanStats,
}

/// Scans `rev` in `repo` with `pack`.
///
/// # Errors
///
/// Returns [`GitError`] when the tree cannot be listed or a blob cannot be read. A single
/// unreadable file is not an error: it is counted in [`ScanStats`], because one deleted or
/// oddly-encoded file must not lose the other findings.
pub async fn scan(repo: &GitRepo, rev: &str, pack: &RulePack) -> Result<ScanReport, GitError> {
    let entries = repo.sized_blobs(rev).await?;
    // The authored rule is the history signals' own, not an approximation of it: the same
    // git attributes, the same lockfile list, the same generated marker. A path is in the
    // returned set when it counts as authored code at this commit.
    let (authored, _excluded) = repo.authored(rev).await?;
    let mut report = ScanReport::default();
    for entry in entries {
        let Some(lang) = Lang::from_path(Path::new(&entry.path)) else {
            continue;
        };
        let rules = pack.for_lang(lang);
        if rules.is_empty() {
            continue;
        }
        if authored.binary_search(&entry.path).is_err() {
            continue;
        }
        // The authored-line count covers every authored file, not just the parsed ones, so it
        // is taken here before the language-specific skips below.
        report.stats.authored_lines += authored_lines(repo, rev, &entry.path).await;
        report.stats.considered += 1;
        if entry.size > MAX_FILE_BYTES {
            report.stats.skipped_large += 1;
            continue;
        }
        scan_one(repo, rev, &entry, lang, &rules, &mut report).await;
    }
    report
        .matches
        .sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    Ok(report)
}

/// The number of lines in one authored file at the commit.
///
/// A file git cannot produce counts as zero rather than failing the scan: the line count is a
/// denominator, and one unreadable blob should lower it by one file, not lose every finding.
/// The trailing newline does not make an extra line (`"a\n"` is one line, not two).
async fn authored_lines(repo: &GitRepo, rev: &str, path: &str) -> u64 {
    match repo.blob_text(rev, path).await {
        Ok(text) => u64::try_from(text.lines().count()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

/// Reads, parses and matches one file, recording what happened in `report`.
async fn scan_one(
    repo: &GitRepo,
    rev: &str,
    entry: &BlobEntry,
    lang: Lang,
    rules: &[&Rule],
    report: &mut ScanReport,
) {
    let Ok(text) = repo.blob_text(rev, &entry.path).await else {
        // git could not turn this entry into a blob (a submodule, a broken object). Counting
        // it keeps the skip visible instead of quietly scanning one file fewer.
        report.stats.skipped_binary += 1;
        return;
    };
    let Ok(doc) = StrDoc::try_new(&text, lang.support()) else {
        *report
            .stats
            .parse_failures
            .entry(lang.dir().to_owned())
            .or_insert(0) += 1;
        return;
    };
    report.stats.scanned += 1;
    let root = AstGrep::doc(doc);
    for rule in rules {
        for node in root.root().find_all(&rule.config.matcher) {
            report.matches.push(to_match(&entry.path, rule, &node));
        }
    }
}

/// One ast-grep match as a [`RuleMatch`].
fn to_match(
    path: &str,
    rule: &Rule,
    node: &ast_grep_core::NodeMatch<'_, StrDoc<ast_grep_language::SupportLang>>,
) -> RuleMatch {
    let start = node.start_pos();
    let end = node.end_pos();
    RuleMatch {
        path: path.to_owned(),
        start_line: line_number(start.line()),
        end_line: line_number(end.line()),
        start_column: column_number(start.column(node.get_node())),
        end_column: column_number(end.column(node.get_node())),
        rule_id: rule.id.clone(),
        rule_path: rule.path.clone(),
        check_id: rule.check_id,
        sub_check: rule.sub_check.clone(),
        message: rule.message.clone(),
    }
}

/// ast-grep counts lines and columns from zero; a reader counts from one.
fn line_number(zero_based: usize) -> u32 {
    u32::try_from(zero_based.saturating_add(1)).unwrap_or(u32::MAX)
}

/// Columns are characters, counted from one.
fn column_number(zero_based: usize) -> u32 {
    u32::try_from(zero_based.saturating_add(1)).unwrap_or(u32::MAX)
}
