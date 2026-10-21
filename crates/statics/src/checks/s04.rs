//! S04 Scaffold leftovers (M1-T08).
//!
//! Formula (action plan, S04):
//!
//! > `rules/scaffolds.toml`: sha256 of files from official templates (create-vite,
//! > create-next-app, uv init) at a recorded version, written by `scripts/scaffold-hashes.sh`.
//! > You write the script; the owner runs it (§8). Until then the file holds only the text
//! > markers, and the hash part is `not_measured` in `parts`. Also add H17's match against these
//! > hashes. Plus the catalog's text markers (`vite-project`, "Add your description here",
//! > `Vite + React + TS`, `[year] [fullname]`).
//!
//! Two parts, and the split is the honest bit: a project that has replaced the visible strings
//! of a template and kept every file is still a template, and only a hash match can say so.
//! While no hash has been recorded, this check reports what the text says and names the half it
//! could not run.
//!
//! A marker whose `contains` is empty means "the file's presence is the marker": the default
//! `public/vite.svg` is the example, since every copy of that asset is the scaffold's own.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::hash::{hex, sha256};
use substance_core::model::{Finding, Verdict};

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;
use substance_core::scaffolds::ScaffoldPack;

/// Judges S04's text half, and records what it could not do.
///
/// # Errors
///
/// Returns the parse error when the embedded `rules/scaffolds.toml` is malformed, which is a
/// build error rather than a property of the repository.
pub async fn scaffold_leftovers(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    pack: &ScaffoldPack,
) -> Result<StaticReport, String> {
    let mut findings: Vec<Finding> = Vec::new();
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();

    for marker in pack.markers() {
        let Some(text) = tree.text(&marker.path).await else {
            continue;
        };
        if !marker.contains.is_empty() && !text.contains(&marker.contains) {
            continue;
        }
        let title = if marker.contains.is_empty() {
            format!("{} ships {} by default", marker.tool, marker.path)
        } else {
            format!(
                "{} left {} in {}",
                marker.tool,
                describe(&marker.contains),
                marker.path
            )
        };
        findings.push(file_finding::at(
            run_id,
            CheckId::S04,
            &marker.sub_check,
            &title,
            Location::file(&marker.path),
        ));
    }
    metrics.insert("text_markers".to_owned(), as_i64(findings.len()));

    // The hash half: compare the recorded digests against the files at the commit.
    let hash_part = if pack.has_hashes() {
        let mut matched = 0_usize;
        for recorded in pack.files() {
            let Some(text) = tree.text(&recorded.path).await else {
                continue;
            };
            if hex(&sha256(text.as_bytes())) != recorded.sha256 {
                continue;
            }
            matched += 1;
            findings.push(file_finding::at(
                run_id,
                CheckId::S04,
                "scaffold-file-hash",
                &format!(
                    "{} of {} is byte-identical to the {} {} template",
                    recorded.path, recorded.tool, recorded.tool, recorded.version
                ),
                Location::file(&recorded.path),
            ));
        }
        metrics.insert("hash_matches".to_owned(), as_i64(matched));
        metrics.insert("hash_files_known".to_owned(), as_i64(pack.files().len()));
        Verdict::Reported
    } else {
        notes.push(
            "the hash half has not run: rules/scaffolds.toml records no file digests until the \
             owner runs scripts/scaffold-hashes.sh (§8), so only text markers are reported"
                .to_owned(),
        );
        Verdict::NotMeasured {
            reason: "scaffold hashes not recorded: run scripts/scaffold-hashes.sh".to_owned(),
        }
    };

    let (findings, cut) = cap(findings);
    notes.extend(cut);
    let mut report = StaticReport::reported(CheckId::S04, metrics)
        .with_findings(findings)
        .with_part("text", Verdict::Reported)
        .with_part("hash", hash_part)
        .with_notes(notes);
    // The overall verdict stays Reported: S04 has no threshold, and one part being
    // not_measured must not turn a measured text result into silence. The part map is where a
    // reader sees what is missing.
    if report.verdict.metrics.is_empty() {
        report = StaticReport::not_measured(CheckId::S04, "no scaffold markers to check");
    }
    Ok(report)
}

/// A marker's text, shortened for a title.
fn describe(contains: &str) -> String {
    let trimmed = contains.trim();
    if trimmed.chars().count() <= 40 {
        return format!("`{trimmed}`");
    }
    let cut: String = trimmed.chars().take(37).collect();
    format!("`{cut}…`")
}
