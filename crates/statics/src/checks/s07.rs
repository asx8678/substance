//! S07 Config contradictions (M1-T08).
//!
//! Formula (action plan, S07):
//!
//! > Two configs for one tool (for example `.eslintrc*` alongside `eslint.config.*`);
//! > `"strict": false` in tsconfig; `#![allow(warnings)]` in a crate root; dependency sets
//! > that differ between `pyproject.toml`, `setup.py` and `requirements.txt`.
//!
//! Four questions, all about disagreement:
//!
//! - **Two configs for one tool.** ESLint's flat config and its legacy file can both exist
//!   while only one is read, so a project can "fix" lint by editing the file nobody loads.
//!   The same shape appears with `vitest.config.*` and `jest.config.*`, and with two
//!   `tsconfig` files that disagree about `strict`.
//! - **`strict: false` in tsconfig.** TypeScript's type checking is opt-in per project;
//!   turning it off is a decision the project is entitled to make and a reader is entitled to
//!   know about.
//! - **`#![allow(warnings)]` in a crate root.** The crate-wide form silences every lint the
//!   compiler has, including the ones that catch real mistakes.
//! - **Dependency sets that differ.** A `pyproject.toml`, a `setup.py` and a
//!   `requirements.txt` all saying different things means no single file describes what the
//!   project installs.
//!
//! Each answer is a hint: a repository may have two ESLint configs because it is mid-migration,
//! which is exactly what a reader should be told.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// Tools whose configuration can exist in both a legacy and a modern file.
///
/// A pair means "these two files configure the same tool", so finding both is the
/// contradiction. The list is deliberately short: a guess would accuse projects of
/// duplicating a config that belongs to two different tools.
const TOOL_CONFIG_PAIRS: &[(&str, &[&str], &[&str])] = &[
    (
        "eslint",
        &[
            ".eslintrc",
            ".eslintrc.js",
            ".eslintrc.cjs",
            ".eslintrc.json",
            ".eslintrc.yml",
        ],
        &[
            "eslint.config.js",
            "eslint.config.mjs",
            "eslint.config.cjs",
            "eslint.config.ts",
        ],
    ),
    ("typescript", &["tsconfig.json"], &["tsconfig.build.json"]),
    (
        "vitest",
        &["vitest.config.ts", "vitest.config.js", "vitest.config.mts"],
        &["jest.config.ts", "jest.config.js", "jest.config.cjs"],
    ),
];

/// Judges S07.
#[must_use]
pub async fn config_contradictions(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let mut findings: Vec<Finding> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let root_files: BTreeSet<String> = tree.root_files().into_iter().map(str::to_owned).collect();

    // 1. Two configs for one tool.
    let mut duplicate_configs = 0_usize;
    for (tool, legacy, modern) in TOOL_CONFIG_PAIRS {
        let found_legacy: Vec<&str> = legacy
            .iter()
            .copied()
            .filter(|name| root_files.contains(*name))
            .collect();
        let found_modern: Vec<&str> = modern
            .iter()
            .copied()
            .filter(|name| root_files.contains(*name))
            .collect();
        if found_legacy.is_empty() || found_modern.is_empty() {
            continue;
        }
        duplicate_configs += 1;
        let names: Vec<&str> = found_legacy
            .iter()
            .chain(found_modern.iter())
            .copied()
            .collect();
        findings.push(file_finding::at(
            run_id,
            CheckId::S07,
            "duplicate-tool-config",
            &format!("{tool} is configured twice: {}", names.join(", ")),
            Location::file(names[0]),
        ));
    }

    // 2. `"strict": false` in a tsconfig.
    let mut strict_off = 0_usize;
    let tsconfigs: Vec<String> = root_files
        .iter()
        .filter(|name| name.starts_with("tsconfig") && name.ends_with(".json"))
        .cloned()
        .collect();
    for path in tsconfigs {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        if let Some(line) = strict_false_line(&text) {
            strict_off += 1;
            findings.push(file_finding::at(
                run_id,
                CheckId::S07,
                "typescript-strict-off",
                &format!("{path} turns TypeScript's strict mode off"),
                Location::lines(&path, line, line),
            ));
        }
    }

    // 3. `#![allow(warnings)]` in a crate root.
    let mut crate_wide_allow = 0_usize;
    let crate_roots = crate_roots(tree);
    for (manifest, root) in crate_roots {
        let Some(text) = tree.text(&root).await else {
            continue;
        };
        if let Some(line) = allow_warnings_line(&text) {
            crate_wide_allow += 1;
            findings.push(file_finding::at(
                run_id,
                CheckId::S07,
                "rust-allow-warnings",
                &format!(
                    "{root} silences every compiler warning for the {} crate",
                    crate_name(&manifest)
                ),
                Location::lines(&root, line, line),
            ));
        }
    }

    // 4. Dependency sets that differ between the three Python manifests.
    let (python_disagreement, python_note) = python_dependencies(tree).await;
    notes.extend(python_note);
    if let Some((path, count)) = &python_disagreement {
        findings.push(file_finding::at(
            run_id,
            CheckId::S07,
            "python-dependency-disagreement",
            &format!(
                "{count} dependencies differ between pyproject.toml, setup.py and requirements.txt"
            ),
            Location::file(path),
        ));
    }

    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert(
        "duplicate_tool_config".to_owned(),
        as_i64(duplicate_configs),
    );
    metrics.insert("typescript_strict_off".to_owned(), as_i64(strict_off));
    metrics.insert("rust_allow_warnings".to_owned(), as_i64(crate_wide_allow));
    metrics.insert(
        "python_disagreements".to_owned(),
        as_i64(python_disagreement.iter().map(|(_, count)| *count).sum()),
    );
    metrics.insert("occurrences".to_owned(), as_i64(findings.len()));

    let (findings, cut) = cap(findings);
    notes.extend(cut);
    StaticReport::reported(CheckId::S07, metrics)
        .with_findings(findings)
        .with_notes(notes)
}

/// The line holding `"strict": false`, when a tsconfig turns strict mode off.
///
/// Text search rather than JSON parsing on purpose: a tsconfig may be JSON with comments, which
/// no strict JSON parser accepts, and the question is only whether the file says `false`.
#[must_use]
pub fn strict_false_line(text: &str) -> Option<u32> {
    for (index, line) in text.lines().enumerate() {
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("\"strict\":false") || compact.contains("'strict':false") {
            return Some(as_line(index));
        }
    }
    None
}

/// The line holding a crate-wide `#![allow(warnings)]`.
#[must_use]
pub fn allow_warnings_line(text: &str) -> Option<u32> {
    for (index, line) in text.lines().enumerate() {
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.starts_with("#![allow(warnings)]") || compact == "#![allow(warnings)]" {
            return Some(as_line(index));
        }
    }
    None
}

/// The crates in the tree: `(Cargo.toml path, the lib.rs or main.rs it declares)`.
///
/// A crate root is the file that carries the crate's inner attributes; `src/lib.rs` and
/// `src/main.rs` are the two Cargo allows, and a workspace member's is the same pair under its
/// own directory.
fn crate_roots(tree: &TrackedTree<'_>) -> Vec<(String, String)> {
    let manifests: Vec<String> = tree
        .paths()
        .into_iter()
        .filter(|path| TrackedTree::file_name(path) == "Cargo.toml")
        .map(str::to_owned)
        .collect();
    let mut roots: Vec<(String, String)> = Vec::new();
    for manifest in manifests {
        let dir = match manifest.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/"),
            None => String::new(),
        };
        for candidate in ["src/lib.rs", "src/main.rs"] {
            let path = format!("{dir}{candidate}");
            if tree.has(&path) {
                roots.push((manifest.clone(), path));
            }
        }
    }
    roots.sort();
    roots
}

/// The crate's name, from its manifest's path.
fn crate_name(manifest: &str) -> &str {
    match manifest.rsplit_once('/') {
        Some((parent, _)) => match parent.rsplit_once('/') {
            Some((_, name)) => name,
            None => parent,
        },
        None => "root",
    }
}

/// Compares the dependency names in `pyproject.toml`, `setup.py` and `requirements.txt`.
///
/// Returns the file to report and how many dependencies differ, when they do, plus a note
/// explaining what could not be compared. The comparison is by name only: two files that list
/// the same packages at different versions are a version disagreement, not "different
/// dependency sets", and the catalog asks about the sets.
async fn python_dependencies(tree: &mut TrackedTree<'_>) -> (Option<(String, usize)>, Vec<String>) {
    let mut sources: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for path in ["pyproject.toml", "setup.py", "requirements.txt"] {
        if !tree.has(path) {
            continue;
        }
        let Some(text) = tree.text(path).await else {
            continue;
        };
        let names = python_dependency_names(path, &text);
        sources.push((path, names));
    }
    if sources.len() < 2 {
        return (
            None,
            vec![
                "fewer than two Python dependency files, so there is nothing to disagree"
                    .to_owned(),
            ],
        );
    }
    let mut notes: Vec<String> = Vec::new();
    let union: BTreeSet<&String> = sources.iter().flat_map(|(_, names)| names.iter()).collect();
    let mut differ: BTreeSet<String> = BTreeSet::new();
    for name in union {
        let present = sources
            .iter()
            .filter(|(_, names)| names.contains(name))
            .count();
        if present != sources.len() {
            differ.insert(name.clone());
        }
    }
    if differ.is_empty() {
        return (None, notes);
    }
    let described: Vec<&str> = differ.iter().take(5).map(String::as_str).collect();
    notes.push(format!(
        "{} not in every file: {}{}",
        differ.len(),
        described.join(", "),
        if differ.len() > described.len() {
            ", …"
        } else {
            ""
        }
    ));
    (Some((sources[0].0.to_owned(), differ.len())), notes)
}

/// The dependency names one Python manifest lists.
///
/// A name is what precedes a version specifier or a key: `requests>=2` and `"requests" = "^2"`
/// both yield `requests`. Parsing is line-based because `setup.py` is Python code and the
/// question is only which names appear in its requirement strings.
#[must_use]
pub fn python_dependency_names(path: &str, text: &str) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut in_requirements_array = path == "setup.py";
    for line in text.lines() {
        let trimmed = line.trim();
        if path == "setup.py" {
            // Only the requirement strings inside the install_requires list.
            if trimmed.contains("install_requires") {
                in_requirements_array = true;
                continue;
            }
            if !in_requirements_array {
                continue;
            }
            if trimmed.starts_with(']') {
                in_requirements_array = false;
                continue;
            }
            if let Some(quoted) = first_quoted(trimmed)
                && let Some(name) = dependency_name(&quoted)
            {
                names.insert(name);
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if path == "requirements.txt" {
            if let Some(name) = dependency_name(trimmed) {
                names.insert(name);
            }
            continue;
        }
        // pyproject.toml: `name = "…"` inside `[project] dependencies` or
        // `[tool.poetry.dependencies]`.
        if trimmed == "[project]" || trimmed.starts_with("dependencies") || trimmed.starts_with("[")
        {
            continue;
        }
        if let Some((key, _)) = trimmed.split_once('=') {
            let key = key.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
            {
                names.insert(key.to_ascii_lowercase());
            }
        }
    }
    names.retain(|name| {
        name != "python"
            && name != "requires-python"
            && name != "name"
            && name != "version"
            && name != "description"
    });
    names
}

/// The name part of a requirement string: `requests[security]>=2 ; python_version>"3"` →
/// `requests`.
#[must_use]
pub fn dependency_name(requirement: &str) -> Option<String> {
    let trimmed = requirement.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.starts_with('#') {
        return None;
    }
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .unwrap_or(trimmed.len());
    let name = &trimmed[..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

/// The first quoted string on a line, if any.
fn first_quoted(line: &str) -> Option<String> {
    let mut chars = line.char_indices();
    let (start, quote) = chars.find(|(_, c)| *c == '"' || *c == '\'')?;
    let rest = &line[start + quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_owned())
}

/// A zero-based line index as a 1-based line number.
fn as_line(index: usize) -> u32 {
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}
