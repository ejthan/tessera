// SPDX-License-Identifier: GPL-3.0-or-later

//! Server connections and how the app reaches the network.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Global, SharedString, Task};
use rust_i18n::t;
use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, KeyAccess, RaftState};
use tessera_core::ServerCapabilities;
use tessera_core::profile::{InvalidNode, Profile};
use tokio::runtime::Handle;
use url::Url;

use crate::runtime::AbortOnDrop;

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

/// Connects to the nodes of a profile with an API key.
pub type Connector = Arc<dyn Fn(Profile, SecretString) -> ConnectFuture + Send + Sync>;

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
    Arc::new(move |profile, key| {
        let task = AbortOnDrop::from_join_handle(handle.spawn(async move {
            let (nodes, options) =
                client_options(&profile).map_err(|e| Error::InvalidUrl(e.to_string()))?;
            let nodes: Vec<&str> = nodes.iter().map(String::as_str).collect();
            let client = Client::new(&nodes, key, options)?;
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
    /// The running attempt. Dropping the state drops it, which cancels the requests.
    pub(crate) task: Option<Task<()>>,
}

impl ConnectionState {
    /// A connection that has not been attempted yet.
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            connected: None,
            connecting: false,
            error: None,
            task: None,
        }
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

/// User-facing text for a failed connect attempt to `profile`.
pub fn error_text(err: &Error, profile: &Profile) -> String {
    let host = profile.nodes.first().map(|n| n.label()).unwrap_or_default();
    let host = host.as_str();
    match err {
        Error::InvalidUrl(_) => t!("connect.error.invalid_url").to_string(),
        Error::Network(_) => t!("connect.error.network", host = host).to_string(),
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
        let c = connect(profile, SecretString::from("k")).await.unwrap();
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
        let result = connect(profile, SecretString::from("k")).await;
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
