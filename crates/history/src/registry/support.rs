//! What the four registry clients share: one fetch, one cap, one note list (M1-T04).
//!
//! The clients differ only in the documents they parse, so the request plumbing, the status
//! mapping and the bookkeeping live here once. A client keeps a [`Documents`] beside its
//! [`SnapshotHttp`] and calls [`Documents::get`]/[`Documents::get_bounded`].

use substance_core::deps::Ecosystem;

use super::{MAX_VERSIONS, RegistryError};
use crate::snapshot::{SnapshotError, SnapshotHttp, SnapshotKind, SnapshotRequest};

/// The bookkeeping every registry client needs.
#[derive(Debug, Default)]
pub struct Documents {
    /// Documents a replay could not answer, sorted by label.
    pub(crate) skipped: Vec<(String, String)>,
    /// Caps a sample hit, sorted.
    pub(crate) notes: Vec<String>,
}

impl Documents {
    /// A fresh, empty record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetches one URL and returns its body text.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::NotFound`] for a 404, [`RegistryError::Refused`] for any
    /// other non-success status, [`RegistryError::Budget`] when the request budget is spent,
    /// and [`RegistryError::Snapshot`] when a replay lacks the document.
    pub async fn get(
        &mut self,
        http: &mut SnapshotHttp,
        ecosystem: Ecosystem,
        name: &str,
        url: &str,
    ) -> Result<String, RegistryError> {
        let request = SnapshotRequest::get(SnapshotKind::Registry, url);
        match http.fetch(&request).await {
            Ok(response) if response.is_success() => Ok(response.body.as_text()),
            Ok(response) => {
                let status = response.status;
                if status == 404 || status == 410 {
                    self.note_skip(name, "the registry has no such package");
                    Err(RegistryError::NotFound {
                        ecosystem,
                        name: name.to_owned(),
                    })
                } else {
                    self.note_skip(name, "the registry refused the request");
                    Err(RegistryError::Refused {
                        ecosystem,
                        name: name.to_owned(),
                        status,
                    })
                }
            }
            Err(SnapshotError::BudgetExhausted { used, .. }) => Err(RegistryError::Budget { used }),
            Err(SnapshotError::Missing { url, .. }) => {
                self.note_skip(name, "not in the snapshot");
                Err(RegistryError::Snapshot { url })
            }
            Err(other) => {
                let url = url.to_owned();
                self.note_skip(name, "the snapshot could not be read");
                let _ = other;
                Err(RegistryError::Snapshot { url })
            }
        }
    }

    /// Records that `label` could not be read, keeping one entry per label.
    pub fn note_skip(&mut self, label: &str, reason: &str) {
        if self.skipped.iter().any(|(seen, _)| seen == label) {
            return;
        }
        self.skipped.push((label.to_owned(), reason.to_owned()));
        self.skipped.sort_by(|left, right| left.0.cmp(&right.0));
    }

    /// Records that a version list stopped at [`MAX_VERSIONS`].
    pub fn note_version_cap(&mut self, package: &str, kept: usize, total: usize) {
        let note =
            format!("{package}: kept the newest {kept} of {total} versions (cap {MAX_VERSIONS})");
        self.note(note);
    }

    /// Records a note, keeping one entry per sentence.
    pub fn note(&mut self, note: String) {
        if self.notes.iter().any(|seen| seen == &note) {
            return;
        }
        self.notes.push(note);
        self.notes.sort();
    }

    /// The skipped list, sorted.
    #[must_use]
    pub fn skipped(&self) -> Vec<(String, String)> {
        self.skipped.clone()
    }

    /// The notes, sorted.
    #[must_use]
    pub fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }
}

/// Keeps the newest `MAX_VERSIONS` entries of a newest-first list, recording the cap.
///
/// The caller must pass the list newest first; each client sorts by publish date before
/// calling this, because npm's `versions` object, PyPI's `releases` map and crates.io's
/// `versions` array all have their own order.
#[must_use]
pub fn cap_versions<T>(package: &str, mut versions: Vec<T>, documents: &mut Documents) -> Vec<T> {
    let total = versions.len();
    if total <= MAX_VERSIONS {
        return versions;
    }
    versions.truncate(MAX_VERSIONS);
    documents.note_version_cap(package, versions.len(), total);
    versions
}

/// A client's shared bookkeeping behind the [`Registry`] trait.
#[must_use]
pub fn skipped_of(documents: &Documents) -> Vec<(String, String)> {
    documents.skipped()
}
