//! `git log -z --numstat -M -C`: commits, identities, trailers and file stats.
//!
//! The record format is one `-z`-terminated chunk per commit followed by one chunk per
//! file, so no path can be confused with a field. Each chunk holds these fields, separated
//! by `\x1f`:
//!
//! ```text
//! %H %P %aN %aE %aI %cN %cE %cI %s %(trailers:unfold,only) %b
//! ```
//!
//! and the file chunks hold `added \t deleted \t path`, with `-` for a binary file. A
//! rename appears the way `git diff --numstat` writes it — `old.txt => new.txt`, or
//! `src/{ => sub}/alpha.txt` when only a directory moved — and with `-z` a rename of a file
//! whose name did not change its basename is written as an empty first field followed by
//! two path chunks. Both shapes are parsed here (see [`parse_numstat`]).

use std::ffi::OsString;

use substance_core::Timestamp;

use super::{GitCommand, GitError, GitRepo, ParseFailure};

/// The field separator inside one commit record.
const FIELD: char = '\u{1f}';

/// How many fields the `--format` string produces.
const FIELDS: usize = 11;

/// One person on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity {
    /// The name, with `.mailmap` applied.
    pub name: String,
    /// The email, with `.mailmap` applied.
    pub email: String,
    /// The instant, in UTC.
    pub at: Timestamp,
}

/// The author and the committer of one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The commit SHA.
    pub sha: String,
    /// The parent SHAs, in git's order; empty for a root commit.
    pub parents: Vec<String>,
    /// Who wrote it.
    pub author: CommitIdentity,
    /// Who committed it.
    pub committer: CommitIdentity,
    /// The first line of the message.
    pub subject: String,
    /// The message after the subject, trimmed.
    pub body: String,
    /// The trailers, `(key, value)` in the order git prints them.
    pub trailers: Vec<(String, String)>,
    /// The files it touched, sorted by path.
    pub files: Vec<FileChange>,
}

impl Commit {
    /// Whether this is a merge commit.
    #[must_use]
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// The value of the first trailer named `key`, compared case-insensitively.
    #[must_use]
    pub fn trailer(&self, key: &str) -> Option<&str> {
        self.trailers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// Added lines over every file, ignoring binary files.
    #[must_use]
    pub fn added(&self) -> u64 {
        self.files.iter().filter_map(|file| file.added).sum()
    }

    /// Deleted lines over every file, ignoring binary files.
    #[must_use]
    pub fn deleted(&self) -> u64 {
        self.files.iter().filter_map(|file| file.deleted).sum()
    }
}

/// One file's line counts in one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The path after the commit.
    pub path: String,
    /// The path before it, when the file was renamed or moved.
    pub old_path: Option<String>,
    /// Lines added; `None` for a binary file, where git writes `-`.
    pub added: Option<u64>,
    /// Lines deleted; `None` for a binary file.
    pub deleted: Option<u64>,
}

impl FileChange {
    /// Whether the file is binary, as git reports it.
    #[must_use]
    pub fn is_binary(&self) -> bool {
        self.added.is_none()
    }

    /// Whether the entry is a rename or a move.
    #[must_use]
    pub fn is_rename(&self) -> bool {
        self.old_path.is_some()
    }

    /// Every path this entry names, the new one first.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        let mut paths = vec![self.path.as_str()];
        if let Some(old) = &self.old_path {
            paths.push(old.as_str());
        }
        paths
    }
}

/// The per-commit line totals a history check sums.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileStats {
    /// Files touched.
    pub files: u64,
    /// Files that are binary.
    pub binaries: u64,
    /// Files renamed or moved.
    pub renames: u64,
    /// Lines added.
    pub added: u64,
    /// Lines deleted.
    pub deleted: u64,
}

impl FileStats {
    /// Totals over `commits`.
    #[must_use]
    pub fn of(commits: &[Commit]) -> Self {
        let mut stats = Self::default();
        for commit in commits {
            stats.files += commit.files.len() as u64;
            stats.binaries += commit.files.iter().filter(|file| file.is_binary()).count() as u64;
            stats.renames += commit.files.iter().filter(|file| file.is_rename()).count() as u64;
            stats.added += commit.added();
            stats.deleted += commit.deleted();
        }
        stats
    }
}

/// The trailers of one commit, keyed by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trailers {
    /// The values, in git's order, with a repeated key kept as a second entry.
    pub entries: Vec<(String, String)>,
}

impl Trailers {
    /// Parses `%(trailers:unfold,only)` output: `Key: value` lines, values already unfolded.
    ///
    /// A line without a colon is skipped rather than treated as an error: git's trailer
    /// detection already decided what a trailer is, and this only splits the key from the
    /// value.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                entries.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
        Self { entries }
    }

    /// Parses trailer output, reporting a line that is not `Key: value`.
    ///
    /// The separator between fields is one byte, so a commit message that happened to
    /// contain it would shift every later field by one. A trailer block is the place that
    /// shows: it is the only field whose lines all carry a colon. Reporting the mismatch
    /// as a parse error is better than recording a subject as a trailer.
    ///
    /// # Errors
    ///
    /// Returns a description when a non-empty line has no `:` separator.
    pub fn parse_checked(text: &str) -> Result<Self, String> {
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line.split_once(':') {
                Some((key, value)) => {
                    entries.push((key.trim().to_owned(), value.trim().to_owned()));
                }
                None => {
                    return Err(format!(
                        "the trailer field holds {line:?}, which is not `Key: value`; a commit \
                         message probably contains the field separator"
                    ));
                }
            }
        }
        Ok(Self { entries })
    }

    /// The first value for `key`, compared case-insensitively.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }
}

impl GitRepo {
    /// Reads the commits reachable from `rev`, newest first.
    ///
    /// The order is `--date-order`: children before parents, newest committer date first
    /// among commits that are not ancestors of each other. That is git's own stable order
    /// for a fixed history, so two runs see one sequence; the checks that need another
    /// order sort the vector themselves.
    ///
    /// Merges get the first-parent diff (`--diff-merges=first-parent`), so a merge
    /// contributes the change its branch brought, once, instead of the same change again
    /// from each parent.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when git fails, times out, or prints something this parser
    /// does not understand — naming the line.
    pub async fn commits(&self, rev: &str) -> Result<Vec<Commit>, GitError> {
        let args = log_args(rev);
        let stdout = self.run(GitCommand::Log, &args).await?;
        parse_log(&stdout).map_err(|failure| self.parse_error(GitCommand::Log, failure))
    }
}

/// The argument vector for [`GitRepo::commits`].
fn log_args(rev: &str) -> Vec<OsString> {
    let format = [
        "%H",
        "%P",
        "%aN",
        "%aE",
        "%aI",
        "%cN",
        "%cE",
        "%cI",
        "%s",
        "%(trailers:unfold,only)",
        "%b",
    ]
    .join("\u{1f}");
    vec![
        OsString::from("log"),
        OsString::from("-z"),
        OsString::from("--numstat"),
        OsString::from("-M"),
        OsString::from("-C"),
        OsString::from("--use-mailmap"),
        OsString::from("--no-color"),
        OsString::from("--date-order"),
        OsString::from("--diff-merges=first-parent"),
        OsString::from(format!("--format={format}")),
        OsString::from(rev),
        OsString::from("--"),
    ]
}

/// Parses `git log -z --numstat --format=…` output.
///
/// # Errors
///
/// Returns a [`ParseFailure`] naming the chunk where the output stopped matching.
fn parse_log(stdout: &str) -> Result<Vec<Commit>, ParseFailure> {
    // Each record is a commit chunk followed by one chunk per file. `split` yields a
    // trailing empty chunk after the final `\0`; the walk stops on it.
    let chunks: Vec<&str> = stdout.split('\0').collect();
    let mut commits = Vec::new();
    let mut index = 0;
    while index < chunks.len() {
        let chunk = chunks.get(index).copied().unwrap_or_default();
        let chunk = chunk.trim_start_matches('\n');
        if chunk.trim().is_empty() {
            index += 1;
            continue;
        }
        let line = commits.len() + 1;
        // `splitn` so a message that happens to contain the separator cannot shift the
        // fields: the body, which comes last, keeps any extra bytes instead of losing
        // them. A separator inside the subject is caught by the trailer check below,
        // which then reports a parse error rather than guessing.
        let fields: Vec<&str> = chunk.splitn(FIELDS, FIELD).collect();
        if fields.len() != FIELDS {
            return Err(ParseFailure::new(
                line,
                format!("expected {FIELDS} fields, found {}", fields.len()),
            ));
        }
        let mut commit = commit_from_fields(&fields, line)?;
        index += 1;
        while let Some(next) = chunks.get(index).copied() {
            if is_commit_start(next) {
                break;
            }
            index += 1;
            let entry = next.trim_start_matches('\n');
            if entry.trim().is_empty() {
                continue;
            }
            // A `-z` rename arrives as an empty path followed by two path chunks: the old
            // path and the new one. Both are consumed here so they are never read as
            // entries of their own.
            if entry
                .splitn(3, '\t')
                .nth(2)
                .is_some_and(|path| path.is_empty())
            {
                let old = chunks.get(index).copied().unwrap_or_default();
                let new = chunks.get(index + 1).copied().unwrap_or_default();
                if old.is_empty() || new.is_empty() {
                    return Err(ParseFailure::new(
                        line,
                        "a rename with fewer than two path chunks".to_owned(),
                    ));
                }
                index += 2;
                let mut parts = entry.splitn(3, '\t');
                let added = count(parts.next().unwrap_or_default(), line)?;
                let deleted = count(parts.next().unwrap_or_default(), line)?;
                commit.files.push(FileChange {
                    path: new.to_owned(),
                    old_path: Some(old.to_owned()),
                    added,
                    deleted,
                });
                continue;
            }
            for change in parse_numstat(entry, line)? {
                commit.files.push(change);
            }
        }
        commit.files.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.old_path.cmp(&right.old_path))
        });
        commits.push(commit);
    }
    Ok(commits)
}

/// Whether a chunk begins a commit record rather than a file entry.
fn is_commit_start(chunk: &str) -> bool {
    chunk.trim_start_matches('\n').contains(FIELD)
}

/// Builds a commit from its `--format` fields.
fn commit_from_fields(fields: &[&str], line: usize) -> Result<Commit, ParseFailure> {
    let parents = fields
        .get(1)
        .map(|text| {
            text.split(' ')
                .filter(|sha| !sha.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let author = identity(
        fields.get(2).copied(),
        fields.get(3).copied(),
        fields.get(4).copied(),
        line,
    )?;
    let committer = identity(
        fields.get(5).copied(),
        fields.get(6).copied(),
        fields.get(7).copied(),
        line,
    )?;
    let subject = fields
        .get(8)
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    let trailers = Trailers::parse_checked(fields.get(9).copied().unwrap_or_default())
        .map_err(|detail| ParseFailure::new(line, detail))?;
    let body = fields
        .get(10)
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    let sha = fields
        .first()
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    if sha.is_empty() {
        return Err(ParseFailure::new(line, "a commit record with no SHA"));
    }
    Ok(Commit {
        sha,
        parents,
        author,
        committer,
        subject,
        body,
        trailers: trailers.entries,
        files: Vec::new(),
    })
}

/// Builds one identity, parsing its ISO-8601 instant.
fn identity(
    name: Option<&str>,
    email: Option<&str>,
    at: Option<&str>,
    line: usize,
) -> Result<CommitIdentity, ParseFailure> {
    let name = name.unwrap_or_default().trim().to_owned();
    let email = email.unwrap_or_default().trim().to_owned();
    let text = at.unwrap_or_default().trim();
    let instant = Timestamp::parse_rfc3339(text).map_err(|error| {
        ParseFailure::new(line, format!("{text:?} is not an ISO-8601 date: {error}"))
    })?;
    Ok(CommitIdentity {
        name,
        email,
        at: instant,
    })
}

/// Parses one `--numstat` entry, which may describe several files when `-z` splits a
/// rename into separate chunks.
///
/// The three shapes git writes:
///
/// ```text
/// 3\t1\tsrc/main.rs                    a plain change
/// 0\t0\told.txt => new.txt             a rename (only with -z: split below)
/// 0\t0\tsrc/{ => sub}/alpha.txt         a rename inside a moved directory
/// 0\t0\t                                with -z, an empty path and two path chunks
/// ```
fn parse_numstat(entry: &str, line: usize) -> Result<Vec<FileChange>, ParseFailure> {
    let mut parts = entry.splitn(3, '\t');
    let added = parts.next().unwrap_or_default();
    let deleted = parts.next().ok_or_else(|| {
        ParseFailure::new(
            line,
            format!("numstat entry {entry:?} has no deleted count"),
        )
    })?;
    let path = parts
        .next()
        .ok_or_else(|| ParseFailure::new(line, format!("numstat entry {entry:?} has no path")))?;
    let (added, deleted) = (count(added, line)?, count(deleted, line)?);
    let changes = match split_rename(path) {
        Some((old, new)) => vec![FileChange {
            path: new,
            old_path: Some(old),
            added,
            deleted,
        }],
        None => vec![FileChange {
            path: path.to_owned(),
            old_path: None,
            added,
            deleted,
        }],
    };
    Ok(changes)
}

/// Reads one count: a number, or `-` for a binary file.
fn count(text: &str, line: usize) -> Result<Option<u64>, ParseFailure> {
    if text == "-" {
        return Ok(None);
    }
    text.parse::<u64>().map(Some).map_err(|_| {
        ParseFailure::new(
            line,
            format!("numstat count {text:?} is neither a number nor '-'"),
        )
    })
}

/// Splits a numstat rename path into `(old, new)`.
///
/// Handles both spellings: `old.txt => new.txt` and `src/{ => sub}/alpha.txt`, where the
/// braced piece is replaced in the prefix. A path that merely contains `=>` in a file name
/// stays whole, because a rename always has ` => ` as a separate piece.
fn split_rename(path: &str) -> Option<(String, String)> {
    if let Some((before, after)) = path.split_once(" => ") {
        if let Some((prefix, old_mid, new_mid, suffix)) = brace_parts(before, after) {
            let old = format!("{prefix}{old_mid}{suffix}");
            let new = format!("{prefix}{new_mid}{suffix}");
            return Some((old, new));
        }
        return Some((before.to_owned(), after.to_owned()));
    }
    None
}

/// Splits `src/{` + ` => ` + `sub}/alpha.txt` into its four pieces.
fn brace_parts<'a>(
    before: &'a str,
    after: &'a str,
) -> Option<(&'a str, &'a str, &'a str, &'a str)> {
    let brace = before.rfind('{')?;
    let prefix = &before[..brace];
    let old_mid = &before[brace + 1..];
    let close = after.find('}')?;
    let new_mid = &after[..close];
    let suffix = &after[close + 1..];
    Some((prefix, old_mid, new_mid, suffix))
}
