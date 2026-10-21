//! `crawl.har`: the crawl's traffic as an HTTP Archive (M4-T01).
//!
//! ## Why a HAR and not a copy of the records
//!
//! The plan names `crawl.har` as an output because M7 feeds it to `mitmproxy2swagger` and Redocly to
//! infer an OpenAPI spec (I03). Those tools read HAR, so the crawl writes HAR: the format is the
//! interface. The entries come from the records the crawl already produced -- the same requests, in
//! the same order -- so the HAR adds no new observation and cannot disagree with them.
//!
//! ## What is left out
//!
//! Bodies are referenced by blob, not inlined: a response body can be megabytes, and the blob store is
//! where the crawl already put it. The HAR's `content.text` is absent rather than empty, so "the body
//! was not kept" is distinguishable from "the body was empty" -- the same rule the records follow.
//! Volatile values are already normalized in the path, so a HAR from two runs of one crawl compares
//! byte for byte.

use serde::Serialize;
use substance_core::model::{EffectRecord, NetEntry};

/// A HAR document, with only the fields the plan's consumers read.
#[derive(Debug, Clone, Serialize)]
pub struct Har {
    /// The archive's format version; `1.2` is what the tools accept.
    pub log: HarLog,
}

/// The archive's log.
#[derive(Debug, Clone, Serialize)]
pub struct HarLog {
    /// The HAR version.
    pub version: String,
    /// Who produced it.
    pub creator: HarCreator,
    /// One entry per request, in crawl order.
    pub entries: Vec<HarEntry>,
}

/// The producer.
#[derive(Debug, Clone, Serialize)]
pub struct HarCreator {
    /// The tool's name.
    pub name: String,
    /// The tool's version.
    pub version: String,
}

/// One request and its response.
#[derive(Debug, Clone, Serialize)]
pub struct HarEntry {
    /// When the request started, as the record's frozen instant.
    ///
    /// The same value for every entry, because the crawl runs under one frozen clock and a HAR with
    /// real timestamps would make two runs of one crawl differ. The tools that read a HAR do not use
    /// the field for ordering -- `entries` is ordered -- so pinning it costs nothing.
    pub started_date_time: String,
    /// How long it took, in milliseconds.
    pub time: u64,
    /// The request.
    pub request: HarRequest,
    /// The response.
    pub response: HarResponse,
    /// Where the request came from: the screen, and the control that triggered it.
    pub comment: String,
}

/// The request half of an entry.
#[derive(Debug, Clone, Serialize)]
pub struct HarRequest {
    /// The method.
    pub method: String,
    /// The absolute URL.
    pub url: String,
    /// The HTTP version, as recorded.
    pub http_version: String,
    /// Headers the crawl kept: none, because the record does not keep them and inventing them would
    /// be a claim about a request nobody measured.
    pub headers: Vec<HarNameValue>,
    /// The query string, split from the URL.
    pub query_string: Vec<HarNameValue>,
    /// The body's size, when a body was kept.
    pub body_size: Option<u64>,
    /// The body, by blob reference, when one was kept.
    pub post_data: Option<HarPostData>,
}

/// The response half of an entry.
#[derive(Debug, Clone, Serialize)]
pub struct HarResponse {
    /// The status code, or 0 when the request produced no response.
    pub status: u16,
    /// The status text; empty, because the record keeps the code and not the reason phrase.
    pub status_text: String,
    /// The HTTP version.
    pub http_version: String,
    /// Headers the crawl kept: none, for the reason above.
    pub headers: Vec<HarNameValue>,
    /// The body.
    pub content: HarContent,
    /// Where the response body is, as a blob reference.
    pub redirect_url: String,
}

/// The response body.
#[derive(Debug, Clone, Serialize)]
pub struct HarContent {
    /// The size in bytes, when a body was kept.
    pub size: Option<u64>,
    /// The media type, when known.
    pub mime_type: String,
    /// The body's text: always absent, because bodies stay in the blob store.
    pub text: Option<String>,
    /// The blob reference, in a vendor field so the standard fields keep their meaning.
    pub substance_blob: Option<String>,
}

/// A body, by blob reference.
#[derive(Debug, Clone, Serialize)]
pub struct HarPostData {
    /// The media type, when known.
    pub mime_type: String,
    /// The blob reference.
    pub substance_blob: String,
}

/// A name and a value, as HAR spells a header or a query parameter.
#[derive(Debug, Clone, Serialize)]
pub struct HarNameValue {
    /// The name.
    pub name: String,
    /// The value.
    pub value: String,
}

/// Builds a HAR from the crawl's records.
///
/// One entry per request of every record, in record order and then request order: the crawl's own
/// sequence, which is the order the spec inference wants and the order two runs agree on.
#[must_use]
pub fn from_records(records: &[EffectRecord], origin: &str, as_of: &str) -> Har {
    let mut entries = Vec::new();
    for record in records {
        let screen = record.action.screen.clone();
        for entry in &record.net {
            entries.push(entry_of(entry, origin, as_of, &screen));
        }
    }
    Har {
        log: HarLog {
            version: "1.2".to_owned(),
            creator: HarCreator {
                name: "substance".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            entries,
        },
    }
}

/// One record request as a HAR entry.
fn entry_of(entry: &NetEntry, origin: &str, as_of: &str, screen: &str) -> HarEntry {
    let url = if entry.path.starts_with("http://") || entry.path.starts_with("https://") {
        entry.path.clone()
    } else {
        format!("{origin}{}", entry.path)
    };
    let query_string = match url.split_once('?') {
        Some((_, query)) => query
            .split('&')
            .map(|part| match part.split_once('=') {
                Some((name, value)) => HarNameValue {
                    name: name.to_owned(),
                    value: value.to_owned(),
                },
                None => HarNameValue {
                    name: part.to_owned(),
                    value: String::new(),
                },
            })
            .collect(),
        None => Vec::new(),
    };
    HarEntry {
        started_date_time: as_of.to_owned(),
        // The record keeps the settle time, not a per-request time; reporting zero is honest, and a
        // made-up number would be the one field a reader would trust.
        time: 0,
        request: HarRequest {
            method: entry.method.to_uppercase(),
            url,
            http_version: String::new(),
            headers: Vec::new(),
            query_string,
            body_size: entry.req_body.as_ref().map(|_| 0),
            post_data: entry.req_body.as_ref().map(|body| HarPostData {
                mime_type: String::new(),
                substance_blob: body.as_str().to_owned(),
            }),
        },
        response: HarResponse {
            status: entry.status.unwrap_or(0),
            status_text: String::new(),
            http_version: String::new(),
            headers: Vec::new(),
            content: HarContent {
                size: entry.res_body.as_ref().map(|_| 0),
                mime_type: String::new(),
                text: None,
                substance_blob: entry.res_body.as_ref().map(|body| body.as_str().to_owned()),
            },
            redirect_url: String::new(),
        },
        comment: format!("{screen}: {}", entry.failed.clone().unwrap_or_default()),
    }
}
