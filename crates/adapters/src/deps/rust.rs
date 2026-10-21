//! The Rust lockfile: `Cargo.lock` (M1-T04).
//!
//! `Cargo.lock` is TOML with one `[[package]]` array: every entry has a `name`, a
//! `version`, and optionally `source` (absent for a workspace member) and `dependencies`.
//! A workspace member has no `source`, which is how the project's own crates are told
//! apart from registry packages; a member is not a dependency of the project, so it is not
//! reported as one.
//!
//! Directness comes from `Cargo.toml`, not from the lockfile: the lockfile does not say
//! which packages the project asked for. `ManifestDeps` carries those names; when it is
//! empty, every package is reported with `direct: false` rather than a guess.
//!
//! The shape was confirmed from the Cargo book (`Cargo.lock` is generated and not meant to
//! be edited; `[[package]]` with `name`, `version`, `source`, `checksum`, `dependencies`).

use substance_core::deps::{Dependency, Ecosystem, LockKind};

use super::{DepsError, LockfileInput, ManifestDeps, clean_version};

/// The members of `[[package]]` as read from the file.
#[derive(Debug, serde::Deserialize)]
struct CargoLock {
    #[serde(default)]
    package: Vec<CargoPackage>,
}

/// One `[[package]]` entry.
#[derive(Debug, serde::Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    #[serde(default)]
    source: Option<String>,
}

/// Reads a `Cargo.lock`.
///
/// # Errors
///
/// Returns [`DepsError`] when the file is not TOML in the `Cargo.lock` shape.
pub fn parse_cargo_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let lock: CargoLock = toml::from_str(input.text)
        .map_err(|error| DepsError::new(input.path, format!("not a Cargo.lock: {error}")))?;
    if lock.package.is_empty() {
        return Err(DepsError::new(input.path, "no [[package]] entry"));
    }
    let mut out = Vec::new();
    for package in lock.package {
        // A workspace member has no `source`; it is the project, not a dependency.
        if package.source.is_none() {
            continue;
        }
        let (direct, dev) = manifest.classify(&package.name, false);
        out.push(Dependency {
            ecosystem: Ecosystem::Crates,
            name: package.name,
            version: clean_version(&package.version),
            direct,
            dev,
            lock_kind: LockKind::Locked,
            manifest_path: input.path.to_owned(),
        });
    }
    Ok(out)
}

/// Reads a `uv.lock`.
///
/// `uv.lock` is TOML with `[[package]]` entries; unlike Cargo, a uv package has no `source`
/// field to tell a workspace member apart, so a package named in the project's own
/// `pyproject.toml`/`uv.lock` `[[package]]` with a `source = { editable = "." }` or
/// `{ virtual = "." }` is skipped: those are the project itself.
///
/// # Errors
///
/// Returns [`DepsError`] when the file is not TOML or holds no package.
pub fn parse_uv_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let lock: UvLock = toml::from_str(input.text)
        .map_err(|error| DepsError::new(input.path, format!("not a uv.lock: {error}")))?;
    if lock.package.is_empty() {
        return Err(DepsError::new(input.path, "no [[package]] entry"));
    }
    let mut out = Vec::new();
    for package in lock.package {
        if package
            .source
            .as_ref()
            .is_some_and(|source| source.is_local())
        {
            continue;
        }
        let (direct, dev) = manifest.classify(&package.name, false);
        out.push(Dependency {
            ecosystem: Ecosystem::PyPi,
            name: normalize_python_name(&package.name),
            version: clean_version(&package.version),
            direct,
            dev,
            lock_kind: LockKind::Locked,
            manifest_path: input.path.to_owned(),
        });
    }
    Ok(out)
}

/// The `[[package]]` members of a `uv.lock`.
#[derive(Debug, serde::Deserialize)]
struct UvLock {
    #[serde(default)]
    package: Vec<UvPackage>,
}

/// One uv package.
#[derive(Debug, serde::Deserialize)]
struct UvPackage {
    name: String,
    version: String,
    #[serde(default)]
    source: Option<UvSource>,
}

/// A uv package's `source` table: `{ registry = "..." }` for a dependency, `{ editable =
/// "." }` or `{ virtual = "." }` for the project itself.
#[derive(Debug, serde::Deserialize)]
struct UvSource {
    #[serde(default)]
    editable: Option<String>,
    #[serde(default)]
    virtual_path: Option<String>,
}

impl UvSource {
    /// Whether this source is the project or a path dependency inside the repository.
    fn is_local(&self) -> bool {
        self.editable.is_some() || self.virtual_path.is_some()
    }
}

/// Reads a `poetry.lock`.
///
/// `poetry.lock` is TOML with `[[package]]` entries, each carrying `name`, `version`,
/// `category` (`main` or `dev`) and `optional`. The category is what marks a development
/// dependency when the project's `pyproject.toml` was not read.
///
/// # Errors
///
/// Returns [`DepsError`] when the file is not TOML or holds no package.
pub fn parse_poetry_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let lock: PoetryLock = toml::from_str(input.text)
        .map_err(|error| DepsError::new(input.path, format!("not a poetry.lock: {error}")))?;
    if lock.package.is_empty() {
        return Err(DepsError::new(input.path, "no [[package]] entry"));
    }
    let mut out = Vec::new();
    for package in lock.package {
        let lockfile_dev = package
            .category
            .as_deref()
            .is_some_and(|category| category == "dev");
        let (direct, dev) = manifest.classify(&package.name, lockfile_dev);
        out.push(Dependency {
            ecosystem: Ecosystem::PyPi,
            name: normalize_python_name(&package.name),
            version: clean_version(&package.version),
            direct,
            dev,
            lock_kind: LockKind::Locked,
            manifest_path: input.path.to_owned(),
        });
    }
    Ok(out)
}

/// The `[[package]]` members of a `poetry.lock`.
#[derive(Debug, serde::Deserialize)]
struct PoetryLock {
    #[serde(default)]
    package: Vec<PoetryPackage>,
}

/// One poetry package.
#[derive(Debug, serde::Deserialize)]
struct PoetryPackage {
    name: String,
    version: String,
    #[serde(default)]
    category: Option<String>,
}

/// Normalizes a Python distribution name the way PEP 503 does: lowercase, with runs of
/// `-`, `_` and `.` collapsed to one `-`.
///
/// The registries key on the normalized name, so `Flask-Cors`, `flask_cors` and
/// `flask.cors` are one package and must not appear as three dependencies.
#[must_use]
pub fn normalize_python_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_separator = false;
    for ch in name.trim().chars() {
        let is_separator = matches!(ch, '-' | '_' | '.');
        if is_separator {
            if !last_was_separator {
                out.push('-');
            }
            last_was_separator = true;
        } else {
            out.extend(ch.to_lowercase());
            last_was_separator = false;
        }
    }
    out.trim_matches('-').to_owned()
}
