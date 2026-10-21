//! Resource limits for repo-controlled commands (M2-T01, §4.1, Linux only).
//!
//! Without a sandbox, the only thing standing between a runaway build and the host is the
//! kernel's own limits. §4.1 asks for them through `prlimit`, which sets them on the child rather
//! than on substance itself:
//!
//! ```text
//! prlimit --as=<bytes> --nofile=<n> --nproc=<n> -- <cmd>
//! ```
//!
//! Two constraints shape this module:
//!
//! - **`prlimit` is a program, not a crate, and only on Linux.** `setrlimit` from a crate limits
//!   substance; `pre_exec` would let it limit a child, but `pre_exec` is `unsafe` and the
//!   workspace forbids `unsafe_code`. So the wrapper is a process, and on a host without it the
//!   limits are simply absent.
//! - **Absent limits must be visible.** A run that silently skipped them would let a reader assume
//!   protection that was not there, so [`Limits::note`] returns the sentence the report carries
//!   when they were not applied (the same shape O08 reads).
//!
//! The defaults are deliberately generous. Their job is to stop a fork bomb or a build that fills
//! the disk, not to make a legitimate build fail: a limit that trips a normal `npm ci` would turn
//! a working app into a boot failure, which is worse than no limit at all.

/// Address space a repo-controlled command may map, in bytes: 8 GiB.
///
/// Generous on purpose. A TypeScript build with a large dependency tree maps a surprising amount,
/// and `--as` counts virtual address space, not resident pages, so the figure is much larger than
/// the memory a build actually uses.
pub const ADDRESS_SPACE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Open file descriptors a repo-controlled command may hold.
///
/// The usual soft limit on Linux is 1024, which `npm` exhausts when it opens a large tree; 4096
/// leaves room without allowing an unbounded descriptor leak.
pub const OPEN_FILES: u32 = 4096;

/// Processes and threads a repo-controlled command may create.
///
/// One `npm ci` legitimately runs a handful of node processes at a time; 1024 is far above that
/// and still far below a fork bomb's rate.
pub const PROCESSES: u32 = 1024;

/// How to wrap a repo-controlled command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limits {
    /// Run it through `prlimit` with the constants above.
    Applied,
    /// Run it unwrapped, because this host has no `prlimit`.
    Unavailable {
        /// Why, in the sentence the report carries.
        why: &'static str,
    },
}

impl Limits {
    /// Decides how repo-controlled commands run on this host.
    ///
    /// Detected once per run rather than per command: the answer cannot change mid-run, and a
    /// `PATH` lookup for every spawn would be waste.
    #[must_use]
    pub fn detect() -> Self {
        if !cfg!(target_os = "linux") {
            return Self::Unavailable {
                why: "resource limits need prlimit, which this host does not provide; \
                      repo-controlled commands ran without limits",
            };
        }
        match std::process::Command::new("prlimit")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) if status.success() => Self::Applied,
            _ => Self::Unavailable {
                why: "prlimit (util-linux) is missing, so repo-controlled commands ran without \
                      limits",
            },
        }
    }

    /// Whether the limits are applied on this host.
    #[must_use]
    pub const fn applied(self) -> bool {
        matches!(self, Self::Applied)
    }

    /// The note the report carries, or `None` when the limits were applied.
    #[must_use]
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Applied => None,
            Self::Unavailable { why } => Some(why),
        }
    }

    /// The program and arguments that run `command` with `args` under the limits.
    ///
    /// The returned vector is what a spawn should use: `prlimit … -- command arg…`. When the
    /// limits are unavailable the command is returned unchanged, so callers do not branch.
    #[must_use]
    pub fn wrap(self, command: &str, args: &[String]) -> (String, Vec<String>) {
        match self {
            Self::Unavailable { .. } => (command.to_owned(), args.to_vec()),
            Self::Applied => {
                let mut wrapped = Vec::with_capacity(args.len() + 5);
                wrapped.push(format!("--as={ADDRESS_SPACE_BYTES}"));
                wrapped.push(format!("--nofile={OPEN_FILES}"));
                wrapped.push(format!("--nproc={PROCESSES}"));
                wrapped.push("--".to_owned());
                wrapped.push(command.to_owned());
                wrapped.extend(args.iter().cloned());
                ("prlimit".to_owned(), wrapped)
            }
        }
    }
}
