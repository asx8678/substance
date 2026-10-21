//! npm: the packument and the downloads API (M1-T04).
//!
//! One `GET https://registry.npmjs.org/{name}` returns the packument: every version the
//! package ever published, the `time` map that dates each of them, a `deprecated` string on
//! the versions npm marked deprecated, and the `repository` the package points at. P02 reads
//! the repository and the first publish date, P03 the publish dates, and P04 the deprecated
//! flags.
//!
//! The downloads API is a second host (`api.npmjs.org`) and a second shape: one point query
//! over a date range returns a single integer. The range is `as_of` minus 30 days to `as_of`
//! (see [`super::download_window_start`]), so a replay asks the same question.
//!
//! A scoped package name (`@scope/name`) is percent-encoded in the URL path, which is what
//! the registry expects; the `/` becomes `%2F`.

use serde::Deserialize;
use substance_core::Timestamp;
use substance_core::deps::Ecosystem;

use super::support::{Documents, cap_versions, skipped_of};
use super::{Registry, RegistryError};
use crate::snapshot::SnapshotHttp;

/// The packument host.
pub const NPM_API: &str = "https://registry.npmjs.org";

/// The downloads host.
pub const NPM_DOWNLOADS_API: &str = "https://api.npmjs.org";

/// One published version of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmVersion {
    /// The version string, as published.
    pub version: String,
    /// When it was published, from the packument's `time` map.
    pub published_at: Option<Timestamp>,
    /// npm's deprecation message, when the version is deprecated.
    pub deprecated: Option<String>,
}

/// What the packument says about one package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NpmPackage {
    /// The package name, as the registry spells it.
    pub name: String,
    /// The newest version, from `dist-tags.latest`.
    pub latest: Option<String>,
    /// When the package was first published.
    pub created_at: Option<Timestamp>,
    /// When it was last published.
    pub modified_at: Option<Timestamp>,
    /// The source repository URL, when the package declares one.
    pub repository: Option<String>,
    /// The versions, newest first, capped at [`super::MAX_VERSIONS`].
    pub versions: Vec<NpmVersion>,
    /// The license, as the package declares it.
    pub license: Option<String>,
}

impl NpmPackage {
    /// The newest version that is not deprecated, when there is one.
    #[must_use]
    pub fn newest_deprecated_free(&self) -> Option<&NpmVersion> {
        self.versions
            .iter()
            .find(|version| version.deprecated.is_none())
    }

    /// Whether every version in the sample is deprecated.
    #[must_use]
    pub fn all_deprecated(&self) -> bool {
        !self.versions.is_empty()
            && self
                .versions
                .iter()
                .all(|version| version.deprecated.is_some())
    }
}

/// The npm client.
#[derive(Debug)]
pub struct Npm {
    http: SnapshotHttp,
    documents: Documents,
    as_of: Timestamp,
}

impl Npm {
    /// A client over `http`, whose downloads window ends at `as_of`.
    #[must_use]
    pub fn new(http: SnapshotHttp, as_of: Timestamp) -> Self {
        Self {
            http,
            documents: Documents::new(),
            as_of,
        }
    }

    /// The packument URL for a package, with a scope's slash encoded.
    #[must_use]
    pub fn packument_url(name: &str) -> String {
        format!("{NPM_API}/{}", encode_name(name))
    }

    /// The downloads URL for the last [`super::DOWNLOAD_WINDOW_DAYS`] days before `as_of`.
    #[must_use]
    pub fn downloads_url(name: &str, as_of: Timestamp) -> String {
        let start = super::download_window_start(as_of);
        let end = super::download_window_end(as_of);
        format!(
            "{NPM_DOWNLOADS_API}/downloads/point/{start}:{end}/{}",
            encode_name(name)
        )
    }

    /// Fetches the packument for `name`.
    ///
    /// # Errors
    ///
    /// See [`RegistryError`]. A 404 is [`RegistryError::NotFound`], which is itself the
    /// answer P02 wants.
    pub async fn package(&mut self, name: &str) -> Result<NpmPackage, RegistryError> {
        let url = Self::packument_url(name);
        let text = self
            .documents
            .get(&mut self.http, Ecosystem::Npm, name, &url)
            .await?;
        let wire: WirePackument =
            serde_json::from_str(&text).map_err(|error| RegistryError::Schema {
                ecosystem: Ecosystem::Npm,
                name: name.to_owned(),
                detail: error.to_string(),
            })?;
        let mut versions: Vec<NpmVersion> = wire
            .versions
            .iter()
            .map(|(version, entry)| NpmVersion {
                version: version.clone(),
                published_at: wire
                    .time
                    .get(version)
                    .and_then(|text| Timestamp::parse_rfc3339(text).ok()),
                deprecated: entry
                    .deprecated
                    .clone()
                    .filter(|message| !message.trim().is_empty()),
            })
            .collect();
        // Newest first by publish date, then by version string, so a cap drops the oldest.
        versions.sort_by(|left, right| {
            right
                .published_at
                .cmp(&left.published_at)
                .then_with(|| right.version.cmp(&left.version))
        });
        let versions = cap_versions(name, versions, &mut self.documents);
        Ok(NpmPackage {
            name: wire.name.unwrap_or_else(|| name.to_owned()),
            latest: wire.dist_tags.and_then(|tags| tags.latest),
            created_at: wire
                .time
                .get("created")
                .and_then(|text| Timestamp::parse_rfc3339(text).ok()),
            modified_at: wire
                .time
                .get("modified")
                .and_then(|text| Timestamp::parse_rfc3339(text).ok()),
            repository: wire.repository.and_then(WireRepository::url),
            versions,
            license: wire.license.and_then(WireLicense::text),
        })
    }

    /// Fetches the download count for the last 30 days before `as_of`.
    ///
    /// # Errors
    ///
    /// See [`RegistryError`].
    pub async fn downloads(&mut self, name: &str) -> Result<u64, RegistryError> {
        let url = Self::downloads_url(name, self.as_of);
        let text = self
            .documents
            .get(&mut self.http, Ecosystem::Npm, name, &url)
            .await?;
        let wire: WireDownloads =
            serde_json::from_str(&text).map_err(|error| RegistryError::Schema {
                ecosystem: Ecosystem::Npm,
                name: name.to_owned(),
                detail: error.to_string(),
            })?;
        Ok(wire.downloads.unwrap_or(0))
    }

    /// The request budget spent so far.
    #[must_use]
    pub fn requests_used(&self) -> u32 {
        self.http.used()
    }
}

impl Registry for Npm {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
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

/// `@scope/name` becomes `@scope%2Fname`, which is the path form the registry serves.
fn encode_name(name: &str) -> String {
    name.replace('/', "%2F")
}

// The packument is a large document; only these members are read, and serde ignores the
// rest. `versions` is an object keyed by version string, and `time` is an object of ISO
// dates keyed by version plus `created` and `modified`.
#[derive(Debug, Deserialize)]
struct WirePackument {
    name: Option<String>,
    #[serde(default)]
    versions: std::collections::BTreeMap<String, WireVersion>,
    #[serde(default)]
    time: std::collections::BTreeMap<String, String>,
    #[serde(rename = "dist-tags")]
    dist_tags: Option<WireDistTags>,
    repository: Option<WireRepository>,
    license: Option<WireLicense>,
}

#[derive(Debug, Deserialize)]
struct WireVersion {
    deprecated: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireDistTags {
    latest: Option<String>,
}

/// `repository` is either a string or `{ "type": "git", "url": "..." }`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireRepository {
    Text(String),
    Object { url: Option<String> },
}

impl WireRepository {
    fn url(self) -> Option<String> {
        let text = match self {
            Self::Text(text) => text,
            Self::Object { url } => url?,
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

/// `license` is either a string or `{ "type": "MIT" }`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireLicense {
    Text(String),
    Object { r#type: Option<String> },
}

impl WireLicense {
    fn text(self) -> Option<String> {
        match self {
            Self::Text(text) | Self::Object { r#type: Some(text) } => Some(text),
            Self::Object { r#type: None } => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireDownloads {
    downloads: Option<u64>,
}
