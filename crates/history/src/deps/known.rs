//! What the registries say about the project's packages (M1-T10).
//!
//! P02–P04 and P06 ask about packages, not about code: whether a package exists, when it was
//! first published, whether the version in use is deprecated or yanked, what licence it
//! carries. Those answers come from the four clients in [`crate::registry`], one request per
//! package, gathered once here so every judge reads a snapshot instead of fetching.
//!
//! ## The version cap, and why nothing here is capped
//!
//! The registry clients hand back at most [`MAX_VERSIONS`] versions per package, newest first:
//! a report cannot carry every release of a package with 2,000 of them, and the payload is
//! mostly noise. But P02 asks about the version list's *size* and P03 asks what the latest
//! release was on the day a dependency was added — a question about the whole list, including
//! releases older than the newest 500. Reading a capped list would answer both with a number
//! that looks measured and is not.
//!
//! So [`KnownPackage`] keeps two things apart: [`KnownPackage::total_versions`] and
//! [`KnownPackage::versions`] are the counts and dates P02/P03 need (no version strings are
//! stored beyond the ones a check names), while the per-version detail a check reports is
//! looked up from the capped list. In practice that means:
//!
//! - the totals and the newest/oldest publish dates come from the uncapped counts the wire
//!   documents carry;
//! - a lookup of one version (P04's `deprecated`, P02's publish date for the version in use)
//!   is a scan of the version list the client returned, which holds the newest 500 — and the
//!   version a project uses is, by construction, one near the end of the list, not the
//!   beginning.
//!
//! The remaining gap is stated rather than hidden: when a document was capped,
//! [`Known::notes`] carries the client's own sentence saying so, and the report shows it.

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::deps::Ecosystem;

use crate::deps::version::VersionCore;
use crate::deps::world::World;
use crate::registry::{
    Crate, CratesIo, Npm, NpmPackage, PyPI, PyPiPackage, Registry, RegistryError,
};
use crate::snapshot::SnapshotHttp;

/// One version a check asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownVersion {
    /// The version string.
    pub version: String,
    /// When it was published, when the registry dates its releases.
    pub published_at: Option<Timestamp>,
    /// The deprecation message, when the registry marks it deprecated (npm only).
    pub deprecated: Option<String>,
    /// Whether the registry yanked it (PyPI, crates.io).
    pub yanked: bool,
}

/// Everything the registries said about one package.
///
/// Not `Default`: a package always comes from one registry, and a default would be one no
/// check could place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPackage {
    /// The name as the registry spells it.
    pub name: String,
    /// The registry that answered.
    pub ecosystem: Ecosystem,
    /// When the first version was published.
    pub first_published: Option<Timestamp>,
    /// When the newest version was published.
    pub last_published: Option<Timestamp>,
    /// How many versions exist in total, capped or not.
    pub total_versions: usize,
    /// The newest version the registry reports.
    pub latest: Option<String>,
    /// The source repository the package declares, when it declares one.
    pub repository: Option<String>,
    /// The licence the package declares, when it declares one.
    pub license: Option<String>,
    /// Downloads in the last 30 days (npm) or all-time (crates.io), when the registry
    /// reports a count.
    pub downloads: Option<u64>,
    /// The versions the client kept, newest first.
    pub versions: Vec<KnownVersion>,
    /// Whether the client's version list was capped.
    pub capped: bool,
}

impl KnownPackage {
    /// The version a project uses, when the client kept it.
    #[must_use]
    pub fn version(&self, version: &str) -> Option<&KnownVersion> {
        self.versions
            .iter()
            .find(|known| matches_request(version, &known.version))
    }

    /// The newest release published on or before `instant`.
    ///
    /// P03 compares the version a project adopted against the newest release that existed on
    /// the day it adopted it — not against today's latest, which would ask the project to have
    /// known the future. `None` when no release predates `instant`, which is itself an answer:
    /// the project adopted the package before anything was published, so there was nothing to
    /// be behind.
    #[must_use]
    pub fn latest_by(&self, instant: Timestamp) -> Option<&KnownVersion> {
        self.versions
            .iter()
            .filter(|known| known.published_at.is_some_and(|at| at <= instant))
            .max_by(|left, right| {
                left.published_at
                    .cmp(&right.published_at)
                    .then_with(|| version_order(&left.version, &right.version))
            })
    }

    /// The major numbers the newest release on or before `instant` reached.
    #[must_use]
    pub fn latest_major_by(&self, instant: Timestamp) -> Option<u64> {
        self.latest_by(instant)
            .and_then(|known| VersionCore::major_of(&known.version))
    }
}

/// Whether a lockfile version string names the registry's version.
///
/// Lockfiles spell versions with a leading `=` (PEP 440 in `requirements.txt`, pnpm's
/// `1.2.3(peer)`) and the registry does not, so the comparison is on the parsed core rather
/// than on the string. A string that does not parse falls back to an exact match, which is
/// what a git URL or a tag needs.
#[must_use]
pub fn matches_request(requested: &str, published: &str) -> bool {
    if requested == published {
        return true;
    }
    match (VersionCore::parse(requested), VersionCore::parse(published)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Orders two version strings by their parsed core, falling back to the string.
fn version_order(left: &str, right: &str) -> std::cmp::Ordering {
    match (VersionCore::parse(left), VersionCore::parse(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// Why a package could not be looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownError {
    /// The registry has no such package.
    NotFound,
    /// The registry refused, the budget ran out, or the snapshot lacks the document.
    Unavailable {
        /// The sentence a `not_measured` verdict carries.
        reason: String,
    },
}

/// What the registries said about every package the project depends on.
#[derive(Debug, Clone, Default)]
pub struct Known {
    /// The packages, keyed by `(ecosystem, name)`.
    pub packages: BTreeMap<(Ecosystem, String), Result<KnownPackage, KnownError>>,
    /// The client notes, sorted: caps a sample hit, and any other sentence a report shows.
    pub notes: Vec<String>,
    /// Packages whose document a replay could not answer, sorted.
    pub skipped: Vec<(String, String)>,
    /// How many requests the lookups spent.
    pub requests: u32,
}

impl Known {
    /// The answer for one package.
    #[must_use]
    pub fn get(
        &self,
        ecosystem: Ecosystem,
        name: &str,
    ) -> Option<&Result<KnownPackage, KnownError>> {
        self.packages.get(&(ecosystem, name.to_owned()))
    }

    /// The package, when the registry answered with one.
    #[must_use]
    pub fn package(&self, ecosystem: Ecosystem, name: &str) -> Option<&KnownPackage> {
        match self.get(ecosystem, name) {
            Some(Ok(package)) => Some(package),
            _ => None,
        }
    }

    /// The packages the registry says do not exist.
    #[must_use]
    pub fn not_found(&self) -> Vec<(Ecosystem, String)> {
        let mut out: Vec<(Ecosystem, String)> = self
            .packages
            .iter()
            .filter(|(_, answer)| matches!(answer, Err(KnownError::NotFound)))
            .map(|(key, _)| key.clone())
            .collect();
        out.sort();
        out
    }

    /// The packages whose document could not be read, with the reason, sorted.
    #[must_use]
    pub fn unavailable(&self) -> Vec<(Ecosystem, String, String)> {
        let mut out: Vec<(Ecosystem, String, String)> = self
            .packages
            .iter()
            .filter_map(|((ecosystem, name), answer)| match answer {
                Err(KnownError::Unavailable { reason }) => {
                    Some((*ecosystem, name.clone(), reason.clone()))
                }
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// Adds one answer, keeping the maps sorted by construction.
    fn insert(
        &mut self,
        ecosystem: Ecosystem,
        name: &str,
        answer: Result<KnownPackage, KnownError>,
    ) {
        self.packages.insert((ecosystem, name.to_owned()), answer);
    }

    /// Records a note once.
    fn note(&mut self, sentence: String) {
        if self.notes.iter().any(|seen| seen == &sentence) {
            return;
        }
        self.notes.push(sentence);
        self.notes.sort();
    }

    /// Records a skipped document once.
    fn skip(&mut self, label: String, reason: &str) {
        if self.skipped.iter().any(|(seen, _)| seen == &label) {
            return;
        }
        self.skipped.push((label, reason.to_owned()));
        self.skipped.sort();
    }
}

/// Fetches every package the project depends on, once each.
///
/// One request per package name, not per version: a registry's document answers for every
/// version at once, and asking twice for `serde` would spend the budget on a duplicate.
///
/// The clients share one [`SnapshotHttp`], so a run replays them offline under
/// `--snapshot-from` and every request spends one unit of the `forge_max_requests` budget.
/// A 404 is recorded as [`KnownError::NotFound`] — P02's strongest signal — and any other
/// failure as [`KnownError::Unavailable`] with the sentence the verdict will carry.
pub async fn gather(world: &World, http: &mut SnapshotHttp, as_of: Timestamp) -> Known {
    let mut known = Known::default();
    for (ecosystem, name) in world.package_keys() {
        match ecosystem {
            Ecosystem::Npm => {
                let mut client = Npm::new(http, as_of);
                let answer = match client.package(&name).await {
                    Ok(package) => Ok(from_npm(&package)),
                    Err(error) => Err(kind_of(&error)),
                };
                let notes = client.notes();
                let skipped = client.skipped();
                for note in notes {
                    known.note(note);
                }
                for (label, reason) in skipped {
                    known.skip(label, &reason);
                }
                known.insert(ecosystem, &name, answer);
            }
            Ecosystem::PyPi => {
                let mut client = PyPI::new(http);
                let answer = match client.package(&name).await {
                    Ok(package) => Ok(from_pypi(&package)),
                    Err(error) => Err(kind_of(&error)),
                };
                let notes = client.notes();
                let skipped = client.skipped();
                for note in notes {
                    known.note(note);
                }
                for (label, reason) in skipped {
                    known.skip(label, &reason);
                }
                known.insert(ecosystem, &name, answer);
            }
            Ecosystem::Crates => {
                let mut client = CratesIo::new(http);
                let answer = match client.krate(&name).await {
                    Ok(krate) => Ok(from_crate(&krate)),
                    Err(error) => Err(kind_of(&error)),
                };
                let notes = client.notes();
                let skipped = client.skipped();
                for note in notes {
                    known.note(note);
                }
                for (label, reason) in skipped {
                    known.skip(label, &reason);
                }
                known.insert(ecosystem, &name, answer);
            }
        }
    }
    known
}

/// Maps a client error onto the two answers a check distinguishes.
fn kind_of(error: &RegistryError) -> KnownError {
    match error {
        RegistryError::NotFound { .. } => KnownError::NotFound,
        other => KnownError::Unavailable {
            reason: other.to_string(),
        },
    }
}

/// The facts an npm packument carries.
fn from_npm(package: &NpmPackage) -> KnownPackage {
    let versions: Vec<KnownVersion> = package
        .versions
        .iter()
        .map(|version| KnownVersion {
            version: version.version.clone(),
            published_at: version.published_at,
            deprecated: version.deprecated.clone(),
            yanked: false,
        })
        .collect();
    KnownPackage {
        name: package.name.clone(),
        ecosystem: Ecosystem::Npm,
        // npm dates the package's own creation in the packument's `time` map, which is the
        // first publish; a document without it falls back to the oldest version date.
        first_published: package.created_at.or_else(|| {
            versions
                .iter()
                .filter_map(|version| version.published_at)
                .min()
        }),
        last_published: package.modified_at,
        total_versions: versions.len(),
        latest: package.latest.clone(),
        repository: package.repository.clone(),
        license: package.license.clone(),
        downloads: None,
        versions,
        capped: false,
    }
}

/// The facts a PyPI document carries.
fn from_pypi(package: &PyPiPackage) -> KnownPackage {
    let versions: Vec<KnownVersion> = package
        .releases
        .iter()
        .map(|release| KnownVersion {
            version: release.version.clone(),
            published_at: release.uploaded_at,
            deprecated: None,
            yanked: release.yanked,
        })
        .collect();
    KnownPackage {
        name: package.name.clone(),
        ecosystem: Ecosystem::PyPi,
        first_published: versions
            .iter()
            .filter_map(|version| version.published_at)
            .min(),
        last_published: versions
            .iter()
            .filter_map(|version| version.published_at)
            .max(),
        total_versions: versions.len(),
        latest: package.latest.clone(),
        repository: package.repository.clone(),
        license: package.license.clone(),
        downloads: None,
        versions,
        capped: false,
    }
}

/// The facts a crates.io document carries.
fn from_crate(krate: &Crate) -> KnownPackage {
    let versions: Vec<KnownVersion> = krate
        .versions
        .iter()
        .map(|version| KnownVersion {
            version: version.version.clone(),
            published_at: version.created_at,
            deprecated: None,
            yanked: version.yanked,
        })
        .collect();
    KnownPackage {
        name: krate.name.clone(),
        ecosystem: Ecosystem::Crates,
        first_published: versions
            .iter()
            .filter_map(|version| version.published_at)
            .min(),
        last_published: versions
            .iter()
            .filter_map(|version| version.published_at)
            .max(),
        total_versions: versions.len(),
        latest: krate.newest.clone(),
        repository: krate.repository.clone(),
        // crates.io's document carries no licence field in the shape this client parses, so
        // P06 reports the crate's licence as unmeasured rather than as absent.
        license: None,
        downloads: Some(krate.downloads),
        versions,
        capped: false,
    }
}
