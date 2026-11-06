// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP client for the Typesense API.

mod analytics;
mod collections;
mod documents;
mod error;
mod event;
mod export;
mod keys;
mod models;
mod raw;
mod search;
mod send;
mod sets;
mod stemming;
mod stopwords;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

pub use collections::{Alias, Collection, Field, SchemaChange, collection_path};
pub use documents::{
    DirtyValues, DocumentQuery, ImportAction, ImportLine, ImportParams, SearchPage, document_path,
    encode_query,
};
pub use error::Error;
pub use event::{BODY_LIMIT, Body, FULL_BODY_MAX, Origin, RequestEvent, RequestHook};
pub use export::{ExportQuery, ExportStream, MAX_LINE, STREAM_STALL};
pub use keys::{ApiKey, CreatedKey, NewKey};
pub use models::{CollectionSummary, Debug, Health, KeyAccess, Metrics, RaftState, Stats};
pub use raw::{RawBody, RawRequest, RawResponse};
pub use reqwest::Method;
pub use search::{
    GET_QUERY_LIMIT, MULTI_SEARCH_LIMIT, MultiSearch, Preset, SearchRoute, multi_route,
    search_route,
};
use secrecy::SecretString;
use serde_json::{Value, json};
pub use sets::{ItemSet, SetKind};
pub use stemming::{StemmingDictionary, StemmingWord};
pub use stopwords::StopwordSet;
use url::Url;

/// The only methods a read-only client sends. Every other one, including PUT (an upsert in
/// Typesense) and any method the raw console builds, is refused.
pub const READ_METHODS: [Method; 2] = [Method::GET, Method::HEAD];

/// Methods retried on another node after a network error or timeout (idempotent ones).
pub const FAILOVER_METHODS: [Method; 4] = [Method::GET, Method::HEAD, Method::PUT, Method::DELETE];

/// Default timeout for every request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout for schema changes, deletes and truncates, which answer only when the server is done.
/// Such a request is not retried after a timeout, only after a network error.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(30 * 60);

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
/// Clones share the active node and the read-only flag.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    nodes: Arc<[Url]>,
    active: Arc<AtomicUsize>,
    key: SecretString,
    timeout: Duration,
    retries: u32,
    read_only: Arc<AtomicBool>,
    hook: Option<RequestHook>,
    origin: Origin,
    keep_response_body: bool,
    keep_request_body: bool,
    /// Set only by `as_read_post`: a `POST` that reads, allowed when read-only.
    read_post: bool,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nodes: Vec<&str> = self.nodes.iter().map(Url::as_str).collect();
        f.debug_struct("Client")
            .field("nodes", &nodes)
            .field("active", &self.active_node())
            .field("key", &"••••")
            .field("timeout", &self.timeout)
            .field("retries", &self.retries)
            .field("read_only", &self.is_read_only())
            .finish_non_exhaustive()
    }
}

/// Parses an `http` or `https` URL (optional path prefix) and makes it end in `/`.
/// `{root}/{name}` with `name` encoded as one path segment.
fn resource_path(root: &str, name: &str) -> Result<String, Error> {
    collections::addressable(name)?;
    Ok(format!("{root}/{}", documents::segment(name)))
}

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
            retries: options.retries,
            read_only: Arc::new(AtomicBool::new(options.read_only)),
            hook: None,
            origin: Origin::User,
            keep_response_body: true,
            keep_request_body: true,
            read_post: false,
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

    /// Locks or unlocks writes, for this client and all its clones.
    pub fn set_read_only(&self, read_only: bool) {
        self.read_only.store(read_only, Ordering::Relaxed);
    }

    /// Whether writes are refused.
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    /// The same client, with its events tagged [`Origin::Polling`].
    pub fn polling(&self) -> Self {
        Self {
            origin: Origin::Polling,
            ..self.clone()
        }
    }

    /// Whether this client's events are tagged as user actions or as polling.
    pub fn origin(&self) -> Origin {
        self.origin
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
    /// An index not below `nodes().len()` is ignored.
    pub fn set_active_node(&self, index: usize) {
        if index < self.nodes.len() {
            self.active.store(index, Ordering::Relaxed);
        }
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

    /// `GET /metrics.json`: CPU, memory, disk and network of the node's host.
    pub async fn metrics(&self) -> Result<Metrics, Error> {
        self.metrics_on(None).await
    }

    /// `GET /stats.json`: request rates and latencies per endpoint.
    pub async fn stats(&self) -> Result<Stats, Error> {
        self.stats_on(None).await
    }

    /// `GET /collections?exclude_fields=fields`: name and document count of every collection.
    pub async fn collections_summary(&self) -> Result<Vec<CollectionSummary>, Error> {
        self.collections_summary_on(None).await
    }

    /// `GET /collections?limit=1&exclude_fields=fields`: proves the key may read collections,
    /// without listing every collection of a large cluster.
    pub async fn collections_probe(&self) -> Result<(), Error> {
        self.collections_probe_on(None).await
    }

    /// `GET /collections`: every collection with its full schema.
    pub async fn collections(&self) -> Result<Vec<Collection>, Error> {
        self.collections_on(None).await
    }

    /// `GET /collections/{name}`.
    pub async fn collection(&self, name: &str) -> Result<Collection, Error> {
        self.collection_on(None, name).await
    }

    /// `GET /aliases`.
    pub async fn aliases(&self) -> Result<Vec<Alias>, Error> {
        self.aliases_on(None).await
    }

    /// `GET /aliases/{name}`.
    pub async fn alias(&self, name: &str) -> Result<Alias, Error> {
        let body = self
            .get_ok(None, &resource_path("aliases", name)?, None)
            .await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `PUT /aliases/{name}`. Creates or silently repoints, and takes any target: the caller
    /// compares the answer.
    pub async fn upsert_alias(&self, name: &str, collection: &str) -> Result<Alias, Error> {
        let path = resource_path("aliases", name)?;
        let body = json!({ "collection_name": collection });
        let text = self
            .write(None, Method::PUT, &path, None, Some(&body), None)
            .await?;
        serde_json::from_str(&text).map_err(Error::Decode)
    }

    /// `DELETE /aliases/{name}`.
    pub async fn delete_alias(&self, name: &str) -> Result<(), Error> {
        let path = resource_path("aliases", name)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }

    /// `POST /collections` with `schema` as the body, sent as is.
    pub async fn create_collection(&self, schema: &Value) -> Result<Collection, Error> {
        self.create_collection_on(None, schema).await
    }

    /// `PATCH /collections/{name}` with `patch` as the body. Uses [`LONG_TIMEOUT`].
    pub async fn update_collection(&self, name: &str, patch: &Value) -> Result<(), Error> {
        self.update_collection_on(None, name, patch).await
    }

    /// `DELETE /collections/{name}`. Uses [`LONG_TIMEOUT`].
    pub async fn delete_collection(&self, name: &str) -> Result<(), Error> {
        self.delete_collection_on(None, name).await
    }

    /// `DELETE /collections/{name}/documents?truncate=true`: the number of documents deleted.
    /// Uses [`LONG_TIMEOUT`].
    pub async fn truncate_collection(&self, name: &str) -> Result<u64, Error> {
        self.truncate_collection_on(None, name).await
    }

    /// `GET /operations/schema_changes`: schema changes in progress, empty when idle.
    pub async fn schema_changes(&self) -> Result<Vec<SchemaChange>, Error> {
        self.schema_changes_on(None).await
    }

    /// `GET /keys`, to learn whether the key is an admin key. Only the status counts:
    /// `200` is [`KeyAccess::Admin`], `401` or `403` is [`KeyAccess::Restricted`].
    ///
    /// Typesense also answers `401` to a wrong key, so confirm the key first with an
    /// authenticated call such as [`Client::debug`]; otherwise a bad key reads as `Restricted`.
    pub async fn keys_probe(&self) -> Result<KeyAccess, Error> {
        self.keys_probe_on(None).await
    }

    /// Sends `method` to `path` without a body, with failover. For tests of the read-only
    /// guard and failover only; not part of the supported API.
    #[doc(hidden)]
    pub async fn request(&self, method: Method, path: &str) -> Result<(u16, String), Error> {
        self.call(None, method, path, None, None, None).await
    }

    // Endpoints are written once; `pin` is `Some` for a `NodeClient`, `None` for the cluster.

    async fn health_on(&self, pin: Option<usize>) -> Result<Health, Error> {
        let (status, body) = self.get(pin, "health", None).await?;
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
        let body = self.get_ok(pin, "debug", None).await?;
        let raw: models::RawDebug = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(Debug {
            version: raw.version,
            state: raw.state.into(),
        })
    }

    async fn metrics_on(&self, pin: Option<usize>) -> Result<Metrics, Error> {
        let body = self.get_ok(pin, "metrics.json", None).await?;
        Metrics::from_json(&body).map_err(Error::Decode)
    }

    async fn stats_on(&self, pin: Option<usize>) -> Result<Stats, Error> {
        let body = self.get_ok(pin, "stats.json", None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn collections_summary_on(
        &self,
        pin: Option<usize>,
    ) -> Result<Vec<CollectionSummary>, Error> {
        let body = self
            .get_ok(pin, "collections", Some("exclude_fields=fields"))
            .await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn collections_probe_on(&self, pin: Option<usize>) -> Result<(), Error> {
        self.get_ok(pin, "collections", Some("limit=1&exclude_fields=fields"))
            .await
            .map(drop)
    }

    async fn collections_on(&self, pin: Option<usize>) -> Result<Vec<Collection>, Error> {
        let body = self.get_ok(pin, "collections", None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn collection_on(&self, pin: Option<usize>, name: &str) -> Result<Collection, Error> {
        let body = self
            .get_ok(pin, &collections::collection_path(name, "")?, None)
            .await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn aliases_on(&self, pin: Option<usize>) -> Result<Vec<Alias>, Error> {
        let body = self.get_ok(pin, "aliases", None).await?;
        let list: collections::AliasList = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(list.aliases)
    }

    async fn create_collection_on(
        &self,
        pin: Option<usize>,
        schema: &Value,
    ) -> Result<Collection, Error> {
        let body = self
            .write(pin, Method::POST, "collections", None, Some(schema), None)
            .await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn update_collection_on(
        &self,
        pin: Option<usize>,
        name: &str,
        patch: &Value,
    ) -> Result<(), Error> {
        let path = collections::collection_path(name, "")?;
        let long = Some(LONG_TIMEOUT);
        self.write(pin, Method::PATCH, &path, None, Some(patch), long)
            .await
            .map(drop)
    }

    async fn delete_collection_on(&self, pin: Option<usize>, name: &str) -> Result<(), Error> {
        let path = collections::collection_path(name, "")?;
        let long = Some(LONG_TIMEOUT);
        self.write(pin, Method::DELETE, &path, None, None, long)
            .await
            .map(drop)
    }

    async fn truncate_collection_on(&self, pin: Option<usize>, name: &str) -> Result<u64, Error> {
        #[derive(serde::Deserialize)]
        struct Deleted {
            num_deleted: u64,
        }
        let path = collections::collection_path(name, "/documents")?;
        let long = Some(LONG_TIMEOUT);
        let query = Some("truncate=true");
        let body = self
            .write(pin, Method::DELETE, &path, query, None, long)
            .await?;
        let deleted: Deleted = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(deleted.num_deleted)
    }

    async fn schema_changes_on(&self, pin: Option<usize>) -> Result<Vec<SchemaChange>, Error> {
        let body = self.get_ok(pin, "operations/schema_changes", None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn keys_probe_on(&self, pin: Option<usize>) -> Result<KeyAccess, Error> {
        match self.get(pin, "keys", None).await? {
            (200, _) => Ok(KeyAccess::Admin),
            (401 | 403, _) => Ok(KeyAccess::Restricted),
            (status, body) => Err(Error::http(status, body)),
        }
    }

    async fn get(
        &self,
        pin: Option<usize>,
        path: &str,
        query: Option<&str>,
    ) -> Result<(u16, String), Error> {
        self.call(pin, Method::GET, path, query, None, None).await
    }

    /// The body of a `2xx` answer to a request with an optional JSON body and timeout.
    async fn write(
        &self,
        pin: Option<usize>,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<String, Error> {
        match self.call(pin, method, path, query, body, timeout).await? {
            (200..300, body) => Ok(body),
            (status, body) => Err(Error::http(status, body)),
        }
    }

    /// The body of a `2xx` answer; any other status is [`Error::Http`].
    async fn get_ok(
        &self,
        pin: Option<usize>,
        path: &str,
        query: Option<&str>,
    ) -> Result<String, Error> {
        match self.get(pin, path, query).await? {
            (200..300, body) => Ok(body),
            (status, body) => Err(Error::http(status, body)),
        }
    }

    /// Sends to node `pin`, or to the active node with failover (spec §5.2): after a network
    /// error or timeout (a network error only when `timeout` overrides the profile's, as for
    /// the [`LONG_TIMEOUT`] calls), a [`FAILOVER_METHODS`] request makes up to `retries` more attempts,
    /// cycling through the nodes in list order from the active one (a single node retries
    /// itself), waiting [`backoff`] before each. The node that answers becomes the active
    /// node, unless someone changed the active node meanwhile.
    async fn call(
        &self,
        pin: Option<usize>,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<(u16, String), Error> {
        if let Some(node) = pin {
            return self.send(node, method, path, query, body, timeout).await;
        }
        let start = self.active_node();
        let retries = if FAILOVER_METHODS.contains(&method) || self.read_post {
            self.retries as usize
        } else {
            0
        };
        let attempts = 1 + retries;
        let mut attempt = 0;
        loop {
            let node = (start + attempt) % self.nodes.len();
            match self
                .send(node, method.clone(), path, query, body, timeout)
                .await
            {
                // A long operation (`timeout` set) that timed out is not retried: the server
                // may still be applying it, and another attempt would wait that long again.
                Err(Error::Network(_)) if attempt + 1 < attempts => {
                    tokio::time::sleep(backoff(attempt)).await;
                    attempt += 1;
                }
                Err(Error::Timeout) if timeout.is_none() && attempt + 1 < attempts => {
                    tokio::time::sleep(backoff(attempt)).await;
                    attempt += 1;
                }
                result => {
                    // Only from `start`, so a concurrent set_active_node() is not undone by a
                    // request that started earlier.
                    if result.is_ok() {
                        let _ = self.active.compare_exchange(
                            start,
                            node,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        );
                    }
                    return result;
                }
            }
        }
    }
}

/// Wait before failover retry `attempt` (0-based): 100 ms × 2ⁿ, capped at 3.2 s because
/// `retries` comes unchecked from the profile.
fn backoff(attempt: usize) -> Duration {
    Duration::from_millis(100) * (1 << attempt.min(5))
}

/// One node of a [`Client`]. Same endpoints, never fails over, so a dead node looks dead.
#[derive(Debug, Clone)]
pub struct NodeClient {
    client: Client,
    index: usize,
}

impl NodeClient {
    /// This node's base URL, always ending in `/`.
    pub fn url(&self) -> &Url {
        &self.client.nodes[self.index]
    }

    /// `GET /health` on this node. See [`Client::health`].
    pub async fn health(&self) -> Result<Health, Error> {
        self.client.health_on(Some(self.index)).await
    }

    /// `GET /debug` on this node. See [`Client::debug`].
    pub async fn debug(&self) -> Result<Debug, Error> {
        self.client.debug_on(Some(self.index)).await
    }

    /// `GET /metrics.json` on this node. See [`Client::metrics`].
    pub async fn metrics(&self) -> Result<Metrics, Error> {
        self.client.metrics_on(Some(self.index)).await
    }

    /// `GET /stats.json` on this node. See [`Client::stats`].
    pub async fn stats(&self) -> Result<Stats, Error> {
        self.client.stats_on(Some(self.index)).await
    }

    /// `GET /collections?exclude_fields=fields` on this node. See [`Client::collections_summary`].
    pub async fn collections_summary(&self) -> Result<Vec<CollectionSummary>, Error> {
        self.client.collections_summary_on(Some(self.index)).await
    }

    /// `GET /collections?limit=1&exclude_fields=fields` on this node.
    /// See [`Client::collections_probe`].
    pub async fn collections_probe(&self) -> Result<(), Error> {
        self.client.collections_probe_on(Some(self.index)).await
    }

    /// `GET /collections` on this node. See [`Client::collections`].
    pub async fn collections(&self) -> Result<Vec<Collection>, Error> {
        self.client.collections_on(Some(self.index)).await
    }

    /// `GET /collections/{name}` on this node. See [`Client::collection`].
    pub async fn collection(&self, name: &str) -> Result<Collection, Error> {
        self.client.collection_on(Some(self.index), name).await
    }

    /// `GET /aliases` on this node. See [`Client::aliases`].
    pub async fn aliases(&self) -> Result<Vec<Alias>, Error> {
        self.client.aliases_on(Some(self.index)).await
    }

    /// `GET /keys` on this node. See [`Client::keys_probe`].
    pub async fn keys_probe(&self) -> Result<KeyAccess, Error> {
        self.client.keys_probe_on(Some(self.index)).await
    }

    /// `POST /collections` on this node. See [`Client::create_collection`].
    pub async fn create_collection(&self, schema: &Value) -> Result<Collection, Error> {
        self.client
            .create_collection_on(Some(self.index), schema)
            .await
    }

    /// `PATCH /collections/{name}` on this node. See [`Client::update_collection`].
    pub async fn update_collection(&self, name: &str, patch: &Value) -> Result<(), Error> {
        self.client
            .update_collection_on(Some(self.index), name, patch)
            .await
    }

    /// `DELETE /collections/{name}` on this node. See [`Client::delete_collection`].
    pub async fn delete_collection(&self, name: &str) -> Result<(), Error> {
        self.client
            .delete_collection_on(Some(self.index), name)
            .await
    }

    /// `DELETE /collections/{name}/documents?truncate=true` on this node.
    /// See [`Client::truncate_collection`].
    pub async fn truncate_collection(&self, name: &str) -> Result<u64, Error> {
        self.client
            .truncate_collection_on(Some(self.index), name)
            .await
    }

    /// `GET /operations/schema_changes` on this node. See [`Client::schema_changes`].
    pub async fn schema_changes(&self) -> Result<Vec<SchemaChange>, Error> {
        self.client.schema_changes_on(Some(self.index)).await
    }

    /// Sends `method` to `path` on this node only. For tests of the read-only guard only;
    /// not part of the supported API.
    #[doc(hidden)]
    pub async fn request(&self, method: Method, path: &str) -> Result<(u16, String), Error> {
        self.client
            .call(Some(self.index), method, path, None, None, None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_of_a_body_prints_lengths_only() {
        let body = event::Body::new(r#"{"value":"sk-secret-body"}"#);
        let shown = format!("{body:?}");
        assert!(!shown.contains("sk-secret-body"), "{shown}");
        assert!(shown.contains("original_len: 26"), "{shown}");
    }

    #[tokio::test]
    async fn long_request_timeout_is_not_retried() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::new(
            &[&server.uri()],
            SecretString::from("k"),
            ClientOptions::default(),
        )
        .unwrap();
        // A per-request timeout marks a long operation: the server may still be working on
        // it, so a retry would only wait again (or repeat the write).
        let long = Some(Duration::from_millis(100));
        let result = client
            .call(None, Method::DELETE, "collections/c", None, None, long)
            .await;
        assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    }

    #[test]
    fn backoff_doubles_and_caps_at_3_2_s() {
        let waits: Vec<u64> = (0..8).map(|n| backoff(n).as_millis() as u64).collect();
        assert_eq!(waits, [100, 200, 400, 800, 1600, 3200, 3200, 3200]);
        assert_eq!(backoff(usize::MAX), Duration::from_millis(3200));
    }
}
