//! H06 Upload dumps: commits with GitHub's browser-upload subject.
//!
//! Formula (action plan, H06):
//!
//! > Commits whose subject is exactly "Add files via upload": count and authored lines.
//!
//! When someone adds files through GitHub's web interface, the commit subject is the one
//! GitHub writes: `Add files via upload`. A project whose history is mostly those commits was
//! assembled by dragging a folder into a browser, not developed. The match is exact — an
//! agent that writes *about* uploading is not the same thing — and the count is weighted by
//! how many authored lines those commits left, so three one-line uploads rank below one
//! 4,000-line drop.
//!
//! The thresholds file has no key for H06; the number is context.

use std::collections::BTreeMap;

use substance_core::thresholds::Thresholds;

use super::HistoryInput;
use super::shape::Judgment;

/// The subject GitHub's uploader writes, exactly.
pub const UPLOAD_SUBJECT: &str = "Add files via upload";

/// Judges H06.
#[must_use]
pub fn upload_dumps(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    let mut uploads = 0u64;
    let mut lines = 0u64;
    let mut contributions: Vec<(String, u64)> = Vec::new();
    for commit in input.human_commits() {
        if commit.subject != UPLOAD_SUBJECT {
            continue;
        }
        uploads += 1;
        let added = input.authored_added(commit);
        lines += added;
        contributions.push((commit.sha.clone(), added.max(1)));
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "uploads".to_owned(),
        i64::try_from(uploads).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "uploaded_lines".to_owned(),
        i64::try_from(lines).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "commits".to_owned(),
        i64::try_from(input.human_commits().len()).unwrap_or(i64::MAX),
    );

    // H06 has no threshold: it is a description of how the files arrived.
    Judgment::measured(metrics).with_evidence(super::top_commits(contributions))
}
