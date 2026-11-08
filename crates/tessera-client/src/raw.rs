// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw requests of the console: any path, once, to the node the title bar names.

use std::fmt;
use std::time::{Duration, Instant};

use reqwest::Method;
use url::Url;

use crate::{Client, Error, LONG_TIMEOUT};

/// A console body and the content type it is sent with.
#[derive(Clone, PartialEq, Eq)]
pub enum RawBody {
    /// Sent as `application/json`.
    Json(String),
    /// Sent as `text/plain`, for JSONL.
    Text(String),
}

impl RawBody {
    /// The text as typed.
    pub fn text(&self) -> &str {
        let (Self::Json(text) | Self::Text(text)) = self;
        text
    }
}

// Lengths only: a body can hold a key.
impl fmt::Debug for RawBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RawBody({} bytes)", self.text().len())
    }
}

/// One request of the console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRequest {
    /// HTTP method.
    pub method: Method,
    /// Path below the node, relative.
    pub path: String,
    /// Query string without the leading `?`.
    pub query: Option<String>,
    /// The body, if any.
    pub body: Option<RawBody>,
}

/// What came back, whatever the status.
#[derive(Clone, PartialEq, Eq)]
pub struct RawResponse {
    /// HTTP status.
    pub status: u16,
    /// Response headers, in the server's order.
    pub headers: Vec<(String, String)>,
    /// The whole body.
    pub body: String,
    /// Time from sending to reading the whole body.
    pub duration: Duration,
}

// Lengths only: `POST /keys` answers with the only copy of a key.
impl fmt::Debug for RawResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("duration", &self.duration)
            .finish_non_exhaustive()
    }
}

/// Whether `url` is the node `base` or below it. Anything else would carry the API key away.
fn check_stays_on_node(base: &Url, url: &Url) -> Result<(), Error> {
    if url.origin() == base.origin() && url.path().starts_with(base.path()) {
        Ok(())
    } else {
        Err(Error::InvalidUrl(base.to_string()))
    }
}

/// A `.` or `..` segment in any spelling a URL parser or a proxy in front of the node
/// resolves, so a path cannot climb to another resource of the same node.
fn has_dot_segment(path: &str) -> bool {
    path.to_ascii_lowercase()
        .replace("%2e", ".")
        .replace("%2f", "/")
        .replace("%5c", "/")
        .split(['/', '\\'])
        .any(|segment| matches!(segment, "." | ".."))
}

/// `path` below `base`, or [`Error::InvalidUrl`]. The app validates first with core's
/// `parse_path`; this keeps the promise if that ever misses a spelling.
fn check_path(base: &Url, path: &str) -> Result<(), Error> {
    let invalid = || Error::InvalidUrl(base.to_string());
    // On an `http` node `join` reads `http:x` as relative but `https:x` as the host `x`, and
    // it strips tabs, line breaks and edge spaces, which this check must not depend on.
    let first = path.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    if first.contains(':')
        || path.chars().any(char::is_control)
        || path.trim() != path
        || has_dot_segment(path)
    {
        return Err(invalid());
    }
    check_stays_on_node(base, &base.join(path).map_err(|_| invalid())?)
}

/// Whether `path` below `base` is `POST /keys`'s route, in any spelling the server takes.
fn is_keys(base: &Url, path: &str) -> bool {
    base.join(path).is_ok_and(|url| {
        let below = url.path().strip_prefix(base.path()).unwrap_or(url.path());
        below.trim_matches('/') == "keys"
    })
}

impl Client {
    /// Sends `request` once to the active node, with [`LONG_TIMEOUT`] and no failover.
    ///
    /// Any HTTP status is `Ok`, because the status is what the console shows. A read-only
    /// client refuses everything but `GET` and `HEAD`, also a `POST` to `multi_search`. A path
    /// that leaves the node is [`Error::InvalidUrl`], and nothing is sent. The event of a
    /// `POST /keys` keeps no body: the answer holds a value, and the request can choose one.
    pub async fn raw(&self, request: RawRequest) -> Result<RawResponse, Error> {
        let node = self.active_node();
        check_path(&self.nodes[node], &request.path)?;
        let body = request.body.map(|body| match body {
            RawBody::Json(text) => (text, "application/json"),
            RawBody::Text(text) => (text, "text/plain"),
        });
        let start = Instant::now();
        let creates_key =
            request.method == Method::POST && is_keys(&self.nodes[node], &request.path);
        let client = if creates_key {
            self.without_bodies()
        } else {
            self.clone()
        };
        let (status, headers, body) = client
            .send_with(
                node,
                request.method,
                &request.path,
                request.query.as_deref(),
                body,
                Some(LONG_TIMEOUT),
            )
            .await?;
        Ok(RawResponse {
            status,
            headers,
            body,
            duration: start.elapsed(),
        })
    }
}
