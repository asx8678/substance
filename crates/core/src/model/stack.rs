//! What an adapter reports about a stack: its routes, its commands, how it booted (M2-T02).
//!
//! These are data, not code, so they live in `core` where the schema exporter can see them and
//! the report can cite them. The adapter *implementations* are in `crates/adapters`.
//!
//! Two of these types exist because a downstream check has to say "not measured" rather than
//! "fine":
//!
//! - A route table is [`Measured`], because an adapter that cannot read routes must not look like
//!   an app with no routes. M2-T02 answers `not_measured("route tables arrive in M6")` for every
//!   stack, and that reason travels to the report unchanged.
//! - A boot records its [`BootRung`], because a dev server started from a README changes what
//!   B28 can conclude: a dev server ships the dev markers B28 looks for, so a clean result from
//!   one proves nothing, and B28 reports `not_measured` instead of `pass`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One route an adapter read out of the code, before any crawl.
///
/// The crawler's route table (M6) is richer; this is the shape an adapter can fill from the
/// filesystem alone, which is what a static read can promise.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RouteEntry {
    /// The route template, for example `/invoices/:id`.
    pub route: String,
    /// Where it was read: a repo-relative path.
    pub source: String,
    /// The HTTP methods the route answers, sorted. Empty when the adapter cannot tell.
    pub methods: Vec<String>,
}

/// Routes read from the code, sorted by route then source.
pub type RouteTable = Vec<RouteEntry>;

/// A command an adapter suggests: what to run, and how to know it worked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CommandSpec {
    /// The program.
    pub program: String,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// The environment the command needs beyond the run's own, sorted by name.
    pub env: std::collections::BTreeMap<String, String>,
}

impl CommandSpec {
    /// A command from a program and its arguments.
    #[must_use]
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: std::collections::BTreeMap::new(),
        }
    }

    /// The command as one line, for a log or a friction entry.
    ///
    /// Not a shell line: it quotes nothing, because it is read by a person looking at a log
    /// rather than by a shell. A friction entry that could be pasted into a shell unsafely would
    /// be worse than one that has to be retyped.
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

/// Which rung of the boot ladder produced a running app (ADR-0017).
///
/// Ordinal on purpose: a caller comparing rungs is asking which is more trustworthy, and rung 1
/// is the least. `Ord` here is *1 < 2 < 3*, which is also the order the ladder tries them.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum BootRung {
    /// The README's shell blocks, run in order. Measures O02 only.
    Readme,
    /// The `substance.toml [app]` commands.
    Config,
    /// What the stack adapter detected.
    Adapter,
}

impl BootRung {
    /// The rung as a number, as the report and the friction log spell it.
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::Readme => 1,
            Self::Config => 2,
            Self::Adapter => 3,
        }
    }

    /// The rung as a lowercase word, for a note.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Readme => "readme",
            Self::Config => "config",
            Self::Adapter => "adapter",
        }
    }

    /// Whether an app started this way may be a dev server.
    ///
    /// The README rung runs whatever the README says, and a README almost always says `npm run
    /// dev`. A dev server carries the framework's development markers, so B28 cannot conclude
    /// anything from it and reports `not_measured` (M2-T02's rule, B28's row in M4).
    #[must_use]
    pub const fn may_be_dev_server(self) -> bool {
        matches!(self, Self::Readme)
    }
}

/// One thing that went wrong while booting, and what fixed it.
///
/// The plan calls this friction evidence, and O02's verdict is "the outcome of the README rung,
/// plus the friction log". A rung that failed and a rung that needed a fix are different facts, so
/// both are recorded: `fix` is what substance had to do, `None` when the command simply failed
/// and the rung was abandoned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Friction {
    /// Which rung it happened on.
    pub rung: BootRung,
    /// Which step: `install`, `build`, `migrate`, `seed` or `start`.
    pub step: String,
    /// The command, as it was run.
    pub command: String,
    /// The exit code, absent when a signal ended it.
    pub code: Option<i32>,
    /// How many bytes into its log the step's output ended, so a reader can find it.
    ///
    /// An offset rather than the text: a failed install can print megabytes, and the report is
    /// not the place to quote them.
    pub log_end: u64,
    /// What substance changed to get past it, when it changed anything.
    ///
    /// `None` is the normal case: a failed rung is abandoned rather than patched, because a run
    /// that edited its own subject would describe code nobody has.
    pub fix: Option<String>,
    /// Why the step could not run at all, when it could not.
    ///
    /// Separate from `fix` because the two answer different questions and a reader of `fix`
    /// assumes substance changed something. `npm is not installed` is not a fix, and a caller that
    /// put it there would be claiming an intervention that did not happen.
    pub reason: Option<String>,
}

/// How a boot ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct BootOutcome {
    /// The rung that produced the running app, when one did.
    pub rung: Option<BootRung>,
    /// Whether an app answered its ready path.
    pub booted: bool,
    /// The port it answered on, when it booted.
    pub port: Option<u16>,
    /// Whether the app may be a dev server, so B28 must report `not_measured`.
    pub may_be_dev_server: bool,
    /// Every failed rung and applied fix, in the order they happened.
    pub friction: Vec<Friction>,
    /// Why no rung booted it, when none did.
    pub failure: Option<String>,
}
