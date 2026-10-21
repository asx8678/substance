//! Mailpit: the app's outgoing mail, caught and read back (M3-T04).
//!
//! ## Why a mailbox instead of a mail library
//!
//! An app in a run must not be able to send real mail, and a check that judges "was a confirmation
//! email sent" needs the message. Mailpit solves both: the app is pointed at its SMTP port, which
//! accepts everything and delivers nothing, and the same messages are read back over its HTTP API.
//! The `[mail] env` block in `substance.toml` is what points the app there, so an app whose SMTP
//! variables are named something else can still be wired up.
//!
//! ## Missing Mailpit is a fact about the run, not a failure
//!
//! If the binary is not on PATH, the email channel is `not_measured("tool missing: mailpit")`. That
//! is the same principle as any channel the engine cannot watch (ADR-0014): a record must never say
//! "no email was sent" when nothing was looking.
//!
//! ## Order without a clock
//!
//! Messages come back in Mailpit's own order, which is arrival order, and the recorder keeps that
//! order. Mailpit's message IDs are not recorded: they count what the whole mailbox saw, so two
//! runs that send the same mail would otherwise differ.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

use substance_core::model::EmailEntry;

use crate::blobs::BlobStore;
use crate::budget::Budget;
use crate::port::free_port;
use crate::process::{ProcessError, ProcessSpec, ServiceProcess};

/// The binary looked up on PATH.
pub const BIN: &str = "mailpit";

/// The path Mailpit answers once it is listening.
const READY_PATH: &str = "/livez";

/// Why Mailpit could not be used.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// The binary is not on PATH.
    #[error("tool missing: mailpit")]
    Missing,
    /// It started but never answered.
    #[error("mailpit did not become ready: {0}")]
    NotReady(String),
    /// It could not be started or stopped.
    #[error("mailpit: {0}")]
    Process(#[from] ProcessError),
    /// The API answered with something this client cannot read.
    #[error("mailpit api: {0}")]
    Api(String),
}

/// A running Mailpit, with the ports the app is told to use.
#[derive(Debug)]
pub struct Mailpit {
    /// The helper process.
    process: ServiceProcess,
    /// The SMTP port the app sends to.
    smtp_port: u16,
    /// The HTTP port the client reads from.
    http_port: u16,
}

impl Mailpit {
    /// Starts Mailpit on two free ports, or says the tool is missing.
    ///
    /// # Errors
    ///
    /// Returns [`MailError::Missing`] when the binary is not on PATH, [`MailError::Process`] when
    /// it cannot be started, and [`MailError::NotReady`] when it starts but never answers.
    pub async fn start(
        run_dir: &Path,
        logs_dir: &Path,
        shutdown: Budget,
    ) -> Result<Self, MailError> {
        if !on_path(BIN) {
            return Err(MailError::Missing);
        }
        let smtp_port = free_port().map_err(|error| MailError::NotReady(error.to_string()))?;
        let http_port = free_port().map_err(|error| MailError::NotReady(error.to_string()))?;
        // Both listeners are pinned to loopback: a run's mailbox is the run's, and Mailpit binds
        // every interface by default.
        let smtp = format!("{}:{smtp_port}", Ipv4Addr::LOCALHOST);
        let http = format!("{}:{http_port}", Ipv4Addr::LOCALHOST);
        let spec = ProcessSpec::new(
            BIN,
            &["--smtp", &smtp, "--listen", &http],
            run_dir,
            "mailpit",
        )
        .with_env("MP_SMTP_AUTH_ACCEPT_ANY", "1")
        .with_env("MP_SMTP_AUTH_ALLOW_INSECURE", "1")
        .with_env("MP_MAX_MESSAGES", "500");
        let mut process = ServiceProcess::spawn(&spec, logs_dir, shutdown).await?;
        if !wait_ready(http_port).await {
            let _ = process.stop(Duration::from_secs(5)).await;
            return Err(MailError::NotReady(format!(
                "no answer from {READY_PATH} on port {http_port}"
            )));
        }
        Ok(Self {
            process,
            smtp_port,
            http_port,
        })
    }

    /// The SMTP port the app sends to.
    #[must_use]
    pub fn smtp_port(&self) -> u16 {
        self.smtp_port
    }

    /// The HTTP API port.
    #[must_use]
    pub fn http_port(&self) -> u16 {
        self.http_port
    }

    /// A client for this mailpit, storing bodies with `blobs`.
    #[must_use]
    pub fn client(&self, blobs: BlobStore) -> MailClient {
        MailClient {
            base: format!("http://{}:{}", Ipv4Addr::LOCALHOST, self.http_port),
            blobs,
        }
    }

    /// Stops it.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError`] when the group cannot be signalled.
    pub async fn stop(&mut self, grace: Duration) -> Result<(), ProcessError> {
        self.process.stop(grace).await.map(|_| ())
    }
}

/// Waits until Mailpit's health path answers, for at most five seconds.
async fn wait_ready(port: u16) -> bool {
    let url = format!("http://{}:{port}{READY_PATH}", Ipv4Addr::LOCALHOST);
    for _ in 0..100 {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            && let Ok(response) = client.get(&url).send().await
            && response.status().is_success()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Whether a program is on PATH.
fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(bin).is_file())
    })
}

/// A message Mailpit caught, as the recorder records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caught {
    /// The entry that goes into the record.
    pub entry: EmailEntry,
}

/// The client for Mailpit's API.
#[derive(Debug, Clone)]
pub struct MailClient {
    /// The base URL, `http://127.0.0.1:<port>`.
    base: String,
    /// Where bodies go.
    blobs: BlobStore,
}

impl MailClient {
    /// Every message caught so far, oldest first, with each body stored as a blob.
    ///
    /// # Errors
    ///
    /// Returns [`MailError::Api`] when the list or one message cannot be read, so a channel that
    /// half-worked is `not_measured` rather than a partial list that reads as complete.
    pub async fn messages(&self) -> Result<Vec<Caught>, MailError> {
        let list = self.get("/api/v1/messages").await?;
        let items = list
            .get("messages")
            .and_then(|messages| messages.as_array())
            .ok_or_else(|| MailError::Api("no messages array".to_owned()))?;
        let mut caught = Vec::new();
        for item in items {
            let id = item
                .get("ID")
                .and_then(|id| id.as_str())
                .ok_or_else(|| MailError::Api("a message without an ID".to_owned()))?;
            let detail = self.get(&format!("/api/v1/message/{id}")).await?;
            caught.push(self.caught(&detail));
        }
        Ok(caught)
    }

    /// One message, from its detail document.
    fn caught(&self, detail: &serde_json::Value) -> Caught {
        let subject = detail
            .get("Subject")
            .and_then(|subject| subject.as_str())
            .unwrap_or_default()
            .to_owned();
        let to = detail
            .get("To")
            .and_then(|to| to.as_array())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| address.get("Address"))
                    .filter_map(|address| address.as_str())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // The text part, falling back to the HTML: a check looking for a confirmation link reads
        // either, and a mail sent only as HTML is common.
        let body_text = detail
            .get("Text")
            .and_then(|text| text.as_str())
            .filter(|text| !text.trim().is_empty())
            .or_else(|| detail.get("HTML").and_then(|html| html.as_str()))
            .unwrap_or_default();
        let body = match body_text.is_empty() {
            true => None,
            false => self.blobs.put(body_text.as_bytes()).ok(),
        };
        Caught {
            entry: EmailEntry { to, subject, body },
        }
    }

    /// GETs one path and parses it.
    async fn get(&self, path: &str) -> Result<serde_json::Value, MailError> {
        let url = format!("{}{path}", self.base);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| MailError::Api(format!("{url}: {error}")))?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|error| MailError::Api(format!("{url}: {error}")))?;
        let body = response
            .text()
            .await
            .map_err(|error| MailError::Api(format!("{url}: {error}")))?;
        serde_json::from_str(&body).map_err(|error| MailError::Api(format!("{url}: {error}")))
    }
}

/// The app's SMTP environment, from `[mail] env` with the placeholders filled in.
///
/// Sorted by name, because the app's environment is part of the run's identity: two runs that give
/// the app the same variables must compare equal, and a sorted map is what makes that true.
#[must_use]
pub fn environment(
    configured: &BTreeMap<String, String>,
    smtp_host: &str,
    smtp_port: u16,
) -> Vec<(String, String)> {
    let port = smtp_port.to_string();
    configured
        .iter()
        .map(|(name, value)| {
            let value = value
                .replace("{smtp_host}", smtp_host)
                .replace("{smtp_port}", &port);
            (name.clone(), value)
        })
        .collect()
}
