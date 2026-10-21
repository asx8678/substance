//! Gathering what V01 and V03 read (M1-T11).
//!
//! The split every family in this crate keeps: IO happens here, once, and the judges in
//! [`super::v01`] and [`super::v03`] are pure functions over the result. Two of the inputs the
//! plan names are already gathered by other passes and are borrowed rather than fetched again:
//!
//! - the commits and the blame come from [`crate::signals::HistoryInput`], which the history
//!   pass already built for H01–H22. Re-running `git log` and `git blame` for V01 would double
//!   the cost of the most expensive part of triage for the same bytes.
//! - the dependency release dates come from [`crate::deps::known::Known`], which P03 already
//!   fetched. V03 needs one date per dependency, and it is the same document.
//!
//! What is new here is small: which instruction files exist at HEAD, and which API vintages the
//! tree still calls. The first is a name lookup over the authored tree; the second is a rule
//! scan, so it goes through the static engine's own [`scan`](substance_statics::scan) rather
//! than a second implementation of file walking.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::Timestamp;
use substance_core::deps::Ecosystem;
use substance_core::provenance::{DeclaredRules, VintageTable};

use crate::deps::known::Known;
use crate::provenance::v03::{KnowledgeInput, VintageHit};

/// One instruction file found at HEAD.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InstructionFileHit {
    /// The path, repo-relative.
    pub path: String,
    /// The tool the file names.
    pub tool: String,
}

/// The declared rules plus the instruction files V01 needs.
#[derive(Debug, Clone, Default)]
pub struct ProvenanceInput {
    /// The declared rules, embedded.
    pub rules: DeclaredRules,
    /// The instruction files present, sorted by path.
    pub instruction_files: Vec<InstructionFileHit>,
    /// Why the rules could not be loaded, when they could not. A build error, reported rather
    /// than crashing the run.
    pub rules_error: Option<String>,
}

/// Everything V03 reads.
#[derive(Debug, Clone, Default)]
pub struct V03Input {
    /// The two bounds' inputs.
    pub knowledge: KnowledgeInput,
    /// The vintage table, embedded.
    pub vintages: VintageTable,
    /// Why the table could not be loaded, when it could not.
    pub table_error: Option<String>,
}

/// Reads the embedded declared rules and finds the instruction files at HEAD.
///
/// `authored` is the history pass's authored file list, which is already sorted, so no second
/// tree walk is needed: an instruction file that is not authored code is not one a tool wrote.
#[must_use]
pub fn gather_declared(authored: &[String]) -> ProvenanceInput {
    let mut input = ProvenanceInput::default();
    match DeclaredRules::load() {
        Ok(rules) => {
            let present: BTreeMap<&str, &str> = authored
                .iter()
                .map(|path| (path.as_str(), path.as_str()))
                .collect();
            let mut found: BTreeSet<InstructionFileHit> = BTreeSet::new();
            for entry in &rules.instruction_files {
                for path in present.values() {
                    if entry.matches(path) {
                        found.insert(InstructionFileHit {
                            path: (*path).to_owned(),
                            tool: entry.tool.clone(),
                        });
                    }
                }
            }
            input.instruction_files = found.into_iter().collect();
            input.rules = rules;
        }
        Err(error) => input.rules_error = Some(error.to_string()),
    }
    input
}

/// Builds V03's input from the adoptions P03 already dated and the registry releases P03
/// already fetched.
///
/// `added_at` maps a dependency to the instant the project first declared it, which is the
/// same map P03 reads. The newest release **on or before** that date is the release the author
/// could have known: a version adopted in 2021 does not imply knowledge of a 2024 release. The
/// bound takes the maximum over dependencies, because one adopted release from 2023 is enough
/// to prove the author knew about 2023.
#[must_use]
pub fn knowledge_from_adoptions(
    added_at: &BTreeMap<(Ecosystem, String), Timestamp>,
    known: &Known,
) -> KnowledgeInput {
    let mut input = KnowledgeInput::default();
    let mut newest: Option<(Timestamp, String)> = None;
    let mut considered: Vec<String> = Vec::new();
    let mut undated = 0usize;
    for (key, adopted) in added_at {
        let (ecosystem, name) = key;
        let Some(package) = known.package(*ecosystem, name) else {
            undated += 1;
            continue;
        };
        match package.latest_by(*adopted) {
            Some(version) => {
                let Some(published) = version.published_at else {
                    undated += 1;
                    continue;
                };
                considered.push(format!("{}:{}", ecosystem.as_str(), name));
                let candidate = (published, format!("{}@{}", name, version.version));
                match &newest {
                    Some((at, _)) if *at >= published => {}
                    _ => newest = Some(candidate),
                }
            }
            None => undated += 1,
        }
    }
    considered.sort();
    considered.dedup();
    if undated > 0 {
        input.missing.push(format!(
            "{undated} dependencies had no dated release on or before the day they were \
             adopted, so they bounded nothing"
        ));
    }
    if newest.is_none() {
        input.missing.push(
            "no adopted dependency had a release date, so the lower bound is unknown".to_owned(),
        );
    }
    input.newest_adoption = newest;
    input.adoptions_considered = considered;
    input
}

/// Builds V03's input from the vintage matches the rule scan produced.
///
/// `matches` is the scan's own output, so a match carries the path and line the report needs
/// as evidence. A vintage the table does not date is dropped here and named by the judge: it
/// can bound nothing, and pretending it did would move the window.
#[must_use]
pub fn vintages_from_matches(
    matches: &[(String, u32, String)],
    vintages: &VintageTable,
) -> Vec<VintageHit> {
    let mut hits: Vec<VintageHit> = Vec::new();
    for (path, line, id) in matches {
        let Some(vintage) = vintages.get(id) else {
            continue;
        };
        hits.push(VintageHit {
            id: id.clone(),
            path: path.clone(),
            line: *line,
            current_to: vintage.current_to,
        });
    }
    hits.sort_by(|left, right| {
        left.current_to
            .cmp(&right.current_to)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
    hits
}

/// Loads the vintage table, reporting a malformed file rather than failing the run.
#[must_use]
pub fn load_vintages() -> V03Input {
    let mut input = V03Input::default();
    match VintageTable::load() {
        Ok(table) => input.vintages = table,
        Err(error) => input.table_error = Some(error.to_string()),
    }
    input
}

/// The instruction-file paths, for a panel that lists them.
#[must_use]
pub fn instruction_paths(hits: &[InstructionFileHit]) -> Vec<String> {
    let mut paths: Vec<String> = hits.iter().map(|hit| hit.path.clone()).collect();
    paths.sort();
    paths.dedup();
    paths
}
