//! crates.io: the crate metadata API (M1-T04).
//!
//! One `GET https://crates.io/api/v1/crates/{name}` returns the crate's downloads and
//! repository and every version it published, with `created_at` and `yanked`. P02 reads the
//! repository, P03 the creation dates, P04 the yanked flags and P07 the downloads.
//!
//! crates.io's crawler policy asks every client to identify itself. [`SnapshotHttp`] already
//! sends `substance/<version>` as the `User-Agent` on every request, which satisfies it; no
//! per-request header and no token is added here.
//!
//! The endpoint shape was confirmed from the crates.io API documentation: a `crate` object
//! with `repository` and `downloads`, and a `versions` array whose members carry `num`,
//! `created_at` and `yanked`.

use serde::Deserialize;
use substance_core::Timestamp;
use substance_core::deps::Ecosystem;

use super::support::{Documents, seen_versions, skipped_of};
use super::{Registry, RegistryError};
use crate::snapshot::SnapshotHttp;

/// The API host.
pub const CRATES_API: &str = "https://crates.io/api/v1/crates";

/// One published version of a crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateVersion {
    /// The version string.
    pub version: String,
    /// When it was published.
    pub created_at: Option<Timestamp>,
    /// Whether it was yanked.
    pub yanked: bool,
}

/// What the API says about one crate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Crate {
    /// The crate name.
    pub name: String,
    /// The newest version the API reports.
    pub newest: Option<String>,
    /// The source repository.
    pub repository: Option<String>,
    /// All-time downloads.
    pub downloads: u64,
    /// The versions, newest first. A long list is kept whole (M1-T10): P02 and P03 read
    /// every release, and a report shows the newest [`super::MAX_VERSIONS`].
    pub versions: Vec<CrateVersion>,
}

/// The crates.io client.
#[derive(Debug)]
pub struct CratesIo<'a> {
    http: &'a mut SnapshotHttp,
    documents: Documents,
}

impl<'a> CratesIo<'a> {
    /// A client over `http`.
    #[must_use]
    pub fn new(http: &'a mut SnapshotHttp) -> Self {
        Self {
            http,
            documents: Documents::new(),
        }
    }

    /// The URL for a crate.
    #[must_use]
    pub fn crate_url(name: &str) -> String {
        format!("{CRATES_API}/{name}")
    }

    /// Fetches `name`.
    ///
    /// # Errors
    ///
    /// See [`RegistryError`].
    pub async fn krate(&mut self, name: &str) -> Result<Crate, RegistryError> {
        let url = Self::crate_url(name);
        let text = self
            .documents
            .get(self.http, Ecosystem::Crates, name, &url)
            .await?;
        let wire: WireCrate =
            serde_json::from_str(&text).map_err(|error| RegistryError::Schema {
                ecosystem: Ecosystem::Crates,
                name: name.to_owned(),
                detail: error.to_string(),
            })?;
        let mut versions: Vec<CrateVersion> = wire
            .versions
            .into_iter()
            .filter_map(|version| {
                let num = version.num?;
                Some(CrateVersion {
                    version: num,
                    created_at: version
                        .created_at
                        .as_deref()
                        .and_then(|text| Timestamp::parse_rfc3339(text).ok()),
                    yanked: version.yanked.unwrap_or(false),
                })
            })
            .collect();
        versions.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.version.cmp(&left.version))
        });
        let versions = seen_versions(name, versions, &mut self.documents);
        let meta = wire.krate.unwrap_or_default();
        Ok(Crate {
            name: meta.name.unwrap_or_else(|| name.to_owned()),
            newest: meta.max_version,
            repository: meta
                .repository
                .filter(|url| !url.trim().is_empty())
                .map(|url| url.trim().to_owned()),
            downloads: meta.downloads.unwrap_or(0),
            versions,
        })
    }

    /// The request budget spent so far.
    #[must_use]
    pub fn requests_used(&self) -> u32 {
        self.http.used()
    }
}

impl Registry for CratesIo<'_> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Crates
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
struct WireCrate {
    #[serde(rename = "crate")]
    krate: Option<WireMeta>,
    #[serde(default)]
    versions: Vec<WireVersion>,
}

#[derive(Debug, Default, Deserialize)]
struct WireMeta {
    name: Option<String>,
    max_version: Option<String>,
    repository: Option<String>,
    downloads: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WireVersion {
    num: Option<String>,
    created_at: Option<String>,
    yanked: Option<bool>,
}
