//! The server-log channel: the error lines the app wrote between two instants (M3-T03).
//!
//! The runner writes each child's streams to `logs/<name>.{stdout,stderr}.log` and records the byte
//! offset of every chunk in `logs/index.jsonl`. Slicing by offset rather than by time is what makes
//! this channel exact: a line written while the action ran is between the two offsets, and a line
//! from the same second before it is not. Time-based slicing would attribute a slow log flush to
//! whichever action happened to be running when it landed.
//!
//! What counts as an error is data (`rules/log-errors.toml`), embedded at build time so a run does
//! not depend on the repository being beside it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The rules file, embedded: a run of an installed `substance` has no `rules/` directory.
pub const LOG_ERRORS_TOML: &str = include_str!("../../../rules/log-errors.toml");

/// One rule from `rules/log-errors.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rule {
    /// The rule's name, which the record can cite.
    pub name: String,
    /// `level`, `stack`, `unhandled` or `note`.
    pub kind: String,
    /// The pattern, matched case-sensitively against one line.
    pub pattern: String,
    /// A second pattern, tried when the first does not match.
    pub alt: Option<String>,
    /// Whether a match counts. A `note` rule documents a decision and counts nothing.
    pub counts: Option<bool>,
}

/// The parsed rules file.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuleSet {
    /// Every rule, in file order.
    #[serde(default)]
    pub rule: Vec<Rule>,
}

/// Why the rules file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// The embedded TOML does not parse, which is a bug rather than a runtime condition.
    #[error("log-errors.toml: {0}")]
    Toml(String),
    /// A rule's pattern is not a valid regular expression.
    #[error("log-errors.toml: rule {name}: {detail}")]
    Pattern {
        /// The rule whose pattern failed.
        name: String,
        /// What the regex engine said.
        detail: String,
    },
}

/// A rule with its patterns compiled.
#[derive(Debug)]
struct CompiledRule {
    name: String,
    patterns: Vec<regex::Regex>,
    counts: bool,
}

/// What the log channel found in one window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogSlice {
    /// Error lines, in the order they appeared.
    pub lines: Vec<String>,
    /// How many lines were examined, error or not.
    pub examined: u32,
    /// The rules that matched, deduplicated and sorted.
    pub matched_rules: Vec<String>,
}

impl LogSlice {
    /// How many error lines the window held.
    #[must_use]
    pub fn error_count(&self) -> u32 {
        u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
    }
}

/// The compiled rules, built once per run.
#[derive(Debug)]
pub struct LogRules {
    rules: Vec<CompiledRule>,
}

impl LogRules {
    /// Compiles the embedded rules.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError`] when the TOML does not parse or a pattern does not compile. Both are
    /// bugs in the data file, and failing loudly is right: a rule that silently does not compile
    /// would make every log look clean.
    pub fn embedded() -> Result<Self, RuleError> {
        Self::parse(LOG_ERRORS_TOML)
    }

    /// Compiles rules from TOML text.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError`] as [`Self::embedded`] does.
    pub fn parse(toml_text: &str) -> Result<Self, RuleError> {
        let set: RuleSet =
            toml::from_str(toml_text).map_err(|error| RuleError::Toml(error.to_string()))?;
        let mut rules = Vec::with_capacity(set.rule.len());
        for rule in set.rule {
            let counts = rule.counts.unwrap_or(true) && rule.kind != "note";
            let mut patterns = Vec::new();
            for source in std::iter::once(&rule.pattern).chain(rule.alt.as_ref()) {
                let compiled = regex::Regex::new(source).map_err(|error| RuleError::Pattern {
                    name: rule.name.clone(),
                    detail: error.to_string(),
                })?;
                patterns.push(compiled);
            }
            rules.push(CompiledRule {
                name: rule.name,
                patterns,
                counts,
            });
        }
        Ok(Self { rules })
    }

    /// Whether one line counts as an error, and which rules it matched.
    fn classify<'a>(&'a self, line: &str) -> (bool, Vec<&'a str>) {
        let mut counts = false;
        let mut matched = Vec::new();
        for rule in &self.rules {
            if rule.patterns.iter().any(|pattern| pattern.is_match(line)) {
                matched.push(rule.name.as_str());
                if rule.counts {
                    counts = true;
                }
            }
        }
        (counts, matched)
    }

    /// Cuts the window and counts the error lines in it.
    #[must_use]
    pub fn slice(&self, text: &str) -> LogSlice {
        let mut slice = LogSlice::default();
        let mut matched_rules: BTreeSet<&str> = BTreeSet::new();
        for line in text.lines() {
            slice.examined = slice.examined.saturating_add(1);
            let (counts, matched) = self.classify(line);
            for name in matched {
                matched_rules.insert(name);
            }
            if counts {
                slice.lines.push(line.to_owned());
            }
        }
        slice.matched_rules = matched_rules.into_iter().map(str::to_owned).collect();
        slice
    }
}

/// One entry of `logs/index.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IndexEntry {
    /// `stdout` or `stderr`.
    pub stream: String,
    /// Where the chunk starts in that stream's file.
    pub offset: u64,
    /// How many bytes it holds.
    pub len: u64,
    /// When it was written, relative to the process start.
    pub monotonic_ms: u64,
}

/// A position in the run's logs: per stream, a byte offset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogMark {
    /// Offset in stdout, absent when stdout was never written.
    pub stdout: Option<u64>,
    /// Offset in stderr, absent when stderr was never written.
    pub stderr: Option<u64>,
}

/// Where the logs are, and how to read a window out of them.
#[derive(Debug, Clone)]
pub struct LogChannel {
    dir: PathBuf,
    name: String,
}

/// Why a window could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// A log file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The IO error.
        source: std::io::Error,
    },
    /// The index could not be parsed.
    #[error("logs/index.jsonl: {detail}")]
    Index {
        /// What was wrong.
        detail: String,
    },
}

impl LogChannel {
    /// The channel for one child's logs: `logs/<name>.{stdout,stderr}.log`.
    #[must_use]
    pub fn new(logs_dir: &Path, name: &str) -> Self {
        Self {
            dir: logs_dir.to_path_buf(),
            name: name.to_owned(),
        }
    }

    /// The mark at the current end of both files.
    ///
    /// Read from the files rather than from the index: the index records chunks the pump has
    /// already written, and a chunk in flight is in the file before it is in the index. The file's
    /// length is the honest answer to "where are we now".
    #[must_use]
    pub fn mark(&self) -> LogMark {
        LogMark {
            stdout: self.length("stdout"),
            stderr: self.length("stderr"),
        }
    }

    /// Reads what was written between two marks.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when a file cannot be read. A mark past the end of its file (the
    /// log was truncated by a restart) yields an empty slice rather than an error, because a
    /// restarted child's fresh log is a legitimate state.
    pub fn between(&self, start: &LogMark, end: &LogMark) -> Result<String, LogError> {
        let mut text = String::new();
        for (stream, from, to) in [
            ("stdout", start.stdout, end.stdout),
            ("stderr", start.stderr, end.stderr),
        ] {
            // An absent `end` means the file did not exist when the window closed, so there is
            // nothing to read. An absent `start` means it did not exist when the window opened,
            // which is offset zero -- not "skip this stream". Skipping it would make a window
            // that begins before the app started (the whole-run case) report no logs at all.
            let Some(to) = to else {
                continue;
            };
            let from = from.unwrap_or(0);
            if to <= from {
                continue;
            }
            let path = self.path(stream);
            let bytes = std::fs::read(&path).map_err(|source| LogError::Io {
                path: path.clone(),
                source,
            })?;
            let from_index = usize::try_from(from).unwrap_or(usize::MAX).min(bytes.len());
            let to_index = usize::try_from(to).unwrap_or(usize::MAX).min(bytes.len());
            text.push_str(&String::from_utf8_lossy(&bytes[from_index..to_index]));
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
        Ok(text)
    }

    /// The index entries written at or after both marks.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Index`] when a line is not an index entry, and [`LogError::Io`] when the
    /// index cannot be read. A missing index is an empty list: a child that never wrote has none.
    pub fn entries_between(&self, start: &LogMark) -> Result<Vec<IndexEntry>, LogError> {
        let path = self.dir.join("index.jsonl");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(LogError::Io { path, source }),
        };
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: IndexEntry =
                serde_json::from_str(line).map_err(|error| LogError::Index {
                    detail: error.to_string(),
                })?;
            let floor = match entry.stream.as_str() {
                "stdout" => start.stdout,
                _ => start.stderr,
            };
            if floor.is_none_or(|floor| entry.offset >= floor) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn path(&self, stream: &str) -> PathBuf {
        self.dir.join(format!("{}.{stream}.log", self.name))
    }

    fn length(&self, stream: &str) -> Option<u64> {
        std::fs::metadata(self.path(stream))
            .ok()
            .map(|metadata| metadata.len())
    }
}
