//! H17 Template divergence: how much of this repository is still its template.
//!
//! Formula (action plan, H17):
//!
//! > When `template_repository` is set: permille of files whose blob SHA equals the
//! > template's (via the API's recursive tree). The match against S04's scaffold hashes is
//! > added in M1-T08.
//!
//! A template repository's files keep their blob SHAs when they are copied, because a blob
//! SHA is the hash of the content: a project that is 96% its template is a template with an
//! edit, whatever its README says. Comparing SHAs rather than paths makes the answer
//! indifferent to renames, and comparing only paths the template also has makes it
//! indifferent to files the project added.
//!
//! Two comparisons, because a project can be someone's template *and* a scaffolder's output:
//!
//! - **template_repository**, when the forge reports one: the SHAs of the files the template
//!   and the project share.
//! - **the scaffold table** (`rules/scaffolds.toml`, from M1-T08): the paths whose content
//!   matches a file an official scaffolder ships, at a recorded version. This answers the
//!   same question for the projects nobody marked as an instance of anything, which is most
//!   of them.
//!
//! Both are pure: the project's blobs come from [`HistoryInput::head_blobs`] and the scaffold
//! table is embedded in the binary, so this module reads nothing. When the table holds no
//! hashes — the normal state until the owner runs `scripts/scaffold-hashes.sh` — the part
//! reports `not_measured` and says why, which is [`ScaffoldPart::missing`].

use std::collections::{BTreeMap, BTreeSet};

use substance_core::model::Verdict;
use substance_core::scaffolds::ScaffoldFile;
use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// Judges H17: how much of this repository is still something someone else generated.
///
/// Two comparisons, and a report needs both because they cover disjoint populations: a
/// project made from a template has a `template_repository`, and a project made by running
/// `create-vite` usually has no template and no README that says so. The scaffold table
/// catches the second kind, so the template comparison wins when it can run and the scaffold
/// comparison answers otherwise, with both sets of metrics when both ran.
///
/// A comparison that cannot run never turns the other into a pass: its reason is a note, and
/// when neither can run the check is `not_measured` with both reasons. That is principle 8
/// applied to a check with two inputs.
#[must_use]
pub fn template_divergence(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let scaffold = scaffold_match(input);
    match template_comparison(input) {
        Ok(mut judgment) => {
            match scaffold {
                Ok(part) => {
                    for (key, value) in part.metrics {
                        judgment.metrics.insert(key, value);
                    }
                    for note in part.notes {
                        judgment = judgment.with_note(note);
                    }
                }
                Err(reason) => {
                    judgment = judgment
                        .with_note(format!("the scaffold comparison could not run: {reason}"));
                }
            }
            judgment
        }
        Err(template_reason) => match scaffold {
            Ok(part) => part.with_note(format!(
                "the template comparison could not run: {template_reason}"
            )),
            Err(scaffold_reason) => Judgment::not_measured(
                format!("neither comparison could run: {template_reason}; {scaffold_reason}"),
                BTreeMap::new(),
            ),
        },
    }
}

/// The template half: the SHAs the project shares with its `template_repository`.
///
/// # Errors
///
/// The reason the comparison cannot run: no forge data, no template, an unreadable template
/// tree, or no shared path to compare.
fn template_comparison(input: &HistoryInput) -> Result<Judgment, String> {
    let forge = input.forge_or_reason()?;
    let template = forge
        .repo
        .as_ref()
        .and_then(|repo| repo.template.clone())
        .ok_or_else(|| {
            "the repository was not made from a template, so there is no template to compare \
             against"
                .to_owned()
        })?;
    if let Some(reason) = forge.unread_reason("template tree") {
        return Err(format!("the template's tree could not be read: {reason}"));
    }
    let template_tree = forge.template_tree.as_ref().ok_or_else(|| {
        "the template's tree was not fetched, so nothing can be compared".to_owned()
    })?;

    // The project's own blobs at HEAD. The tree is read from git rather than from the API
    // because the checkout is already here and the API's tree of this repository would be a
    // second request for data substance has.
    let mut ours: BTreeMap<&str, &str> = BTreeMap::new();
    for (path, sha) in &input.head_blobs {
        ours.insert(path.as_str(), sha.as_str());
    }
    let template_paths: BTreeSet<&str> = template_tree
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    let mut shared = 0u64;
    let mut identical = 0u64;
    let mut changed: Vec<String> = Vec::new();
    for (path, sha) in &ours {
        if !template_paths.contains(path) {
            continue;
        }
        shared += 1;
        let same = template_tree
            .iter()
            .any(|entry| entry.path == *path && entry.sha == *sha);
        if same {
            identical += 1;
        } else if changed.len() < 20 {
            changed.push((*path).to_owned());
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "files".to_owned(),
        i64::try_from(ours.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "template_files".to_owned(),
        i64::try_from(template_tree.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "shared_files".to_owned(),
        i64::try_from(shared).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "identical_files".to_owned(),
        i64::try_from(identical).unwrap_or(i64::MAX),
    );
    let permille = super::shape::permille(identical, shared);
    if let Some(permille) = permille {
        metrics.insert(
            "template_match_permille".to_owned(),
            i64::try_from(permille).unwrap_or(i64::MAX),
        );
    }

    let mut judgment = Judgment::measured(metrics);
    if changed.len() < usize::try_from(shared).unwrap_or(usize::MAX) {
        judgment = judgment.with_note(format!(
            "the first {} changed files are listed; the count covers all of them",
            changed.len()
        ));
    }
    if !changed.is_empty() {
        judgment = judgment.with_note(format!(
            "changed since the template: {}",
            changed.join(", ")
        ));
    }
    if shared == 0 {
        return Err(
            "the project shares no path with its template, so there is no divergence to measure"
                .to_owned(),
        );
    }
    Ok(judgment.with_note(format!("the repository was made from {}", template.slug())))
}

/// The scaffold half: how many of the project's files are byte-identical to a file an official
/// scaffolder ships.
///
/// # Errors
///
/// The reason the comparison cannot run, which is also what the `scaffolds` part reports: no
/// table, no recorded hashes, or nothing in the table to compare against. A tree that matches
/// nothing is *not* an error — it is the answer "this is not anyone's scaffold".
fn scaffold_match(input: &HistoryInput) -> Result<Judgment, String> {
    let part = &input.scaffold;
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "scaffold_files".to_owned(),
        i64::try_from(part.recorded_files).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "matched_files".to_owned(),
        i64::try_from(part.matched_paths.len()).unwrap_or(i64::MAX),
    );
    if let Some(reason) = &part.missing {
        return Err(reason.clone());
    }
    let Some(permille) = part.permille() else {
        return Err("the scaffold table has no files to compare against".to_owned());
    };
    metrics.insert("scaffold_permille".to_owned(), i64::from(permille));
    let mut judgment = Judgment::measured(metrics);
    if part.matched_paths.is_empty() {
        return Ok(judgment.with_note(
            "no file matches a recorded scaffold hash, so this tree is not a scaffold's output",
        ));
    }
    if let Some(tool) = &part.tool {
        judgment = judgment.with_note(format!(
            "{permille} permille of the files {tool} ships are unchanged, so {} of them are \
             still that scaffold's",
            part.matched_paths.len()
        ));
    }
    Ok(judgment.with_note(format!(
        "files a scaffolder ships unchanged: {}",
        part.matched_paths.join(", ")
    )))
}

/// The `parts` key for the scaffold comparison: the half of H17 that needs the table.
pub const SCAFFOLD_PART: &str = "scaffolds";

/// The `scaffolds` part's verdict: measured, or `not_measured` with the reason it could not be.
///
/// One function so the reason a part is missing has one home: no table, no hashes recorded, or
/// no files to compare all read differently in a report.
#[must_use]
pub fn scaffold_part(part: &ScaffoldPart) -> (String, Verdict) {
    let verdict = match &part.missing {
        Some(reason) => Verdict::NotMeasured {
            reason: reason.clone(),
        },
        None => Verdict::Reported,
    };
    (SCAFFOLD_PART.to_owned(), verdict)
}

/// The scaffold comparison's inputs, filled in by [`HistoryInput::gather`].
///
/// The table is embedded and every read is done once, in `gather`, so H17 stays a pure
/// function of its input like every other signal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScaffoldPart {
    /// The hashes the table records, or `None` when the table holds none.
    ///
    /// `None` is not zero: a table with no hashes has not answered the question, while a
    /// table with hashes and no matches has answered "this is not a scaffold".
    pub recorded_files: usize,
    /// The project paths whose content matches a recorded hash, sorted.
    pub matched_paths: Vec<String>,
    /// The scaffolder with the most matches, when there are any.
    pub tool: Option<String>,
    /// Why the comparison could not run, when it could not.
    pub missing: Option<String>,
}

impl ScaffoldPart {
    /// A part that could not be measured, carrying the reason.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            missing: Some(reason.into()),
            ..Self::default()
        }
    }

    /// The match as a share of the recorded files, in permille, or `None` when nothing was
    /// recorded.
    #[must_use]
    pub fn permille(&self) -> Option<u32> {
        let matched = u64::try_from(self.matched_paths.len()).ok()?;
        let recorded = u64::try_from(self.recorded_files).ok()?;
        u32::try_from(super::shape::permille(matched, recorded)?).ok()
    }

    /// Counts the project's content hashes against the table's recorded ones.
    ///
    /// `ours` is `(path, sha256 of the content)` for every file at HEAD. Both sides are
    /// lowercase hex, and the comparison is on the digest alone: a scaffolder's file that a
    /// project moved to another path is still that scaffolder's file.
    #[must_use]
    pub fn compare(ours: &[(String, String)], table: &[ScaffoldFile]) -> Self {
        if table.is_empty() {
            return Self::unavailable(
                "rules/scaffolds.toml records no file hashes: the owner has not run scripts/scaffold-hashes.sh",
            );
        }
        let mut matched: Vec<(String, String)> = Vec::new();
        for (path, digest) in ours {
            let Some(file) = table
                .iter()
                .find(|file| file.sha256.eq_ignore_ascii_case(digest))
            else {
                continue;
            };
            matched.push((path.clone(), file.tool.clone()));
        }
        matched.sort();
        // The nearest scaffold: the tool with the most matched files. A tie breaks on the tool
        // name so the answer is stable, and the counts are accumulated in a sorted vec rather
        // than a map because the number of tools is three.
        let mut counts: Vec<(String, usize)> = Vec::new();
        for (_, tool) in &matched {
            match counts.iter_mut().find(|(name, _)| name == tool) {
                Some((_, count)) => *count += 1,
                None => counts.push((tool.clone(), 1)),
            }
        }
        counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        Self {
            recorded_files: table.len(),
            matched_paths: matched.into_iter().map(|(path, _)| path).collect(),
            tool: counts.first().map(|(name, _)| name.clone()),
            missing: None,
        }
    }
}
