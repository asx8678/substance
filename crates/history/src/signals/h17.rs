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
//! **The scaffold half is not built here.** M1-T08 adds the comparison against the hashes in
//! `rules/scaffolds.toml`, which needs the script M1-T08 also writes and the owner runs. Until
//! then this check answers a `not_measured` part named `scaffolds`, and its overall verdict is
//! that part's: a template-less repository is not a pass, it is a question this build cannot
//! ask. The hook is [`SCAFFOLD_PART`], and it is one `parts` entry rather than a second check.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::model::Verdict;
use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// The `parts` key for the scaffold-hash comparison M1-T08 adds.
pub const SCAFFOLD_PART: &str = "scaffolds";

/// Judges H17.
#[must_use]
pub fn template_divergence(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let forge = match input.forge_or_reason() {
        Ok(forge) => forge,
        Err(reason) => return Judgment::not_measured(reason, BTreeMap::new()),
    };
    let template = forge.repo.as_ref().and_then(|repo| repo.template.clone());
    let Some(template) = template else {
        return Judgment::not_measured(
            "the repository was not made from a template, so there is no template to compare \
             against",
            BTreeMap::new(),
        );
    };
    if let Some(reason) = forge.unread_reason("template tree") {
        return Judgment::not_measured(
            format!("the template's tree could not be read: {reason}"),
            BTreeMap::new(),
        );
    }
    let Some(template_tree) = forge.template_tree.as_ref() else {
        return Judgment::not_measured(
            "the template's tree was not fetched, so nothing can be compared",
            BTreeMap::new(),
        );
    };

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
            i64::try_from(permille).unwrap_or(1000),
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
    // The scaffold half, absent until M1-T08. Naming it keeps the check's verdict honest: what
    // is reported here is the template comparison only.
    judgment = judgment.with_note(format!(
        "the `{SCAFFOLD_PART}` part is not built yet (M1-T08), so only the template comparison ran"
    ));
    if shared == 0 {
        return Judgment::not_measured(
            "the project shares no path with its template, so there is no divergence to measure",
            judgment.metrics,
        );
    }
    judgment.with_note(format!("the repository was made from {}", template.slug()))
}

/// What the check would report if the scaffold half were the only input.
///
/// M1-T08 replaces this with the comparison against `rules/scaffolds.toml`'s hashes. It is a
/// named function rather than an inline `not_measured` so the part's reason has one home.
#[must_use]
pub fn scaffold_part() -> (String, Verdict) {
    (
        SCAFFOLD_PART.to_owned(),
        Verdict::NotMeasured {
            reason: "the scaffold hashes arrive in M1-T08".to_owned(),
        },
    )
}
