//! The dependency vocabulary: ecosystems, lockfiles and one dependency record (M1-T04).
//!
//! Two crates need the same words. `substance-adapters` parses lockfiles into
//! [`Dependency`] values; `substance-history` must agree with it on which files are
//! lockfiles, so the authored-line rule does not count a lockfile as code while the
//! dependency report treats it as a manifest. One list here is the single source of truth
//! for both, and a lockfile the parsers do not understand is still recognised as a
//! lockfile — it is excluded from authored lines whether or not it can be parsed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Every file name treated as a lockfile, sorted.
///
/// The first group is what the parsers in `substance-adapters` read; the rest are
/// lockfiles of ecosystems substance does not judge yet, listed so that the authored-line
/// rule excludes them too. `bun.lockb` is the old binary Bun lockfile and `bun.lock` the
/// text one; both are lockfiles, and only the text one can be parsed.
pub const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "Gemfile.lock",
    "bun.lock",
    "bun.lockb",
    "composer.lock",
    "go.sum",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "poetry.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
];

/// Whether `file_name` names a lockfile.
#[must_use]
pub fn is_lockfile(file_name: &str) -> bool {
    LOCKFILES.contains(&file_name)
}

/// A package ecosystem, spelled the way the registries spell it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    /// crates.io, for Rust.
    #[serde(rename = "crates")]
    Crates,
    /// The npm registry, for JavaScript and TypeScript.
    #[serde(rename = "npm")]
    Npm,
    /// PyPI, for Python.
    #[serde(rename = "pypi")]
    PyPi,
}

impl Ecosystem {
    /// The registry's own spelling: `crates`, `npm` or `pypi`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Crates => "crates",
            Self::Npm => "npm",
            Self::PyPi => "pypi",
        }
    }

    /// The deps.dev system name for this ecosystem.
    ///
    /// deps.dev spells them `CARGO`, `NPM` and `PYPI` in URL paths.
    #[must_use]
    pub const fn deps_dev_system(self) -> &'static str {
        match self {
            Self::Crates => "CARGO",
            Self::Npm => "NPM",
            Self::PyPi => "PYPI",
        }
    }

    /// Every ecosystem, in the order reports list them.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Crates, Self::Npm, Self::PyPi]
    }
}

impl std::fmt::Display for Ecosystem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a dependency's version is pinned by a lockfile or only constrained by a specifier.
///
/// A `requirements.txt` line is the only place the distinction shows up in one file: `flask`
/// or `flask>=3` is [`LockKind::Loose`] — an install may resolve to a version published
/// after the run — while `flask==3.0.0` is [`LockKind::Locked`]. Every lockfile is
/// [`LockKind::Locked`] by construction.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum LockKind {
    /// The exact version is recorded.
    Locked,
    /// Only a range or a name is recorded.
    Loose,
}

/// One dependency, as a lockfile or a requirements file records it.
///
/// The sort key is `(ecosystem, name, version, manifest_path)`, spelled out in
/// [`Dependency::sort_key`] so two runs produce one order.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Dependency {
    /// Which registry the package comes from.
    pub ecosystem: Ecosystem,
    /// The package name, normalized by the registry's own rule.
    pub name: String,
    /// The exact version, or the specifier text for a loose requirement.
    pub version: String,
    /// Whether the project's own manifest asks for it, as opposed to a transitive package.
    pub direct: bool,
    /// Whether it is used only for development, tests or building.
    pub dev: bool,
    /// Whether the version is pinned.
    pub lock_kind: LockKind,
    /// The lockfile or manifest this record came from, repo-relative.
    pub manifest_path: String,
}

impl Dependency {
    /// The fields that decide the order of a dependency list.
    ///
    /// Ecosystem first, then name, version and the file it came from: a report grouped by
    /// ecosystem reads top to bottom, and two lockfiles naming the same package at
    /// different versions stay distinct and adjacent.
    #[must_use]
    pub fn sort_key(&self) -> (Ecosystem, &str, &str, &str) {
        (
            self.ecosystem,
            self.name.as_str(),
            self.version.as_str(),
            self.manifest_path.as_str(),
        )
    }
}

/// Sorts a dependency list by [`Dependency::sort_key`] and drops exact duplicates.
///
/// Duplicates are real: pnpm's lockfile records one package once per importer that
/// depends on it, and poetry repeats a package across groups. One record per
/// `(ecosystem, name, version, manifest_path)` is what a check wants to count.
#[must_use]
pub fn sorted_unique(mut list: Vec<Dependency>) -> Vec<Dependency> {
    list.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    list.dedup();
    list
}
