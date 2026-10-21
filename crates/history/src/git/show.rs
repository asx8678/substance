//! `git show -U0 -M -C`: the hunks of one commit, one file at a time.
//!
//! H08, H09 and T05 need the lines a commit removed and added, not just their counts.
//! The parser reads git's unified diff with no context lines, so every hunk is exactly
//! the changed lines:
//!
//! ```text
//! diff --git a/src/a.rs b/src/b.rs
//! rename from src/a.rs
//! rename to src/b.rs
//! Binary files a/old.bin and b/new.bin differ
//! --- a/src/a.rs
//! +++ b/src/a.rs
//! @@ -3 +3 @@ fn main() {
//! -old line
//! +new line
//! \ No newline at end of file
//! ```
//!
//! Three shapes need care. A rename is reported by the `rename from`/`rename to` lines
//! before any hunk, and a rename with no content change has no hunks at all. A binary
//! file has a `Binary files … differ` line instead of hunks. A merge commit has no diff
//! at all unless `--diff-merges=first-parent` is passed, which `hunks()` does, so a merge
//! is read as the change its branch brought.

use std::ffi::OsString;

use super::{GitCommand, GitError, GitRepo, ParseFailure};

/// Which side of a hunk a line is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkLineKind {
    /// A line the commit removed.
    Removed,
    /// A line the commit added.
    Added,
}

/// One changed line, without its leading `-` or `+`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    /// Which side of the change it is on.
    pub kind: HunkLineKind,
    /// The line's text, without its newline.
    pub text: String,
}

/// One hunk: where it sits and which lines it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The line number the hunk starts at on the old side, 0 for a pure insertion.
    pub old_start: u64,
    /// How many lines it covers on the old side.
    pub old_lines: u64,
    /// The line number the hunk starts at on the new side, 0 for a pure deletion.
    pub new_start: u64,
    /// How many lines it covers on the new side.
    pub new_lines: u64,
    /// The hunk header's trailing text, for example the enclosing function; empty when
    /// git printed none.
    pub section: String,
    /// The changed lines, in the order git printed them.
    pub lines: Vec<HunkLine>,
}

impl Hunk {
    /// The removed lines' text, in order.
    #[must_use]
    pub fn removed(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|line| line.kind == HunkLineKind::Removed)
            .map(|line| line.text.as_str())
            .collect()
    }

    /// The added lines' text, in order.
    #[must_use]
    pub fn added(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|line| line.kind == HunkLineKind::Added)
            .map(|line| line.text.as_str())
            .collect()
    }
}

/// Every change one commit made to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHunks {
    /// The path after the commit.
    pub path: String,
    /// The path before it, when the entry is a rename or a move.
    pub old_path: Option<String>,
    /// Whether git calls the file binary; such a file has no hunks.
    pub binary: bool,
    /// The hunks, in the order git printed them.
    pub hunks: Vec<Hunk>,
}

impl FileHunks {
    /// Whether the file was renamed or moved.
    #[must_use]
    pub fn is_rename(&self) -> bool {
        self.old_path.is_some()
    }

    /// Every removed line, over every hunk.
    #[must_use]
    pub fn removed(&self) -> Vec<&str> {
        self.hunks.iter().flat_map(Hunk::removed).collect()
    }

    /// Every added line, over every hunk.
    #[must_use]
    pub fn added(&self) -> Vec<&str> {
        self.hunks.iter().flat_map(Hunk::added).collect()
    }
}

/// What one commit changed, by path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathsInCommit {
    /// The files it touched, sorted by path.
    pub files: Vec<FileHunks>,
}

impl PathsInCommit {
    /// The changed paths, sorted.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|file| file.path.as_str()).collect()
    }

    /// Whether any touched file is binary.
    #[must_use]
    pub fn has_binary(&self) -> bool {
        self.files.iter().any(|file| file.binary)
    }

    /// Whether any touched file was renamed or moved.
    #[must_use]
    pub fn has_rename(&self) -> bool {
        self.files.iter().any(FileHunks::is_rename)
    }
}

/// The state the line reader is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// Before any `diff --git` line.
    Idle,
    /// Inside a file header, before its first hunk.
    Header,
    /// Inside a hunk.
    Hunk,
}

impl GitRepo {
    /// The hunks `sha` introduced, one entry per file it touched.
    ///
    /// A merge commit is read through `--diff-merges=first-parent`, so it contributes the
    /// change its branch brought; without that flag git prints no diff for a merge and the
    /// hunks would silently be empty.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when git fails, times out, or prints a hunk header or mode line
    /// this parser does not understand, naming the line.
    pub async fn hunks(&self, sha: &str) -> Result<Vec<FileHunks>, GitError> {
        let args = vec![
            OsString::from("show"),
            OsString::from("-U0"),
            OsString::from("--no-color"),
            OsString::from("--no-ext-diff"),
            OsString::from("-M"),
            OsString::from("-C"),
            OsString::from("--diff-merges=first-parent"),
            OsString::from("--format="),
            OsString::from(sha),
            OsString::from("--"),
        ];
        let stdout = self.run(GitCommand::Show, &args).await?;
        parse_show(&stdout).map_err(|failure| self.parse_error(GitCommand::Show, failure))
    }
}

/// Parses `git show -U0` output.
///
/// # Errors
///
/// Returns a [`ParseFailure`] with the line number where the output stopped matching.
fn parse_show(stdout: &str) -> Result<Vec<FileHunks>, ParseFailure> {
    let mut files: Vec<FileHunks> = Vec::new();
    let mut state = Reading::Idle;
    for (index, raw) in stdout.lines().enumerate() {
        let number = index + 1;
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            let (_, new) = split_diff_paths(rest, number)?;
            files.push(FileHunks {
                path: new,
                old_path: None,
                binary: false,
                hunks: Vec::new(),
            });
            state = Reading::Header;
            continue;
        }
        if matches!(state, Reading::Idle) {
            // Anything before the first `diff --git` is the commit header. With
            // `--format=` there should be none; skipping is safer than guessing.
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        match state {
            Reading::Header => {
                if let Some(path) = raw.strip_prefix("rename from ") {
                    file.old_path = Some(path.to_owned());
                } else if let Some(path) = raw.strip_prefix("rename to ") {
                    file.path = path.to_owned();
                } else if raw.starts_with("Binary files ") || raw == "GIT binary patch" {
                    file.binary = true;
                } else if raw.starts_with("@@ ") {
                    let hunk = parse_hunk_header(raw, number)?;
                    file.hunks.push(hunk);
                    state = Reading::Hunk;
                }
            }
            Reading::Hunk => {
                if raw.starts_with("@@ ") {
                    let hunk = parse_hunk_header(raw, number)?;
                    if let Some(file) = files.last_mut() {
                        file.hunks.push(hunk);
                    }
                } else if let Some(text) = raw.strip_prefix('-') {
                    push_line(files.last_mut(), HunkLineKind::Removed, text);
                } else if let Some(text) = raw.strip_prefix('+') {
                    push_line(files.last_mut(), HunkLineKind::Added, text);
                } else if raw.starts_with("\\ No newline at end of file") {
                    // The marker belongs to the line before it, which is already stored;
                    // it carries no text of its own.
                } else if raw.starts_with("Binary files ") {
                    if let Some(file) = files.last_mut() {
                        file.binary = true;
                    }
                    state = Reading::Header;
                } else {
                    state = Reading::Header;
                }
            }
            Reading::Idle => {}
        }
    }
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.old_path.cmp(&right.old_path))
    });
    Ok(files)
}

/// Appends one changed line to the last hunk of the last file.
fn push_line(file: Option<&mut FileHunks>, kind: HunkLineKind, text: &str) {
    if let Some(hunk) = file.and_then(|file| file.hunks.last_mut()) {
        hunk.lines.push(HunkLine {
            kind,
            text: text.to_owned(),
        });
    }
}

/// Splits `a/old b/new` from a `diff --git` line.
///
/// Paths are unquoted already (`core.quotePath=false`), and a rename's two paths differ,
/// so the split is at the ` b/` that separates them.
fn split_diff_paths(rest: &str, line: usize) -> Result<(String, String), ParseFailure> {
    let rest = rest.trim();
    let split = rest
        .rfind(" b/")
        .ok_or_else(|| ParseFailure::new(line, format!("diff header {rest:?} has no \" b/\"")))?;
    let old = rest[..split].trim().to_owned();
    let new = rest[split + 1..].trim().to_owned();
    let old = old.strip_prefix("a/").unwrap_or(&old).to_owned();
    let new = new.strip_prefix("b/").unwrap_or(&new).to_owned();
    Ok((old, new))
}

/// Parses `@@ -1,2 +3,4 @@ section` into a hunk.
fn parse_hunk_header(raw: &str, line: usize) -> Result<Hunk, ParseFailure> {
    let body = raw.strip_prefix("@@ ").unwrap_or(raw);
    let end = body
        .find(" @@")
        .ok_or_else(|| ParseFailure::new(line, format!("hunk header {raw:?} has no closing @@")))?;
    let ranges = &body[..end];
    let section = body[end + 3..].trim().to_owned();
    let mut parts = ranges.split(' ');
    let old = parts
        .next()
        .ok_or_else(|| ParseFailure::new(line, format!("hunk header {raw:?} has no old range")))?;
    let new = parts
        .next()
        .ok_or_else(|| ParseFailure::new(line, format!("hunk header {raw:?} has no new range")))?;
    let (old_start, old_lines) = parse_range(old, '-', line)?;
    let (new_start, new_lines) = parse_range(new, '+', line)?;
    Ok(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        section,
        lines: Vec::new(),
    })
}

/// Parses `-1,2` or `+3` into `(start, count)`.
fn parse_range(text: &str, sign: char, line: usize) -> Result<(u64, u64), ParseFailure> {
    let body = text.strip_prefix(sign).ok_or_else(|| {
        ParseFailure::new(line, format!("range {text:?} does not start with {sign}"))
    })?;
    let mut parts = body.split(',');
    let start = parts
        .next()
        .unwrap_or_default()
        .parse::<u64>()
        .map_err(|_| ParseFailure::new(line, format!("range {text:?} has no start")))?;
    let count = match parts.next() {
        Some(count) => count
            .parse::<u64>()
            .map_err(|_| ParseFailure::new(line, format!("range {text:?} has no count")))?,
        None => 1,
    };
    Ok((start, count))
}
