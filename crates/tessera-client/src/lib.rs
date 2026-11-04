// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP client for the Typesense API.

mod error;
mod event;
mod models;
mod send;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub use error::Error;
pub use event::{BODY_LIMIT, Body, Origin, RequestEvent, RequestHook};
pub use models::{Debug, Health, RaftState};
pub use reqwest::Method;
use secrecy::SecretString;
use url::Url;

/// Default timeout for every request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How a [`Client`] talks to its nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOptions {
    /// Timeout for each request.
    pub timeout: Duration,
    /// Extra attempts on other nodes after a network error or timeout.
    pub retries: u32,
    /// URL of the node to use first. If it is not among the nodes, it is added in front.
    pub nearest_node: Option<String>,
    /// PEM file with extra root certificates, trusted in addition to the system's.
    pub ca_file: Option<PathBuf>,
    /// Accept any server certificate. Dangerous.
    pub allow_insecure: bool,
    /// Refuse every request that would write.
    pub read_only: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            retries: 2,
            nearest_node: None,
            ca_file: None,
            allow_insecure: false,
            read_only: false,
        }
    }
}

/// A connection to a Typesense cluster of one or more nodes.
///
/// Clones share the active node.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    nodes: Arc<[Url]>,
    active: Arc<AtomicUsize>,
    key: SecretString,
    timeout: Duration,
    hook: Option<RequestHook>,
    origin: Origin,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nodes: Vec<&str> = self.nodes.iter().map(Url::as_str).collect();
        f.debug_struct("Client")
            .field("nodes", &nodes)
            .field("active", &self.active_node())
            .field("key", &"••••")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// Parses an `http` or `https` URL (optional path prefix) and makes it end in `/`.
fn parse_node(input: &str) -> Result<Url, Error> {
    let invalid = || Error::InvalidUrl(input.to_string());
    let mut url = Url::parse(input.trim()).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(invalid());
    }
    // Without a trailing slash, `join` would replace the last path segment.
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn read_ca_file(path: &std::path::Path) -> Result<Vec<reqwest::Certificate>, Error> {
    let fail = |why: String| Error::Tls(format!("CA file {}: {why}", path.display()));
    let pem = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| fail(e.to_string()))?;
    if certs.is_empty() {
        return Err(fail("no PEM certificate found".into()));
    }
    Ok(certs)
}

impl Client {
    /// Creates a client for `nodes` (each `http` or `https`, optional path prefix).
    ///
    /// Reads `options.ca_file` once, here. An empty `nodes` is [`Error::InvalidUrl`].
    pub fn new(nodes: &[&str], key: SecretString, options: ClientOptions) -> Result<Self, Error> {
        let mut urls = nodes
            .iter()
            .map(|n| parse_node(n))
            .collect::<Result<Vec<_>, _>>()?;
        if urls.is_empty() {
            return Err(Error::InvalidUrl(String::new()));
        }
        let mut active = 0;
        if let Some(nearest) = &options.nearest_node {
            let nearest = parse_node(nearest)?;
            active = match urls.iter().position(|u| *u == nearest) {
                Some(index) => index,
                None => {
                    urls.insert(0, nearest);
                    0
                }
            };
        }
        // Typesense never redirects. Following one would send X-TYPESENSE-API-KEY to
        // whatever host the redirect names (reqwest strips only Authorization and cookies),
        // for example an SSO proxy's login page. A 3xx surfaces as Error::Http instead.
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .tls_danger_accept_invalid_certs(options.allow_insecure);
        if let Some(path) = &options.ca_file {
            builder = builder.tls_certs_merge(read_ca_file(path)?);
        }
        let http = builder.build().map_err(Error::Network)?;
        Ok(Self {
            http,
            nodes: urls.into(),
            active: Arc::new(AtomicUsize::new(active)),
            key,
            timeout: options.timeout,
            hook: None,
            origin: Origin::User,
        })
    }

    /// Replaces the request timeout from [`ClientOptions::timeout`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Installs a hook that observes every request.
    pub fn with_hook(mut self, hook: RequestHook) -> Self {
        self.hook = Some(hook);
        self
    }

    /// The same client, with its events tagged [`Origin::Polling`].
    pub fn polling(&self) -> Self {
        Self {
            origin: Origin::Polling,
            ..self.clone()
        }
    }

    /// All nodes, normalized to end in `/`. A nearest node not among `nodes` comes first.
    pub fn nodes(&self) -> &[Url] {
        &self.nodes
    }

    /// Index into [`Client::nodes`] of the node requests go to.
    pub fn active_node(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Makes `index` the active node, for this client and all its clones.
    ///
    /// # Panics
    ///
    /// If `index` is not below `nodes().len()`.
    pub fn set_active_node(&self, index: usize) {
        assert!(index < self.nodes.len(), "node index {index} out of range");
        self.active.store(index, Ordering::Relaxed);
    }

    /// A client for node `index` alone. It never fails over.
    ///
    /// # Panics
    ///
    /// If `index` is not below `nodes().len()`.
    pub fn node(&self, index: usize) -> NodeClient {
        assert!(index < self.nodes.len(), "node index {index} out of range");
        NodeClient {
            client: self.clone(),
            index,
        }
    }

    /// The active node's base URL, always ending in `/`.
    pub fn base_url(&self) -> &Url {
        &self.nodes[self.active_node()]
    }

    /// `GET /health`. A `503` with `{"ok": false}` (server starting or lagging) is not an error.
    pub async fn health(&self) -> Result<Health, Error> {
        self.health_on(None).await
    }

    /// `GET /debug`: server version and Raft role.
    pub async fn debug(&self) -> Result<Debug, Error> {
        self.debug_on(None).await
    }

    // Endpoints are written once; `pin` is `Some` for a `NodeClient`, `None` for the cluster.

    async fn health_on(&self, pin: Option<usize>) -> Result<Health, Error> {
        let (status, body) = self.get(pin, "health").await?;
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

    async fn debug_on(&self, pin: Option<usize>) -> Result<Debug, Error> {
        let (status, body) = self.get(pin, "debug").await?;
        if !(200..300).contains(&status) {
            return Err(Error::http(status, body));
        }
        let raw: models::RawDebug = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(Debug {
            version: raw.version,
            state: raw.state.into(),
        })
    }

    async fn get(&self, pin: Option<usize>, path: &str) -> Result<(u16, String), Error> {
        // Task 7: failover over the other nodes goes here when `pin` is `None`.
        let node = pin.unwrap_or_else(|| self.active_node());
        self.send(node, Method::GET, path, None, None).await
    }
}

/// One node of a [`Client`]. Same endpoints, never fails over, so a dead node looks dead.
#[derive(Debug, Clone)]
pub struct NodeClient {
    client: Client,
    index: usize,
}

impl NodeClient {
    /// `GET /health` on this node. See [`Client::health`].
    pub async fn health(&self) -> Result<Health, Error> {
        self.client.health_on(Some(self.index)).await
    }

    /// `GET /debug` on this node. See [`Client::debug`].
    pub async fn debug(&self) -> Result<Debug, Error> {
        self.client.debug_on(Some(self.index)).await
    }
}
