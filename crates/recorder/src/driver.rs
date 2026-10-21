//! The driver host: one child process, one command at a time, until it dies (M3-T02).
//!
//! The driver (`driver/dist/main.js`) is Node speaking one JSON command per line on stdin and
//! answering with JSON events on stdout. This module owns the other end of that pipe: it writes a
//! line, reads until the line's `done`, and enforces a deadline while doing it.
//!
//! Two rules shape the whole file.
//!
//! **A command is a transaction.** The driver answers every input line with exactly one `done`,
//! so the host counts `done`s rather than guessing when a command is finished. A line that
//! produced no `done` leaves the stream in an unknown state, which is why a timeout is followed by
//! a kill rather than by reading further: the next `done` might belong to the command that timed
//! out.
//!
//! **A dead driver is a missing channel, not a failed run.** After one restart that also fails,
//! the host stops trying and every later call returns [`Measured::not_measured`]. A recorder that
//! cannot see the browser has not learned that the app is broken, and saying so is the whole point
//! of [`Measured`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use substance_core::model::{DriverCommand, DriverResult, Measured, RecordPart};
use substance_runner::process::{ProcessError, ProcessSpec, ServiceProcess};
use tokio::sync::mpsc;

/// How long one command may take before the host gives up on it.
///
/// Derived from the settle cap rather than fixed: an `act` cannot answer sooner than the page
/// settles, so a deadline shorter than the settle cap would time out on a correct action. The
/// multiplier leaves room for the browser to start, the navigation to load, and the round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverTimeouts {
    /// The cap on one command.
    pub command: Duration,
    /// How long the driver gets to exit before it is killed.
    pub shutdown: Duration,
}

impl DriverTimeouts {
    /// Timeouts for a run whose settle cap is `settle_timeout_ms`.
    #[must_use]
    pub fn for_settle(settle_timeout_ms: u32, shutdown_grace_s: u32) -> Self {
        let settle = u64::from(settle_timeout_ms);
        Self {
            // Three settles' worth, plus ten seconds: an `open_context` has to launch a browser,
            // and a `goto` has to load a page before it can even begin to settle.
            command: Duration::from_millis(settle.saturating_mul(3).saturating_add(10_000)),
            shutdown: Duration::from_secs(u64::from(shutdown_grace_s)),
        }
    }
}

impl Default for DriverTimeouts {
    fn default() -> Self {
        let budgets = substance_core::config::Budgets::default();
        Self::for_settle(budgets.settle_timeout_ms, budgets.shutdown_grace_s)
    }
}

/// Why the host could not answer.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The driver could not be started, or a write failed.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// A line on stdout was not a driver event.
    ///
    /// A protocol error, not an app failure: the two programs disagree about their own format,
    /// and every later record from this stream would be untrustworthy.
    #[error("driver protocol: {detail}")]
    Protocol {
        /// What was wrong, with the offending line summarized.
        detail: String,
    },
    /// The command did not answer inside its deadline.
    #[error("driver did not answer {cmd} within {}s", .seconds)]
    Timeout {
        /// The command that timed out.
        cmd: String,
        /// The deadline it was given.
        seconds: u64,
    },
    /// The driver exited while a command was outstanding.
    #[error("driver exited while running {cmd}")]
    Exited {
        /// The command that was in flight.
        cmd: String,
    },
    /// The host is dead: it already restarted once and failed again.
    #[error("driver crashed: {reason}")]
    Dead {
        /// What went wrong the second time.
        reason: String,
    },
}

impl DriverError {
    /// Whether a restart could plausibly fix this.
    ///
    /// The driver is a separate process with its own memory; a crashed browser, a closed page or
    /// a killed Node are all fixed by starting one more. A protocol error is not: starting again
    /// gives the same disagreement.
    #[must_use]
    pub fn is_crash(&self) -> bool {
        match self {
            Self::Exited { .. } | Self::Timeout { .. } => true,
            // A broken pipe is how a dead child announces itself to the next write, and the
            // driver's stdin is the only thing substance writes. Reading it as "the program is
            // missing" would leave a host with a corpse for a child, alive by its own account,
            // failing every command with an error no restart was ever tried for.
            Self::Process(ProcessError::Io { source, .. }) => {
                source.kind() == std::io::ErrorKind::BrokenPipe
            }
            Self::Process(_) | Self::Protocol { .. } | Self::Dead { .. } => false,
        }
    }
}

/// Where the driver program lives.
///
/// Resolved from the installation, not from the working directory: the driver runs inside a work
/// copy of the app under test, which of course has no `driver/` of its own. `SUBSTANCE_DRIVER`
/// overrides it, for an installed layout or for the probes.
#[must_use]
pub fn driver_program() -> PathBuf {
    if let Ok(path) = std::env::var("SUBSTANCE_DRIVER") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../driver/dist/main.js")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../driver/dist/main.js"))
}

/// Where to start the driver, and how long to wait for it.
#[derive(Debug, Clone)]
pub struct DriverConfig {
    /// The driver program: `driver/dist/main.js` for real, a fixture for a probe.
    pub program: PathBuf,
    /// The cap on one command and on the driver's shutdown.
    pub timeouts: DriverTimeouts,
}

impl DriverConfig {
    /// The configured driver in the installation, with the given timeouts.
    #[must_use]
    pub fn new(timeouts: DriverTimeouts) -> Self {
        Self {
            program: driver_program(),
            timeouts,
        }
    }

    /// A specific program, for a probe that needs to drive a fixture.
    #[must_use]
    pub fn with_program(program: impl Into<PathBuf>, timeouts: DriverTimeouts) -> Self {
        Self {
            program: program.into(),
            timeouts,
        }
    }
}

/// One event, as it arrived.
#[derive(Debug, Clone)]
enum Event {
    /// The protocol's `ready`, sent once when the driver starts.
    Ready,
    /// The protocol's `done`: this line's command is finished.
    Done,
    /// A record part from an action.
    RecordPart(Box<RecordPart>),
    /// A result from a command that answers with data.
    Result(Box<DriverResult>),
    /// A failure the driver reported.
    Error {
        /// The command the driver blamed.
        cmd: String,
        /// Its message, with session secrets already replaced.
        message: String,
        /// Whether the driver kept its context.
        recoverable: bool,
    },
}

/// A running driver, with the state that decides whether it can still be used.
#[derive(Debug)]
pub struct DriverHost {
    /// The child, or `None` after it died and could not be restarted.
    process: Option<ServiceProcess>,
    /// Parsed stdout lines.
    events: mpsc::Receiver<String>,
    /// What stdout was last seen as: the deadline has to cover the read, not just the write.
    timeouts: DriverTimeouts,
    /// The spec, kept so a restart starts the same command.
    spec: ProcessSpec,
    /// The logs directory, for a restart.
    logs_dir: PathBuf,
    /// The caller's `open_context`, kept so a restart can reopen the context.
    ///
    /// Without this a restart would leave the host with a live driver and no page, and every
    /// later command would fail for a reason that has nothing to do with the app.
    open: Option<DriverCommand>,
    /// How many restarts have been attempted.
    restarts: u8,
    /// Why the host is dead, once it is.
    dead: Option<String>,
}

impl DriverHost {
    /// Starts a driver and waits for its `ready`.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Process`] when the program cannot be started (Node missing, the
    /// `dist` not built) and [`DriverError::Protocol`] when it does not say `ready` first.
    pub async fn spawn(
        run_dir: &Path,
        logs_dir: &Path,
        config: &DriverConfig,
    ) -> Result<Self, DriverError> {
        let logs_dir = logs_dir.to_path_buf();
        let timeouts = config.timeouts;
        let spec = driver_spec(&config.program, run_dir);
        let (process, events) = start(&spec, &logs_dir, timeouts).await?;
        let mut host = Self {
            process: Some(process),
            events,
            timeouts,
            spec,
            logs_dir,
            open: None,
            restarts: 0,
            dead: None,
        };
        host.await_ready().await?;
        Ok(host)
    }

    /// Whether the host can still answer. `false` means every call reports not_measured.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.dead.is_none() && self.process.is_some()
    }

    /// Why the host is dead, when it is.
    #[must_use]
    pub fn dead_reason(&self) -> Option<&str> {
        self.dead.as_deref()
    }

    /// Sends one command and reads its events up to and including the `done`.
    ///
    /// A command that returns data answers with a [`DriverResult`]; one that observes an action
    /// answers with a [`RecordPart`]. The events between are reported through `on_event` in the
    /// order the driver sent them, which is the only place an `error` event can be seen without
    /// being turned into a failure: a page that logs a console error is not a failed command.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Dead`] once a restart has already failed, [`DriverError::Timeout`]
    /// when the deadline fires, [`DriverError::Exited`] when the driver dies mid-command, and
    /// [`DriverError::Protocol`] on a line that is not an event.
    pub async fn send(&mut self, command: &DriverCommand) -> Result<DriverResponse, DriverError> {
        let cmd_name = command_name(command);
        if let Some(reason) = &self.dead {
            return Err(DriverError::Dead {
                reason: reason.clone(),
            });
        }

        match self.exchange(command).await {
            Ok(response) => Ok(response),
            Err(error) if error.is_crash() && self.restarts == 0 => {
                tracing::warn!(cmd = %cmd_name, %error, "driver failed; restarting once");
                self.restarts = 1;
                self.restart(&error.to_string()).await?;
                self.exchange(command).await
            }
            Err(error) if error.is_crash() => {
                self.mark_dead(&error.to_string());
                Err(DriverError::Dead {
                    reason: error.to_string(),
                })
            }
            Err(error) => Err(error),
        }
    }

    /// Sends one command and reports a crash as [`Measured::not_measured`].
    ///
    /// This is the shape the recorder uses: it has a record to fill either way, and "the browser
    /// died" is a fact about the channel rather than a reason to abandon the action.
    pub async fn record(&mut self, command: &DriverCommand) -> Measured<RecordPart> {
        match self.send(command).await {
            Ok(DriverResponse {
                part: Some(part), ..
            }) => Measured::observed(*part),
            Ok(DriverResponse {
                error: Some(error), ..
            }) => Measured::not_measured(format!("driver reported {error}")),
            Ok(DriverResponse {
                part: None,
                error: None,
                ..
            }) => Measured::not_measured("the driver answered without a record part"),
            Err(error) => Measured::not_measured(error.to_string()),
        }
    }

    /// Stops the driver. Safe to call twice, and safe to call when it is already dead.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] when the group cannot be signalled.
    pub async fn stop(&mut self) -> Result<(), ProcessError> {
        if let Some(mut process) = self.process.take() {
            process.stop(self.timeouts.shutdown).await?;
        }
        self.consolidate_log();
        Ok(())
    }

    /// Writes `logs/driver.log`, the one file the plan names, from the driver's stderr.
    ///
    /// The runner logs every child the same way: `driver.stdout.log`, `driver.stderr.log` and an
    /// `index.jsonl` of byte offsets. That split is not decoration -- M3-T03 slices a server log by
    /// the offsets in the index, and the stderr file cannot be read without it. But the plan says
    /// the driver's stderr goes to `logs/driver.log`, and a person debugging a run should not have
    /// to know which of three files to open for the driver's complaints. So both exist: the indexed
    /// pair for the recorder, and this copy for the reader. It is a copy of the bytes, not a
    /// summary, so the two can never disagree.
    fn consolidate_log(&self) {
        let source = self.logs_dir.join("driver.stderr.log");
        let Ok(bytes) = std::fs::read(&source) else {
            // No stderr file means the child never started, which the caller has already been
            // told by the error that brought it here.
            return;
        };
        let _ = std::fs::write(self.logs_dir.join("driver.log"), bytes);
    }

    /// The exchange itself, with no restart logic.
    async fn exchange(&mut self, command: &DriverCommand) -> Result<DriverResponse, DriverError> {
        let cmd_name = command_name(command);
        let line = serde_json::to_string(command).map_err(|error| DriverError::Protocol {
            detail: format!("cannot serialize {cmd_name}: {error}"),
        })?;

        // Scoped so the borrow of `self.process` ends before the loop: the timeout arm has to
        // kill the child, and a live borrow would make that impossible exactly when it is needed.
        {
            let Some(process) = self.process.as_mut() else {
                return Err(DriverError::Exited { cmd: cmd_name });
            };
            process
                .write_stdin(format!("{line}\n").as_bytes())
                .await
                .map_err(DriverError::Process)?;
        }

        // Remembered before the answer, so a restart can reopen the same context.
        if matches!(command, DriverCommand::OpenContext(_)) {
            self.open = Some(command.clone());
        }

        let mut response = DriverResponse::default();
        // One deadline for the whole command, not one per event: a driver that answers in a
        // trickle would otherwise reset the clock with every line and never time out.
        let deadline = tokio::time::Instant::now() + self.timeouts.command;
        loop {
            let line = match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(line)) => line,
                Ok(None) => {
                    return Err(DriverError::Exited { cmd: cmd_name });
                }
                Err(_) => {
                    // The answer may still arrive. Killing is deliberate: a late answer would be
                    // read while the host waits for the next command's `done`, and the two
                    // responses would be attributed to the wrong commands. A fresh process is the
                    // only state in which the next answer is trustworthy.
                    self.kill().await;
                    return Err(DriverError::Timeout {
                        cmd: cmd_name,
                        seconds: self.timeouts.command.as_secs(),
                    });
                }
            };

            // A protocol error is not repairable by restarting, but the stream is now
            // untrustworthy, so the driver is stopped rather than left desynchronized.
            let event = match parse_event(&line) {
                Ok(event) => event,
                Err(error) => {
                    self.kill().await;
                    return Err(error);
                }
            };
            match event {
                Event::Ready => {
                    // Only expected at start; a second one means the driver restarted itself
                    // behind the host's back, which the next command would misattribute.
                    return Err(DriverError::Protocol {
                        detail: "unexpected ready while a command was in flight".to_owned(),
                    });
                }
                Event::Done => {}
                Event::RecordPart(part) => response.part = Some(part),
                Event::Result(result) => response.result = Some(result),
                Event::Error {
                    cmd,
                    message,
                    recoverable,
                } => {
                    response.error = Some(message);
                    response.error_cmd = Some(cmd);
                    response.recoverable = recoverable;
                }
            }

            if is_done(&line) {
                break;
            }
        }
        Ok(response)
    }

    /// Waits for the `ready` the driver sends once, before any command.
    async fn await_ready(&mut self) -> Result<(), DriverError> {
        let deadline = self.timeouts.command;
        let line = match tokio::time::timeout(deadline, self.events.recv()).await {
            Ok(Some(line)) => line,
            Ok(None) => {
                return Err(DriverError::Protocol {
                    detail: "the driver exited before saying ready".to_owned(),
                });
            }
            Err(_) => {
                return Err(DriverError::Protocol {
                    detail: format!(
                        "the driver did not say ready within {}s",
                        deadline.as_secs()
                    ),
                });
            }
        };
        match parse_event(&line)? {
            Event::Ready => Ok(()),
            other => Err(DriverError::Protocol {
                detail: format!("the first line was not ready: {other:?}"),
            }),
        }
    }

    /// Starts a fresh driver and reopens the context it had, if it had one.
    async fn restart(&mut self, cause: &str) -> Result<(), DriverError> {
        self.kill().await;
        tracing::warn!(cause, "restarting the driver");
        let (process, events) = match start(&self.spec, &self.logs_dir, self.timeouts).await {
            Ok(started) => started,
            Err(error) => {
                self.mark_dead(&error.to_string());
                return Err(DriverError::Dead {
                    reason: format!("restart failed: {error}"),
                });
            }
        };
        self.process = Some(process);
        self.events = events;
        // The context lived in the process that died, so it is gone with it. Reopening it is the
        // difference between a restart that helps and one that leaves every command failing for a
        // reason that has nothing to do with the app.
        if let Some(open) = self.open.clone() {
            if let Err(error) = self.await_ready_and_open(&open).await {
                self.mark_dead(&error.to_string());
                return Err(DriverError::Dead {
                    reason: error.to_string(),
                });
            }
        } else if let Err(error) = self.await_ready().await {
            self.mark_dead(&error.to_string());
            return Err(DriverError::Dead {
                reason: error.to_string(),
            });
        }
        Ok(())
    }

    /// Reads `ready` and then replays `open_context`, without the restart logic that would recurse.
    async fn await_ready_and_open(&mut self, open: &DriverCommand) -> Result<(), DriverError> {
        self.await_ready().await?;
        // Deliberately `exchange`, not `send`: a restart is already in progress, and a nested
        // restart would recurse until the stack ran out.
        self.exchange(open).await.map(|_| ())
    }

    /// Kills the child without the grace period, for a command that already timed out.
    async fn kill(&mut self) {
        if let Some(mut process) = self.process.take() {
            // A short grace: the command is already overdue, and waiting the full shutdown
            // window would spend the budget of the command that comes next.
            let _ = process.stop(Duration::from_secs(2)).await;
        }
    }

    fn mark_dead(&mut self, reason: &str) {
        tracing::error!(reason, "the driver host is dead");
        self.consolidate_log();
        self.dead = Some(reason.to_owned());
        self.process = None;
    }
}

/// What one command produced.
#[derive(Debug, Clone, Default)]
pub struct DriverResponse {
    /// The record part, for a command that observed an action.
    pub part: Option<Box<RecordPart>>,
    /// The result, for a command that answers with data.
    pub result: Option<Box<DriverResult>>,
    /// The driver's error message, when it reported one.
    pub error: Option<String>,
    /// The command the driver blamed, when it reported an error.
    pub error_cmd: Option<String>,
    /// Whether the driver said the context survived.
    pub recoverable: bool,
}

impl DriverResponse {
    /// The record part, or why there is none.
    #[must_use]
    pub fn into_part(self) -> Measured<RecordPart> {
        match (self.part, self.error) {
            (Some(part), _) => Measured::observed(*part),
            (None, Some(error)) => Measured::not_measured(format!("driver reported {error}")),
            (None, None) => Measured::not_measured("the driver answered without a record part"),
        }
    }
}

/// The spec for the driver, in the run's directory.
///
/// `SUBSTANCE_BLOB_DIR` is passed rather than the driver's own `open_context`, because the blob
/// directory is a property of the run and not of the command: a caller that forgot it would write
/// bodies nowhere.
fn driver_spec(program: &Path, run_dir: &Path) -> ProcessSpec {
    let directive = program.display().to_string();
    let mut spec = ProcessSpec::new(
        "node",
        &[directive.as_str()],
        run_dir,
        // The log name the plan uses: `logs/driver.log`.
        "driver",
    );
    // Both pipes: the protocol runs on stdin and stdout, and stderr is the log.
    spec.piped_stdin = true;
    spec.piped_stdout = true;
    spec
}

/// Starts the child and the task that parses its stdout into lines.
///
/// The pump is a task, not a loop here: the driver answers only after a command is written, so a
/// function that drained stdout to completion before returning would wait for a line that cannot
/// be sent until it returns.
async fn start(
    spec: &ProcessSpec,
    logs_dir: &Path,
    timeouts: DriverTimeouts,
) -> Result<(ServiceProcess, mpsc::Receiver<String>), DriverError> {
    let mut process = ServiceProcess::spawn(
        spec,
        logs_dir,
        substance_runner::Budget::after("driver", timeouts.command),
    )
    .await?;
    // The receiver is taken rather than read through the process, so the host can write to stdin
    // while the pump reads stdout: one borrow of the process cannot span both halves.
    let stdout = process.take_stdout();
    let (sender, receiver) = mpsc::channel(64);
    if let Some(mut stdout) = stdout {
        tokio::spawn(async move {
            let mut buffer = Vec::new();
            while let Some(mut chunk) = stdout.recv().await {
                buffer.append(&mut chunk);
                // A chunk can hold half a line or five lines; the protocol only promises that a
                // line ends with a newline, never that a read ends there.
                while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=index).collect();
                    let text = String::from_utf8_lossy(&line).trim().to_owned();
                    if text.is_empty() {
                        continue;
                    }
                    // A receiver that was dropped means the host is finished with the driver; the
                    // pump ends rather than growing a buffer nobody reads.
                    if sender.send(text).await.is_err() {
                        return;
                    }
                }
            }
            // The child closed stdout. A partial last line is still a line for the protocol's
            // purposes: the driver writes whole lines, so anything here is the last one before it
            // died, and reporting it is better than dropping the evidence.
            let tail = String::from_utf8_lossy(&buffer).trim().to_owned();
            if !tail.is_empty() {
                let _ = sender.send(tail).await;
            }
        });
    }
    Ok((process, receiver))
}

/// Reads a driver event out of one line.
fn parse_event(line: &str) -> Result<Event, DriverError> {
    let value: Value = serde_json::from_str(line).map_err(|error| DriverError::Protocol {
        detail: format!("not JSON: {error}: {}", summarize(line)),
    })?;
    let kind = value
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(|| DriverError::Protocol {
            detail: format!("no \"event\" string: {}", summarize(line)),
        })?
        .to_owned();

    // The payload is a `RecordPart` or a `DriverResult` flattened beside its tag, so the tag is
    // removed before the payload is deserialized: `RecordPart` has `deny_unknown_fields` and would
    // otherwise reject `event` as a field it does not have. A tagged `DriverEvent` would not work
    // here either -- `RecordPart` is not a newtype variant of it, so serde_json cannot match the
    // flattened shape to it. Doing the removal is cheaper than a wrapper type whose only job is to
    // be deserialized once.
    let payload = || -> Result<Value, DriverError> {
        let mut object = match value.clone() {
            Value::Object(object) => object,
            _ => {
                return Err(DriverError::Protocol {
                    detail: format!("an event must be an object: {}", summarize(line)),
                });
            }
        };
        object.remove("event");
        Ok(Value::Object(object))
    };

    match kind.as_str() {
        "ready" => Ok(Event::Ready),
        "record_part" => serde_json::from_value::<RecordPart>(payload()?)
            .map(|part| Event::RecordPart(Box::new(part)))
            .map_err(|error| DriverError::Protocol {
                detail: format!("record_part does not match the schema: {error}"),
            }),
        "result" => serde_json::from_value::<DriverResult>(payload()?)
            .map(|result| Event::Result(Box::new(result)))
            .map_err(|error| DriverError::Protocol {
                detail: format!("result does not match the schema: {error}"),
            }),
        "error" => serde_json::from_value::<substance_core::model::DriverError>(payload()?)
            .map(|error| Event::Error {
                cmd: error.cmd,
                message: error.message,
                recoverable: error.recoverable,
            })
            .map_err(|error| DriverError::Protocol {
                detail: format!("error event does not match the schema: {error}"),
            }),
        "done" => Ok(Event::Done),
        other => Err(DriverError::Protocol {
            detail: format!("unknown event {other:?}"),
        }),
    }
}

fn is_done(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("event")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|kind| kind == "done")
}

/// A command's name, for messages.
fn command_name(command: &DriverCommand) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|value| value.get("cmd").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "command".to_owned())
}

/// The first part of a line, for an error message about a line that is not one.
fn summarize(line: &str) -> String {
    let trimmed = line.trim();
    match trimmed.char_indices().nth(200) {
        Some((index, _)) => format!("{}… ({} bytes)", &trimmed[..index], trimmed.len()),
        None => trimmed.to_owned(),
    }
}
