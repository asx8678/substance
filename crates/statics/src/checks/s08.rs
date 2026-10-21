//! S08 Docs honesty (M1-T08).
//!
//! Formula (action plan, S08):
//!
//! > Commands in README `sh`/`bash`/`shell`/`console` blocks that don't exist (npm, pnpm or yarn
//! > scripts; make and just targets; cargo bins; referenced paths). Environment variables read
//! > in code (`process.env.X`, `import.meta.env.X`, `os.environ[...]`, `os.getenv`,
//! > `std::env::var`, `env!`) but missing from the README and `.env.example`.
//!
//! Two halves, both about a claim that does not hold:
//!
//! - **Commands**: a README that says `npm run build` promises a `build` script. If
//!   `package.json` has no such script, the first thing a reader tries fails. The same for
//!   `make <target>` (no such target in the Makefile), `cargo <bin>` (no such `[[bin]]` or
//!   `src/bin/`), and a path the README names that the commit does not contain.
//! - **Environment variables**: code reads `process.env.DATABASE_URL`, so a reader needs to
//!   know it exists. If neither the README nor `.env.example` mentions it, setup fails at the
//!   first query with an error that says nothing about configuration.
//!
//! The check is deliberately conservative about both. A command is only reported when the file
//! it depends on is present and readable — a README that documents `npm run build` beside no
//! `package.json` at all is a documentation-only repository, not a lie. And a path is only
//! reported when it looks like a path (it has a `/` or a known source extension), because a
//! bare word in a shell block is usually an argument.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// Files a README may be.
const README_NAMES: &[&str] = &[
    "README.md",
    "README.markdown",
    "README.rst",
    "README.txt",
    "README",
];

/// Shell-block fences the README may use.
const SHELL_FENCES: &[&str] = &["sh", "bash", "shell", "console", "zsh"];

/// Judges S08.
#[must_use]
pub async fn docs_honesty(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let Some(readme_path) = tree.first_of(README_NAMES) else {
        return StaticReport::not_measured(
            CheckId::S08,
            "no README at the commit, so there are no documented commands to check",
        );
    };
    let Some(readme) = tree.text(&readme_path).await else {
        return StaticReport::not_measured(
            CheckId::S08,
            format!(
                "{readme_path} could not be read, so there are no documented commands to check"
            ),
        );
    };

    let mut findings: Vec<Finding> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();

    // 1. Commands that do not exist.
    let npm_scripts = npm_script_names(tree).await;
    let make_targets = make_target_names(tree).await;
    let cargo_bins = cargo_binary_names(tree).await;
    let mut unknown_commands = 0_usize;
    let mut missing_paths = 0_usize;
    for (line_no, command) in shell_lines(&readme) {
        for (kind, program, argument) in parsed_commands(&command) {
            let missing = match (kind.as_str(), program.as_str()) {
                (_, "npm") | (_, "pnpm") | (_, "yarn") => {
                    matches!(argument.as_str(), "run" | "") || {
                        // `npm run build`, `pnpm build`, `yarn build`.
                        let script = script_argument(&argument);
                        script.is_some_and(|name| !npm_scripts.contains(name))
                    }
                }
                (_, "make") | (_, "just") => {
                    !argument.is_empty() && !make_targets.contains(&argument)
                }
                (_, "cargo") => {
                    // `cargo run --bin x` and `cargo install --path .` are covered by the path
                    // half; a bare `cargo <name>` is only meaningful when it names a binary.
                    !argument.is_empty()
                        && !is_cargo_subcommand(&argument)
                        && !cargo_bins.contains(&argument)
                }
                _ => false,
            };
            if missing {
                unknown_commands += 1;
                findings.push(file_finding::at(
                    run_id,
                    CheckId::S08,
                    "readme-command-missing",
                    &format!("{readme_path} documents `{command}`, which does not exist"),
                    Location::lines(&readme_path, line_no, line_no),
                ));
                continue;
            }
            // Referenced paths: a token with a slash, or one of the languages' source files,
            // that the commit does not track.
            if let Some(word) = command.split_whitespace().last()
                && looks_like_path(word)
                && !tree.has(word.trim_start_matches("./"))
            {
                missing_paths += 1;
                findings.push(file_finding::at(
                    run_id,
                    CheckId::S08,
                    "readme-path-missing",
                    &format!("{readme_path} points at `{word}`, which the commit does not track"),
                    Location::lines(&readme_path, line_no, line_no),
                ));
            }
        }
    }

    // 2. Environment variables read in code but undocumented.
    let documented = documented_env(tree, &readme).await;
    let read_in_code = env_vars(tree).await;
    let mut undocumented = 0_usize;
    for (name, path, line_no) in &read_in_code {
        if documented.contains(name) {
            continue;
        }
        undocumented += 1;
        findings.push(file_finding::at(
            run_id,
            CheckId::S08,
            "env-var-undocumented",
            &format!("{path}:{line_no} reads `{name}`, which neither the README nor .env.example mentions"),
            Location::lines(path, *line_no, *line_no),
        ));
    }

    metrics.insert("readme_commands".to_owned(), as_i64(unknown_commands));
    metrics.insert("readme_paths".to_owned(), as_i64(missing_paths));
    metrics.insert("env_vars_read".to_owned(), as_i64(read_in_code.len()));
    metrics.insert("env_vars_undocumented".to_owned(), as_i64(undocumented));
    metrics.insert("occurrences".to_owned(), as_i64(findings.len()));
    if npm_scripts.is_empty() && !tree.has("package.json") {
        notes.push("no package.json, so npm/pnpm/yarn commands were not checked".to_owned());
    }
    if make_targets.is_empty() && !tree.has("Makefile") {
        notes.push("no Makefile, so make targets were not checked".to_owned());
    }

    let (findings, cut) = cap(findings);
    notes.extend(cut);
    StaticReport::reported(CheckId::S08, metrics)
        .with_findings(findings)
        .with_notes(notes)
}

/// The `script` name a package-manager command names, if any.
fn script_argument(argument: &str) -> Option<&str> {
    let mut parts = argument.split_whitespace();
    match parts.next() {
        Some("run") | Some("run-script") => parts.next(),
        Some(name) if !name.starts_with('-') => Some(name),
        _ => None,
    }
}

/// Whether `cargo <name>` is cargo's own subcommand rather than a binary.
fn is_cargo_subcommand(name: &str) -> bool {
    matches!(
        name,
        "add"
            | "bench"
            | "build"
            | "check"
            | "clean"
            | "clippy"
            | "doc"
            | "fetch"
            | "fix"
            | "fmt"
            | "generate-lockfile"
            | "help"
            | "init"
            | "install"
            | "locate-project"
            | "login"
            | "metadata"
            | "new"
            | "package"
            | "publish"
            | "remove"
            | "report"
            | "run"
            | "rustc"
            | "test"
            | "tree"
            | "uninstall"
            | "update"
            | "vendor"
            | "verify-project"
            | "version"
            | "yank"
    )
}

/// Whether a token looks like a path worth checking.
fn looks_like_path(word: &str) -> bool {
    let cleaned = word.trim_matches(|c: char| c == '`' || c == '"' || c == '\'');
    if cleaned.starts_with('-') || cleaned.contains("://") || cleaned.is_empty() {
        return false;
    }
    if cleaned.contains('/') {
        return !cleaned.starts_with('/');
    }
    matches!(
        cleaned.rsplit_once('.'),
        Some((
            _,
            "ts" | "tsx" | "js" | "jsx" | "py" | "rs" | "json" | "toml" | "yml" | "yaml"
        ))
    )
}

/// The lines of the README's shell blocks: `(1-based line number, the command)`.
///
/// A block may hold several commands; each is reported on its own line. Comment lines inside a
/// block (`# install`) are skipped: they document a sequence, they do not promise a command.
#[must_use]
pub fn shell_lines(readme: &str) -> Vec<(u32, String)> {
    let mut lines: Vec<(u32, String)> = Vec::new();
    let mut in_block = false;
    for (index, line) in readme.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(fence) = trimmed.strip_prefix("```") {
            let language = fence.trim().to_ascii_lowercase();
            if in_block {
                in_block = false;
            } else if SHELL_FENCES.contains(&language.as_str()) {
                in_block = true;
            }
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        // A prompt marker is not part of the command.
        let command = trimmed
            .strip_prefix("$ ")
            .or_else(|| trimmed.strip_prefix("> "))
            .unwrap_or(trimmed)
            .to_owned();
        lines.push((
            u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX),
            command,
        ));
    }
    lines
}

/// Splits a command line into `(kind, program, argument)` for the programs S08 knows.
///
/// `kind` is `invocation` for everything here; it exists so the match in the caller reads as a
/// decision about what kind of command it is, which is where the next program would slot in.
#[must_use]
pub fn parsed_commands(command: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for segment in command.split("&&").flat_map(|part| part.split('|')) {
        let mut words = segment.split_whitespace();
        let Some(program) = words.next() else {
            continue;
        };
        let program = program.rsplit('/').next().unwrap_or(program).to_owned();
        let rest: Vec<&str> = words.collect();
        let argument = match program.as_str() {
            "npm" | "pnpm" | "yarn" => strip_flags(&rest, &["--silent", "-s"]).join(" "),
            "make" | "just" => strip_flags(&rest, &["-C"]).join(" "),
            "cargo" => strip_flags(&rest, &["+nightly"])
                .first()
                .map(|word| (*word).to_owned())
                .unwrap_or_default(),
            _ => rest.join(" "),
        };
        out.push(("invocation".to_owned(), program, argument));
    }
    out
}

/// Drops leading flags from a word list.
fn strip_flags<'a>(words: &[&'a str], skip: &[&str]) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = Vec::new();
    let mut skipping = true;
    for word in words {
        if skipping && (word.starts_with('-') || skip.contains(word)) {
            continue;
        }
        skipping = false;
        out.push(word);
    }
    out
}

/// The script names `package.json` defines.
async fn npm_script_names(tree: &mut TrackedTree<'_>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let Some(text) = tree.text("package.json").await else {
        return names;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return names;
    };
    if let Some(serde_json::Value::Object(scripts)) = value.get("scripts") {
        for key in scripts.keys() {
            names.insert(key.clone());
        }
    }
    names
}

/// The targets a Makefile or justfile defines.
async fn make_target_names(tree: &mut TrackedTree<'_>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for file in ["Makefile", "makefile", "justfile", "Justfile"] {
        let Some(text) = tree.text(file).await else {
            continue;
        };
        for line in text.lines() {
            if line.starts_with('\t') || line.trim().is_empty() {
                continue;
            }
            if let Some((head, _)) = line.split_once(':') {
                let target = head.trim();
                if !target.is_empty()
                    && !target.starts_with('#')
                    && !target.contains('=')
                    && !target.contains(' ')
                {
                    names.insert(target.to_owned());
                }
            }
        }
    }
    names
}

/// The binary names a Rust project defines: `[[bin]]` names and `src/bin/*.rs` files.
async fn cargo_binary_names(tree: &mut TrackedTree<'_>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for path in tree.paths() {
        if let Some(rest) = path.strip_prefix("src/bin/")
            && let Some(stem) = rest.strip_suffix(".rs")
        {
            names.insert(stem.to_owned());
        }
        if let Some(rest) = path.strip_suffix("/src/bin/main.rs") {
            names.insert(TrackedTree::file_name(rest).to_owned());
        }
    }
    names
}

/// The environment variables the README and `.env.example` mention.
async fn documented_env(tree: &mut TrackedTree<'_>, readme: &str) -> BTreeSet<String> {
    let mut documented: BTreeSet<String> = env_references(readme).into_iter().collect();
    if let Some(text) = tree.text(".env.example").await {
        for name in env_references(&text) {
            documented.insert(name);
        }
    }
    documented
}

/// Every `NAME` mentioned the way an env file or a README mentions one.
fn env_references(text: &str) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // `.env.example` form and README form: `NAME=value`, `` `NAME` ``.
        if let Some((key, _)) = trimmed.split_once('=') {
            let key = key.trim().trim_start_matches("export ");
            if is_env_name(key) {
                names.insert(key.to_owned());
            }
        }
        for token in trimmed.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if is_env_name(token) && token.len() >= 4 {
                names.insert(token.to_owned());
            }
        }
    }
    names
}

/// Whether `name` is spelled the way an environment variable is.
fn is_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with('_')
}

/// The environment variables the code reads: `(name, path, 1-based line)`.
///
/// Sorted by name then path then line, so the report is stable.
async fn env_vars(tree: &mut TrackedTree<'_>) -> Vec<(String, String, u32)> {
    let paths: Vec<String> = tree.paths().into_iter().map(str::to_owned).collect();
    let mut out: Vec<(String, String, u32)> = Vec::new();
    for path in paths {
        if crate::lang::Lang::from_path(std::path::Path::new(&path)).is_none() {
            continue;
        }
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
            for name in line_env_names(line) {
                out.push((name, path.clone(), line_no));
            }
        }
    }
    out.sort();
    out.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
    out
}

/// The env names read on one line, in any of the five languages.
#[must_use]
pub fn line_env_names(line: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for marker in [
        "process.env.",
        "import.meta.env.",
        "os.environ[",
        "os.environ.get(",
        "os.getenv(",
        "std::env::var(",
        "env!(",
        "env::var(",
    ] {
        let mut from = 0;
        while let Some(found) = line[from..].find(marker) {
            let start = from + found + marker.len();
            let rest = &line[start..];
            let rest = rest.trim_start_matches(['"', '\'']);
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            if is_env_name(candidate) {
                names.push(candidate.to_owned());
            }
            from = start;
        }
    }
    names
}
