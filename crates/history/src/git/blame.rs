//! `git blame -w -M -C --line-porcelain`: which commit wrote each surviving line.
//!
//! `-w` ignores whitespace-only changes, `-M` finds lines moved within a file and `-C`
//! finds lines copied from other files; without that move and copy detection the big-bang
//! ratio would credit a reformat or a file split to the refactor instead of the original
//! author (ADR-0015).
//!
//! `git blame` has no `--use-mailmap` (this git prints `error: unknown option '(null)'`
//! and its usage list has no such flag, unlike `git log`). Identities are therefore
//! mapped afterwards through `git check-mailmap`, once per file, so the same person under
//! two addresses counts once — which is what `.mailmap` exists for and what H11's bus
//! factor depends on.
//!
//! The porcelain format repeats a full header for every line (that is the difference from
//! `--porcelain`), so the parser needs no commit cache:
//!
//! ```text
//! 4ac0fd6… 1 1 2
//! author Adam
//! author-mail <a@example.invalid>
//! author-time 1792583348
//! author-tz +0000
//! committer …
//! summary M0-T03: …
//! filename crates/core/src/clock.rs
//! \t the line's text
//! ```
//!
//! A group header (`<sha> <old> <new> <count>`) opens a run of `count` lines that share
//! the author; the line text follows the header block, one per blamed line.

use std::collections::BTreeMap;
use std::ffi::OsString;

use substance_core::Timestamp;
use tokio::task::JoinSet;

use super::{GitCommand, GitError, GitRepo, ParseFailure};

/// One blamed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// The 1-based line number in the blamed revision.
    pub line: u32,
    /// The commit that last touched it.
    pub commit: String,
    /// The author's name, with `.mailmap` applied.
    pub author: String,
    /// The author's email, with `.mailmap` applied.
    pub email: String,
    /// When the author wrote it.
    pub at: Timestamp,
}

/// Every blamed line of one file, sorted by line number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlameMap {
    /// The file the lines belong to.
    pub path: String,
    /// The lines, sorted by line number.
    pub lines: Vec<BlameLine>,
    /// Whether git says the file is binary, so nothing can be blamed.
    pub binary: bool,
}

impl BlameMap {
    /// How many lines the file has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the file has no blamed lines.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The commits that own at least one line, sorted by SHA.
    #[must_use]
    pub fn commits(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = self.lines.iter().map(|line| line.commit.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }
}

/// How much of the requested set was actually blamed (§4.3: a count, not a time).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlameCoverage {
    /// The authored files the run found.
    pub files_total: u32,
    /// The files blame actually covered.
    pub files_blamed: u32,
    /// Lines blamed over those files.
    pub lines_blamed: u64,
    /// Whether `blame_max_files` stopped the run before every file was blamed.
    pub truncated: bool,
}

impl BlameCoverage {
    /// Blamed files over authored files, in permille; `None` when there were no files.
    ///
    /// This is the number a reader needs next to a big-bang ratio: "91% of files were
    /// blamed" says how much of the tree the ratio rests on. Permille keeps the report free
    /// of floats (ADR-0009), and a budget-truncated run reports 1000 only when it covered
    /// every file.
    #[must_use]
    pub fn file_permille(&self) -> Option<u32> {
        if self.files_total == 0 {
            return None;
        }
        let blamed = self.files_blamed.min(self.files_total);
        Some(blamed.saturating_mul(1000) / self.files_total)
    }
}

/// The blame of every file in one run, plus what it covered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlameReport {
    /// The blamed files, sorted by path.
    pub files: Vec<BlameMap>,
    /// What the run managed to blame.
    pub coverage: BlameCoverage,
    /// Files that could not be blamed, with git's reason, sorted by path.
    pub failures: Vec<(String, String)>,
}

impl BlameReport {
    /// Every line of every file, in path then line order.
    pub fn lines(&self) -> impl Iterator<Item = (&str, &BlameLine)> {
        self.files
            .iter()
            .flat_map(|file| file.lines.iter().map(|line| (file.path.as_str(), line)))
    }

    /// Blamed lines per commit, sorted by commit SHA.
    #[must_use]
    pub fn lines_per_commit(&self) -> BTreeMap<String, u64> {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for (_, line) in self.lines() {
            *counts.entry(line.commit.clone()).or_default() += 1;
        }
        counts
    }
}

/// The state of the porcelain reader.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reading {
    /// Between groups.
    Idle,
    /// Inside a header block for `remaining` lines.
    Header {
        /// The commit this group belongs to.
        commit: String,
        /// The first line number of the group.
        next_line: u32,
        /// How many lines are left to read from the header's list.
        remaining: u32,
        /// The author found so far.
        author: String,
        /// The author's email so far.
        email: String,
        /// The author's instant so far, absent until `author-time` is read.
        at: Option<Timestamp>,
    },
}

impl GitRepo {
    /// Blames one file at `rev`.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when git fails, times out, or prints a porcelain block this
    /// parser does not understand, naming the line.
    pub async fn blame(&self, rev: &str, path: &str) -> Result<BlameMap, GitError> {
        let args = vec![
            OsString::from("blame"),
            OsString::from("-w"),
            OsString::from("-M"),
            OsString::from("-C"),
            OsString::from("--line-porcelain"),
            OsString::from(rev),
            OsString::from("--"),
            OsString::from(path),
        ];
        let stdout = self.run(GitCommand::Blame, &args).await?;
        let mut map = parse_blame(&stdout, path)
            .map_err(|failure| self.parse_error(GitCommand::Blame, failure))?;
        self.apply_mailmap(&mut map).await?;
        Ok(map)
    }

    /// Rewrites every line's identity to its `.mailmap` form.
    ///
    /// One `git check-mailmap` call per distinct contact, batched into as few calls as the
    /// argument list allows. A contact git cannot map comes back unchanged, so a repository
    /// without a `.mailmap` costs one extra call and changes nothing.
    async fn apply_mailmap(&self, map: &mut BlameMap) -> Result<(), GitError> {
        let mut contacts: Vec<String> = Vec::new();
        for line in &map.lines {
            let contact = format!("{} <{}>", line.author, line.email);
            if !contacts.contains(&contact) {
                contacts.push(contact);
            }
        }
        if contacts.is_empty() {
            return Ok(());
        }
        contacts.sort_unstable();
        let mut mapped: std::collections::BTreeMap<String, (String, String)> =
            std::collections::BTreeMap::new();
        for chunk in contacts.chunks(MAILMAP_CHUNK) {
            let mut args = vec![OsString::from("check-mailmap")];
            args.extend(chunk.iter().map(OsString::from));
            let stdout = self.run(GitCommand::CheckMailmap, &args).await?;
            for (request, answer) in chunk.iter().zip(stdout.lines()) {
                if let Some((name, email)) = split_contact(answer) {
                    mapped.insert(request.clone(), (name, email));
                }
            }
        }
        for line in &mut map.lines {
            let contact = format!("{} <{}>", line.author, line.email);
            if let Some((name, email)) = mapped.get(&contact) {
                line.author.clone_from(name);
                line.email.clone_from(email);
            }
        }
        Ok(())
    }
}

/// Blames `paths` at `rev`, at most `max_files` of them, in sorted path order.
///
/// The budget is a count, applied in sorted order, so the same repository and budget
/// always cover the same files (§4.3) — a wall-clock cap would let a faster machine see
/// more of the tree. The order in which the worker pool starts a file never changes which
/// files are covered: the budget is taken from the sorted list before any work starts, and
/// the report is sorted by path again at the end.
///
/// # Errors
///
/// Returns [`GitError`] only when the worker pool itself cannot run. A file that git
/// cannot blame is recorded in [`BlameReport::failures`], because one unreadable path
/// should not lose the other four thousand.
pub async fn blame_all(
    repo: &GitRepo,
    rev: &str,
    paths: &[String],
    max_files: u32,
) -> Result<BlameReport, GitError> {
    let mut sorted: Vec<&String> = paths.iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    let total = u32::try_from(sorted.len()).unwrap_or(u32::MAX);
    let budget = usize::try_from(max_files).unwrap_or(usize::MAX);
    let truncated = sorted.len() > budget;
    let selected: Vec<String> = sorted
        .iter()
        .take(budget)
        .map(|path| (*path).clone())
        .collect();
    let workers = worker_count(selected.len());
    let mut report = BlameReport {
        coverage: BlameCoverage {
            files_total: total,
            files_blamed: 0,
            lines_blamed: 0,
            truncated,
        },
        ..BlameReport::default()
    };
    if selected.is_empty() {
        return Ok(report);
    }
    // One joined task per file, admitted a window at a time, so the number of live child
    // processes is bounded by the CPU count rather than by the size of the repository.
    let mut queue = selected.iter();
    let mut tasks: JoinSet<Result<BlameMap, (String, String)>> = JoinSet::new();
    for _ in 0..workers {
        if let Some(path) = queue.next() {
            spawn_blame(&mut tasks, repo.clone(), rev.to_owned(), path.clone());
        }
    }
    while let Some(joined) = tasks.join_next().await {
        if let Ok(outcome) = joined {
            match outcome {
                Ok(map) => {
                    report.coverage.lines_blamed += map.len() as u64;
                    report.coverage.files_blamed += 1;
                    report.files.push(map);
                }
                Err((path, detail)) => report.failures.push((path, detail)),
            }
        }
        if let Some(path) = queue.next() {
            spawn_blame(&mut tasks, repo.clone(), rev.to_owned(), path.clone());
        }
    }
    report
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    report.failures.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(report)
}

/// Starts one blame task.
fn spawn_blame(
    tasks: &mut JoinSet<Result<BlameMap, (String, String)>>,
    repo: GitRepo,
    rev: String,
    path: String,
) {
    tasks.spawn(async move {
        let named = path.clone();
        match repo.blame(&rev, &path).await {
            Ok(map) => Ok(map),
            Err(error) => Err((named, error.to_string())),
        }
    });
}

/// Splits `Name <email>` into its parts; `None` when the text is not a contact.
fn split_contact(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let open = text.rfind('<')?;
    let close = text.rfind('>')?;
    if close < open {
        return None;
    }
    let name = text[..open].trim().to_owned();
    let email = text[open + 1..close].trim().to_owned();
    Some((name, email))
}

/// How many contacts one `git check-mailmap` call may name.
const MAILMAP_CHUNK: usize = 200;

/// How many blames to run at once: the CPU count, and never more than there is work.
fn worker_count(files: usize) -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    cpus.min(files).max(1)
}

/// Parses `git blame --line-porcelain` output for `path`.
///
/// # Errors
///
/// Returns a [`ParseFailure`] naming the output line where the reader lost its place.
fn parse_blame(stdout: &str, path: &str) -> Result<BlameMap, ParseFailure> {
    let mut map = BlameMap {
        path: path.replace('\\', "/"),
        lines: Vec::new(),
        // git refuses to blame a binary file and says so instead of printing groups.
        binary: stdout.contains("Binary file"),
    };
    let mut state = Reading::Idle;
    for (index, raw) in stdout.lines().enumerate() {
        let number = index + 1;
        match &mut state {
            Reading::Idle => {
                let header = match parse_group_header(raw) {
                    Some(header) => header,
                    None => continue,
                };
                state = Reading::Header {
                    commit: header.commit,
                    next_line: header.new_start,
                    remaining: header.count,
                    author: String::new(),
                    email: String::new(),
                    at: None,
                };
            }
            Reading::Header {
                commit,
                next_line,
                remaining,
                author,
                email,
                at,
            } => {
                if raw.starts_with('\t') {
                    let instant = at.ok_or_else(|| {
                        ParseFailure::new(number, "a blamed line before its author-time")
                    })?;
                    map.lines.push(BlameLine {
                        line: *next_line,
                        commit: commit.clone(),
                        author: author.clone(),
                        email: email.clone(),
                        at: instant,
                    });
                    *next_line += 1;
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0 {
                        state = Reading::Idle;
                    }
                } else if let Some(name) = raw.strip_prefix("author ") {
                    *author = name.to_owned();
                } else if let Some(mail) = raw.strip_prefix("author-mail ") {
                    *email = mail.trim_matches(['<', '>']).to_owned();
                } else if let Some(seconds) = raw.strip_prefix("author-time ") {
                    *at = parse_unix(seconds, number)?;
                } else if raw.starts_with("filename ") {
                    // The group repeats the file name; the caller already knows it.
                } else if raw.starts_with("summary ") || raw.starts_with("boundary") {
                    // Header detail the blame map does not use.
                }
            }
        }
    }
    map.lines.sort_by_key(|line| line.line);
    Ok(map)
}

/// One porcelain group header.
struct GroupHeader {
    /// The commit SHA.
    commit: String,
    /// The first line number on the new side.
    new_start: u32,
    /// How many lines the group holds.
    count: u32,
}

/// Parses `<sha> <old> <new> <count>`.
fn parse_group_header(raw: &str) -> Option<GroupHeader> {
    let mut parts = raw.split(' ');
    let commit = parts.next()?;
    if commit.len() < 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let _old = parts.next()?;
    let new_start = parts.next()?.parse::<u32>().ok()?;
    let count = parts.next().unwrap_or("1").parse::<u32>().ok()?;
    Some(GroupHeader {
        commit: commit.to_owned(),
        new_start,
        count,
    })
}

/// Reads `author-time 1792583348` as a UTC instant.
fn parse_unix(text: &str, line: usize) -> Result<Option<Timestamp>, ParseFailure> {
    let seconds = text
        .trim()
        .parse::<i64>()
        .map_err(|_| ParseFailure::new(line, format!("author-time {text:?} is not a number")))?;
    match time::OffsetDateTime::from_unix_timestamp(seconds) {
        Ok(instant) => Ok(Some(Timestamp::new(instant))),
        Err(error) => Err(ParseFailure::new(
            line,
            format!("author-time {seconds} is out of range: {error}"),
        )),
    }
}
