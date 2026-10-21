//! What a database adapter reports: its capabilities, the databases it made, its snapshots
//! (M2-T03).
//!
//! These are data, so they live in `core` where the schema exporter sees them and the report can
//! cite them. The adapters themselves are in `crates/adapters`.
//!
//! ## Why capabilities are [`Measured`]
//!
//! A database channel needs things the host may not have: `wal_level = logical`, a decoding plugin,
//! `pg_stat_statements`. Each is a separate question, and each can fail on its own -- a Postgres
//! server can be perfectly usable for the app while offering no row diff. Folding them into one
//! `bool` would make "the row diff was empty" indistinguishable from "we could not watch rows",
//! which is the difference between `pass` and `not_measured` for B03, B04 and B09. So each channel
//! carries its own reason, and the reason travels to the report unchanged.
//!
//! ## Why a connection URL is not here
//!
//! [`DbHandle`] holds the database's *name* and the port and host it lives on, never the admin URL
//! or a password. The app's URL is built when the app is started, from the admin URL the process
//! read out of its environment, and a type that could be serialized with credentials in it would
//! eventually be logged. [`RedactedUrl`] is what a `Display` or an error message gets instead.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::SnapshotId;
use crate::model::Measured;

/// The logical-decoding plugin a Postgres server offered.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum DecodingPlugin {
    /// `wal2json`, which reports column names and types in its output.
    Wal2json,
    /// `test_decoding`, which ships with Postgres and reports a textual change description.
    TestDecoding,
}

impl DecodingPlugin {
    /// The plugin's name, as Postgres knows it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wal2json => "wal2json",
            Self::TestDecoding => "test_decoding",
        }
    }
}

/// What a database adapter can actually do on this host.
///
/// Every field is [`Measured`] except the engine name, because "we asked and the answer was no" and
/// "we could not ask" lead a check to the same verdict but a reader to different places.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Capabilities {
    /// The engine, for the report's coverage section.
    pub engine: String,
    /// Which logical-decoding plugin the server has, when it has one.
    pub logical_decoding: Measured<DecodingPlugin>,
    /// Whether `pg_stat_statements` is preloaded, so query counts are available.
    pub query_stats: Measured<()>,
    /// Whether the admin role may create databases. A bool, not `Measured`: this one is answered
    /// by trying, and "we could not ask" is reported through [`Capabilities::problems`] instead,
    /// because without CREATEDB nothing else works and provision fails loudly rather than quietly.
    pub createdb: bool,
    /// Every capability that is missing, in a fixed order, for `substance doctor` and the report.
    ///
    /// Empty means every channel is available. The list is what a reader needs: a check that says
    /// `not_measured` should be explainable in one line, and this is the shared place the lines
    /// come from.
    pub problems: Vec<String>,
}

impl Capabilities {
    /// Whether the row-diff channel is available.
    #[must_use]
    pub fn can_diff_rows(&self) -> bool {
        self.logical_decoding.value().is_some()
    }

    /// Whether query counts are available.
    #[must_use]
    pub fn can_count_queries(&self) -> bool {
        self.query_stats.value().is_some()
    }

    /// Why the row-diff channel is missing, when it is.
    #[must_use]
    pub fn row_diff_reason(&self) -> Option<&str> {
        match &self.logical_decoding {
            Measured::NotMeasured { reason } => Some(reason),
            Measured::Measured { .. } => None,
        }
    }

    /// Why the query-count channel is missing, when it is.
    #[must_use]
    pub fn query_count_reason(&self) -> Option<&str> {
        match &self.query_stats {
            Measured::NotMeasured { reason } => Some(reason),
            Measured::Measured { .. } => None,
        }
    }
}

/// A connection URL with its credentials removed.
///
/// The only string form of an admin URL that may be logged, put in an error, or serialized into the
/// manifest. It keeps the scheme, the host, the port and the database name, because those are what
/// a reader needs to find the server, and drops everything that could be a credential: the user,
/// the password, and any query string (a `?password=` style parameter is rare but not impossible).
///
/// Built by [`RedactedUrl::parse`], which never fails: a string it cannot parse is not echoed back,
/// because echoing an unparseable credential-bearing URL is exactly the leak this type prevents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(transparent)]
pub struct RedactedUrl(String);

impl RedactedUrl {
    /// A placeholder for a URL that could not be parsed.
    pub const UNPARSEABLE: &'static str = "<unparseable url, redacted>";

    /// Redacts a connection URL.
    #[must_use]
    pub fn parse(url: &str) -> Self {
        Self::try_parse(url).unwrap_or_else(|()| Self(Self::UNPARSEABLE.to_owned()))
    }

    /// The redacted text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses `scheme://user:password@host:port/db?params` into `scheme://host:port/db`.
    ///
    /// Written by hand rather than with a URL crate because the rule has to be conservative: an
    /// input this function does not understand must produce [`Self::UNPARSEABLE`], never a
    /// best-effort echo.
    fn try_parse(url: &str) -> Result<Self, ()> {
        let (scheme, rest) = url.split_once("://").ok_or(())?;
        if scheme.is_empty()
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        {
            return Err(());
        }
        // Everything after the last `@` is the authority; everything before it is credentials.
        // A password may itself contain `@` and `/`, so the *last* `@` before the path is the
        // right separator, which is why this does not split at the first one.
        let authority_and_path = match rest.rsplit_once('@') {
            Some((_credentials, tail)) => tail,
            None => rest,
        };
        // Drop the query string and fragment: they can carry a password as a parameter, which is
        // why `?password=...` must not survive redaction. Each cut is applied to the *result* of
        // the previous one: writing `.map_or(original, ...)` twice would silently reinstate the
        // original string and leak the very parameter this is dropping.
        let after_query = authority_and_path
            .split_once('?')
            .map_or(authority_and_path, |(head, _)| head);
        let without_query = after_query
            .split_once('#')
            .map_or(after_query, |(head, _)| head);
        // Keep host, port and database; drop nothing else, because none of the rest is a secret.
        let authority = without_query.trim_end_matches('/');
        if authority.is_empty() {
            return Err(());
        }
        let mut text = String::with_capacity(scheme.len() + authority.len() + 3);
        text.push_str(scheme);
        text.push_str("://");
        text.push_str(authority);
        Ok(Self(text))
    }
}

impl std::fmt::Display for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The databases one run owns.
///
/// Named from the run id so two runs never share a database, and derived rather than stored so a
/// caller cannot point them at a database it does not own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DbHandle {
    /// The engine that made it.
    pub engine: String,
    /// The database the app is pointed at, for example `substance_abcdef123456`.
    pub database: String,
    /// The template every probe is reset from, for example `substance_abcdef123456_seeded`.
    pub seeded: String,
    /// The host and port the server is on, credentials removed.
    pub server: RedactedUrl,
    /// The environment variable the app reads its URL from.
    pub url_env: String,
    /// The SQLite file, relative to the work copy, when the engine is SQLite.
    pub sqlite_path: Option<String>,
}

/// The short run tag a database name is built from: the first `TAG_CHARS` characters of the id.
///
/// Twelve characters of `run_<26 base32 chars>` are 60 bits of the id, which is far more than
/// enough to keep two runs apart while keeping the name short enough to read in a `\l` listing.
///
/// Taken with `chars().take()` rather than a byte slice because a slice panics on an id shorter
/// than the count: an id is never short today, but a panic in a name function would abort a run at
/// the worst possible moment, and there is no reason for a naming rule to be able to do that.
#[must_use]
pub fn run_tag(id: &crate::ids::RunId) -> String {
    id.as_str().chars().take(TAG_CHARS).collect()
}

/// How many characters of a run id a database or project name keeps.
pub const TAG_CHARS: usize = 12;

/// The database a snapshot lives in, named from the snapshot id.
///
/// A snapshot in Postgres is a database, not a file, so its name has to be derivable from the id
/// alone: `CREATE DATABASE x TEMPLATE y` needs a name, and a run that stored the name instead of
/// deriving it could reset from a database an earlier run left behind. The id is already
/// `snap_<26 base32 chars>`, so this function only has to add the `_probe_` prefix a reader needs to
/// tell a snapshot apart from the run's own databases at a glance.
#[must_use]
pub fn snapshot_database(id: &SnapshotId) -> String {
    format!(
        "{}_{}",
        SnapshotId::PREFIX,
        id.as_str().trim_start_matches("snap_")
    )
}
