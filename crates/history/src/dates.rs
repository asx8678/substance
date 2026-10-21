//! When each dependency was added: one history walk per manifest (M1-T04).
//!
//! P03 (born-stale versions) and P04 (deprecated on arrival) compare a dependency's
//! registry publish dates with the instant the project adopted it. That instant comes from
//! git: the first commit whose diff adds the name to a manifest.
//!
//! Three properties matter more than speed here:
//!
//! - **One walk per manifest, not one per dependency.** `git log -p -- <manifest>` reads the
//!   file's whole history once, and every direct dependency is matched against that one
//!   stream. A repository with 200 dependencies would otherwise start 200 git processes.
//! - **Bounded.** The walk passes `-n <budget>`, because a manifest rewritten daily for five
//!   years is 1,800 diffs while a dependency in today's lockfile cannot have been added
//!   longer ago than the walk's tail. The budget is a count, never a clock (§4.3,
//!   ADR-0009), and a walk that hits it is reported in [`DatesReport::truncated`].
//! - **Oldest first.** `git log` reads newest first, so the *last* commit in the stream that
//!   adds a name is its first appearance; the parser keeps overwriting the record as it
//!   reads, so no sort of the output is needed and no dated record can be mistaken for the
//!   newest one.
//!
//! Manifests are found by walking the tracked tree, not by guessing root paths: a Rust
//! workspace keeps its dependencies in `crates/*/Cargo.toml`, and substance is such a
//! workspace. Every manifest of a known kind is walked once; the walks are independent, so
//! they are sorted and then run one after another, each with its own budget.
//!
//! A dependency the walk never sees added — a manifest older than the clone's history, or a
//! name spelled differently in the lockfile — gets no record and is listed as unknown
//! rather than dated to the root commit.

use std::ffi::OsString;

use substance_core::Timestamp;
use substance_core::deps::Ecosystem;

use crate::git::{GitCommand, GitError, GitRepo, ParseFailure};

/// How many commits one manifest walk reads before it stops.
///
/// 2,000 diffs is far more than a manifest's real history and bounds the work a hostile
/// repository can cause. The project config's `dates_max_commits` replaces it; [`walks`]
/// takes the budget as an argument so the caller decides.
pub const DEFAULT_MAX_COMMITS: u32 = 2_000;

/// The manifest file names whose history is walked, and the ecosystem each speaks for.
///
/// Sorted by file name, so two runs walk them in one order. `requirements-dev.txt` and its
/// siblings are parsed for dependencies but not walked: a development requirement is not
/// what P03 and P04 judge.
const MANIFESTS: &[(&str, Ecosystem)] = &[
    ("Cargo.toml", Ecosystem::Crates),
    ("package.json", Ecosystem::Npm),
    ("pyproject.toml", Ecosystem::PyPi),
    ("requirements.txt", Ecosystem::PyPi),
];

/// When one direct dependency first appeared in a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyDate {
    /// Which registry the package comes from.
    pub ecosystem: Ecosystem,
    /// The package name, as the manifest spells it.
    pub name: String,
    /// The commit that first added it.
    pub commit_sha: String,
    /// That commit's author date.
    pub added_at: Timestamp,
    /// The manifest it was added to, repo-relative.
    pub manifest_path: String,
}

impl DependencyDate {
    /// The sort key: `(ecosystem, name)`, so a report lists one package's history once.
    #[must_use]
    pub fn sort_key(&self) -> (Ecosystem, &str) {
        (self.ecosystem, self.name.as_str())
    }
}

/// What the walks produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatesReport {
    /// The dates found, sorted by `(ecosystem, name, manifest path)`.
    pub dates: Vec<DependencyDate>,
    /// Manifests that were walked, sorted.
    pub walked: Vec<String>,
    /// Whether any walk stopped at the commit budget.
    pub truncated: bool,
    /// Names no walk saw added, sorted by `(ecosystem, name)`.
    pub unknown: Vec<(Ecosystem, String)>,
}

/// The manifest names this module knows, sorted.
#[must_use]
pub fn manifest_names() -> Vec<&'static str> {
    MANIFESTS.iter().map(|(name, _)| *name).collect()
}

/// The ecosystem a manifest name speaks for.
#[must_use]
pub fn ecosystem_of(manifest: &str) -> Option<Ecosystem> {
    MANIFESTS
        .iter()
        .find(|(name, _)| *name == manifest)
        .map(|(_, ecosystem)| *ecosystem)
}

/// Walks every manifest of a known kind once and dates each direct dependency.
///
/// `direct` is the names to look for, one entry per ecosystem; the same name may appear
/// more than once (a workspace with several crates depending on `serde`) and the first
/// addition wins, because that is when the project adopted it.
///
/// # Errors
///
/// Returns [`GitError`] when listing the tree or a walk fails. A repository with no
/// manifest of a kind simply has no walks for it.
pub async fn walks(
    repo: &GitRepo,
    rev: &str,
    direct: &[(Ecosystem, String)],
    max_commits: u32,
) -> Result<DatesReport, GitError> {
    let mut report = DatesReport::default();
    let mut wanted: Vec<(Ecosystem, String)> = direct.to_vec();
    wanted.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(report);
    }
    let budget = if max_commits == 0 {
        DEFAULT_MAX_COMMITS
    } else {
        max_commits
    };
    // One pass over the tracked tree finds every manifest; the list is sorted, so the walks
    // run in one order for one tree.
    let mut found: Vec<(String, Ecosystem)> = Vec::new();
    for path in repo.tree(rev).await? {
        let name = path.rsplit('/').next().unwrap_or(path.as_str());
        if let Some(ecosystem) = ecosystem_of(name) {
            found.push((path, ecosystem));
        }
    }
    found.sort();
    // A dependency may be declared in more than one manifest (a workspace). The earliest
    // addition is the one reported, so records are collected first and reduced once.
    let mut dated: Vec<DependencyDate> = Vec::new();
    for (path, ecosystem) in &found {
        let names: Vec<String> = wanted
            .iter()
            .filter(|(kind, _)| kind == ecosystem)
            .map(|(_, name)| name.clone())
            .collect();
        if names.is_empty() {
            continue;
        }
        let walk = repo.manifest_history(rev, path, budget).await?;
        report.walked.push(path.clone());
        if walk.len() >= budget as usize {
            report.truncated = true;
        }
        for name in &names {
            if let Some((sha, at)) = first_added(&walk, name, *ecosystem) {
                dated.push(DependencyDate {
                    ecosystem: *ecosystem,
                    name: name.clone(),
                    commit_sha: sha,
                    added_at: at,
                    manifest_path: path.clone(),
                });
            }
        }
    }
    // One record per (ecosystem, name): the earliest addition, then the manifest that has
    // it, so the reduction is total even when two manifests add a name in one commit.
    dated.sort_by(|left, right| {
        left.sort_key()
            .cmp(&right.sort_key())
            .then_with(|| left.added_at.cmp(&right.added_at))
            .then_with(|| left.manifest_path.cmp(&right.manifest_path))
    });
    dated.dedup_by(|left, right| left.sort_key() == right.sort_key());
    report.unknown = wanted
        .iter()
        .filter(|(ecosystem, name)| {
            !dated
                .iter()
                .any(|date| date.ecosystem == *ecosystem && date.name == *name)
        })
        .cloned()
        .collect();
    report.dates = dated;
    report.walked.sort();
    report.walked.dedup();
    Ok(report)
}

/// One commit of a manifest's history: its identity and the lines it added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestCommit {
    /// The commit SHA.
    pub sha: String,
    /// Its author date.
    pub at: Timestamp,
    /// The lines it added to the manifest, without the leading `+`.
    pub added: Vec<String>,
}

impl GitRepo {
    /// Reads `path`'s history at `rev`, newest first, at most `max_commits` commits.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when git fails or prints a header this parser cannot read.
    pub async fn manifest_history(
        &self,
        rev: &str,
        path: &str,
        max_commits: u32,
    ) -> Result<Vec<ManifestCommit>, GitError> {
        let args = vec![
            OsString::from("log"),
            OsString::from("-p"),
            OsString::from("-U0"),
            OsString::from("--no-color"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-merges"),
            OsString::from("--use-mailmap"),
            OsString::from(format!("-n{max_commits}")),
            OsString::from("--format=%x1e%H%x1f%aI"),
            OsString::from(rev),
            OsString::from("--"),
            OsString::from(path),
        ];
        let stdout = self.run(GitCommand::Log, &args).await?;
        parse_manifest_log(&stdout).map_err(|failure| self.parse_error(GitCommand::Log, failure))
    }
}

/// Parses `git log -p -U0 --format=%x1e%H%x1f%aI` output.
///
/// The record separator `\x1e` opens every commit, and the field separator `\x1f` splits
/// the SHA from the author date. Added lines are taken from the `+` lines of each diff,
/// skipping the `+++` file header and every other line type.
///
/// # Errors
///
/// Returns a [`ParseFailure`] naming the line and what was expected.
fn parse_manifest_log(stdout: &str) -> Result<Vec<ManifestCommit>, ParseFailure> {
    let mut commits: Vec<ManifestCommit> = Vec::new();
    for (index, raw) in stdout.lines().enumerate() {
        let number = index + 1;
        if let Some(header) = raw.strip_prefix('\u{1e}') {
            let (sha, date) = header.split_once('\u{1f}').ok_or_else(|| {
                ParseFailure::new(number, "a commit header without its field separator")
            })?;
            let at = Timestamp::parse_rfc3339(date.trim()).map_err(|error| {
                ParseFailure::new(number, format!("an unreadable author date: {error}"))
            })?;
            commits.push(ManifestCommit {
                sha: sha.trim().to_owned(),
                at,
                added: Vec::new(),
            });
            continue;
        }
        let Some(commit) = commits.last_mut() else {
            continue;
        };
        if raw.starts_with("+++") || raw.starts_with("---") || raw.starts_with("@@") {
            continue;
        }
        if let Some(text) = raw.strip_prefix('+') {
            commit.added.push(text.to_owned());
        }
    }
    Ok(commits)
}

/// The oldest commit in `walk` that adds `name`, or `None`.
///
/// `walk` is newest first, so the answer is the last match: the loop keeps replacing the
/// record as it reads older commits.
fn first_added(
    walk: &[ManifestCommit],
    name: &str,
    ecosystem: Ecosystem,
) -> Option<(String, Timestamp)> {
    let mut found: Option<(String, Timestamp)> = None;
    for commit in walk {
        if commit
            .added
            .iter()
            .any(|line| declares(line, name, ecosystem))
        {
            found = Some((commit.sha.clone(), commit.at));
        }
    }
    found
}

/// Whether one added manifest line declares a dependency called `name`.
///
/// Each manifest spells a dependency differently, so the test is:
///
/// - `Cargo.toml`: `name = ...`, `name.workspace = true`, or a `[dependencies.name]`
///   section header.
/// - `package.json`: `"name": ...` inside a dependency block. The line test alone cannot
///   see the block, so a quoted key anywhere counts; a script named like a dependency would
///   be a false positive, which is why the caller matches only names the lockfile reports.
/// - `pyproject.toml`: `name = ...` or `name[extra] = ...`.
/// - `requirements.txt`: a requirement line naming it, with any specifier.
#[must_use]
pub fn declares(line: &str, name: &str, ecosystem: Ecosystem) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    match ecosystem {
        Ecosystem::Crates => {
            // `[dependencies.serde]`, `[dev-dependencies.serde]`,
            // `[workspace.dependencies.serde]`. Only a line that *starts* with `[` is a
            // section header: `serde = { features = ["x"] }` is a dependency line that
            // happens to contain a bracket.
            if let Some(header) = trimmed
                .strip_prefix('[')
                .and_then(|rest| rest.split_once(']'))
            {
                return header
                    .0
                    .split('.')
                    .next_back()
                    .is_some_and(|last| last.trim() == name);
            }
            let key = trimmed.split(['=', '.']).next().unwrap_or_default().trim();
            key == name
        }
        Ecosystem::Npm => {
            // `"name": "^1.0.0"` or `"name":` at the start of a nested object.
            let key = trimmed.trim_start_matches('"');
            match key.split_once('"') {
                Some((key, rest)) => key == name && rest.trim_start().starts_with(':'),
                None => false,
            }
        }
        Ecosystem::PyPi => {
            // `name = "^1.0"`, `name[extra] == 1.0`, or `name==1.0` in requirements.
            let head = trimmed
                .split(['=', '<', '>', '!', '~', '[', ' '])
                .next()
                .unwrap_or_default()
                .trim();
            head.eq_ignore_ascii_case(name)
        }
    }
}
