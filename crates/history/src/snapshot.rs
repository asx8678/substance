//! Snapshotting external data: every forge and registry request is saved, and
//! `--snapshot-from` replays it without the network (M1-T01).
//!
//! Two properties matter. A response is stored under a key derived from the request, so
//! replay finds it without guessing; and no credential is ever written, because the
//! saved headers come from an allowlist rather than from a denylist.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use data_encoding::BASE64;
use serde::{Deserialize, Serialize};
use substance_core::Timestamp;
use substance_core::hash::{hex, sha256};

/// How many times one request is attempted before it is reported as failed.
const MAX_ATTEMPTS: u32 = 3;

/// The pause between attempts: fixed, with no jitter, so two runs retry on one schedule.
const BACKOFF: Duration = Duration::from_millis(500);

/// How long one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The response headers that are saved.
///
/// An allowlist, not a denylist: `Authorization`, `Cookie`, `Set-Cookie`, `Proxy-Authorization`
/// and every header substance has never heard of are simply not in this list, so a token
/// cannot reach the disk by being renamed.
const SAVED_HEADERS: &[&str] = &[
    "cache-control",
    "content-type",
    "date",
    "etag",
    "last-modified",
    "link",
    "location",
    "retry-after",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
];

/// Words that must not appear in a saved header's name (lowercase, as compared).
///
/// A compile-time assertion below checks the allowlist against this list, so adding an
/// entry such as `set-cookie` or `x-api-key` fails the build instead of leaking a
/// credential on the next run. ADR-0009 forbids tests, so the invariant lives here.
const FORBIDDEN_HEADER_MARKERS: &[&str] =
    &["auth", "cookie", "token", "secret", "apikey", "session"];

/// Whether `haystack` contains `needle`, as a `const fn` so the check can be static.
const fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        let mut offset = 0;
        let mut matched = true;
        while offset < needle.len() {
            if haystack[start + offset] != needle[offset] {
                matched = false;
                break;
            }
            offset += 1;
        }
        if matched {
            return true;
        }
        start += 1;
    }
    false
}

// The allowlist may not name a credential, whatever it is called.
const _: () = {
    let mut header = 0;
    while header < SAVED_HEADERS.len() {
        let name = SAVED_HEADERS[header].as_bytes();
        let mut marker = 0;
        while marker < FORBIDDEN_HEADER_MARKERS.len() {
            assert!(
                !contains_bytes(name, FORBIDDEN_HEADER_MARKERS[marker].as_bytes()),
                "a saved snapshot header must not name a credential"
            );
            marker += 1;
        }
        header += 1;
    }
};

/// Which directory a snapshot lives under, and so which kind of data it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    /// Forge objects: issues, pull requests, releases.
    Forge,
    /// Package-registry metadata.
    Registry,
}

impl SnapshotKind {
    /// The subdirectory name.
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Self::Forge => "forge",
            Self::Registry => "registry",
        }
    }
}

/// The accept type GitHub answers plain JSON with.
pub const JSON_ACCEPT: &str = "application/vnd.github+json";

/// A request whose response is worth keeping.
///
/// The fields are private to this module on purpose: `auth` is the one secret the pipeline
/// handles, so the only ways to set it are [`SnapshotRequest::with_auth`], which takes it
/// by value, and the redacted [`std::fmt::Debug`] impl below, which prints a marker
/// instead of the value.
pub struct SnapshotRequest {
    kind: SnapshotKind,
    method: String,
    url: String,
    body: String,
    accept: String,
    headers: BTreeMap<String, String>,
    auth: Option<String>,
}

impl std::fmt::Debug for SnapshotRequest {
    /// Prints the request without its credential, so a log line or an error message can
    /// name the request and cannot carry the token.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SnapshotRequest")
            .field("kind", &self.kind)
            .field("method", &self.method)
            .field("url", &self.url)
            .field("body", &self.body)
            .field("accept", &self.accept)
            .field("headers", &self.headers)
            .field("auth", &self.auth.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl PartialEq for SnapshotRequest {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.method == other.method
            && self.url == other.url
            && self.body == other.body
            && self.accept == other.accept
    }
}

impl Eq for SnapshotRequest {}

impl SnapshotRequest {
    /// A GET with no credential, asking for JSON.
    #[must_use]
    pub fn get(kind: SnapshotKind, url: &str) -> Self {
        Self {
            kind,
            method: "GET".to_owned(),
            url: url.to_owned(),
            body: String::new(),
            accept: JSON_ACCEPT.to_owned(),
            headers: BTreeMap::new(),
            auth: None,
        }
    }

    /// Asks for a different media type, such as `application/vnd.github.star+json`.
    ///
    /// The type is not part of the snapshot key: GitHub's plain and starred responses
    /// differ only by an extra field, so a replay of a run that asked for stars answers a
    /// later plain request with the same document.
    #[must_use]
    pub fn with_accept(mut self, accept: &str) -> Self {
        self.accept = accept.to_owned();
        self
    }

    /// Adds a request header, such as the API version GitHub asks clients to pin.
    ///
    /// Request headers are never saved — only the response's allowlisted headers are — so
    /// this cannot put anything on disk. An unencodable header fails the request rather
    /// than being dropped.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(name.to_owned(), value.to_owned());
        self
    }

    /// Adds a credential, which live mode sends and no file records.
    #[must_use]
    pub fn with_auth(mut self, auth: Option<String>) -> Self {
        self.auth = auth;
        self
    }

    /// The key this request is filed under: `sha256(method || url || body)` in hex.
    #[must_use]
    pub fn key(&self) -> String {
        let mut preimage = Vec::new();
        preimage.extend_from_slice(self.method.as_bytes());
        preimage.push(b'\n');
        preimage.extend_from_slice(self.url.as_bytes());
        preimage.push(b'\n');
        preimage.extend_from_slice(self.body.as_bytes());
        hex(&sha256(&preimage))
    }
}

/// A response body, as JSON when it parses and as base64 otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotBody {
    /// A JSON document, kept as JSON so a reader can inspect it.
    Json(serde_json::Value),
    /// Anything else, base64 so the bytes survive.
    Base64(String),
}

impl SnapshotBody {
    /// Wraps raw bytes, preferring JSON when they parse.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        match serde_json::from_slice::<serde_json::Value>(bytes) {
            Ok(value) => Self::Json(value),
            Err(_) => Self::Base64(BASE64.encode(bytes)),
        }
    }

    /// The body as bytes.
    #[must_use]
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            Self::Json(value) => canonical_bytes(value),
            Self::Base64(text) => BASE64.decode(text.as_bytes()).unwrap_or_default(),
        }
    }

    /// The body as text, with invalid UTF-8 replaced.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Json(value) => value.to_string(),
            Self::Base64(_) => String::from_utf8_lossy(&self.as_bytes()).into_owned(),
        }
    }
}

/// One saved response, and the instant it was captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SnapshotResponse {
    /// The request key, so a file names itself.
    pub key: String,
    /// The method that was sent.
    pub method: String,
    /// The URL that was requested.
    pub url: String,
    /// The status code.
    pub status: u16,
    /// The saved headers, from the allowlist only.
    pub headers: BTreeMap<String, String>,
    /// The body.
    pub body: SnapshotBody,
    /// The instant the response was captured; a replay keeps the original.
    pub as_of: Timestamp,
}

impl SnapshotResponse {
    /// Whether the status is 2xx.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Why external data could not be fetched or replayed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The request budget (`forge_max_requests`) is used up.
    #[error("budget exhausted: {used} of {budget} external requests made")]
    BudgetExhausted {
        /// How many were made.
        used: u32,
        /// The cap.
        budget: u32,
    },
    /// `--snapshot-from` was given, but this request was not saved there.
    #[error("the snapshot in {path} has no entry for {url}")]
    Missing {
        /// The URL that was not found.
        url: String,
        /// The file that was looked for.
        path: PathBuf,
    },
    /// The response could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
    /// The request failed after every attempt.
    #[error("{url}: {detail}")]
    Request {
        /// The URL that failed.
        url: String,
        /// What went wrong.
        detail: String,
    },
}

/// Saves and replays external responses for one run.
///
/// Cloning it gives another handle onto the **same** budget: the counter lives behind an
/// `Arc<AtomicU32>`, so every handle a run hands out — the registry clients', the forge
/// client's — spends from one `forge_max_requests` cap. A clone with its own counter would
/// have let each client spend the whole budget, which is the failure this type exists to
/// prevent. An atomic rather than a `Cell` because [`Forge`](crate::forge::Forge) requires
/// `Send + Sync`.
#[derive(Debug, Clone)]
pub struct SnapshotHttp {
    client: reqwest::Client,
    root: PathBuf,
    as_of: Timestamp,
    budget: u32,
    used: std::sync::Arc<std::sync::atomic::AtomicU32>,
    replay: bool,
}

impl SnapshotHttp {
    /// A client that fetches and saves under `root` (the run directory).
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Request`] when the HTTP client cannot be built, which
    /// means the TLS backend failed to initialize.
    pub fn live(root: &Path, as_of: Timestamp, budget: u32) -> Result<Self, SnapshotError> {
        Ok(Self {
            client: build_client()?,
            root: root.to_path_buf(),
            as_of,
            budget,
            used: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            replay: false,
        })
    }

    /// A client that replays the responses saved under `root`, without the network.
    ///
    /// The budget is not consumed in replay mode: nothing is fetched, so nothing can
    /// exceed a request cap.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Request`] when the HTTP client cannot be built. A replay
    /// never uses it, but keeping one type means the caller does not branch.
    pub fn replay(root: &Path) -> Result<Self, SnapshotError> {
        Ok(Self {
            client: build_client()?,
            root: root.to_path_buf(),
            as_of: Timestamp::parse_rfc3339("1970-01-01T00:00:00Z").map_err(|error| {
                SnapshotError::Request {
                    url: "snapshot replay".to_owned(),
                    detail: error.to_string(),
                }
            })?,
            budget: 0,
            used: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            replay: true,
        })
    }

    /// Whether this client replays instead of fetching.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        self.replay
    }

    /// How many requests have been made so far.
    #[must_use]
    pub fn used(&self) -> u32 {
        self.used.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Fetches `request`, or replays it, in that order.
    ///
    /// The budget is spent per request, in call order; callers issue requests in a fixed
    /// order, so the same input hits the same cap (§4.3).
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::BudgetExhausted`] once the cap is reached,
    /// [`SnapshotError::Missing`] when a replay has no saved entry, and
    /// [`SnapshotError::Request`] when every attempt failed.
    pub async fn fetch(
        &mut self,
        request: &SnapshotRequest,
    ) -> Result<SnapshotResponse, SnapshotError> {
        let path = self.path_of(request);
        if self.replay {
            return read_response(&path, request);
        }
        // A fetch takes `&mut self`, so no two requests race; the ordering only has to make
        // the count visible to the other handles, which relaxed ordering does.
        let used = self.used.load(std::sync::atomic::Ordering::Relaxed);
        if used >= self.budget {
            return Err(SnapshotError::BudgetExhausted {
                used,
                budget: self.budget,
            });
        }
        self.used
            .store(used.saturating_add(1), std::sync::atomic::Ordering::Relaxed);
        let response = self.fetch_with_retries(request).await?;
        write_response(&path, &response)?;
        Ok(response)
    }

    /// Attempts the request up to [`MAX_ATTEMPTS`] times with a fixed backoff.
    async fn fetch_with_retries(
        &self,
        request: &SnapshotRequest,
    ) -> Result<SnapshotResponse, SnapshotError> {
        let mut attempt = 1;
        loop {
            let outcome = self.attempt(request).await;
            match outcome {
                Ok(response) if !retryable(response.status) => return Ok(response),
                Ok(response) if attempt >= MAX_ATTEMPTS => return Ok(response),
                Ok(_) => {}
                Err(error) if attempt >= MAX_ATTEMPTS => return Err(error),
                Err(_) => {}
            }
            tokio::time::sleep(BACKOFF).await;
            attempt += 1;
        }
    }

    /// One attempt.
    async fn attempt(&self, request: &SnapshotRequest) -> Result<SnapshotResponse, SnapshotError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|error| {
            SnapshotError::Request {
                url: request.url.clone(),
                detail: error.to_string(),
            }
        })?;
        let mut builder = self
            .client
            .request(method, &request.url)
            .timeout(REQUEST_TIMEOUT);
        builder = builder.header(reqwest::header::ACCEPT, request.accept.as_str());
        for (name, value) in &request.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                SnapshotError::Request {
                    url: request.url.clone(),
                    detail: "a request header name is not valid".to_owned(),
                }
            })?;
            let value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                SnapshotError::Request {
                    url: request.url.clone(),
                    detail: format!("the request header {name} has an invalid value"),
                }
            })?;
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body.clone());
        }
        if let Some(auth) = &request.auth {
            builder = builder.header(reqwest::header::AUTHORIZATION, auth.as_str());
        }
        let response = builder
            .send()
            .await
            .map_err(|error| SnapshotError::Request {
                url: request.url.clone(),
                detail: error.to_string(),
            })?;
        let status = response.status().as_u16();
        let headers = saved_headers(response.headers());
        let bytes = response
            .bytes()
            .await
            .map_err(|error| SnapshotError::Request {
                url: request.url.clone(),
                detail: error.to_string(),
            })?;
        Ok(SnapshotResponse {
            key: request.key(),
            method: request.method.clone(),
            url: request.url.clone(),
            status,
            headers,
            body: SnapshotBody::of(&bytes),
            as_of: self.as_of,
        })
    }

    /// Where `request`'s response is filed, or was filed by an earlier run.
    #[must_use]
    pub fn path_of(&self, request: &SnapshotRequest) -> PathBuf {
        self.root
            .join(request.kind.dir())
            .join(format!("{}.json", request.key()))
    }
}

/// Builds the HTTP client: rustls, a bounded pool, and no redirect following, so a
/// request cannot silently become a request to another host that was never snapshotted.
fn build_client() -> Result<reqwest::Client, SnapshotError> {
    reqwest::Client::builder()
        .user_agent(format!("substance/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| SnapshotError::Request {
            url: "client".to_owned(),
            detail: error.to_string(),
        })
}

/// Copies only the allowlisted headers, in sorted order.
fn saved_headers(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    let mut saved = BTreeMap::new();
    for (name, value) in headers {
        let lower = name.as_str().to_ascii_lowercase();
        if !SAVED_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if let Ok(text) = value.to_str() {
            saved.insert(lower, text.to_owned());
        }
    }
    saved
}

/// Whether a status is worth another attempt: a server error or a rate limit.
const fn retryable(status: u16) -> bool {
    status >= 500 || status == 429
}

/// Writes one response as canonical pretty JSON.
fn write_response(path: &Path, response: &SnapshotResponse) -> Result<(), SnapshotError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SnapshotError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let value = serde_json::to_value(response).map_err(|error| SnapshotError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidData, error),
    })?;
    std::fs::write(
        path,
        substance_core::hash::canonical_json_pretty_value(&value),
    )
    .map_err(|source| SnapshotError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Reads one saved response, or reports that the snapshot does not have it.
fn read_response(
    path: &Path,
    request: &SnapshotRequest,
) -> Result<SnapshotResponse, SnapshotError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SnapshotError::Missing {
                url: request.url.clone(),
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(SnapshotError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_json::from_str(&text).map_err(|error| SnapshotError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidData, error),
    })
}

/// Canonical JSON for a stored body, so an equal document stores equal bytes.
fn canonical_bytes(value: &serde_json::Value) -> Vec<u8> {
    substance_core::hash::canonical_json_value(value)
}
