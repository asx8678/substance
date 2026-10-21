//! Streaming a child's output to disk with a byte-offset index (M2-T01, §4.1).
//!
//! §4.1 asks for `logs/` plus "a byte-offset index", and the index has a job beyond bookkeeping:
//! a later check (O03) reads a log by offset to see what the app printed around a moment. That
//! makes the offset's meaning the important design question, and the answer is **the offset in
//! the file**, not a running count kept here:
//!
//! - The writer appends whatever arrived from the pipe, so a chunk can end mid-line, and a
//!   second chunk finishes it. Counting lines here would either split that line or hold it back.
//! - Therefore each index entry is one write: `offset` is where those bytes land, `len` is how
//!   many there are. `offset + len` is the next entry's offset, and a reader who wants whole
//!   lines takes an offset and reads onward in the file.
//! - The timestamps are `Instant`-derived milliseconds since the child started. §4.3 forbids a
//!   wall clock here and a serialized `Instant`, and a monotonic offset from a known start is
//!   neither: it says "1.4 s after this process started", which is the question a log reader
//!   actually asks.
//!
//! Entries are appended as JSON lines, so a reader can follow the index while the process runs
//! and a crash mid-write costs at most the last line.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Which stream a log write came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl Stream {
    /// The stream as the file name spells it: `stdout` or `stderr`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// One write to one log file, as `logs/index.jsonl` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LogEntry {
    /// Which stream.
    pub stream: Stream,
    /// The byte offset the write started at.
    pub offset: u64,
    /// How many bytes were written.
    pub len: u64,
    /// Milliseconds since the child process started.
    pub monotonic_ms: u64,
}

/// Why a log could not be written.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// A log file or the index could not be written.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
}

/// The two log files of one child process and the index that describes them.
#[derive(Debug, Clone)]
pub struct LogSink {
    /// Where the index goes.
    index_path: PathBuf,
    /// Where stdout goes.
    stdout_path: PathBuf,
    /// Where stderr goes.
    stderr_path: PathBuf,
    /// The instant the child started, for `monotonic_ms`.
    started: Instant,
}

impl LogSink {
    /// Creates `dir/<name>.{stdout,stderr}.log` and `dir/index.jsonl`, truncating anything
    /// already there.
    ///
    /// Truncating rather than appending is deliberate: a name is one process in one run, so a
    /// leftover file from an earlier attempt would put another process's bytes at the offsets
    /// this index describes.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when a file cannot be created.
    pub fn create(dir: &Path, name: &str, started: Instant) -> Result<Self, LogError> {
        let sink = Self {
            index_path: dir.join("index.jsonl"),
            stdout_path: dir.join(format!("{name}.stdout.log")),
            stderr_path: dir.join(format!("{name}.stderr.log")),
            started,
        };
        for path in [&sink.stdout_path, &sink.stderr_path, &sink.index_path] {
            write_file(path, b"")?;
        }
        Ok(sink)
    }

    /// The path of one stream's log file.
    #[must_use]
    pub fn path_of(&self, stream: Stream) -> &Path {
        match stream {
            Stream::Stdout => &self.stdout_path,
            Stream::Stderr => &self.stderr_path,
        }
    }

    /// The index's path.
    #[must_use]
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    /// Appends `bytes` to `stream`'s log and records where they landed.
    ///
    /// The offset is read back from the file after the write rather than tracked in a field: a
    /// field would be a second source of truth that a failed write could leave wrong, and every
    /// later offset would then be wrong too.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when either file cannot be written.
    pub fn append(&self, stream: Stream, bytes: &[u8]) -> Result<LogEntry, LogError> {
        if bytes.is_empty() {
            // A zero-length write put nothing anywhere, so there is nothing to index: an entry
            // claiming a write at an offset would be noise in a file a reader follows. The
            // returned entry still says where the stream ends, which is what a caller asking
            // about an empty chunk wants to know.
            return Ok(LogEntry {
                stream,
                offset: self.end_offset(stream)?,
                len: 0,
                monotonic_ms: self.monotonic_ms(),
            });
        }
        let path = self.path_of(stream).to_path_buf();
        // The offset is the file's current length, read before the write, and it has to be read
        // from the file rather than from the handle's position: an `O_APPEND` handle starts at
        // position 0 and only seeks to the end as part of each `write`, so `stream_position`
        // reports 0 and every entry after the first would claim to start at the beginning.
        let offset = self.end_offset(stream)?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|source| LogError::Io {
                path: path.clone(),
                source,
            })?;
        file.write_all(bytes).map_err(|source| LogError::Io {
            path: path.clone(),
            source,
        })?;
        let entry = LogEntry {
            stream,
            offset,
            len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            monotonic_ms: self.monotonic_ms(),
        };
        self.write_entry(&entry)?;
        Ok(entry)
    }

    /// Reads back everything the index says, in the order it was written.
    ///
    /// Used by a reader that wants the whole log, and by the probe that checks the offsets line
    /// up with the file.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the index cannot be read, and treats a malformed line as an
    /// error rather than skipping it: a truncated index is a defect worth seeing.
    pub fn entries(&self) -> Result<Vec<LogEntry>, LogError> {
        let text = std::fs::read_to_string(&self.index_path).map_err(|source| LogError::Io {
            path: self.index_path.clone(),
            source,
        })?;
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: LogEntry = serde_json::from_str(line).map_err(|error| LogError::Io {
                path: self.index_path.clone(),
                source: io::Error::new(io::ErrorKind::InvalidData, error),
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Milliseconds since the child started.
    fn monotonic_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Where one stream's log currently ends: the offset the next write starts at.
    fn end_offset(&self, stream: Stream) -> Result<u64, LogError> {
        let path = self.path_of(stream).to_path_buf();
        std::fs::metadata(&path)
            .map(|meta| meta.len())
            .map_err(|source| LogError::Io { path, source })
    }

    /// Appends one JSON line to the index.
    fn write_entry(&self, entry: &LogEntry) -> Result<(), LogError> {
        let mut line = serde_json::to_vec(entry).map_err(|error| LogError::Io {
            path: self.index_path.clone(),
            source: io::Error::new(io::ErrorKind::InvalidData, error),
        })?;
        line.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.index_path)
            .map_err(|source| LogError::Io {
                path: self.index_path.clone(),
                source,
            })?;
        file.write_all(&line).map_err(|source| LogError::Io {
            path: self.index_path.clone(),
            source,
        })
    }
}

/// Writes `bytes` to `path`, creating it.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), LogError> {
    std::fs::write(path, bytes).map_err(|source| LogError::Io {
        path: path.to_path_buf(),
        source,
    })
}
