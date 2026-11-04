// SPDX-License-Identifier: GPL-3.0-or-later

//! The active server connection and how the app reaches the network.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Context, Global};
use rust_i18n::t;
use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, RaftState};
use tessera_core::ServerCapabilities;
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

/// Future returned by a [`Connector`].
pub type ConnectFuture = Pin<Box<dyn Future<Output = Result<ServerInfo, Error>> + Send>>;

/// Connects to the server at a URL with an API key.
pub type Connector = Arc<dyn Fn(String, SecretString) -> ConnectFuture + Send + Sync>;

/// The connector views use. The app installs [`tokio_connector`]; UI tests install fakes,
/// because GPUI's test executor does not wait for real network I/O.
pub struct ConnectorGlobal(pub Connector);

impl Global for ConnectorGlobal {}

/// The real connector: `GET /health`, then `GET /debug`, on the tokio runtime.
pub fn tokio_connector(handle: Handle) -> Connector {
    Arc::new(move |url, key| {
        let task = AbortOnDrop::from_join_handle(handle.spawn(async move {
            let client = Client::new(&[&url], key, ClientOptions::default())?;
            let health = client.health().await?;
            let start = Instant::now();
            let debug = client.debug().await?;
            Ok(ServerInfo {
                url: client.base_url().clone(),
                capabilities: ServerCapabilities::from_version(&debug.version),
                version: debug.version,
                state: debug.state,
                healthy: health.ok,
                latency: start.elapsed(),
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

/// The window's current connection.
#[derive(Default)]
pub struct Connection {
    /// `None` until a connect attempt succeeds.
    pub info: Option<ServerInfo>,
}

impl Connection {
    /// Replaces the connection and notifies observers.
    pub fn set(&mut self, info: ServerInfo, cx: &mut Context<Self>) {
        self.info = Some(info);
        cx.notify();
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

/// User-facing text for a failed connect attempt.
pub fn error_text(err: &Error, host: &str) -> String {
    match err {
        Error::InvalidUrl(_) => t!("connect.error.invalid_url").to_string(),
        Error::Network(_) => t!("connect.error.network", host = host).to_string(),
        Error::Timeout => t!("connect.error.timeout", host = host).to_string(),
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

    #[tokio::test]
    async fn tokio_connector_builds_server_info() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;
        Mock::given(path("/debug"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"state":1,"version":"30.2"}"#),
            )
            .mount(&server)
            .await;
        let connect = tokio_connector(Handle::current());
        let info = connect(server.uri(), SecretString::from("k"))
            .await
            .unwrap();
        assert_eq!(info.version, "30.2");
        assert_eq!(info.state, RaftState::Leader);
        assert!(info.healthy);
        assert!(info.capabilities.supported);
        assert_eq!(info.url.as_str(), format!("{}/", server.uri()));
    }

    #[tokio::test]
    async fn tokio_connector_reports_client_errors() {
        let connect = tokio_connector(Handle::current());
        let result = connect("localhost:8108".into(), SecretString::from("k")).await;
        assert!(matches!(result, Err(Error::InvalidUrl(_))));
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
        assert_eq!(
            error_text(&err, "example.com:443"),
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
        assert_eq!(
            error_text(&err, "localhost:8108"),
            "Typesense answered 401: Forbidden - a valid `x-typesense-api-key` header must be sent."
        );
    }
}
