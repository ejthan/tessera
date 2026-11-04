// SPDX-License-Identifier: GPL-3.0-or-later

//! What the client reports about each request.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use reqwest::Method;
use url::Url;

/// Bodies in a [`RequestEvent`] are cut to at most this many bytes (1 MiB).
pub const BODY_LIMIT: usize = 1 << 20;

/// Called once per request, after the response or error.
pub type RequestHook = Arc<dyn Fn(&RequestEvent) + Send + Sync>;

/// What a [`RequestHook`] learns about a request. Never contains the API key.
#[derive(Debug, Clone)]
pub struct RequestEvent {
    /// Unique per process, increasing.
    pub id: u64,
    /// Wall-clock time the request started.
    pub started_at: SystemTime,
    /// Base URL of the node the request went to.
    pub node: Url,
    /// HTTP method.
    pub method: Method,
    /// Request path including any base path prefix, for example `/health`.
    pub path: String,
    /// Query string without the leading `?`.
    pub query: Option<String>,
    /// Headers the client set, except `X-TYPESENSE-API-KEY`.
    pub request_headers: Vec<(String, String)>,
    /// The first 4 characters of the API key, for a redacted display.
    pub key_prefix: String,
    /// Request body, if one was sent.
    pub request_body: Option<Body>,
    /// Response body, if one arrived.
    pub response_body: Option<Body>,
    /// HTTP status, or `None` if no response arrived.
    pub status: Option<u16>,
    /// Time from sending the request to reading the whole body.
    pub duration: Duration,
    /// Response body size in bytes.
    pub size: Option<u64>,
    /// Who caused the request.
    pub origin: Origin,
    /// Display text of the error, if the request failed without a response.
    pub error: Option<String>,
}

/// A request or response body, cut to [`BODY_LIMIT`] bytes on a UTF-8 boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// The (possibly cut) text.
    pub text: Arc<str>,
    /// Length in bytes before cutting.
    pub original_len: usize,
}

impl Body {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            text: Arc::from(&text[..text.floor_char_boundary(BODY_LIMIT)]),
            original_len: text.len(),
        }
    }
}

/// Who caused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A user action.
    User,
    /// Background polling, for example the overview's refresh.
    Polling,
}

pub(crate) fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
