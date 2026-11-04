// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP client for the Typesense API.

mod error;
mod models;

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use error::Error;
pub use models::{Debug, Health, RaftState};
pub use reqwest::Method;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// Default timeout for every request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Called once per request, after the response or error.
pub type RequestHook = Arc<dyn Fn(&RequestEvent) + Send + Sync>;

/// What a [`RequestHook`] learns about a request. Never contains the API key.
#[derive(Debug, Clone)]
pub struct RequestEvent {
    /// HTTP method.
    pub method: Method,
    /// Request path including any base path prefix, for example `/health`.
    pub path: String,
    /// HTTP status, or `None` if no response arrived.
    pub status: Option<u16>,
    /// Time from sending the request to reading the whole body.
    pub duration: Duration,
}

/// A connection to one Typesense node.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    key: SecretString,
    timeout: Duration,
    hook: Option<RequestHook>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .field("key", &"••••")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Creates a client for the server at `base_url` (`http` or `https`, optional path prefix).
    pub fn new(base_url: &str, api_key: SecretString) -> Result<Self, Error> {
        let invalid = || Error::InvalidUrl(base_url.to_string());
        let mut base = Url::parse(base_url.trim()).map_err(|_| invalid())?;
        if !matches!(base.scheme(), "http" | "https") || base.host().is_none() {
            return Err(invalid());
        }
        // Without a trailing slash, `join` would replace the last path segment.
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder().build().map_err(Error::Network)?;
        Ok(Self {
            http,
            base,
            key: api_key,
            timeout: DEFAULT_TIMEOUT,
            hook: None,
        })
    }

    /// Replaces the default request timeout of [`DEFAULT_TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Installs a hook that observes every request.
    pub fn with_hook(mut self, hook: RequestHook) -> Self {
        self.hook = Some(hook);
        self
    }

    /// The normalized base URL, always ending in `/`.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// `GET /health`. A `503` with `{"ok": false}` (server starting or lagging) is not an error.
    pub async fn health(&self) -> Result<Health, Error> {
        let (status, body) = self.get("health").await?;
        if (200..300).contains(&status) {
            return serde_json::from_str(&body).map_err(Error::Decode);
        }
        if status == 503
            && let Ok(health) = serde_json::from_str::<Health>(&body)
        {
            return Ok(health);
        }
        Err(Error::http(status, body))
    }

    /// `GET /debug`: server version and Raft role.
    pub async fn debug(&self) -> Result<Debug, Error> {
        let (status, body) = self.get("debug").await?;
        if !(200..300).contains(&status) {
            return Err(Error::http(status, body));
        }
        let raw: models::RawDebug = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(Debug {
            version: raw.version,
            state: raw.state.into(),
        })
    }

    async fn get(&self, relative: &str) -> Result<(u16, String), Error> {
        let url = self
            .base
            .join(relative)
            .map_err(|_| Error::InvalidUrl(self.base.to_string()))?;
        let start = Instant::now();
        let result = async {
            let response = self
                .http
                .get(url.clone())
                .header("X-TYPESENSE-API-KEY", self.key.expose_secret())
                .timeout(self.timeout)
                .send()
                .await
                .map_err(Error::from_reqwest)?;
            let status = response.status().as_u16();
            let body = response.text().await.map_err(Error::from_reqwest)?;
            Ok((status, body))
        }
        .await;
        let event = RequestEvent {
            method: Method::GET,
            path: url.path().to_string(),
            status: result.as_ref().ok().map(|(s, _)| *s),
            duration: start.elapsed(),
        };
        tracing::debug!(
            method = %event.method,
            path = %event.path,
            status = ?event.status,
            ms = event.duration.as_millis() as u64,
            "typesense request"
        );
        if let Some(hook) = &self.hook {
            hook(&event);
        }
        result
    }
}
