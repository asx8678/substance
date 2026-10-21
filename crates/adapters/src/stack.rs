//! The `StackAdapter` trait and the repo view adapters read (M2-T02).
//!
//! An adapter answers six questions about a repository: am I this stack, how do I boot it, how do
//! I migrate it, what routes does it have, how do I test it, and what am I called. The trait is the
//! plan's, verbatim, because everything downstream — the boot ladder, O04's migration run, B32's
//! route table, M10's tests — was designed against it.
//!
//! ## Why the view is read-only
//!
//! [`RepoView`] hands an adapter the work copy and nothing else. It cannot spawn a process, write
//! a file or reach the network, which is what makes `detect` and `boot_plan` pure enough to call
//! for every adapter on every repository: a detector that had to run `npm ls` would be a detector
//! that executes repo-controlled code during triage, exactly what §4.1 forbids.
//!
//! ## Why "not measured" is in the trait's return type
//!
//! `route_table` returns [`Measured<RouteTable>`], not `RouteTable`. An adapter that cannot read
//! routes — because its stack's route table arrives in M6, or because the repository is laid out
//! unusually — must not look like an app with no routes. The same reasoning as ADR-0013: an empty
//! answer and an unasked question are different facts, and only one of them is a pass.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use substance_core::config::ProjectConfig;
use substance_core::model::{CommandSpec, Measured, RouteTable};

/// A read-only view of the work copy.
///
/// Every read goes through here so an adapter has no way to write, spawn or fetch. The struct is
/// cheap: it holds a path and nothing else.
#[derive(Debug, Clone)]
pub struct RepoView {
    /// The work copy's root.
    root: PathBuf,
}

impl RepoView {
    /// A view over `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// The work copy's root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether a repo-relative path exists.
    #[must_use]
    pub fn exists(&self, relative: &str) -> bool {
        self.root.join(relative).exists()
    }

    /// A repo-relative path's text, when it exists and is valid UTF-8.
    ///
    /// `None` for a missing file and for one that is not UTF-8, because an adapter's next move is
    /// the same in both cases: it cannot read this file and must try something else. A caller that
    /// needs to tell them apart uses [`Self::exists`] as well.
    #[must_use]
    pub fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(relative)).ok()
    }

    /// The repo-relative paths directly inside `relative`, sorted.
    ///
    /// Directories are returned with a trailing `/` so a caller can tell them apart without a
    /// second call, and the list is sorted so two runs of the same detection produce the same
    /// evidence in the same order (ADR-0009).
    #[must_use]
    pub fn list(&self, relative: &str) -> Vec<String> {
        let dir = self.root.join(relative);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            names.push(match is_dir {
                true => format!("{name}/"),
                false => name,
            });
        }
        names.sort();
        names
    }

    /// The repository's `package.json`, parsed, when it has one that parses.
    #[must_use]
    pub fn package_json(&self) -> Option<PackageJson> {
        let text = self.read("package.json")?;
        PackageJson::parse(&text)
    }
}

/// The parts of `package.json` an adapter needs.
///
/// Deliberately not a full document: an adapter that read every field would start depending on
/// fields the format allows to change, and the four here are the ones the boot table uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageJson {
    /// The package's name.
    pub name: String,
    /// The scripts, sorted by name.
    pub scripts: BTreeMap<String, String>,
    /// Every dependency name, runtime and development, sorted and merged.
    pub dependencies: Vec<String>,
}

impl PackageJson {
    /// Parses the parts this module needs, or `None` when the document is not usable.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let object = value.as_object()?;
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut scripts = BTreeMap::new();
        if let Some(map) = object.get("scripts").and_then(serde_json::Value::as_object) {
            for (key, value) in map {
                if let Some(script) = value.as_str() {
                    scripts.insert(key.clone(), script.to_owned());
                }
            }
        }
        let mut dependencies: Vec<String> = Vec::new();
        for field in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(map) = object.get(field).and_then(serde_json::Value::as_object) {
                dependencies.extend(map.keys().cloned());
            }
        }
        dependencies.sort();
        dependencies.dedup();
        Some(Self {
            name,
            scripts,
            dependencies,
        })
    }

    /// Whether `name` is a dependency.
    #[must_use]
    pub fn depends_on(&self, name: &str) -> bool {
        self.dependencies.iter().any(|dep| dep == name)
    }

    /// Whether a script exists.
    #[must_use]
    pub fn has_script(&self, name: &str) -> bool {
        self.scripts.contains_key(name)
    }
}

/// Why an adapter believes a repository is its stack.
///
/// Evidence is a list of repo-relative paths and short sentences, sorted: a detection that says
/// "I found `next.config.ts`" can be checked, and one that says "looks like Next" cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// The adapter's id, so a caller can log which one claimed the repository.
    pub adapter: &'static str,
    /// Repo-relative paths that decided it, sorted.
    pub evidence: Vec<String>,
    /// How strongly it matched, for choosing between adapters that both claim the repository.
    pub confidence: Confidence,
}

/// How sure a detector is.
///
/// A framework dependency is [`Self::Strong`]: it is in `package.json`, so it is installed. A
/// script or a directory name is [`Self::Weak`]: it suggests the framework without proving it, and
/// a `config` rung pointing at a different stack should win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// A file name or script suggests it.
    Weak,
    /// The framework or the front-end build tool is a dependency.
    Strong,
}

/// What an adapter proposes for one boot.
///
/// Pure data: [`crate::boot::boot`] runs it. Keeping the plan separate from the run is what lets a
/// probe print a boot plan for a fixture without spawning anything, and what makes the ladder able
/// to compare rungs before it executes one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootPlan {
    /// The adapter or rung that produced it, for the outcome.
    pub source: String,
    /// Install command.
    pub install: Option<CommandSpec>,
    /// Build command.
    pub build: Option<CommandSpec>,
    /// Migration command (O04).
    pub migrate: Option<CommandSpec>,
    /// Seed command, from the config only: no adapter can guess a seed script.
    pub seed: Option<CommandSpec>,
    /// The long-running server.
    pub start: Option<CommandSpec>,
    /// Extra environment for every step, sorted by name.
    pub env: BTreeMap<String, String>,
    /// The variable the server reads its port from.
    pub port_env: String,
    /// The path that answers once the server is up.
    pub ready_path: String,
}

impl BootPlan {
    /// Whether the plan can boot anything: a start command is the minimum.
    #[must_use]
    pub fn can_boot(&self) -> bool {
        self.start.is_some()
    }
}

/// A migration tool an adapter recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationTool {
    /// The tool's name, as the plan spells it: `prisma`, `drizzle`, `typeorm` or `knex`.
    pub tool: &'static str,
    /// The command that applies migrations.
    pub command: CommandSpec,
}

/// One stack's adapter.
///
/// `Send + Sync` so an adapter can be stored in a table and used from several stages; the
/// implementations are zero-sized types, so this costs nothing.
pub trait StackAdapter: Send + Sync {
    /// The adapter's id: `ts-next`, `ts-vite`, `ts-node-server`.
    fn id(&self) -> &'static str;

    /// Whether this repository is this stack, and why.
    fn detect(&self, repo: &RepoView) -> Option<Detection>;

    /// The boot plan for this repository.
    fn boot_plan(&self, repo: &RepoView, cfg: &ProjectConfig) -> BootPlan;

    /// The migration tool, when the repository uses one this adapter knows.
    fn migrations(&self, repo: &RepoView) -> Option<MigrationTool>;

    /// The routes read from the code.
    fn route_table(&self, repo: &RepoView) -> Measured<RouteTable>;

    /// The command that runs the repository's own tests.
    fn test_command(&self, repo: &RepoView) -> Option<CommandSpec>;
}

/// Why route tables are absent in this build.
///
/// One constant, because M6 fills it in: the reason in the report and the code that produces it
/// should change together.
pub const ROUTES_LATER: &str = "route tables arrive in M6";
