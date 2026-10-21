//! The egress proxy: a logging forward proxy with a per-run CA (M3-T04, ADR-0021).
//!
//! ## What it is for
//!
//! §4.1 is blunt about this: without a sandbox substance cannot *block* egress, so it does the next
//! best thing. The app is pointed at a proxy that refuses every host not on the run's allow list, and
//! every request it sees -- allowed or refused -- is recorded with its bodies. The manifest records
//! `egress_enforced: false`, because a client that ignores the proxy variables simply is not seen;
//! ADR-0021 names which clients those are.
//!
//! ## Deny by default, and what a denial looks like
//!
//! A host that is not allowed gets `502 Bad Gateway` with a JSON body that says so. Deliberately a
//! *response* rather than a dropped connection: a check asking "did the app try to reach a paid API"
//! needs the attempt in the record, and an app that hangs on a reset gives the run nothing. It is
//! also what lets this module's probe use a hostname that does not resolve: the request is refused
//! before anything is connected to.
//!
//! ## The CA, and why its key is not a file
//!
//! TLS interception needs a CA the app trusts. The certificate is written to
//! `runs/<run_id>/egress/ca.pem`, because the app has to read it from somewhere; the private key
//! never leaves memory, because a key on disk is a key in a backup, in a ticket attachment, and in a
//! run directory that outlives the run.
//!
//! ## Order without a clock
//!
//! Entries are ordered by a counter, not a timestamp: two runs of the same action must produce the
//! same sequence, and a wall clock would make them differ while saying nothing the order does not
//! (ADR-0009). A window is two counter values, taken around an action.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hudsucker::certificate_authority::RcgenAuthority;
use hudsucker::hyper::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST};
use hudsucker::hyper::{Method, Request, Response, StatusCode};
use hudsucker::rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
};
// `aws_lc_rs` rather than `ring`: the workspace's rustls is already built with it (Cargo.lock
// shows `aws-lc-rs` under rustls), and asking for `ring` would pull a second backend in.
use hudsucker::rustls::crypto::aws_lc_rs as provider;
use hudsucker::{Body, HttpContext, HttpHandler, Proxy, RequestOrResponse};

use crate::blobs::BlobStore;
use crate::port::free_port;
use substance_core::BlobRef;

/// The directory inside the run that holds the CA certificate.
pub const EGRESS_DIR: &str = "egress";

/// How many server certificates one run may cache.
///
/// One per host. Bounded so a redirect chain through generated hostnames cannot grow the cache
/// without limit.
const CERT_CACHE: u64 = 512;

/// How much of one body is kept in its blob.
///
/// A run that downloads a large file through the proxy must not put it in the run directory twice.
/// The cap is generous for anything a check judges -- a form, JSON, an HTML page, a confirmation
/// email -- and bounds the run's size. The size recorded on the entry is the real one, before the
/// cap, so a truncated body is visibly truncated rather than silently short.
pub const BODY_CAP: usize = 1024 * 1024;

/// Why the proxy could not start.
#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    /// The CA could not be generated.
    #[error("cannot generate the run's certificate authority: {0}")]
    Authority(String),
    /// Something the CA needs could not be written or read.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The IO error.
        source: std::io::Error,
    },
    /// The proxy failed to build or to start listening.
    #[error("cannot start the egress proxy: {0}")]
    Build(String),
    /// No port could be allocated for it.
    #[error("cannot allocate a port for the egress proxy: {0}")]
    Port(std::io::Error),
}

/// One request the proxy saw.
///
/// `index` is the proxy's own counter rather than a clock (ADR-0009). Everything else is what a
/// check reads: who was contacted, with what, and what came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// Where in the sequence this request was: 0 is the first since the proxy started.
    pub index: u64,
    /// The host contacted, without its port.
    pub host: String,
    /// The method, as the proxy saw it.
    pub method: String,
    /// The path and query.
    pub path: String,
    /// The status, or 0 while the response was still in flight.
    pub status: u16,
    /// Whether the run's allow list refused it.
    pub denied: bool,
    /// Whether the proxied request or its upstream leg failed (a refused connection, a reset).
    ///
    /// Distinct from `denied`: a denial is substance's decision and is evidence about what the app
    /// tried to reach, while a failure is the network's. A run that learns a paid API was attempted
    /// needs the first; a run reading an ordinary success needs to know it did not get one.
    pub failed: bool,
    /// The request body, as a blob, when it had one.
    pub request_body: Option<BlobRef>,
    /// The response body, as a blob, when it had one.
    pub response_body: Option<BlobRef>,
    /// The request body's real size, before the cap.
    pub request_bytes: u64,
    /// The response body's real size, before the cap.
    pub response_bytes: u64,
}

/// The proxy's log, which the recorder slices into a window.
///
/// Shared rather than copied: the handler is cloned per connection, and a handler that cloned its
/// log would give every connection a private history.
#[derive(Debug, Clone, Default)]
pub struct EgressLog {
    inner: Arc<Mutex<Vec<Observed>>>,
    /// Requests whose response has not come back yet, as (connection, index) in request order.
    ///
    /// The connection address is the correlation key because hudsucker does not carry a request's
    /// extensions over to its response (its client builds a new request), and a map keyed by
    /// anything about the request itself would collide. A connection is served one request at a
    /// time, so the first pending entry on the same connection is that response's request. Ordered
    /// and keyed by an address rather than hashed, so the pairing is the same in every run.
    pending: Arc<Mutex<Vec<(SocketAddr, u64)>>>,
}

impl EgressLog {
    /// An empty log, which is where a run starts.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The mark a window starts at: the number of entries so far.
    #[must_use]
    pub fn mark(&self) -> u64 {
        self.inner
            .lock()
            .map(|entries| entries.len() as u64)
            .unwrap_or_default()
    }

    /// The entries from `mark` onwards, in order.
    ///
    /// A mark past the end yields an empty list rather than an error: "nothing happened" is a real
    /// answer, and it is the answer a run with no egress must be able to record.
    #[must_use]
    pub fn since(&self, mark: u64) -> Vec<Observed> {
        self.inner
            .lock()
            .map(|entries| {
                entries
                    .iter()
                    .skip(usize::try_from(mark).unwrap_or(usize::MAX))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every entry, for a whole-run window.
    #[must_use]
    pub fn all(&self) -> Vec<Observed> {
        self.since(0)
    }

    /// Appends one entry, returning its index.
    ///
    /// A poisoned lock means a handler panicked mid-request; the entry is dropped rather than taking
    /// the proxy down, because the run still has its other evidence.
    fn push(&self, mut observed: Observed) -> u64 {
        match self.inner.lock() {
            Ok(mut entries) => {
                observed.index = entries.len() as u64;
                entries.push(observed);
                entries.len() as u64 - 1
            }
            Err(_) => 0,
        }
    }

    /// Notes that the request at `index` is waiting for its response on `client`.
    fn await_response(&self, client: SocketAddr, index: u64) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.push((client, index));
        }
    }

    /// Takes the oldest request waiting on `client`, which is the one a response belongs to.
    fn take_pending(&self, client: SocketAddr) -> Option<u64> {
        let mut pending = self.pending.lock().ok()?;
        let position = pending.iter().position(|(waiting, _)| *waiting == client)?;
        Some(pending.remove(position).1)
    }

    /// Fills in the response half of the entry at `index`.
    fn complete(&self, index: u64, status: u16, response: Option<BlobRef>, bytes: u64) {
        if let Ok(mut entries) = self.inner.lock()
            && let Some(entry) = entries.get_mut(usize::try_from(index).unwrap_or(usize::MAX))
        {
            entry.status = status;
            entry.response_body = response;
            entry.response_bytes = bytes;
        }
    }

    /// Fails the entry at `index`: the request never got a response.
    fn fail(&self, index: u64) {
        if let Ok(mut entries) = self.inner.lock()
            && let Some(entry) = entries.get_mut(usize::try_from(index).unwrap_or(usize::MAX))
        {
            entry.status = 502;
            entry.failed = true;
        }
    }
}

/// A running proxy, with the CA the app must trust and the log the recorder reads.
#[derive(Debug)]
pub struct Egress {
    /// The proxy's address: `127.0.0.1:<port>`.
    addr: SocketAddr,
    /// The PEM certificate the app is told to trust.
    ca_pem: String,
    /// Where that certificate was written.
    ca_path: PathBuf,
    /// Every request the proxy saw.
    log: EgressLog,
    /// Dropping or firing this shuts the proxy down.
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    /// The proxy's task, so `stop` can wait for it.
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Egress {
    /// Starts a proxy for this run: refuses everything not in `allow`, records everything it sees.
    ///
    /// `run_dir` is where the CA certificate goes; `blobs` is where the bodies go.
    ///
    /// # Errors
    ///
    /// Returns [`EgressError`] when the CA cannot be made or written, no port is free, or the proxy
    /// cannot be built or does not come up.
    pub async fn start(
        run_dir: &Path,
        allow: &BTreeSet<String>,
        blobs: BlobStore,
    ) -> Result<Self, EgressError> {
        let (issuer, ca_pem) = authority()?;
        let directory = run_dir.join(EGRESS_DIR);
        std::fs::create_dir_all(&directory).map_err(|source| EgressError::Io {
            path: directory.clone(),
            source,
        })?;
        let ca_path = directory.join("ca.pem");
        std::fs::write(&ca_path, ca_pem.as_bytes()).map_err(|source| EgressError::Io {
            path: ca_path.clone(),
            source,
        })?;

        let port = free_port().map_err(EgressError::Port)?;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let log = EgressLog::new();
        let handler = Handler {
            allow: allow.clone(),
            log: log.clone(),
            blobs,
        };

        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let ca = RcgenAuthority::new(issuer, CERT_CACHE, provider::default_provider());
        let proxy = Proxy::builder()
            .with_addr(addr)
            .with_ca(ca)
            .with_rustls_connector(provider::default_provider())
            .with_http_handler(handler)
            .with_graceful_shutdown(async move {
                let _ = receiver.await;
            })
            .build()
            .map_err(|error| EgressError::Build(error.to_string()))?;

        let task = tokio::spawn(async move {
            let _ = proxy.start().await;
        });
        // Bound before returning: `free_port` released the port, so an app that connected at once
        // could race the listener and fail for a reason that has nothing to do with the app.
        if !wait_for_listener(addr).await {
            return Err(EgressError::Build(format!(
                "the proxy did not start listening on {addr}"
            )));
        }

        Ok(Self {
            addr,
            ca_pem,
            ca_path,
            log,
            shutdown: Some(sender),
            task: Some(task),
        })
    }

    /// The proxy's `host:port`.
    #[must_use]
    pub fn address(&self) -> String {
        self.addr.to_string()
    }

    /// The `http://host:port` form a proxy variable wants.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The CA certificate in PEM form.
    #[must_use]
    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    /// Where the CA certificate was written.
    #[must_use]
    pub fn ca_path(&self) -> &Path {
        &self.ca_path
    }

    /// The log the recorder slices.
    #[must_use]
    pub fn log(&self) -> &EgressLog {
        &self.log
    }

    /// Stops the proxy and waits for its task. Safe to call twice.
    pub async fn stop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    /// The environment that points a process at this proxy.
    ///
    /// Every variable is here because clients disagree about which one they read: `curl` and most C
    /// programs take the uppercase pair; Node's `undici` ignores all of them unless
    /// `NODE_USE_ENV_PROXY` is set; Python's `requests` takes `REQUESTS_CA_BUNDLE`; OpenSSL-based
    /// clients take `SSL_CERT_FILE`; Go's default transport reads `HTTPS_PROXY` and has no bundle
    /// variable at all (ADR-0021 says traffic that ignores these is invisible).
    ///
    /// `no_proxy` carries the loopback names plus `extra_no_proxy` (the database host). Loopback is
    /// not the outside world: the app's own origin, its database and Mailpit must keep working while
    /// the proxy refuses everything else, and MITM-ing the app's own port would break it for nothing.
    #[must_use]
    pub fn env(&self, extra_no_proxy: &[String]) -> Vec<(String, String)> {
        let proxy = self.url();
        let mut no_proxy: Vec<String> = vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
            "::1".to_owned(),
        ];
        no_proxy.extend(extra_no_proxy.iter().cloned());
        no_proxy.sort();
        no_proxy.dedup();
        let no_proxy = no_proxy.join(",");
        let bundle = self.ca_path.display().to_string();
        vec![
            ("HTTP_PROXY".to_owned(), proxy.clone()),
            ("HTTPS_PROXY".to_owned(), proxy.clone()),
            ("http_proxy".to_owned(), proxy.clone()),
            ("https_proxy".to_owned(), proxy.clone()),
            ("NO_PROXY".to_owned(), no_proxy.clone()),
            ("no_proxy".to_owned(), no_proxy),
            ("NODE_EXTRA_CA_CERTS".to_owned(), bundle.clone()),
            ("NODE_USE_ENV_PROXY".to_owned(), "1".to_owned()),
            ("SSL_CERT_FILE".to_owned(), bundle.clone()),
            ("REQUESTS_CA_BUNDLE".to_owned(), bundle.clone()),
            ("CURL_CA_BUNDLE".to_owned(), bundle),
        ]
    }
}

impl Drop for Egress {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
    }
}

/// Builds a CA for this run: the issuer, and its certificate in PEM form.
///
/// The key pair lives in the issuer and in nothing else. Nothing writes it anywhere, which is the
/// point: a run directory is kept after the run for the report, and a CA key in it would let whoever
/// holds the directory impersonate every host to the next run.
fn authority() -> Result<(hudsucker::rcgen::Issuer<'static, KeyPair>, String), EgressError> {
    let key = KeyPair::generate().map_err(|error| EgressError::Authority(error.to_string()))?;
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "substance run CA");
    name.push(DnType::OrganizationName, "substance");
    params.distinguished_name = name;
    // `is_ca` is what lets the certificates this signs be accepted by a client told to trust this
    // one, and `KeyCertSign` is what lets it sign them at all.
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        hudsucker::rcgen::KeyUsagePurpose::KeyCertSign,
        hudsucker::rcgen::KeyUsagePurpose::DigitalSignature,
        hudsucker::rcgen::KeyUsagePurpose::CrlSign,
    ];
    let certificate = params
        .self_signed(&key)
        .map_err(|error| EgressError::Authority(error.to_string()))?;
    let pem = certificate.pem();
    Ok((hudsucker::rcgen::Issuer::new(params, key), pem))
}

/// Waits until something accepts connections on `addr`, for at most a second.
async fn wait_for_listener(addr: SocketAddr) -> bool {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

/// The handler: decides whether a host is allowed, and records what it sees.
#[derive(Clone)]
struct Handler {
    /// The hosts the run allows, as the config spells them.
    allow: BTreeSet<String>,
    /// Where the entries go.
    log: EgressLog,
    /// Where the bodies go.
    blobs: BlobStore,
}

impl Handler {
    /// Whether `host` is allowed: an exact match, or a subdomain of an allowed host.
    ///
    /// The suffix rule is what makes one allowance usable with a CDN, and it is *suffix* rather than
    /// "ends with" on purpose: `evil-api.example.com` is not `api.example.com`, and a plain
    /// ends-with test would turn a one-host allowance into a wildcard for anyone who can register a
    /// name.
    fn allows(&self, host: &str) -> bool {
        let lowered = host.to_ascii_lowercase();
        self.allow.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            lowered == allowed || lowered.ends_with(&format!(".{allowed}"))
        })
    }

    /// Reads a body into bytes, capped, and stores it as a blob.
    ///
    /// Returns the bytes to send on, the blob (when there was a body) and the real length. The bytes
    /// are returned because a buffered body has to be replayed to the client: hyper's body can be
    /// read once.
    async fn buffer(
        &self,
        body: Body,
    ) -> (Option<hudsucker::hyper::body::Bytes>, Option<BlobRef>, u64) {
        // `BodyExt::collect` from `http_body_util` is what reads a hyper body; hudsucker neither
        // re-exports it nor offers a `to_bytes`, so the body is collected here and the bytes kept.
        use http_body_util::BodyExt as _;
        let collected = body.collect().await;
        let Ok(collected) = collected else {
            return (None, None, 0);
        };
        let bytes = collected.to_bytes();
        let length = bytes.len() as u64;
        if bytes.is_empty() {
            return (Some(bytes), None, 0);
        }
        let reference = self.blobs.put(&bytes).ok();
        (Some(bytes), reference, length)
    }
}

impl HttpHandler for Handler {
    async fn handle_request(&mut self, ctx: &HttpContext, req: Request<Body>) -> RequestOrResponse {
        // A CONNECT asks for the TLS tunnel: it carries the target host but no path and no body.
        // Recorded and refused like any other request, so a blocked HTTPS host shows up in the log
        // instead of only in the app's own error.
        let is_connect = req.method() == Method::CONNECT;
        let (mut parts, body) = req.into_parts();
        let host = parts
            .uri
            .host()
            .map(str::to_owned)
            .or_else(|| {
                parts
                    .headers
                    .get(HOST)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.split(':').next().unwrap_or(value).to_owned())
            })
            .unwrap_or_default();
        let path = parts
            .uri
            .path_and_query()
            .map(|path| path.as_str().to_owned())
            .unwrap_or_default();
        let method = parts.method.as_str().to_owned();

        let (bytes, request_body, request_bytes) = match is_connect {
            // A CONNECT's body is empty by construction, and waiting on it would be waiting for a
            // stream that only produces data after the tunnel is up.
            true => (None, None, 0),
            false => self.buffer(body).await,
        };

        if !self.allows(&host) {
            let index = self.log.push(Observed {
                index: 0,
                host: host.clone(),
                method,
                path,
                status: StatusCode::BAD_GATEWAY.as_u16(),
                denied: true,
                failed: false,
                request_body,
                response_body: None,
                request_bytes,
                response_bytes: 0,
            });
            let payload = format!(
                "{{\"denied\":true,\"host\":\"{}\",\"index\":{index}}}\n",
                escape(&host)
            );
            let response = Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .unwrap_or_else(|_| Response::new(Body::empty()));
            return RequestOrResponse::Response(response);
        }

        // Allowed. The index is reserved before the request goes out, so allowed and refused
        // requests order the same way; `handle_response` fills in the status and the response body,
        // finding the entry through the connection the response comes back on.
        let index = self.log.push(Observed {
            index: 0,
            host,
            method,
            path,
            status: 0,
            denied: false,
            failed: false,
            request_body,
            response_body: None,
            request_bytes,
            response_bytes: 0,
        });
        self.log.await_response(ctx.client_addr, index);
        let request = match bytes {
            Some(bytes) => {
                parts.headers.remove(CONTENT_LENGTH);
                Request::from_parts(parts, Body::from(bytes))
            }
            None => Request::from_parts(parts, Body::empty()),
        };
        RequestOrResponse::Request(request)
    }

    async fn handle_response(&mut self, ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
        let (mut parts, body) = res.into_parts();
        let status = parts.status.as_u16();
        let (bytes, response_body, response_bytes) = self.buffer(body).await;
        // The request waiting on this connection. Without it the entry cannot be found: the log is
        // shared, and a response is not necessarily the most recent entry.
        if let Some(index) = self.log.take_pending(ctx.client_addr) {
            self.log
                .complete(index, status, response_body, response_bytes);
        }
        match bytes {
            Some(bytes) => {
                parts.headers.remove(CONTENT_LENGTH);
                Response::from_parts(parts, Body::from(bytes))
            }
            None => Response::from_parts(parts, Body::empty()),
        }
    }

    /// The upstream leg failed: there is no response to record, and a request that looks complete
    /// with status 0 would read as "still in flight" forever.
    async fn handle_error(
        &mut self,
        ctx: &HttpContext,
        _error: hudsucker::hyper_util::client::legacy::Error,
    ) -> Response<Body> {
        if let Some(index) = self.log.take_pending(ctx.client_addr) {
            self.log.fail(index);
        }
        let payload = "{\"failed\":true}\n";
        Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(payload))
            .unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

/// A string with the characters JSON needs escaped, escaped.
///
/// Only the two that matter for a host name, which is what this is used on: a host cannot contain a
/// quote or a backslash, but a parsed URI is not the place to assume that.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}
