//! The package registries: npm, PyPI, crates.io and deps.dev (M1-T04).
//!
//! P02–P04 and P07 need facts only a registry has: when a version was published, whether it
//! was later deprecated or yanked, where its source lives, how many times it was
//! downloaded, and how many packages depend on it. Each client fetches one kind of document
//! through [`SnapshotHttp`], so a run's registry replies replay offline under
//! `--snapshot-from` and every request costs one unit of the `forge_max_requests` budget.
//!
//! Four decisions shape the module:
//!
//! - **No credential is involved.** All four APIs are anonymous, so no request carries an
//!   `Authorization` header and no token can leak. crates.io asks clients to identify
//!   themselves with a `User-Agent`, which [`SnapshotHttp`] already sends for every request;
//!   the registry clients add nothing.
//!
//! - **Time only moves backwards from `as_of`.** The npm downloads query needs a date range;
//!   it is computed as `as_of` minus 30 days through core's [`window_start`], never read from
//!   the wall clock, so the same run replays to the same URL. The clients take `as_of` for
//!   exactly this reason.
//!
//! - **A long version list says so.** npm's packument holds every version ever published. The
//!   clients keep them all (P02 and P03 read every release) and [`MAX_VERSIONS`] bounds what a
//!   report shows; when a list exceeds it, [`Registry::notes`] carries the sentence saying so,
//!   so a reader knows the report shows a sample.
//!
//! - **A missing document is a typed gap, not an error.** A 404 means the package does not
//!   exist — which is itself P02's strongest signal — and answers
//!   [`RegistryError::NotFound`]. A snapshot that lacks the entry answers
//!   [`RegistryError::Snapshot`] and is named in [`Registry::skipped`], so a check reports
//!   `not_measured` rather than inventing a version history.
//!
//! The endpoint shapes below were confirmed from each registry's own documentation rather
//! than by calling the API:
//!
//! | Registry | Endpoint | Fields |
//! | --- | --- | --- |
//! | npm | `GET https://registry.npmjs.org/{name}` | `versions`, `time` (a map of version to ISO date plus `created`/`modified`), `deprecated` per version, `repository` |
//! | npm downloads | `GET https://api.npmjs.org/downloads/point/{start}:{end}/{name}` | `downloads`, `start`, `end` |
//! | PyPI | `GET https://pypi.org/pypi/{name}/json` | `info.project_urls`, `info.version`, `releases` (version to files with `upload_time_iso_8601`), `urls[].yanked` |
//! | crates.io | `GET https://crates.io/api/v1/crates/{name}` | `crate.repository`, `crate.downloads`, `versions[].num`, `versions[].created_at`, `versions[].yanked` |
//! | deps.dev | `GET https://api.deps.dev/v3/systems/{system}/packages/{name}` | `versions[].versionKey.version` |
//! | deps.dev dependents | `GET https://api.deps.dev/v3/systems/{system}/packages/{name}/versions/{version}:dependents` | `totalDependents` |

pub mod cratesio;
pub mod depsdev;
pub mod npm;
pub mod pypi;
mod support;

use substance_core::Timestamp;
use substance_core::clock::window_start;
use substance_core::deps::Ecosystem;

pub use cratesio::{CRATES_API, Crate, CrateVersion, CratesIo};
pub use depsdev::{DEPS_DEV_API, Dependents, DepsDev};
pub use npm::{NPM_API, NPM_DOWNLOADS_API, Npm, NpmPackage, NpmVersion};
pub use pypi::{PYPI_API, PyPI, PyPiPackage, PyPiRelease};
pub use support::Documents;

/// How many versions of one package a report shows.
///
/// A report limit, not a parsing limit (M1-T10). The clients keep every version a registry
/// returns, because P02 asks how many exist and P03 asks what the newest release was on the
/// day a dependency was adopted: questions about the whole list. Storing two thousand version
/// strings for one package would dominate a report, so the payload that lists versions shows
/// the newest 500, and the note recorded beside it says the list was longer.
pub const MAX_VERSIONS: usize = 500;

/// How many days the npm downloads query covers.
///
/// 30 days is the "last month" the plan asks for. The range is `as_of` minus this many days
/// to `as_of`, so two runs of one instant ask the same question.
pub const DOWNLOAD_WINDOW_DAYS: u32 = 30;

/// The first day of the downloads window, as `YYYY-MM-DD` in UTC.
#[must_use]
pub fn download_window_start(as_of: Timestamp) -> String {
    let start = window_start(as_of, DOWNLOAD_WINDOW_DAYS);
    let date = start.as_offset_date_time().date();
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// The last day of the downloads window, as `YYYY-MM-DD` in UTC.
#[must_use]
pub fn download_window_end(as_of: Timestamp) -> String {
    let date = as_of.as_offset_date_time().date();
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Why a registry could not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The registry answered 404: the package does not exist (P02's strongest signal).
    #[error("{ecosystem} has no package {name}")]
    NotFound {
        /// The registry.
        ecosystem: Ecosystem,
        /// The package name as it was asked for.
        name: String,
    },
    /// The registry refused the request: rate limited or blocked.
    #[error("{ecosystem} refused the request for {name} ({status})")]
    Refused {
        /// The registry.
        ecosystem: Ecosystem,
        /// The package name.
        name: String,
        /// The status code.
        status: u16,
    },
    /// The `forge_max_requests` budget ran out.
    #[error("registry request budget exhausted after {used} requests")]
    Budget {
        /// How many requests were made.
        used: u32,
    },
    /// The snapshot this run replays does not hold the document.
    #[error("the registry snapshot has no entry for {url}")]
    Snapshot {
        /// The URL that was missing or unreadable.
        url: String,
    },
    /// The document was not the shape this client parses.
    #[error("the {ecosystem} document for {name} is not what this build parses: {detail}")]
    Schema {
        /// The registry.
        ecosystem: Ecosystem,
        /// The package name.
        name: String,
        /// What was wrong.
        detail: String,
    },
}

/// What every registry client offers, for a caller holding one behind a trait object.
///
/// The package-level methods differ per registry (npm has downloads, deps.dev has
/// dependents), so the trait covers only the bookkeeping every client shares: how many
/// requests were spent and which documents could not be read.
pub trait Registry {
    /// The ecosystem this client speaks for.
    fn ecosystem(&self) -> Ecosystem;

    /// How many requests have been spent so far.
    fn requests_used(&self) -> u32;

    /// The documents a replay could not answer, sorted, with the reason.
    fn skipped(&self) -> Vec<(String, String)>;

    /// The caps a sample hit, sorted, with a sentence a report can quote.
    fn notes(&self) -> Vec<String>;
}
