//! PyPI: the JSON API (M1-T04).
//!
//! One `GET https://pypi.org/pypi/{name}/json` returns the project's metadata and every
//! release it ever uploaded. P02 reads `project_urls` (a package with no source link is a
//! slopsquatting signal), P03 the upload dates, and P04 the `yanked` flags.
//!
//! Two shapes need care. `releases` is an object keyed by version whose values are lists of
//! files; a release with an empty list is a version that was created and deleted, and it has
//! no upload date. Each file carries `upload_time_iso_8601` (the modern field) and
//! `upload_time` (the old one, without a timezone); the ISO field is preferred and the list
//! is reduced to its earliest file, because that is when the version itself appeared.
//!
//! The endpoint shape was confirmed from PyPI's JSON API documentation and the Warehouse
//! response it describes: `info`, `releases`, and per-file `yanked`, `yanked_reason`,
//! `upload_time_iso_8601`.

use serde::Deserialize;
use substance_core::Timestamp;
use substance_core::deps::Ecosystem;

use super::support::{Documents, cap_versions, skipped_of};
use super::{Registry, RegistryError};
use crate::snapshot::SnapshotHttp;

/// The JSON API host.
pub const PYPI_API: &str = "https://pypi.org/pypi";

/// One uploaded release of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyPiRelease {
    /// The version string.
    pub version: String,
    /// When its first file was uploaded.
    pub uploaded_at: Option<Timestamp>,
    /// Whether every file of the release is yanked; a partly yanked release stays usable.
    pub yanked: bool,
    /// Why it was yanked, when PyPI recorded a reason.
    pub yanked_reason: Option<String>,
}

/// What the JSON API says about one package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PyPiPackage {
    /// The project name, as PyPI spells it.
    pub name: String,
    /// The newest version PyPI reports.
    pub latest: Option<String>,
    /// The source repository, from `project_urls` or `home_page`.
    pub repository: Option<String>,
    /// The license text or expression.
    pub license: Option<String>,
    /// The releases, newest first, capped at [`super::MAX_VERSIONS`].
    pub releases: Vec<PyPiRelease>,
}

impl PyPiPackage {
    /// The newest release that is not yanked.
    #[must_use]
    pub fn newest_yanked_free(&self) -> Option<&PyPiRelease> {
        self.releases.iter().find(|release| !release.yanked)
    }
}

/// The PyPI client.
#[derive(Debug)]
pub struct PyPI {
    http: SnapshotHttp,
    documents: Documents,
}

impl PyPI {
    /// A client over `http`.
    #[must_use]
    pub fn new(http: SnapshotHttp) -> Self {
        Self {
            http,
            documents: Documents::new(),
        }
    }

    /// The JSON URL for a package. PyPI normalizes names itself, so no encoding is added.
    #[must_use]
    pub fn package_url(name: &str) -> String {
        format!("{PYPI_API}/{name}/json")
    }

    /// Fetches `name`.
    ///
    /// # Errors
    ///
    /// See [`RegistryError`].
    pub async fn package(&mut self, name: &str) -> Result<PyPiPackage, RegistryError> {
        let url = Self::package_url(name);
        let text = self
            .documents
            .get(&mut self.http, Ecosystem::PyPi, name, &url)
            .await?;
        let wire: WireProject =
            serde_json::from_str(&text).map_err(|error| RegistryError::Schema {
                ecosystem: Ecosystem::PyPi,
                name: name.to_owned(),
                detail: error.to_string(),
            })?;
        let mut releases: Vec<PyPiRelease> = wire
            .releases
            .iter()
            .map(|(version, files)| {
                let uploaded_at = files
                    .iter()
                    .filter_map(|file| {
                        file.upload_time_iso_8601
                            .as_deref()
                            .and_then(|text| Timestamp::parse_rfc3339(text).ok())
                    })
                    .min();
                // A release is yanked only when every file is: a partly yanked release is
                // still installable, so it is not what P04 reports.
                let yanked =
                    !files.is_empty() && files.iter().all(|file| file.yanked.unwrap_or(false));
                PyPiRelease {
                    version: version.clone(),
                    uploaded_at,
                    yanked,
                    yanked_reason: files
                        .iter()
                        .find_map(|file| file.yanked_reason.clone())
                        .filter(|reason| !reason.trim().is_empty()),
                }
            })
            .collect();
        releases.sort_by(|left, right| {
            right
                .uploaded_at
                .cmp(&left.uploaded_at)
                .then_with(|| right.version.cmp(&left.version))
        });
        let releases = cap_versions(name, releases, &mut self.documents);
        let mut info = wire.info.unwrap_or_default();
        let repository = info.repository();
        Ok(PyPiPackage {
            name: info.name.take().unwrap_or_else(|| name.to_owned()),
            latest: info.version.take(),
            repository,
            license: info
                .license
                .take()
                .filter(|text| !text.trim().is_empty() && text.trim() != "UNKNOWN"),
            releases,
        })
    }

    /// The request budget spent so far.
    #[must_use]
    pub fn requests_used(&self) -> u32 {
        self.http.used()
    }
}

impl Registry for PyPI {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::PyPi
    }

    fn requests_used(&self) -> u32 {
        self.http.used()
    }

    fn skipped(&self) -> Vec<(String, String)> {
        skipped_of(&self.documents)
    }

    fn notes(&self) -> Vec<String> {
        self.documents.notes()
    }
}

#[derive(Debug, Deserialize)]
struct WireProject {
    info: Option<WireInfo>,
    #[serde(default)]
    releases: std::collections::BTreeMap<String, Vec<WireFile>>,
}

#[derive(Debug, Default, Deserialize)]
struct WireInfo {
    name: Option<String>,
    version: Option<String>,
    license: Option<String>,
    home_page: Option<String>,
    project_urls: Option<std::collections::BTreeMap<String, String>>,
}

impl WireInfo {
    /// The source repository, preferring a `Source`/`Repository`/`Code` entry in
    /// `project_urls` over `home_page`, which is often a documentation site.
    fn repository(&mut self) -> Option<String> {
        if let Some(urls) = self.project_urls.as_ref() {
            for (key, url) in urls {
                let lower = key.to_ascii_lowercase();
                if ["source", "repository", "code", "github", "homepage"]
                    .iter()
                    .any(|marker| lower.contains(marker))
                    && !url.trim().is_empty()
                {
                    return Some(url.trim().to_owned());
                }
            }
        }
        self.home_page
            .take()
            .filter(|url| !url.trim().is_empty())
            .map(|url| url.trim().to_owned())
    }
}

#[derive(Debug, Deserialize)]
struct WireFile {
    #[serde(default)]
    yanked: Option<bool>,
    #[serde(default)]
    yanked_reason: Option<String>,
    #[serde(default)]
    upload_time_iso_8601: Option<String>,
}
