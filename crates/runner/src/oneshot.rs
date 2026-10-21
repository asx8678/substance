//! One-shot repo-controlled commands: install, build, migrate, seed (M2-T02, §4.1).
//!
//! These are the commands that run and exit, as opposed to the long-running server
//! [`crate::AppProcess`] supervises. They need the same two guarantees: `&ExecPermit`, because the
//! repository controls what they run, and a log with a byte-offset index, because a failed install
//! is the first thing a reader of a failed run wants to see.
//!
//! ## Why not `run_tool_capture`
//!
//! `crate::tool` captures both streams into memory. That is right for `git rev-parse`, whose output
//! is a line, and wrong for `npm ci`, which can print megabytes that a reader needs on disk and
//! that would otherwise sit in the run's memory. So this module streams to the log sink and keeps
//! only the tail it needs for an error message.
//!
//! ## The timeout is a safety cap
//!
//! A step that hangs must not hang the run. When the cap fires the step is killed and reports
//! [`OneShotError::Timeout`], which a caller turns into friction rather than a verdict (§4.3: a
//! wall-clock limit never decides a check).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;

use crate::limits::Limits;
use crate::logs::{LogError, LogSink, Stream};
use crate::permit::ExecPermit;
use crate::process::{OWN_ENV, home_env};

/// How much of a step's output is kept for an error message.
///
/// The log file has all of it; this tail is only so an error can show the last few lines, which is
/// where a build's actual complaint usually is.
const TAIL_BYTES: usize = 4096;

/// What a one-shot step produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct StepOutput {
    /// Whether it exited 0.
    pub success: bool,
    /// The exit code, absent when a signal ended it.
    pub code: Option<i32>,
    /// The last few kilobytes of stderr, then stdout, for an error message.
    pub tail: String,
    /// The byte offset where the output of the stream that spoke ended.
    ///
    /// "The stream that spoke" is stderr when it wrote anything and stdout otherwise, because a
    /// failing step explains itself on stderr while a successful one often says nothing on stdout
    /// at all. Measuring stdout alone gave a zero for every failing build, which is the one case
    /// the offset exists for.
    ///
    /// This is what a [`substance_core::model::Friction`] entry cites, so a reader can open the log
    /// at the right place instead of searching it. It is an offset in whichever of the two files the
    /// entry's `tail` came from.
    pub log_end: u64,
    /// How long it ran, for the friction log. Not serialized into a record (ADR-0009).
    #[serde(skip)]
    pub elapsed_ms: u64,
}

/// Why a one-shot step could not be run.
#[derive(Debug, thiserror::Error)]
pub enum OneShotError {
    /// The program is not on `PATH`.
    #[error("{program} is not installed (or not on PATH)")]
    Missing {
        /// The program that was not found.
        program: String,
    },
    /// The step did not finish inside its cap and was killed.
    #[error("{program} did not finish within {seconds}s")]
    Timeout {
        /// The program that timed out.
        program: String,
        /// The cap it exceeded, in whole seconds.
        seconds: u64,
    },
    /// The process could not be started or its output read.
    #[error("cannot run {program}: {source}")]
    Io {
        /// The program that failed.
        program: String,
        /// The IO error.
        source: io::Error,
    },
    /// A log file could not be written.
    #[error(transparent)]
    Log(#[from] LogError),
}

/// One command to run, with the environment and limits it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneShot {
    /// The program.
    pub program: String,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// The directory to run in.
    pub cwd: PathBuf,
    /// Extra environment beyond the fixed list and the run's home, sorted by name.
    pub env: BTreeMap<String, String>,
    /// The run's home directory.
    pub home: Option<PathBuf>,
    /// How to limit it.
    pub limits: Limits,
    /// The name its log files use, for example `install` or `build`.
    pub name: String,
}

impl OneShot {
    /// A step that runs `program` with `args` in `cwd`, logging as `name`.
    #[must_use]
    pub fn new(program: &str, args: &[String], cwd: &Path, name: &str) -> Self {
        Self {
            program: program.to_owned(),
            args: args.to_vec(),
            cwd: cwd.to_path_buf(),
            env: BTreeMap::new(),
            home: None,
            limits: Limits::Unavailable {
                why: "this step did not say how to limit its command",
            },
            name: name.to_owned(),
        }
    }

    /// A step from a program and string arguments.
    #[must_use]
    pub fn from_parts(program: &str, args: &[&str], cwd: &Path, name: &str) -> Self {
        Self::new(
            program,
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            cwd,
            name,
        )
    }

    /// Points the step's home and caches at `home`.
    #[must_use]
    pub fn with_home(mut self, home: &Path) -> Self {
        self.home = Some(home.to_path_buf());
        self
    }

    /// Limits the step.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Adds one environment variable.
    #[must_use]
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_owned(), value.to_owned());
        self
    }

    /// The command as one line, for a friction entry.
    #[must_use]
    pub fn display(&self) -> String {
        let mut text = self.program.clone();
        for arg in &self.args {
            text.push(' ');
            text.push_str(arg);
        }
        text
    }
}

/// Runs one step to completion, streaming its output to `logs_dir`.
///
/// A nonzero exit is **not** an error: a failed build is a fact the caller records as friction and
/// reasons about. Only a step that could not run (missing program, timeout, IO failure) is an
/// `Err`.
///
/// # Errors
///
/// Returns [`OneShotError::Missing`], [`OneShotError::Timeout`], [`OneShotError::Io`] or
/// [`OneShotError::Log`].
///
pub async fn run(
    step: &OneShot,
    logs_dir: &Path,
    cap: Duration,
    _permit: &ExecPermit,
) -> Result<StepOutput, OneShotError> {
    let sink = LogSink::create(logs_dir, &step.name, Instant::now())?;
    let (program, args) = step.limits.wrap(&step.program, &step.args);
    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(&step.cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (name, value) in OWN_ENV {
        command.env(name, value);
    }
    if let Some(home) = &step.home {
        for (name, value) in home_env(home) {
            command.env(name, value);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    for (name, value) in &step.env {
        command.env(name, value);
    }

    let started = Instant::now();
    let mut child = command.spawn().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            OneShotError::Missing {
                program: step.program.clone(),
            }
        } else {
            OneShotError::Io {
                program: step.program.clone(),
                source,
            }
        }
    })?;

    // The pumps run concurrently with the wait, so a step that fills the pipe buffer cannot block
    // on its own output. Each keeps the last `TAIL_BYTES` for the error message.
    let out_sink = sink.clone();
    let mut out_pipe = child.stdout.take();
    let out_pump = async move {
        let mut tail = Vec::new();
        let mut buffer = vec![0_u8; 8192];
        while let Some(pipe) = out_pipe.as_mut() {
            match pipe.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    if out_sink.append(Stream::Stdout, chunk).is_err() {
                        break;
                    }
                    tail.extend_from_slice(chunk);
                    if tail.len() > TAIL_BYTES {
                        let cut = tail.len() - TAIL_BYTES;
                        tail.drain(..cut);
                    }
                }
            }
        }
        tail
    };
    let err_sink = sink.clone();
    let mut err_pipe = child.stderr.take();
    let err_pump = async move {
        let mut tail = Vec::new();
        let mut buffer = vec![0_u8; 8192];
        while let Some(pipe) = err_pipe.as_mut() {
            match pipe.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    if err_sink.append(Stream::Stderr, chunk).is_err() {
                        break;
                    }
                    tail.extend_from_slice(chunk);
                    if tail.len() > TAIL_BYTES {
                        let cut = tail.len() - TAIL_BYTES;
                        tail.drain(..cut);
                    }
                }
            }
        }
        tail
    };

    // The pumps are joined only after the child is done, and a timed-out child is killed before
    // that join: joining them first would mean waiting for a pipe that the still-running child
    // holds open, which turned a one-second cap into a thirty-second wait. The order here is the
    // whole reason this is not a `join!`.
    let status = match tokio::time::timeout(cap, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(source)) => {
            return Err(OneShotError::Io {
                program: step.program.clone(),
                source,
            });
        }
        Err(_) => {
            // SIGKILL, then reap: the step is not a server with grandchildren, so its own pid is
            // the whole tree that matters. `kill_on_drop` would do this too, but only after the
            // pump join below, which is the wait being avoided.
            let _ = child.start_kill();
            let _ = child.wait().await;
            let (_, out_tail, err_tail) = tokio::join!(async {}, out_pump, err_pump);
            let _ = (&out_tail, &err_tail);
            return Err(OneShotError::Timeout {
                program: step.program.clone(),
                seconds: cap.as_secs(),
            });
        }
    };
    let (_, out_tail, err_tail) = tokio::join!(async {}, out_pump, err_pump);
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let entries = sink.entries().unwrap_or_default();
    let end_of = |stream: Stream| {
        entries
            .iter()
            .filter(|entry| entry.stream == stream)
            .map(|entry| entry.offset.saturating_add(entry.len))
            .max()
            .unwrap_or(0)
    };
    let stderr_end = end_of(Stream::Stderr);
    let log_end = match stderr_end {
        0 => end_of(Stream::Stdout),
        end => end,
    };
    let mut tail = String::from_utf8_lossy(&err_tail).into_owned();
    if tail.trim().is_empty() {
        tail = String::from_utf8_lossy(&out_tail).into_owned();
    }
    Ok(StepOutput {
        success: status.success(),
        code: status.code(),
        tail,
        log_end,
        elapsed_ms,
    })
}
