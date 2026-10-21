//! Lockfile parsers: one function per format, one dependency shape (M1-T04).
//!
//! The dependency checks (P01–P07) need one list per repository: which packages, at which
//! versions, and which of them the project asked for itself. Every format here yields the
//! same [`Dependency`], so a check never branches on which lockfile a project ships.
//!
//! Three rules hold across the formats:
//!
//! - **A version is either pinned or it is not.** A lockfile pins by construction
//!   ([`LockKind::Locked`]); a `requirements.txt` line is pinned only when it says `==`,
//!   and a range is [`LockKind::Loose`], because an install may then resolve to a release
//!   published after the run.
//! - **Output is sorted.** [`substance_core::deps::sorted_unique`] sorts by
//!   `(ecosystem, name, version, manifest_path)` and drops exact duplicates, so one
//!   repository always produces one vector.
//! - **A parse failure is typed and names the file.** No parser panics on a malformed
//!   file, a BOM, a comment where a value was expected, or a format newer than this code.
//!
//! Which lockfile wins when a repository has several for one ecosystem is the locked-install
//! table's question, not each format's: [`LockfileKind::install_preference`] records the
//! order, and [`choose`] applies it, so npm's `package-lock.json` beats a stray `yarn.lock`
//! in the same directory exactly as `npm ci` would.

pub mod python;
pub mod rust;
pub mod ts;

use std::collections::BTreeMap;
use std::path::Path;

use substance_core::deps::{Dependency, Ecosystem, sorted_unique};

pub use python::{RequirementsReport, parse_requirements, read_requirements};
pub use rust::{parse_cargo_lock, parse_poetry_lock, parse_uv_lock};
pub use ts::{parse_bun_lock, parse_package_lock, parse_pnpm_lock, parse_yarn_lock};

/// Which parser reads a file, and what the locked-install table does with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LockfileKind {
    /// `package-lock.json` or `npm-shrinkwrap.json`; `npm ci`.
    PackageLock,
    /// `pnpm-lock.yaml`; `pnpm install --frozen-lockfile`.
    PnpmLock,
    /// `yarn.lock`, either the v1 text format or berry; `yarn install --immutable`.
    YarnLock,
    /// `bun.lock`; `bun install --frozen-lockfile`.
    BunLock,
    /// `Cargo.lock`; `cargo build --locked`.
    CargoLock,
    /// `uv.lock`; `uv sync --locked`.
    UvLock,
    /// `poetry.lock`; `poetry install`.
    PoetryLock,
    /// `requirements*.txt`; a pinned install.
    Requirements,
}

impl LockfileKind {
    /// The parser for a file name, or `None` for a file that is not a lockfile this crate
    /// reads.
    ///
    /// `requirements*.txt` matches by prefix, because projects name the file
    /// `requirements.txt`, `requirements-dev.txt`, `requirements_prod.txt` and so on.
    #[must_use]
    pub fn of_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        match name {
            "package-lock.json" | "npm-shrinkwrap.json" => Some(Self::PackageLock),
            "pnpm-lock.yaml" => Some(Self::PnpmLock),
            "yarn.lock" => Some(Self::YarnLock),
            "bun.lock" => Some(Self::BunLock),
            "Cargo.lock" => Some(Self::CargoLock),
            "uv.lock" => Some(Self::UvLock),
            "poetry.lock" => Some(Self::PoetryLock),
            other if other.starts_with("requirements") && other.ends_with(".txt") => {
                Some(Self::Requirements)
            }
            _ => None,
        }
    }

    /// The ecosystem the file describes.
    #[must_use]
    pub const fn ecosystem(self) -> Ecosystem {
        match self {
            Self::PackageLock | Self::PnpmLock | Self::YarnLock | Self::BunLock => Ecosystem::Npm,
            Self::CargoLock => Ecosystem::Crates,
            Self::UvLock | Self::PoetryLock | Self::Requirements => Ecosystem::PyPi,
        }
    }

    /// How strongly the locked-install table prefers this file within its ecosystem.
    ///
    /// The order follows the table in the architecture plan: a project that ships a
    /// `package-lock.json` installs with `npm ci`, so that lockfile is the one the report
    /// trusts, and another npm lockfile beside it is noted rather than merged. Lower wins.
    #[must_use]
    pub const fn install_preference(self) -> u8 {
        match self {
            Self::PackageLock => 0,
            Self::PnpmLock => 1,
            Self::BunLock => 2,
            Self::YarnLock => 3,
            Self::CargoLock => 0,
            Self::UvLock => 0,
            Self::PoetryLock => 1,
            Self::Requirements => 2,
        }
    }

    /// The lockfile's own spelling, used in reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PackageLock => "package-lock",
            Self::PnpmLock => "pnpm-lock",
            Self::YarnLock => "yarn-lock",
            Self::BunLock => "bun-lock",
            Self::CargoLock => "cargo-lock",
            Self::UvLock => "uv-lock",
            Self::PoetryLock => "poetry-lock",
            Self::Requirements => "requirements",
        }
    }
}

/// Why a lockfile could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {detail}")]
pub struct DepsError {
    /// The file, repo-relative when the caller knows it.
    pub path: String,
    /// What was wrong with it.
    pub detail: String,
}

impl DepsError {
    /// A failure naming `path`.
    #[must_use]
    pub fn new(path: &str, detail: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            detail: detail.into(),
        }
    }
}

/// What the project's own manifests declare, so a lockfile's packages can be marked
/// direct and dev.
///
/// Built from `package.json`, `Cargo.toml` and `pyproject.toml`. When a manifest is not
/// available the sets are empty, and every package in its lockfile is reported with
/// `direct: false, dev: false`: a plausible guess would be worse than a stated gap, and a
/// check that needs directness reports it as unknown instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestDeps {
    /// Names the project asks for at runtime, sorted.
    pub direct: Vec<String>,
    /// Names the project asks for in development, tests or a build only, sorted.
    pub dev: Vec<String>,
}

impl ManifestDeps {
    /// Builds the sets from unsorted names.
    #[must_use]
    pub fn new(mut direct: Vec<String>, mut dev: Vec<String>) -> Self {
        direct.sort_unstable();
        direct.dedup();
        dev.sort_unstable();
        dev.dedup();
        Self { direct, dev }
    }

    /// Whether `name` is a direct runtime dependency.
    #[must_use]
    pub fn is_direct(&self, name: &str) -> bool {
        self.direct.iter().any(|candidate| candidate == name)
    }

    /// Whether `name` is a direct development dependency.
    #[must_use]
    pub fn is_dev(&self, name: &str) -> bool {
        self.dev.iter().any(|candidate| candidate == name)
    }

    /// Whether the manifests were read at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.direct.is_empty() && self.dev.is_empty()
    }

    /// Reads a `package.json`: `dependencies` is runtime, `devDependencies` development,
    /// and `optionalDependencies`/`peerDependencies` are neither asked for nor installed by
    /// the project itself, so they count as direct runtime names only when nothing else
    /// claims them.
    ///
    /// # Errors
    ///
    /// Returns [`DepsError`] when the file is not JSON.
    pub fn from_package_json(path: &str, text: &str) -> Result<Self, DepsError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| DepsError::new(path, format!("not JSON: {error}")))?;
        let mut direct = names_of(&value, "dependencies");
        direct.extend(names_of(&value, "optionalDependencies"));
        let dev = names_of(&value, "devDependencies");
        Ok(Self::new(direct, dev))
    }

    /// Whether a package with this name and `dev` flag is what the manifest asked for.
    ///
    /// A package the lockfile marks dev but the manifest lists as runtime is runtime: the
    /// manifest is the project's own statement, the lockfile only records what an install
    /// resolved to.
    #[must_use]
    fn classify(&self, name: &str, lockfile_dev: bool) -> (bool, bool) {
        let direct = self.is_direct(name) || self.is_dev(name);
        let dev = self.is_dev(name) || (lockfile_dev && !self.is_direct(name));
        (direct, dev)
    }
}

/// The string keys of a JSON object member, sorted.
fn names_of(value: &serde_json::Value, key: &str) -> Vec<String> {
    let mut names: Vec<String> = value
        .get(key)
        .and_then(serde_json::Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    names.sort_unstable();
    names
}

/// One lockfile to parse.
#[derive(Debug, Clone, Copy)]
pub struct LockfileInput<'a> {
    /// The path as the report spells it: repo-relative, forward slashes.
    pub path: &'a str,
    /// The file's text.
    pub text: &'a str,
}

/// Parses one lockfile, choosing the parser from its file name.
///
/// `manifest` carries the project's own dependency names when they were read; an empty one
/// is allowed and yields `direct: false, dev: false` records.
///
/// # Errors
///
/// Returns [`DepsError`] when the file name is not a recognised lockfile, or when the file
/// is not in the shape its parser expects.
pub fn parse_lockfile(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let path = Path::new(input.path);
    let Some(kind) = LockfileKind::of_path(path) else {
        return Err(DepsError::new(
            input.path,
            "not a lockfile this build parses",
        ));
    };
    let mut dependencies = match kind {
        LockfileKind::PackageLock => parse_package_lock(input, manifest)?,
        LockfileKind::PnpmLock => parse_pnpm_lock(input, manifest)?,
        LockfileKind::YarnLock => parse_yarn_lock(input, manifest)?,
        LockfileKind::BunLock => parse_bun_lock(input, manifest)?,
        LockfileKind::CargoLock => parse_cargo_lock(input, manifest)?,
        LockfileKind::UvLock => parse_uv_lock(input, manifest)?,
        LockfileKind::PoetryLock => parse_poetry_lock(input, manifest)?,
        LockfileKind::Requirements => parse_requirements(input, manifest)?,
    };
    dependencies = sorted_unique(std::mem::take(&mut dependencies));
    Ok(dependencies)
}

/// Picks the lockfile the locked-install table would use, per ecosystem.
///
/// `paths` is every lockfile found, repo-relative. The result is sorted by ecosystem and,
/// within one ecosystem, by [`LockfileKind::install_preference`], so the first entry for an
/// ecosystem is the one to parse; [`choose`] returns exactly that map, and the caller can
/// name the files it skipped.
#[must_use]
pub fn choose(paths: &[String]) -> BTreeMap<Ecosystem, (String, LockfileKind)> {
    let mut chosen: BTreeMap<Ecosystem, (String, LockfileKind)> = BTreeMap::new();
    let mut sorted: Vec<&String> = paths.iter().collect();
    sorted.sort();
    for path in sorted {
        let Some(kind) = LockfileKind::of_path(Path::new(path)) else {
            continue;
        };
        let candidate = (path.clone(), kind);
        match chosen.get(&kind.ecosystem()) {
            Some((_, current)) if current.install_preference() <= kind.install_preference() => {}
            _ => {
                chosen.insert(kind.ecosystem(), candidate);
            }
        }
    }
    chosen
}

/// The version string without a leading `v`, which some lockfiles carry.
///
/// A leading `v` is not part of a semver version, and the registry APIs key on the plain
/// form; leaving it would make `v1.2.3` and `1.2.3` two different packages.
#[must_use]
pub(crate) fn clean_version(text: &str) -> String {
    let trimmed = text.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    // A version starts with a digit; `version` fields sometimes hold a path or a URL, which
    // must not be mistaken for one.
    if stripped.starts_with(|ch: char| ch.is_ascii_digit()) {
        stripped.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Strips a registry prefix such as `npm:` or `@scope/` handling from a specifier.
#[must_use]
pub(crate) fn strip_protocol(specifier: &str) -> &str {
    for prefix in ["npm:", "cargo:", "pypi:"] {
        if let Some(rest) = specifier.strip_prefix(prefix) {
            return rest;
        }
    }
    specifier
}
