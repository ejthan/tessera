// SPDX-License-Identifier: GPL-3.0-or-later

//! Server connections and how the app reaches the network.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{App, Context, EventEmitter, Global, SharedString, Task, Window};
use rust_i18n::t;
use secrecy::SecretString;
use tessera_client::{
    Client, ClientOptions, CollectionSummary, Error, KeyAccess, RaftState, RequestHook, Stats,
};
use tessera_core::ServerCapabilities;
use tessera_core::metrics::MetricsHistory;
use tessera_core::profile::{InvalidNode, Profile};
use tokio::runtime::Handle;
use url::Url;

use crate::collections::{CollectionsBackendGlobal, CollectionsCache};
use crate::poller::{CollectionsFetcherGlobal, DEFAULT_INTERVAL, HISTORY_WINDOW, NodeStatus};
use crate::runtime::AbortOnDrop;
use crate::views::unlock;

/// What Tessera learned about a server while connecting.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// Normalized base URL.
    pub url: Url,
    /// Version exactly as the server reports it, for example `"30.2"`.
    pub version: String,
    /// Raft role of the node.
    pub state: RaftState,
    /// Result of `GET /health`.
    pub healthy: bool,
    /// Features derived from the version.
    pub capabilities: ServerCapabilities,
    /// Duration of the `GET /debug` request.
    pub latency: Duration,
}

impl ServerInfo {
    /// `host:port` for display.
    pub fn host_label(&self) -> String {
        host_label(self.url.as_str())
    }
}

/// A successful connect: the client to keep using and what the server told us.
#[derive(Debug, Clone)]
pub struct Connected {
    /// Client built from the profile, with the key.
    pub client: Client,
    /// Version, role, health and latency.
    pub info: ServerInfo,
    /// Whether the key may list API keys.
    pub access: KeyAccess,
}

/// Future returned by a [`Connector`].
pub type ConnectFuture = Pin<Box<dyn Future<Output = Result<Connected, Error>> + Send>>;

/// Connects to the nodes of a profile with an API key; `RequestHook` sees every request.
pub type Connector = Arc<dyn Fn(Profile, SecretString, RequestHook) -> ConnectFuture + Send + Sync>;

/// The connector the app uses. The app installs [`tokio_connector`]; UI tests install fakes,
/// because GPUI's test executor does not wait for real network I/O.
pub struct ConnectorGlobal(pub Connector);

impl Global for ConnectorGlobal {}

/// Highest retry count passed to the client; an unchecked huge value would hang requests.
const MAX_RETRIES: u32 = 10;
/// Highest timeout passed to the client, in seconds.
const MAX_TIMEOUT_SECS: u32 = 300;

/// Node URLs and client options for `profile`.
pub fn client_options(profile: &Profile) -> Result<(Vec<String>, ClientOptions), InvalidNode> {
    let nodes = profile
        .nodes
        .iter()
        .map(|n| n.url().map(String::from))
        .collect::<Result<Vec<_>, _>>()?;
    let nearest_node = profile
        .nearest_node
        .as_ref()
        .map(|n| n.url().map(String::from))
        .transpose()?;
    let options = ClientOptions {
        timeout: Duration::from_secs(profile.timeout_secs.clamp(1, MAX_TIMEOUT_SECS).into()),
        retries: profile.retries.min(MAX_RETRIES),
        nearest_node,
        ca_file: profile.tls.ca_file.clone(),
        allow_insecure: profile.tls.allow_insecure,
        read_only: profile.read_only,
    };
    Ok((nodes, options))
}

/// The real connector, on the tokio runtime: `GET /health`, `GET /debug` (which also proves
/// the key is valid), `GET /collections?limit=1&exclude_fields=fields` and the `GET /keys`
/// probe. `AppState::connect` checks the nodes first, so `InvalidUrl` here is a fallback.
pub fn tokio_connector(handle: Handle) -> Connector {
    Arc::new(move |profile, key, hook| {
        let task = AbortOnDrop::from_join_handle(handle.spawn(async move {
            let (nodes, options) =
                client_options(&profile).map_err(|e| Error::InvalidUrl(e.to_string()))?;
            let nodes: Vec<&str> = nodes.iter().map(String::as_str).collect();
            let client = Client::new(&nodes, key, options)?.with_hook(hook);
            let health = client.health().await?;
            let start = Instant::now();
            let debug = client.debug().await?;
            let latency = start.elapsed();
            client.collections_probe().await?;
            // Only after `debug` succeeded: a wrong key would also read as restricted.
            let access = client.keys_probe().await?;
            Ok(Connected {
                info: ServerInfo {
                    url: client.base_url().clone(),
                    capabilities: ServerCapabilities::from_version(&debug.version),
                    version: debug.version,
                    state: debug.state,
                    healthy: health.ok,
                    latency,
                },
                access,
                client,
            })
        }));
        Box::pin(async move {
            match task.await {
                Ok(result) => result,
                // The task is only aborted when this future is dropped, so a JoinError here
                // is always a panic inside the task.
                Err(e) => std::panic::resume_unwind(e.into_panic()),
            }
        })
    })
}

/// One connection of the window: the profile, and the client once connected.
pub struct ConnectionState {
    /// The saved or temporary profile.
    pub profile: Profile,
    /// `None` until a connect attempt succeeds.
    pub connected: Option<Connected>,
    /// A connect attempt is running.
    pub connecting: bool,
    /// Full text of the last failed attempt.
    pub error: Option<SharedString>,
    /// The last attempt failed because no key is stored.
    pub missing_key: bool,
    /// The running attempt. Dropping the state drops it, which cancels the requests.
    pub(crate) task: Option<Task<()>>,
    /// Polled status per node, indexed like `Client::nodes`.
    pub nodes: Vec<NodeStatus>,
    /// `/metrics.json` history per node, indexed like `nodes`. Kept while the tab is closed.
    pub history: Vec<MetricsHistory>,
    /// The active node's last `/stats.json`.
    pub stats: Option<Stats>,
    /// Collections and document counts; loaded when an overview opens and on refresh.
    pub collections: Option<Vec<CollectionSummary>>,
    /// Full text of the last failed collections load.
    pub collections_error: Option<SharedString>,
    /// Writes are unlocked for this session on a read-only profile. Never saved.
    pub writes_unlocked: bool,
    /// The connection was closed by the user; a dialog still open for it does nothing.
    pub closed: bool,
    /// Polling is paused by the user.
    pub paused: bool,
    /// Time between two polls of a node, kept for the session.
    pub interval: Duration,
    /// Collections with schemas and aliases, shared by the collection views.
    pub collection_cache: CollectionsCache,
    /// A truncate or delete dialog is loading its numbers.
    pub danger_loading: bool,
    collections_task: Option<Task<()>>,
    cache_task: Option<Task<()>>,
}

/// The schema of the named collection changed on the server; views showing it reload.
pub struct CollectionChanged(pub String);

impl EventEmitter<CollectionChanged> for ConnectionState {}

impl ConnectionState {
    /// A connection that has not been attempted yet.
    pub fn new(profile: Profile) -> Self {
        Self {
            connected: None,
            connecting: false,
            error: None,
            missing_key: false,
            task: None,
            nodes: vec![NodeStatus::default(); profile.nodes.len()],
            history: (0..profile.nodes.len())
                .map(|_| MetricsHistory::new(HISTORY_WINDOW))
                .collect(),
            stats: None,
            collections: None,
            collections_error: None,
            writes_unlocked: false,
            closed: false,
            paused: false,
            interval: DEFAULT_INTERVAL,
            collections_task: None,
            collection_cache: CollectionsCache::default(),
            danger_loading: false,
            cache_task: None,
            profile,
        }
    }

    /// Runs `then` after this update on a writable connection (deferred, so `then` may read
    /// or update the connection); on a read-only one it first opens the unlock dialog and runs
    /// `then` after the unlock, once the dialog has closed.
    pub fn ensure_writable(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Window, &mut App) + 'static,
    ) {
        if self.writes_locked() {
            unlock::open(cx.entity().downgrade(), &self.profile, then, window, cx);
        } else {
            window.defer(cx, then);
        }
    }

    /// A write first asks to unlock: the profile is read-only and writes are not unlocked.
    /// Write controls show the lock then (spec §7.3).
    pub fn writes_locked(&self) -> bool {
        self.profile.read_only && !self.writes_unlocked
    }

    /// Allows writes until [`Self::lock`] or disconnect. Nothing is written to disk.
    pub fn unlock(&mut self, cx: &mut Context<Self>) {
        self.set_locked(false, cx);
    }

    /// Restores the profile's read-only guard.
    pub fn lock(&mut self, cx: &mut Context<Self>) {
        self.set_locked(true, cx);
    }

    fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        // Unlocking needs the client whose guard it lifts.
        if !self.profile.read_only || (!locked && self.connected.is_none()) {
            return;
        }
        self.writes_unlocked = !locked;
        if let Some(c) = &self.connected {
            c.client.set_read_only(locked);
        }
        cx.notify();
    }

    /// Pauses or resumes polling. Resuming polls right away.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        cx.notify();
    }

    /// Sets the polling interval (at least 1 s) for the rest of the session and polls right away.
    pub fn set_interval(&mut self, interval: Duration, cx: &mut Context<Self>) {
        self.interval = interval.max(Duration::from_secs(1));
        cx.notify();
    }

    /// Sends further requests to node `index`. Its stats show from its next poll on.
    pub fn set_active_node(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(connected) = &self.connected else {
            return;
        };
        if index < connected.client.nodes().len() && index != connected.client.active_node() {
            connected.client.set_active_node(index);
            self.stats = None;
            cx.notify();
        }
    }

    /// Reloads the collections as a user action (⌘R / Ctrl+R).
    pub fn refresh_collections(&mut self, cx: &mut Context<Self>) {
        self.load_collections(false, cx);
    }

    /// Loads collections and aliases into [`Self::collection_cache`]. A load still running is
    /// cancelled. The old list stays visible until the new one arrives; a failure keeps it
    /// and sets the error.
    pub fn load_collection_list(&mut self, cx: &mut Context<Self>) {
        let Some(connected) = &self.connected else {
            return;
        };
        let load = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .list(connected.client.clone());
        self.collection_cache.loading = true;
        self.cache_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |state, cx| {
                let cache = &mut state.collection_cache;
                cache.loading = false;
                match result {
                    Ok((collections, aliases)) => {
                        cache.collections = Some(collections);
                        let profile = &state.profile;
                        cache.aliases_error = aliases
                            .as_ref()
                            .err()
                            .map(|e| error_text(e, profile).into());
                        cache.aliases = aliases.unwrap_or_default();
                        cache.loaded_at = Some(Instant::now());
                        cache.error = None;
                    }
                    Err(e) => cache.error = Some(error_text(&e, &state.profile).into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Drops the cached list (and a running load) and loads it again.
    pub fn invalidate_collections(&mut self, cx: &mut Context<Self>) {
        self.collection_cache = CollectionsCache::default();
        self.cache_task = None;
        self.load_collection_list(cx);
    }

    /// Reloads the collection list and tells the views of collection `name` to reload.
    pub fn collection_changed(&mut self, name: String, cx: &mut Context<Self>) {
        self.invalidate_collections(cx);
        cx.emit(CollectionChanged(name));
    }

    /// Loads the collections through the polling client (`polling`) or the user's client.
    /// A load still running is cancelled.
    pub(crate) fn load_collections(&mut self, polling: bool, cx: &mut Context<Self>) {
        let Some(connected) = &self.connected else {
            return;
        };
        let client = if polling {
            connected.client.polling()
        } else {
            connected.client.clone()
        };
        let load = (cx.global::<CollectionsFetcherGlobal>().0)(client);
        self.collections_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |state, cx| {
                match result {
                    Ok(collections) => {
                        state.collections = Some(collections);
                        state.collections_error = None;
                    }
                    Err(e) => {
                        state.collections_error = Some(error_text(&e, &state.profile).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

/// `host:port` of a URL, or the input itself if it does not parse.
pub fn host_label(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_string();
            Some(match u.port_or_known_default() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| url.to_string())
}

/// `e` and its sources joined by ": ", e.g. down to "Connection refused". reqwest errors
/// name the URL, never a header, so the key cannot appear.
fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    std::iter::successors(Some(e), |e| e.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

/// User-facing text for a failed connect attempt to `profile`.
pub fn error_text(err: &Error, profile: &Profile) -> String {
    let host = profile.nodes.first().map(|n| n.label()).unwrap_or_default();
    node_error_text(err, &host, profile)
}

/// User-facing text for a failed schema change, truncate or delete of `profile`: these wait
/// [`tessera_client::LONG_TIMEOUT`], and the server may still be working after it.
pub fn long_error_text(err: &Error, profile: &Profile) -> String {
    match err {
        Error::Timeout => {
            let host = profile.nodes.first().map(|n| n.label()).unwrap_or_default();
            let minutes = tessera_client::LONG_TIMEOUT.as_secs() / 60;
            t!("connect.error.long_timeout", host = host, minutes = minutes).to_string()
        }
        _ => error_text(err, profile),
    }
}

/// User-facing text for a failed request to `host` (`host:port`) of `profile`.
pub fn node_error_text(err: &Error, host: &str, profile: &Profile) -> String {
    match err {
        Error::InvalidUrl(_) => t!("connect.error.invalid_url").to_string(),
        Error::Network(e) => {
            t!("connect.error.network", host = host, detail = chain(e)).to_string()
        }
        Error::Timeout => t!(
            "connect.error.timeout",
            host = host,
            seconds = profile.timeout_secs.clamp(1, MAX_TIMEOUT_SECS)
        )
        .to_string(),
        Error::Http { status, .. } => match err.typesense_message() {
            Some(message) => {
                t!("connect.error.http", status = status, message = message).to_string()
            }
            None => t!("connect.error.not_typesense", host = host, status = status).to_string(),
        },
        Error::Decode(_) => t!("connect.error.decode", host = host).to_string(),
        Error::Tls(detail) => t!("connect.error.tls", host = host, detail = detail).to_string(),
        Error::ReadOnly { method } => t!("connect.error.read_only", method = method).to_string(),
        Error::InvalidKey => t!("connect.error.invalid_key").to_string(),
        Error::UnaddressableId(id) => t!("connect.error.unaddressable_id", id = id).to_string(),
        Error::LineTooLong { limit } => {
            t!("connect.error.line_too_long", limit = limit).to_string()
        }
        Error::Stream(detail) => t!("connect.error.stream", detail = detail).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tessera_core::profile::{Node, Protocol};

    fn node(host: &str, port: u16) -> Node {
        Node {
            protocol: Protocol::Http,
            host: host.into(),
            port,
            path: String::new(),
        }
    }

    #[tokio::test]
    async fn tokio_connector_builds_connection() {
        use wiremock::matchers::{path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        for (route, status, body) in [
            ("/health", 200, r#"{"ok":true}"#),
            ("/debug", 200, r#"{"state":1,"version":"30.2"}"#),
            ("/keys", 401, r#"{"message":"Forbidden"}"#),
        ] {
            Mock::given(path(route))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(path("/collections"))
            .and(query_param("limit", "1"))
            .and(query_param("exclude_fields", "fields"))
            .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
            .expect(1)
            .mount(&server)
            .await;
        let addr = server.address();
        let profile = Profile::new("t", vec![node(&addr.ip().to_string(), addr.port())]);
        let connect = tokio_connector(Handle::current());
        let c = connect(profile, SecretString::from("k"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert_eq!(c.info.version, "30.2");
        assert_eq!(c.info.state, RaftState::Leader);
        assert!(c.info.healthy);
        assert!(c.info.capabilities.supported);
        assert_eq!(c.info.url.as_str(), format!("{}/", server.uri()));
        assert_eq!(c.access, KeyAccess::Restricted);
    }

    #[tokio::test]
    async fn invalid_node_is_a_connect_error() {
        let connect = tokio_connector(Handle::current());
        let profile = Profile::new("t", vec![node("ts.example.com/x", 8108)]);
        let result = connect(profile, SecretString::from("k"), Arc::new(|_| {})).await;
        assert!(matches!(result, Err(Error::InvalidUrl(_))));
    }

    #[test]
    fn client_options_follow_the_profile() {
        let mut profile = Profile::new("t", vec![node("::1", 8108), node("b", 1)]);
        profile.retries = u32::MAX;
        profile.timeout_secs = 12;
        profile.read_only = true;
        profile.nearest_node = Some(node("near", 8108));
        profile.tls.allow_insecure = true;
        let (nodes, options) = client_options(&profile).unwrap();
        assert_eq!(nodes, ["http://[::1]:8108/", "http://b:1/"]);
        assert_eq!(options.retries, MAX_RETRIES);
        assert_eq!(options.timeout, Duration::from_secs(12));
        assert_eq!(options.nearest_node.as_deref(), Some("http://near:8108/"));
        assert!(options.read_only && options.allow_insecure);
        profile.nearest_node = Some(node("bad host", 1));
        assert!(client_options(&profile).is_err());
    }

    #[test]
    fn non_typesense_body_is_not_shown_as_typesense_message() {
        rust_i18n::set_locale("en");
        let html = "<!DOCTYPE html><html><body>404 Not Found</body></html>".to_string();
        let err = Error::Http {
            status: 404,
            message: html.clone(),
            body: html,
        };
        let profile = Profile::new("t", vec![node("example.com", 443)]);
        assert_eq!(
            error_text(&err, &profile),
            "example.com:443 answered HTTP 404, which does not look like Typesense. Check the URL and path prefix."
        );
    }

    #[tokio::test]
    async fn network_error_text_names_the_cause() {
        rust_i18n::set_locale("en");
        let client = tessera_client::Client::new(
            &["http://127.0.0.1:9"],
            SecretString::from("k-secret-1234"),
            tessera_client::ClientOptions {
                retries: 0,
                ..Default::default()
            },
        )
        .unwrap();
        let err = client.health().await.unwrap_err();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
        let profile = Profile::new("t", vec![node("127.0.0.1", 9)]);
        let text = error_text(&err, &profile);
        assert!(text.starts_with("Could not reach 127.0.0.1:9: "), "{text}");
        // The cause chain down to the refused connection, without the key.
        assert!(
            text.contains("/health") && text.contains("connect"),
            "{text}"
        );
        assert!(!text.contains("k-secret"), "{text}");
    }

    #[test]
    fn long_operation_timeout_names_the_long_timeout() {
        rust_i18n::set_locale("en");
        let profile = Profile::new("t", vec![node("localhost", 8108)]);
        assert_eq!(
            long_error_text(&Error::Timeout, &profile),
            "localhost:8108 did not finish within 30 minutes. The server may still be applying the change; reload to see its current state."
        );
        let other = Error::InvalidKey;
        assert_eq!(
            long_error_text(&other, &profile),
            error_text(&other, &profile)
        );
    }

    #[test]
    fn invalid_key_text_does_not_show_the_key() {
        rust_i18n::set_locale("en");
        let profile = Profile::new("t", vec![node("localhost", 8108)]);
        assert_eq!(
            error_text(&Error::InvalidKey, &profile),
            "The API key contains a character that cannot be sent, such as a line break. Enter the key again."
        );
    }

    #[test]
    fn host_label_adds_default_port() {
        assert_eq!(host_label("http://localhost:8108/"), "localhost:8108");
        assert_eq!(host_label("https://example.com/ts/"), "example.com:443");
        assert_eq!(host_label("localhost:8108"), "localhost:8108");
    }

    #[test]
    fn http_error_text_keeps_full_message() {
        rust_i18n::set_locale("en");
        let err = Error::Http {
            status: 401,
            message: "Forbidden - a valid `x-typesense-api-key` header must be sent.".into(),
            body: String::new(),
        };
        let profile = Profile::new("t", vec![node("localhost", 8108)]);
        assert_eq!(
            error_text(&err, &profile),
            "Typesense answered 401: Forbidden - a valid `x-typesense-api-key` header must be sent."
        );
    }
}
