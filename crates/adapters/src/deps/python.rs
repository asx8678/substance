//! `requirements*.txt`: the one place where a version may or may not be pinned (M1-T04).
//!
//! A requirements file is not a lockfile. `flask` and `flask>=3` let an install resolve to
//! whatever is current when the sandbox runs; `flask==3.0.0` fixes the version. The plan's
//! rule is that only `==` counts as locked, so the parser records which it saw in
//! [`LockKind`] instead of pretending every line is a pin.
//!
//! The readers handle what real files contain: comments, blank lines, line continuations,
//! `-r`/`-c` includes (recorded, not followed), options such as `-e .` and
//! `--index-url`, environment markers (`; python_version < "3.11"`), extras
//! (`requests[security]==2.31.0`), and hashes (`--hash=sha256:...`). A line the reader does
//! not understand is skipped rather than turned into a fake dependency, and the count of
//! skipped lines is available through [`RequirementsReport`] so a check can say the file was
//! only partly read.
//!
//! The file shape was confirmed from pip's requirements-file documentation (comments,
//! `==`/`>=`/`~=` specifiers, extras, environment markers, `--hash` options and `-r` files).

use substance_core::deps::{Dependency, Ecosystem, LockKind};

use super::rust::normalize_python_name;
use super::{DepsError, LockfileInput, ManifestDeps};

/// What a requirements file yielded, with the lines the reader could not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementsReport {
    /// The dependencies, unsorted.
    pub dependencies: Vec<Dependency>,
    /// Lines that looked like options, includes or prose rather than a requirement.
    pub skipped: Vec<String>,
    /// The files this one includes with `-r`/`-c`, repo-relative.
    pub includes: Vec<String>,
}

/// Reads a `requirements*.txt`.
///
/// `dev` is true for a file whose name says so (`requirements-dev.txt`,
/// `requirements_dev.txt`, `requirements-test.txt`): the file name is the only signal a
/// requirements file gives.
///
/// # Errors
///
/// Returns [`DepsError`] when the file holds no requirement at all — an empty file is a
/// mistake worth naming, not an empty dependency list.
pub fn parse_requirements(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let report = read_requirements(input, manifest);
    if report.dependencies.is_empty() {
        return Err(DepsError::new(input.path, "no requirement line was found"));
    }
    Ok(report.dependencies)
}

/// Reads a requirements file and keeps the lines it could not use.
#[must_use]
pub fn read_requirements(input: LockfileInput<'_>, manifest: &ManifestDeps) -> RequirementsReport {
    let dev_file = file_is_dev(input.path);
    let mut report = RequirementsReport {
        dependencies: Vec::new(),
        skipped: Vec::new(),
        includes: Vec::new(),
    };
    for logical in logical_lines(input.text) {
        let line = strip_environment_marker(&logical);
        if line.trim().is_empty() {
            continue;
        }
        if let Some(include) = requirement_include(&line) {
            report.includes.push(include);
            continue;
        }
        match parse_requirement(&line, dev_file, input.path, manifest) {
            Some(dependency) => report.dependencies.push(dependency),
            None => report.skipped.push(line.trim().to_owned()),
        }
    }
    report.includes.sort();
    report.includes.dedup();
    report
}

/// Joins continuation lines and drops comments, yielding one requirement per logical line.
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let line = strip_comment(raw);
        let trimmed = line.trim_end();
        if let Some(head) = trimmed.strip_suffix('\\') {
            pending.push_str(head);
            pending.push(' ');
            continue;
        }
        let mut full = std::mem::take(&mut pending);
        full.push_str(trimmed);
        if !full.trim().is_empty() {
            lines.push(full);
        }
    }
    if !pending.trim().is_empty() {
        lines.push(pending);
    }
    lines
}

/// Removes a `#` comment, which starts at the beginning of a line or after whitespace.
///
/// A `#` inside a URL fragment (`...#egg=name`) is not a comment.
fn strip_comment(line: &str) -> String {
    let mut previous_was_space = true;
    for (index, ch) in line.char_indices() {
        if ch == '#' && previous_was_space {
            return line[..index].to_owned();
        }
        previous_was_space = ch.is_whitespace();
    }
    line.to_owned()
}

/// Removes a `;`-separated environment marker, which is not part of the version.
fn strip_environment_marker(line: &str) -> String {
    match line.find(';') {
        Some(index) => line[..index].to_owned(),
        None => line.to_owned(),
    }
}

/// The target of a `-r`/`--requirement` or `-c`/`--constraint` line.
fn requirement_include(line: &str) -> Option<String> {
    let trimmed = line.trim();
    for flag in ["-r", "--requirement", "-c", "--constraint"] {
        if let Some(rest) = trimmed.strip_prefix(flag) {
            let rest = rest.trim_start_matches('=').trim();
            if !rest.is_empty() {
                return Some(rest.to_owned());
            }
        }
    }
    None
}

/// Whether the file name marks a development requirements file.
fn file_is_dev(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    ["dev", "test", "lint", "ci", "docs"]
        .iter()
        .any(|marker| name.contains(marker))
}

/// Parses one requirement line, or answers `None` for a line that is not one.
fn parse_requirement(
    line: &str,
    dev_file: bool,
    path: &str,
    manifest: &ManifestDeps,
) -> Option<Dependency> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // Options and editables are not requirements.
    if line.starts_with('-') {
        return None;
    }
    // A direct URL or a local path: `name @ https://...` or `./local`.
    if let Some((name, _url)) = line.split_once(" @ ") {
        let name = normalize_python_name(name.trim());
        if name.is_empty() {
            return None;
        }
        let (direct, dev) = manifest.classify(&name, dev_file);
        return Some(Dependency {
            ecosystem: Ecosystem::PyPi,
            name,
            version: "@url".to_owned(),
            direct,
            dev,
            lock_kind: LockKind::Loose,
            manifest_path: path.to_owned(),
        });
    }
    if line.starts_with("./") || line.starts_with("/") || line.starts_with("git+") {
        return None;
    }
    // Split the name (with optional extras) from the specifier.
    let mut split_at = line.len();
    for (index, ch) in line.char_indices() {
        if matches!(ch, '=' | '<' | '>' | '!' | '~' | '[' | ' ' | '\t') {
            split_at = index;
            break;
        }
    }
    let name = normalize_python_name(&line[..split_at]);
    if name.is_empty() {
        return None;
    }
    let rest = line[split_at..].trim();
    // Drop extras: `requests[security]==2.31.0`.
    let rest = match rest.strip_prefix('[') {
        Some(tail) => match tail.split_once(']') {
            Some((_, after)) => after.trim(),
            None => rest,
        },
        None => rest,
    };
    // Drop hashes: `flask==3.0.0 --hash=sha256:...`.
    let specifier = match rest.split_once("--hash") {
        Some((head, _)) => head.trim(),
        None => rest,
    };
    let (version, lock_kind) = match specifier.strip_prefix("==") {
        // A `==` with a trailing wildcard, `==3.0.*`, is not a pin: it still resolves to a
        // version that may not exist yet.
        Some(pinned) if !pinned.trim().is_empty() && !pinned.contains('*') => {
            (pinned.trim().to_owned(), LockKind::Locked)
        }
        Some(pinned) => (pinned.trim().to_owned(), LockKind::Loose),
        None if specifier.is_empty() => (String::from("*"), LockKind::Loose),
        None => (specifier.to_owned(), LockKind::Loose),
    };
    let (direct, dev) = manifest.classify(&name, dev_file);
    Some(Dependency {
        ecosystem: Ecosystem::PyPi,
        name,
        version,
        direct,
        dev,
        lock_kind,
        manifest_path: path.to_owned(),
    })
}
