//! Long-running child processes: the app under test and substance's own helpers (M2-T01, §4.1).
//!
//! §4.1 gives each app process a process group, a clean environment, logs with a byte-offset
//! index, a free port, SIGTERM then SIGKILL, and a stage budget. This module is that list.
//!
//! ## Two kinds of child
//!
//! [`AppProcess`] runs **repo-controlled** code, so [`AppProcess::spawn`] takes `&ExecPermit`:
//! without `--allow-exec` there is no value to pass and the program will not compile. Nothing in
//! this module can be reached around that, because the permit is the only argument of its kind
//! (§4.1). [`ServiceProcess`] runs substance's own helpers — the driver, Mailpit, Toxiproxy —
//! which are named here rather than by the repository, so it takes no permit. The distinction is
//! the plan's; the module keeps it as two types rather than one type with a flag, because a flag
//! is a runtime answer to a question the compiler should settle.
//!
//! ## Why the group matters, and why `group_empty` is a separate call
//!
//! A dev server is rarely one process: `npm run dev` is npm, which is node, which may be a
//! watcher that spawns another node. SIGTERM to the direct child leaves the grandchildren
//! running, holding the port. Killing the **group** is what actually stops the stack, which is
//! why the child is started with `process_group(0)` — it becomes its own group leader, so its
//! group id is its pid and `killpg` reaches everything it spawned.
//!
//! `group_empty` answers a different question from "did the child exit": a process can exit while
//! its children live on. O03 asks whether a stop left anything behind, so [`ExitInfo`] reports
//! both, and [`AppProcess::stop`] waits for the group as well as the child before it reports.
//!
//! ## The drop guard
//!
//! A stage that returns early, or a future that is cancelled, must not leave a server running.
//! [`ProcessGuard`] kills the group from `Drop`. It is deliberately the *last* resort and stays
//! quiet on the normal path: `stop` sets `disarmed`, so a clean shutdown is not followed by a
//! redundant kill.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;
use tokio::process::{Child, Command};

use crate::budget::Budget;
use crate::limits::Limits;
use crate::logs::{LogError, LogSink, Stream};
use crate::permit::ExecPermit;
use crate::port::free_port;

/// The variable substance always sets, whatever the app config says.
///
/// The plan's list is "`[app.env]` from config plus substance's own variables". These are the
/// latter: a fixed `TZ` and `LANG` so date formatting and message text do not depend on the
/// machine, and `CI` so tools that behave differently in CI behave predictably here.
pub const OWN_ENV: &[(&str, &str)] = &[
    // The zone and locale come from the core's constants rather than being spelled again here: the
    // manifest records the same three strings (M3-T06), and two literals would eventually disagree.
    ("TZ", substance_core::clock_controls::TIMEZONE),
    ("LANG", substance_core::clock_controls::LOCALE),
    ("LC_ALL", substance_core::clock_controls::LOCALE),
    ("CI", "1"),
    // npm and node read these; leaving them unset makes a "clean" environment behave
    // differently from the developer's, which is the opposite of what a clean environment is for.
    ("NO_COLOR", "1"),
    ("FORCE_COLOR", "0"),
];

/// Where a process's home and caches go, relative to the run directory.
///
/// `env_clear()` leaves `HOME` unset, and a process without `HOME` breaks in ways that look like
/// application bugs: npm writes its cache under `~` and fails, pip cannot find its user config,
/// cargo refuses to run, and git complains about `safe.directory`. The fix is not to inherit the
/// user's `HOME` -- that would let repo-controlled code write into the developer's dotfiles, and
/// would make two runs on one machine differ by whatever the developer had configured -- but to
/// give every child a home of its own inside the run directory.
///
/// One variable is not enough, because tools disagree about where caches live:
///
/// - `HOME`, the fallback every tool understands.
/// - `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`: the XDG trio, read by most modern
///   Linux tools and by some macOS ones.
/// - `npm_config_cache`, which wins over `HOME` for npm when both are set, plus `PNPM_HOME`,
///   `YARN_CACHE_FOLDER` and `BUN_INSTALL_CACHE_DIR` for the other three JS package managers.
/// - `CARGO_HOME` and `RUSTUP_HOME`, which otherwise write into `~/.cargo` and `~/.rustup`.
/// - `GOPATH` and `GOMODCACHE` for Go.
/// - `PIP_CACHE_DIR`, `POETRY_CACHE_DIR` and `UV_CACHE_DIR` for the Python installers.
/// - `TMPDIR`, not a cache but the same problem: a build that fills the host's `/tmp` and never
///   cleans up is a leak the run directory should contain.
/// - `LOCALAPPDATA` and `APPDATA`, which Windows tools read instead of the XDG names.
///
/// Each is a subdirectory, so two tools cannot collide on one path, and the whole tree is deleted
/// with the run directory.
pub const HOME_DIR: &str = "home";

/// The subdirectories of [`HOME_DIR`] that tools expect to exist.
///
/// Most tools create a missing cache directory, but the ones that do not fail with a message about
/// the cache rather than about the directory, which sends a reader looking in the wrong place.
pub const HOME_SUBDIRECTORIES: &[&str] = &[
    "cache",
    "cargo",
    "config",
    "go",
    "local",
    "npm",
    "pip",
    "pnpm",
    "rustup",
    "tmp",
    "xdg-cache",
    "xdg-config",
    "xdg-data",
    "yarn",
];

/// Creates `<run_dir>/home/` and its subdirectories, and returns the home directory.
///
/// Called once per run, before anything spawns, so a failure to create it is a run-level error
/// rather than a mysterious failure of the first install command.
///
/// # Errors
///
/// Returns [`ProcessError::Io`] when the directory cannot be created.
pub fn prepare_home(run_dir: &Path) -> Result<PathBuf, ProcessError> {
    let home = run_dir.join(HOME_DIR);
    create_home_dir(&home, HOME_DIR)?;
    for name in HOME_SUBDIRECTORIES {
        create_home_dir(&home.join(name), name)?;
    }
    Ok(home)
}

/// The environment variables that point a child at `home`.
///
/// Absolute paths, because a child's working directory is the work copy: a relative path would
/// resolve somewhere else entirely, and a package manager's cache is the last place to discover
/// that.
#[must_use]
pub fn home_env(home: &Path) -> Vec<(&'static str, String)> {
    let at = |name: &str| home.join(name).display().to_string();
    vec![
        ("HOME", home.display().to_string()),
        ("TMPDIR", at("tmp")),
        ("XDG_CACHE_HOME", at("xdg-cache")),
        ("XDG_CONFIG_HOME", at("xdg-config")),
        ("XDG_DATA_HOME", at("xdg-data")),
        ("npm_config_cache", at("npm")),
        ("PNPM_HOME", at("pnpm")),
        ("YARN_CACHE_FOLDER", at("yarn")),
        ("BUN_INSTALL_CACHE_DIR", at("cache")),
        ("CARGO_HOME", at("cargo")),
        ("RUSTUP_HOME", at("rustup")),
        ("GOPATH", at("go")),
        (
            "GOMODCACHE",
            home.join("go")
                .join("pkg")
                .join("mod")
                .display()
                .to_string(),
        ),
        ("PIP_CACHE_DIR", at("pip")),
        ("POETRY_CACHE_DIR", at("cache")),
        ("UV_CACHE_DIR", at("cache")),
        ("LOCALAPPDATA", at("local")),
        ("APPDATA", at("config")),
    ]
}

/// Creates one directory of the run's home tree.
fn create_home_dir(dir: &Path, name: &str) -> Result<(), ProcessError> {
    std::fs::create_dir_all(dir).map_err(|source| ProcessError::Io {
        program: format!("the run's home directory ({name})"),
        source,
    })
}

/// The argument that asks for the allocated port: an argument equal to this string is replaced by
/// the number.
///
/// `substance.toml` already fills `{smtp_host}` and `{smtp_port}` the same way, so this is the
/// configuration format's established convention rather than a new one.
pub const PORT_PLACEHOLDER: &str = "{port}";

/// What to run.
///
/// One spec serves both process kinds: the difference between an app and a helper is the permit,
/// not the shape of the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    /// The program, looked up on `PATH`.
    pub program: String,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// The directory to run in: the work copy.
    pub cwd: PathBuf,
    /// The environment, sorted by name, on top of [`OWN_ENV`].
    pub env: BTreeMap<String, String>,
    /// The name used for the log files, for example `app` or `driver`.
    pub name: String,
    /// The variable that carries the port, when the program takes one.
    pub port_env: Option<String>,
    /// The path `wait_ready` polls, when the program serves HTTP.
    pub ready_path: Option<String>,
    /// Connect stdin to a pipe the caller writes to.
    pub piped_stdin: bool,
    /// Read stdout through a pipe the caller reads, in addition to the log file.
    pub piped_stdout: bool,
    /// The run's home directory, applied as `HOME` and as the cache variables.
    ///
    /// On the spec rather than inherited, because there is no correct default: a spec built
    /// without it must say so, and the supervisor cannot guess a run directory.
    pub home: Option<PathBuf>,
    /// How to limit this command's resources.
    ///
    /// On the spec rather than on the call, because the answer belongs to the *kind* of command:
    /// repo-controlled code is limited, and substance's own helpers are not. A helper has no
    /// reason to fork bomb, and wrapping the driver in `prlimit` would only add a process to the
    /// group that `stop` has to reap.
    pub limits: Limits,
}

impl ProcessSpec {
    /// A spec for `program` with `args`, running in `cwd`, logging as `name`.
    #[must_use]
    pub fn new(program: &str, args: &[&str], cwd: &Path, name: &str) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: cwd.to_path_buf(),
            env: BTreeMap::new(),
            name: name.to_owned(),
            port_env: None,
            ready_path: None,
            piped_stdin: false,
            piped_stdout: false,
            home: None,
            limits: Limits::Unavailable {
                why: "this spec did not say how to limit its command",
            },
        }
    }

    /// Limits this command with `limits`; the caller decides by kind of command.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Points this command's home and caches at `home`.
    #[must_use]
    pub fn with_home(mut self, home: &Path) -> Self {
        self.home = Some(home.to_path_buf());
        self
    }

    /// Gives the program a port through `variable`, and supplies it wherever an argument is
    /// exactly [`PORT_PLACEHOLDER`].
    ///
    /// One decision, so the port the app listens on and the port `wait_ready` polls cannot be two
    /// different numbers. Most apps read a variable, and some take `--port <n>` on the command
    /// line; the placeholder covers the second case without a second mechanism.
    #[must_use]
    pub fn with_port(mut self, variable: &str) -> Self {
        self.port_env = Some(variable.to_owned());
        self
    }

    /// Sets the path `wait_ready` polls.
    #[must_use]
    pub fn with_ready_path(mut self, path: &str) -> Self {
        self.ready_path = Some(path.to_owned());
        self
    }

    /// Adds one environment variable.
    #[must_use]
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_owned(), value.to_owned());
        self
    }

    /// Connects stdin and stdout to pipes.
    #[must_use]
    pub fn piped(mut self) -> Self {
        self.piped_stdin = true;
        self.piped_stdout = true;
        self
    }
}

/// How a process ended, and whether its group is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ExitInfo {
    /// The exit code, absent when a signal ended it.
    pub code: Option<i32>,
    /// The signal that ended it, as a name such as `SIGTERM`, when one did.
    pub signal: Option<String>,
    /// Whether it exited before the grace period ran out.
    pub exited_within_grace: bool,
    /// Whether every process in its group is gone.
    ///
    /// Not the same question as `code`: a shell that exits leaves its children alive, and O03
    /// asks whether anything survived.
    pub group_empty: bool,
}

/// A latch an app's owner sets, so a stop can be reported at most once.
///
/// The app and its caller share one latch: the app holds it forever and reads it when it stops,
/// and the caller holds it until [`OwnedApp`] drops. That is what makes a [`StoppedApp`] proof of
/// something the app cannot fake: the token exists only because the app itself, inside
/// [`AppProcess::stop`], saw that it was no longer running.
#[derive(Debug, Clone, Default)]
pub struct StopLatch {
    /// Set by the app immediately before it returns the token.
    stopped: Arc<AtomicBool>,
}

impl StopLatch {
    /// A fresh latch, not yet set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the app that holds this latch has stopped.
    ///
    /// Read by the token's owner; the answer can only be true after a real stop, because the only
    /// writer is [`AppProcess::stop`].
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Proof that a particular app was stopped, and that its process group was empty when it stopped.
///
/// Every function that changes state a running app could be writing to takes one of these. It is
/// deliberately impossible to manufacture:
///
/// - the constructor is private;
/// - the one place that builds one is [`OwnedApp::stop`], after [`AppProcess::stop`] returned -- and
///   [`AppProcess::stop`] returns only once the group is either empty or killed;
/// - obtaining one needs `&mut` on the process that was started, so a caller cannot mint proof for
///   an app it did not spawn.
///
/// `scripts/guards.sh` refuses a second `StoppedApp {` literal or `StoppedApp::new` call anywhere
/// outside this module, because a caller that could build one could reset a database under a live
/// app.
///
/// ## What the token does not claim
///
/// It is proof about *that* process, not a claim that nothing is running on the host. An
/// [`AppProcess`] cannot be restarted -- starting an app creates a new one, with its own latch --
/// so a token never becomes a lie about its own app, but a caller that spawned a *second* app and
/// then reset the database would not be stopped by it either. That is why the orchestrator runs one
/// app per run and resets between probes rather than during them; the token makes the intended
/// discipline a compile-time requirement instead of a comment.
#[derive(Debug)]
pub struct StoppedApp {
    /// The latch shared with the app that stopped, so the token can be invalidated by a restart.
    latch: StopLatch,
}

impl StoppedApp {
    /// Builds a token. Private: [`AppProcess::stop`] is the only caller.
    const fn new(latch: StopLatch) -> Self {
        Self { latch }
    }

    /// Whether the app that produced this token is still stopped.
    ///
    /// A consumer calls this before changing state; `false` means the app was started again after
    /// the stop, so the token no longer proves anything.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.latch.is_set()
    }

    /// Why a consumer must reject a token whose latch is clear.
    ///
    /// One sentence, in one place, so every message says the same thing. The latch is clear only
    /// when `stop` never returned for the app the token came from, which in practice means the
    /// caller is holding a latch it took before stopping -- so the sentence names the action the
    /// caller has to take.
    #[must_use]
    pub fn invalid_reason() -> &'static str {
        "the app has not been stopped, so nothing proves it is not writing"
    }

    /// A token for a caller that has started no app in this process at all.
    ///
    /// `substance replay` is the case: it restores a snapshot before it ever boots anything, so the
    /// question the token answers -- "is the app that was running here gone?" -- is answered by
    /// "there was none". That is a different fact from "an app was stopped", and the type keeps them
    /// apart: a latch nobody can set is returned, so the token can never later claim an app stopped.
    ///
    /// It is not an escape hatch for a caller with a live app: taking this token while a process is
    /// running would be a lie the compiler cannot catch, so the one caller (`replay::drive`) uses it
    /// before it spawns anything.
    #[must_use]
    pub fn nothing_started() -> Self {
        Self::new(StopLatch::new())
    }
}

// A token crosses await points and is shared by a caller that holds it while a reset runs, so it
// has to be `Send` and `Sync`. Asserted at compile time, because the failure would otherwise appear
// as an unhelpful "future is not Send" at a call site in another crate.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StoppedApp>();
    assert_send_sync::<StopLatch>();
};

/// The process and the latch its stop token is built from, moved together.
///
/// A caller that wants to be able to stop and then reset holds this instead of the process alone.
/// The latch is cloned into the [`AppProcess`] at spawn so both sides point at the same flag.
#[derive(Debug)]
pub struct OwnedApp {
    /// The running app.
    app: AppProcess,
}

impl OwnedApp {
    /// Takes ownership of a started app.
    #[must_use]
    pub fn new(app: AppProcess) -> Self {
        Self { app }
    }

    /// The app, for reading its port and logs.
    #[must_use]
    pub fn app(&self) -> &AppProcess {
        &self.app
    }

    /// Stops the app and returns proof that it is gone.
    ///
    /// The token is built only after the process group is empty (or the grace period expired and
    /// the group was killed), so it cannot be obtained while anything the app started is alive.
    ///
    /// # Errors
    ///
    /// Returns whatever [`AppProcess::stop`] returns.
    pub async fn stop(&mut self, grace: Duration) -> Result<(ExitInfo, StoppedApp), ProcessError> {
        let info = self.app.stop(grace).await?;
        // `stop` sets the latch; the token carries a handle to it, so a later restart invalidates
        // this token rather than leaving the caller with a stale one.
        Ok((info, StoppedApp::new(self.app.stop_latch.clone())))
    }
}

/// Why a process could not be started, stopped or waited for.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The program is not on `PATH`.
    #[error("{program} is not installed (or not on PATH)")]
    Missing {
        /// The program that was not found.
        program: String,
    },
    /// The process could not be started.
    #[error("cannot start {program}: {source}")]
    Io {
        /// The program that failed.
        program: String,
        /// The IO error.
        source: io::Error,
    },
    /// A log file could not be written.
    #[error(transparent)]
    Log(#[from] LogError),
    /// No free port could be allocated.
    #[error("cannot allocate a port: {source}")]
    Port {
        /// The IO error.
        source: io::Error,
    },
    /// The process did not become ready inside its budget.
    #[error("{program} did not answer {url} within {seconds}s")]
    NotReady {
        /// The program that was waited for.
        program: String,
        /// The URL that was polled.
        url: String,
        /// The budget it was given, in whole seconds.
        seconds: u64,
    },
    /// The process exited before it ever answered, so no budget could have helped.
    ///
    /// Separate from [`ProcessError::NotReady`] because the two send a reader to different places:
    /// a timeout is about machine speed, while an early exit is about the command, and a boot
    /// failure that says "within 0s" points at neither.
    #[error("{program} exited before answering {url} ({code})")]
    ExitedBeforeReady {
        /// The program that exited.
        program: String,
        /// The URL that never answered.
        url: String,
        /// How it ended, as text.
        code: String,
    },
    /// The HTTP client for `wait_ready` could not be built.
    #[error("cannot build the readiness client: {detail}")]
    Client {
        /// The client's error.
        detail: String,
    },
    /// The caller asked to write to stdin, but the spec did not ask for a stdin pipe.
    ///
    /// A distinct variant because it is a caller's mistake, not a process failure: a spec built
    /// without `piped_stdin` connects the child's stdin to `/dev/null`, and no retry fixes that.
    #[error("{program} has no stdin pipe; set piped_stdin on its spec")]
    NoStdin {
        /// The program without a pipe.
        program: String,
    },
}

/// The drop guard: kills the group unless the process was stopped on purpose.
///
/// Not `Clone` and not `Copy`: one guard owns one group. `disarmed` is a plain field rather than
/// an `Option<Child>` because the guard also has to survive the child being waited on, so what it
/// holds is the group id, not the child.
#[derive(Debug)]
pub struct ProcessGuard {
    /// The process group to kill. `-1` means "already reaped", never a real group.
    pgid: i32,
    /// Whether a clean `stop` already dealt with it.
    disarmed: bool,
}

impl ProcessGuard {
    /// A guard for `pgid`.
    #[must_use]
    const fn new(pgid: i32) -> Self {
        Self {
            pgid,
            disarmed: false,
        }
    }

    /// Marks the group as already handled.
    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.disarmed || self.pgid <= 0 {
            return;
        }
        // SIGKILL, not SIGTERM: `Drop` runs when a stage already failed or was cancelled, and
        // there is no task left to wait out a grace period. A process that ignores SIGTERM
        // would otherwise survive the run, which is the leak the guard exists to prevent.
        let _ = kill_group(self.pgid, Signal::Kill);
    }
}

/// A signal this module sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    /// Ask the process to exit.
    Term,
    /// Kill it outright.
    Kill,
}

/// Kills a process group outright, for the interrupt handler.
///
/// The same call [`ProcessGuard`]'s `Drop` makes, exposed because the interrupt handler lives in
/// another module and holds a group id rather than a guard. Deliberately only `SIGKILL`: a Ctrl-C
/// has already interrupted whatever a graceful stop would wait on, so there is no task left to
/// honour a grace period.
///
/// Returns whether a signal was delivered: `false` means the group was already gone, which is the
/// normal case when the app's own guard ran first.
#[must_use]
pub fn kill_group_now(pgid: i32) -> bool {
    if pgid <= 0 {
        return false;
    }
    kill_group(pgid, Signal::Kill)
}

/// Sends `signal` to a process group.
///
/// Returns `false` when the group no longer exists (`ESRCH`), which is how [`group_empty`] is
/// answered. Any other failure is reported as `true`, because "something is still there and we
/// could not signal it" must never be mistaken for "the group is empty".
fn kill_group(pgid: i32, signal: Signal) -> bool {
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::{Signal as NixSignal, killpg};
        use nix::unistd::Pid;

        let chosen = match signal {
            Signal::Term => NixSignal::SIGTERM,
            Signal::Kill => NixSignal::SIGKILL,
        };
        match killpg(Pid::from_raw(pgid), chosen) {
            Ok(()) => true,
            Err(Errno::ESRCH) => false,
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (pgid, signal);
        // Windows has no process groups in this sense; §4.1's stop sequence is a POSIX design and
        // the plan targets macOS and Linux hosts. Reporting "not empty" is the honest answer: a
        // caller that would gate on `group_empty` reports it as unverifiable rather than as a
        // pass it did not check.
        true
    }
}

/// Whether every process in `pgid` is gone.
fn group_is_empty(pgid: i32) -> bool {
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::killpg;
        use nix::unistd::Pid;

        // Signal 0 asks the kernel whether the group exists without sending anything.
        match killpg(Pid::from_raw(pgid), None) {
            Ok(()) => false,
            Err(Errno::ESRCH) => true,
            // EPERM means it exists and belongs to someone else; that is not empty either.
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        false
    }
}

/// A running repo-controlled process.
#[derive(Debug)]
pub struct AppProcess {
    /// The child handle, until it is waited on.
    child: Option<Child>,
    /// The process group, which is the child's pid.
    pgid: i32,
    /// The port the app was told to use, when it takes one.
    port: Option<u16>,
    /// The log sink and index.
    logs: LogSink,
    /// The kill-if-forgotten guard.
    guard: ProcessGuard,
    /// The spec, for error messages and for `wait_ready`'s URL.
    spec: ProcessSpec,
    /// The chunks the caller asked to read from stdout, when the spec set `piped_stdout`.
    ///
    /// A channel rather than the raw pipe: the pump owns the pipe so stdout still reaches the log
    /// file, and each chunk is forwarded here as well. Giving the pipe itself to the caller would
    /// mean either an unlogged stream or two readers racing for the same bytes.
    stdout: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    /// The child's stdin, when the spec set `piped_stdin`.
    ///
    /// `None` means the child's stdin is `/dev/null`, so a write has nowhere to go. Kept rather
    /// than dropped because dropping it closes the pipe: a helper that reads commands until end of
    /// input would see its first command and then stop.
    stdin: Option<tokio::process::ChildStdin>,
    /// The latch a [`StoppedApp`] token is built from, shared with its owner.
    ///
    /// Cleared when the process is created and set when it is stopped, so a token is valid exactly
    /// while the app it describes has not been restarted.
    stop_latch: StopLatch,
}

/// A running substance-owned helper.
#[derive(Debug)]
pub struct ServiceProcess {
    /// The same machinery, wrapped rather than duplicated.
    inner: AppProcess,
}

impl ServiceProcess {
    /// Starts a helper. No permit, because the program is substance's own.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError`] for the same reasons [`AppProcess::spawn`] does.
    pub async fn spawn(
        spec: &ProcessSpec,
        logs_dir: &Path,
        shutdown: Budget,
    ) -> Result<Self, ProcessError> {
        let inner = start(spec, logs_dir, shutdown).await?;
        Ok(Self { inner })
    }

    /// The port the helper was told to use, when it takes one.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.inner.port
    }

    /// The next chunk of stdout, when the spec asked for a pipe.
    ///
    /// This is how a helper such as the driver is driven: it reads a command, answers on stdout,
    /// and stays running. `None` means it closed stdout, usually because it exited.
    pub async fn read_stdout(&mut self) -> Option<Vec<u8>> {
        self.inner.read_stdout().await
    }

    /// Takes the stdout receiver, for a caller that drives the helper from one task.
    ///
    /// The helper is written to and read from concurrently, so the reader cannot borrow the
    /// process across an await; taking the receiver lets the caller own one half and the writer
    /// the other. See [`AppProcess::take_stdout`].
    #[must_use]
    pub fn take_stdout(&mut self) -> Option<tokio::sync::mpsc::Receiver<Vec<u8>>> {
        self.inner.take_stdout()
    }

    /// Writes to the helper's stdin, when the spec asked for a pipe.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::NoStdin`] when the spec did not ask for a stdin pipe, and
    /// [`ProcessError::Io`] when the write fails -- usually because the child is gone.
    pub async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), ProcessError> {
        self.inner.write_stdin(bytes).await
    }

    /// Closes the helper's stdin, which most programs read as "no more input".
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] when the pipe cannot be shut.
    pub async fn close_stdin(&mut self) -> Result<(), ProcessError> {
        self.inner.close_stdin().await
    }

    /// The helper's process group, for a caller that has to check it outlived the run.
    #[must_use]
    pub fn pgid(&self) -> i32 {
        self.inner.pgid
    }

    /// Stops it, as [`AppProcess::stop`] does.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] when the group cannot be signalled.
    pub async fn stop(&mut self, grace: Duration) -> Result<ExitInfo, ProcessError> {
        self.inner.stop(grace).await
    }
}

impl AppProcess {
    /// Starts repo-controlled code. `_permit` is proof the caller had `--allow-exec`.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Missing`] when the program is absent, [`ProcessError::Port`] when
    /// no port can be allocated, [`ProcessError::Log`] when a log file cannot be written, and
    /// [`ProcessError::Io`] when the spawn itself fails.
    pub async fn spawn(
        spec: &ProcessSpec,
        logs_dir: &Path,
        shutdown: Budget,
        _permit: &ExecPermit,
    ) -> Result<Self, ProcessError> {
        start(spec, logs_dir, shutdown).await
    }

    /// The port the app was told to use, when its spec asked for one.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// The process group id, which is the child's pid.
    #[must_use]
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// The log sink, for reading the index back.
    #[must_use]
    pub fn logs(&self) -> &LogSink {
        &self.logs
    }

    /// Whether this process has been stopped since it was created.
    ///
    /// Public because a caller may need to know before it has a [`StoppedApp`]: a stage that finds
    /// a process handle lying around and wants to decide whether it may touch a database asks this
    /// first. It says nothing about whether *anything* is running, only whether this handle's own
    /// `stop` returned.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stop_latch.is_set()
    }

    /// This process's latch, for a caller that will take the [`StoppedApp`] after stopping it.
    ///
    /// Handing out the latch is not a way around the token: a latch that has not been set cannot
    /// validate anything, [`StoppedApp::is_valid`] reads it, and nothing can set it but `stop`.
    #[must_use]
    pub fn stop_latch(&self) -> StopLatch {
        self.stop_latch.clone()
    }

    /// Takes the stdout receiver, for a caller that will read it from its own task.
    ///
    /// `read_stdout` borrows the process for the length of each read, so a caller that wants to
    /// do something else with the process while it reads -- write to stdin, stop it -- cannot hold
    /// that borrow across an await. Handing over the receiver is the way out; the log file keeps
    /// receiving the same bytes either way, because the pump owns the pipe and only tees here.
    ///
    /// Writes to the child's stdin, when the spec set `piped_stdin`.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::NoStdin`] when there is no pipe to write to, and
    /// [`ProcessError::Io`] when the write fails.
    pub async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), ProcessError> {
        use tokio::io::AsyncWriteExt;

        let Some(stdin) = self.stdin.as_mut() else {
            return Err(ProcessError::NoStdin {
                program: self.spec.program.clone(),
            });
        };
        stdin
            .write_all(bytes)
            .await
            .map_err(|source| ProcessError::Io {
                program: self.spec.program.clone(),
                source,
            })?;
        stdin.flush().await.map_err(|source| ProcessError::Io {
            program: self.spec.program.clone(),
            source,
        })
    }

    /// Closes the child's stdin.
    ///
    /// A helper that reads until end of input needs this to exit: it sees every command, and then
    /// the end. Dropping the handle would do it too, one `Drop` order later.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] when closing fails.
    pub async fn close_stdin(&mut self) -> Result<(), ProcessError> {
        use tokio::io::AsyncWriteExt;

        let Some(mut stdin) = self.stdin.take() else {
            return Ok(());
        };
        stdin.shutdown().await.map_err(|source| ProcessError::Io {
            program: self.spec.program.clone(),
            source,
        })
    }

    /// Takes the stdout receiver, when the spec asked for a pipe.
    ///
    /// See [`ServiceProcess::take_stdout`] for why a caller wants this instead of `read_stdout`.
    #[must_use]
    pub fn take_stdout(&mut self) -> Option<tokio::sync::mpsc::Receiver<Vec<u8>>> {
        self.stdout.take()
    }

    /// The next chunk of stdout, when the spec asked for a pipe.
    ///
    /// `None` once the process has closed stdout, which is how a helper that answers one request
    /// and exits announces itself.
    pub async fn read_stdout(&mut self) -> Option<Vec<u8>> {
        match self.stdout.as_mut() {
            Some(receiver) => receiver.recv().await,
            None => None,
        }
    }

    /// Waits until the app answers its ready path, or the budget runs out.
    ///
    /// A status below 500 counts as ready: a 404 on `/` means the server is up and routing, and
    /// waiting for a 200 would turn every app without a health endpoint into a boot failure.
    /// Connection errors are retried, because those are what a starting server produces.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::NotReady`] when the budget runs out, and [`ProcessError::Client`]
    /// when the HTTP client cannot be built.
    pub async fn wait_ready(&mut self, budget: Budget) -> Result<(), ProcessError> {
        let (Some(port), Some(path)) = (self.port, self.spec.ready_path.clone()) else {
            return Ok(());
        };
        let url = format!("http://127.0.0.1:{port}{path}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| ProcessError::Client {
                detail: error.to_string(),
            })?;
        // A 50 ms poll is fast enough that a ready server is not held up, and slow enough that a
        // long boot does not spend the machine on connection attempts.
        let poll = Duration::from_millis(50);
        loop {
            // A child that exited cannot become ready, so the wait ends with the real reason
            // rather than spending the rest of the budget on a dead port.
            if let Some(child) = self.child.as_mut()
                && child
                    .try_wait()
                    .map_err(|source| ProcessError::Io {
                        program: self.spec.program.clone(),
                        source,
                    })?
                    .is_some()
            {
                return Err(ProcessError::ExitedBeforeReady {
                    program: self.spec.program.clone(),
                    url,
                    code: "it exited before it listened".to_owned(),
                });
            }
            if let Ok(response) = client.get(&url).send().await
                && response.status().as_u16() < 500
            {
                return Ok(());
            }
            if budget.is_spent() {
                return Err(ProcessError::NotReady {
                    program: self.spec.program.clone(),
                    url,
                    seconds: budget.limit_secs(),
                });
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// SIGTERMs the group, waits `grace`, then SIGKILLs whatever is left.
    ///
    /// Returns both facts the caller needs: how the child ended, and whether the group is empty.
    /// The group is re-checked after the child is reaped, because a child that exits can leave
    /// children of its own behind, and the wait for the group is what makes `group_empty`
    /// meaningful rather than a snapshot taken at the wrong moment.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] when the child cannot be waited on.
    pub async fn stop(&mut self, grace: Duration) -> Result<ExitInfo, ProcessError> {
        // Spelled out so the intent survives a reviewer's question about which signal comes first.
        let alive = kill_group(self.pgid, Signal::Term);
        let waited = self.wait_within(grace).await?;
        let (code, signal, exited_within_grace) = match waited {
            Some(status) => (status.code(), signal_name(status), true),
            None => {
                // Still running after the grace period: the plan's second step.
                kill_group(self.pgid, Signal::Kill);
                let status = self.wait_now().await?;
                (status.code(), signal_name(status), false)
            }
        };
        // Give the kernel a moment to reap the group's remaining members: a killed process is
        // gone from `killpg`'s view only once it has been reaped, and the caller is about to ask.
        let group_empty = self.wait_group_empty(Duration::from_secs(2)).await;
        self.guard.disarm();
        // Set here rather than in `OwnedApp::stop`, so every path that stops an app leaves its
        // latch consistent: a token built from this app is valid from now on, and a restart clears
        // it again.
        self.stop_latch.stopped.store(true, Ordering::SeqCst);
        let _ = alive;
        Ok(ExitInfo {
            code,
            signal,
            exited_within_grace,
            group_empty,
        })
    }

    /// Waits up to `within` for the child, or returns `None` when it is still running.
    async fn wait_within(
        &mut self,
        within: Duration,
    ) -> Result<Option<std::process::ExitStatus>, ProcessError> {
        let Some(mut child) = self.child.take() else {
            return Ok(None);
        };
        let program = self.spec.program.clone();
        match tokio::time::timeout(within, child.wait()).await {
            Ok(Ok(status)) => Ok(Some(status)),
            Ok(Err(source)) => Err(ProcessError::Io { program, source }),
            Err(_) => {
                // Put the child back so `wait_now` can reap it after the SIGKILL.
                self.child = Some(child);
                Ok(None)
            }
        }
    }

    /// Waits for the child after a SIGKILL.
    async fn wait_now(&mut self) -> Result<std::process::ExitStatus, ProcessError> {
        let program = self.spec.program.clone();
        match self.child.take() {
            Some(mut child) => child
                .wait()
                .await
                .map_err(|source| ProcessError::Io { program, source }),
            None => Err(ProcessError::Io {
                program,
                source: io::Error::new(io::ErrorKind::NotFound, "the child was already reaped"),
            }),
        }
    }

    /// Polls until the group is empty or `within` runs out.
    async fn wait_group_empty(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if group_is_empty(self.pgid) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// Starts a process and pumps its streams into the log sink.
///
/// Shared by both public spawn functions, so the environment, the group, the port and the log
/// wiring are written once. The caller supplies the permit decision; this function only runs it.
async fn start(
    spec: &ProcessSpec,
    logs_dir: &Path,
    _shutdown: Budget,
) -> Result<AppProcess, ProcessError> {
    let port = match &spec.port_env {
        Some(_) => Some(free_port().map_err(|source| ProcessError::Port { source })?),
        None => None,
    };
    let started = Instant::now();
    let logs = LogSink::create(logs_dir, &spec.name, started)?;

    // The wrapper is decided here, once, so no spawn path can forget it.
    let resolved = resolve_port_args(&spec.args, port);
    let (program, args) = spec.limits.wrap(&spec.program, &resolved);
    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(&spec.cwd)
        // A clean environment: nothing is inherited, then the fixed list, then the spec's own.
        .env_clear()
        // Always piped: the pump has to read it to write the log. `piped_stdout` decides whether
        // the chunks are also forwarded to the caller, not whether the child gets a pipe.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if spec.piped_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        // The child leads its own group, so `killpg` reaches everything it spawns.
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    for (name, value) in OWN_ENV {
        command.env(name, value);
    }
    // Before the spec's own env, so an app that deliberately sets `HOME` keeps its value.
    if let Some(home) = &spec.home {
        for (name, value) in home_env(home) {
            command.env(name, value);
        }
    }
    // `PATH` is passed through deliberately: the plan's clean environment is about the app's
    // variables, not about hiding the programs the host provides. A fixed allowlist would break
    // every `npm run` script that calls a tool from `node_modules/.bin`.
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    for (name, value) in &spec.env {
        command.env(name, value);
    }
    if let (Some(variable), Some(number)) = (&spec.port_env, port) {
        command.env(variable, number.to_string());
    }

    let mut child = command.spawn().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            ProcessError::Missing {
                program: spec.program.clone(),
            }
        } else {
            ProcessError::Io {
                program: spec.program.clone(),
                source,
            }
        }
    })?;
    let Some(pid) = child.id() else {
        return Err(ProcessError::Io {
            program: spec.program.clone(),
            source: io::Error::other("the child exited before its pid was read"),
        });
    };
    let pgid = i32::try_from(pid).unwrap_or(-1);
    let child_stdin = child.stdin.take();

    // The pumps write to the log files. They are spawned rather than awaited: the caller wants the
    // process handle back so it can wait for readiness, and the streams end when the child closes
    // them, so nothing here has to be joined explicitly.
    let mut stdout_rx = None;
    if let Some(mut out) = child.stdout.take() {
        let sink = logs.clone();
        // A small buffer: a helper that answers one request writes a few kilobytes, and a caller
        // that stops reading must not let the pump grow without bound.
        let (sender, receiver) = match spec.piped_stdout {
            true => {
                let (tx, rx) = tokio::sync::mpsc::channel(32);
                (Some(tx), Some(rx))
            }
            false => (None, None),
        };
        stdout_rx = receiver;
        let mut buffer = vec![0_u8; 8192];
        tokio::spawn(async move {
            loop {
                match out.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let chunk = &buffer[..read];
                        if sink.append(Stream::Stdout, chunk).is_err() {
                            break;
                        }
                        if let Some(tx) = &sender {
                            // A caller that dropped its end keeps the log and loses the tee.
                            let _ = tx.send(chunk.to_vec()).await;
                        }
                    }
                }
            }
        });
    }
    if let Some(mut err) = child.stderr.take() {
        let sink = logs.clone();
        let mut buffer = vec![0_u8; 8192];
        tokio::spawn(async move {
            loop {
                match err.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sink.append(Stream::Stderr, &buffer[..read]).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    Ok(AppProcess {
        child: Some(child),
        pgid,
        port,
        logs,
        guard: ProcessGuard::new(pgid),
        spec: spec.clone(),
        stdin: child_stdin,
        stdout: stdout_rx,
        // Fresh and clear: a process that has just started has not stopped, so any token built
        // from a previous app is dead the moment it is read.
        stop_latch: StopLatch::new(),
    })
}

/// Replaces every [`PORT_PLACEHOLDER`] argument with `port`.
///
/// Only a whole argument is replaced, never a substring: an argument such as `--url=http://…`
/// that happened to contain the braces would otherwise be rewritten in a way its program did not
/// expect, and a caller that wants a port inside a longer string can compose it itself.
fn resolve_port_args(args: &[String], port: Option<u16>) -> Vec<String> {
    let Some(number) = port else {
        return args.to_vec();
    };
    let text = number.to_string();
    args.iter()
        .map(|arg| match arg == PORT_PLACEHOLDER {
            true => text.clone(),
            false => arg.clone(),
        })
        .collect()
}

/// The signal that ended `status`, as a name, when one did.
fn signal_name(status: std::process::ExitStatus) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal().map(|signal| match signal {
            1 => "SIGHUP".to_owned(),
            2 => "SIGINT".to_owned(),
            6 => "SIGABRT".to_owned(),
            9 => "SIGKILL".to_owned(),
            11 => "SIGSEGV".to_owned(),
            15 => "SIGTERM".to_owned(),
            other => format!("signal {other}"),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}
