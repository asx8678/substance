//! deps.dev v3: how many packages depend on a package (M1-T04, H14).
//!
//! Two calls answer one question. The version listing
//! `GET https://api.deps.dev/v3/systems/{system}/packages/{name}` gives the published
//! versions, and the dependents call
//! `GET https://api.deps.dev/v3/systems/{system}/packages/{name}/versions/{version}:dependents`
//! gives a count. H14 uses the count for the version a project actually depends on, because
//! a package nobody else uses is what "real usage" is asking about.
//!
//! The systems are spelled `CARGO`, `NPM` and `PYPI` in the path, which
//! [`Ecosystem::deps_dev_system`] supplies. No credential is needed.
//!
//! The endpoint shapes were confirmed from deps.dev's v3 API documentation: `versions[]`
//! with `versionKey.version`, and a dependents response carrying `totalDependents` (a
//! string in the API, converted here).

use serde::Deserialize;
use substance_core::deps::Ecosystem;

use super::support::{Documents, skipped_of};
use super::{Registry, RegistryError};
use crate::snapshot::SnapshotHttp;

/// The API host.
pub const DEPS_DEV_API: &str = "https://api.deps.dev/v3";

/// What deps.dev reports about one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependents {
    /// The ecosystem asked about.
    pub ecosystem: Ecosystem,
    /// The package name.
    pub package: String,
    /// The version asked about.
    pub version: String,
    /// How many packages depend on it, direct and transitive.
    pub total: u64,
}

/// The deps.dev client.
#[derive(Debug)]
pub struct DepsDev {
    http: SnapshotHttp,
    documents: Documents,
}

impl DepsDev {
    /// A client over `http`.
    #[must_use]
    pub fn new(http: SnapshotHttp) -> Self {
        Self {
            http,
            documents: Documents::new(),
        }
    }

    /// The versions URL for a package.
    #[must_use]
    pub fn package_url(ecosystem: Ecosystem, name: &str) -> String {
        format!(
            "{DEPS_DEV_API}/systems/{}/packages/{name}",
            ecosystem.deps_dev_system()
        )
    }

    /// The dependents URL for one version.
    #[must_use]
    pub fn dependents_url(ecosystem: Ecosystem, name: &str, version: &str) -> String {
        format!(
            "{DEPS_DEV_API}/systems/{}/packages/{name}/versions/{version}:dependents",
            ecosystem.deps_dev_system()
        )
    }

    /// The versions deps.dev knows for a package, newest first.
    ///
    /// # Errors
    ///
    /// See [`RegistryError`].
    pub async fn versions(
        &mut self,
        ecosystem: Ecosystem,
        name: &str,
    ) -> Result<Vec<String>, RegistryError> {
        let url = Self::package_url(ecosystem, name);
        let text = self
            .documents
            .get(&mut self.http, ecosystem, name, &url)
            .await?;
        let wire: WirePackage =
            serde_json::from_str(&text).map_err(|error| RegistryError::Schema {
                ecosystem,
                name: name.to_owned(),
                detail: error.to_string(),
            })?;
        let mut versions: Vec<String> = wire
            .versions
            .into_iter()
            .filter_map(|entry| entry.version_key.and_then(|key| key.version))
            .collect();
        // Newest first is the API's order; re-sorting would need a semver comparison, which
        // this module does not own, so the API order is kept and documented.
        versions.dedup();
        Ok(versions)
    }

    /// How many packages depend on one version.
    ///
    /// # Errors
    ///
    /// See [`RegistryError`].
    pub async fn dependents(
        &mut self,
        ecosystem: Ecosystem,
        name: &str,
        version: &str,
    ) -> Result<Dependents, RegistryError> {
        let url = Self::dependents_url(ecosystem, name, version);
        let label = format!("{name}@{version}");
        let text = self
            .documents
            .get(&mut self.http, ecosystem, &label, &url)
            .await?;
        let wire: WireDependents =
            serde_json::from_str(&text).map_err(|error| RegistryError::Schema {
                ecosystem,
                name: name.to_owned(),
                detail: error.to_string(),
            })?;
        Ok(Dependents {
            ecosystem,
            package: name.to_owned(),
            version: version.to_owned(),
            total: wire
                .total_dependents
                .as_deref()
                .and_then(|text| text.parse::<u64>().ok())
                // The v3 response also carries a numeric field in some revisions.
                .or(wire.total_dependents_count)
                .unwrap_or(0),
        })
    }

    /// The request budget spent so far.
    #[must_use]
    pub fn requests_used(&self) -> u32 {
        self.http.used()
    }
}

impl Registry for DepsDev {
    fn ecosystem(&self) -> Ecosystem {
        // deps.dev answers for all three, so the trait's ecosystem is the one it was asked
        // about last; a report names the ecosystem on each record instead.
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

#[derive(Debug, Deserialize)]
struct WirePackage {
    #[serde(default)]
    versions: Vec<WireVersionEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireVersionEntry {
    version_key: Option<WireVersionKey>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireVersionKey {
    version: Option<String>,
}

/// The dependents response: `totalDependents` is a string in the documented v3 shape, and a
/// number in some responses, so both are accepted.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDependents {
    #[serde(default)]
    total_dependents: Option<String>,
    #[serde(default)]
    total_dependents_count: Option<u64>,
}
