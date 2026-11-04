// SPDX-License-Identifier: GPL-3.0-or-later

//! Client tests against a wiremock server.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{Client, Error, Health, RaftState, RequestEvent};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-key-123";

fn key() -> SecretString {
    SecretString::from(KEY)
}

async fn mock(server: &MockServer, p: &str, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn health_ok() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let client = Client::new(&server.uri(), key()).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: true });
}

#[tokio::test]
async fn health_503_not_ok_is_not_an_error() {
    let server = MockServer::start().await;
    mock(&server, "/health", 503, r#"{"ok":false}"#).await;
    let client = Client::new(&server.uri(), key()).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: false });
}

#[tokio::test]
async fn debug_parses_version_and_leader() {
    let server = MockServer::start().await;
    mock(&server, "/debug", 200, r#"{"state":1,"version":"30.2"}"#).await;
    let client = Client::new(&server.uri(), key()).unwrap();
    let debug = client.debug().await.unwrap();
    assert_eq!(debug.version, "30.2");
    assert_eq!(debug.state, RaftState::Leader);
}

#[tokio::test]
async fn debug_maps_states() {
    for (body, expected) in [
        (r#"{"state":4,"version":"30.2"}"#, RaftState::Follower),
        (r#"{"state":3,"version":"30.2"}"#, RaftState::Other(3)),
        (r#"{"version":"30.2"}"#, RaftState::Unknown),
    ] {
        let server = MockServer::start().await;
        mock(&server, "/debug", 200, body).await;
        let client = Client::new(&server.uri(), key()).unwrap();
        assert_eq!(client.debug().await.unwrap().state, expected, "{body}");
    }
}

#[tokio::test]
async fn sends_api_key_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/debug"))
        .and(header("X-TYPESENSE-API-KEY", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"state":1,"version":"30.2"}"#))
        .mount(&server)
        .await;
    let client = Client::new(&server.uri(), key()).unwrap();
    assert!(client.debug().await.is_ok());
}

#[tokio::test]
async fn http_error_keeps_full_message() {
    let msg = "Forbidden - a valid `x-typesense-api-key` header must be sent.";
    let server = MockServer::start().await;
    mock(
        &server,
        "/debug",
        401,
        &format!(r#"{{"message": "{msg}"}}"#),
    )
    .await;
    let client = Client::new(&server.uri(), key()).unwrap();
    match client.debug().await {
        Err(Error::Http {
            status, message, ..
        }) => {
            assert_eq!(status, 401);
            assert_eq!(message, msg);
        }
        other => panic!("expected Http error, got {other:?}"),
    }
}

#[tokio::test]
async fn http_error_without_message_uses_body() {
    let server = MockServer::start().await;
    mock(&server, "/debug", 500, "oops").await;
    let client = Client::new(&server.uri(), key()).unwrap();
    match client.debug().await {
        Err(Error::Http {
            status: 500,
            message,
            ..
        }) => assert_eq!(message, "oops"),
        other => panic!("expected Http 500, got {other:?}"),
    }
}

#[tokio::test]
async fn base_url_with_path_prefix_keeps_prefix() {
    let server = MockServer::start().await;
    mock(&server, "/typesense/health", 200, r#"{"ok":true}"#).await;
    let client = Client::new(&format!("{}/typesense", server.uri()), key()).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: true });
}

#[tokio::test]
async fn trailing_slash_base_works() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let client = Client::new(&format!("{}/", server.uri()), key()).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: true });
}

#[test]
fn rejects_non_http_scheme() {
    for input in ["localhost:8108", "ftp://example.com", "not a url"] {
        assert!(
            matches!(Client::new(input, key()), Err(Error::InvalidUrl(_))),
            "{input} should be rejected"
        );
    }
}

#[tokio::test]
async fn closed_port_is_network_error() {
    let client = Client::new("http://127.0.0.1:9", key()).unwrap();
    assert!(matches!(client.health().await, Err(Error::Network(_))));
}

#[tokio::test]
async fn slow_server_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"ok":true}"#)
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    let client = Client::new(&server.uri(), key())
        .unwrap()
        .with_timeout(Duration::from_millis(100));
    assert!(matches!(client.health().await, Err(Error::Timeout)));
}

#[tokio::test]
async fn hook_fires_once_per_request() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, Option<u16>)>::new()));
    let (c, s) = (count.clone(), seen.clone());
    let client = Client::new(&server.uri(), key())
        .unwrap()
        .with_hook(Arc::new(move |e: &RequestEvent| {
            c.fetch_add(1, Ordering::SeqCst);
            s.lock().unwrap().push((e.path.clone(), e.status));
        }));
    client.health().await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(seen.lock().unwrap()[0], ("/health".to_string(), Some(200)));
}

#[tokio::test]
async fn key_never_printed() {
    let server = MockServer::start().await;
    mock(
        &server,
        "/debug",
        401,
        &format!(r#"{{"message": "bad key {KEY}?"}}"#),
    )
    .await;
    let client = Client::new(&server.uri(), key()).unwrap();
    assert!(!format!("{client:?}").contains(KEY));
    // The server may echo text, but our own wrappers must not add the key.
    let err = client.debug().await.unwrap_err();
    let shown = format!("{err} {err:?}").replace(&format!("bad key {KEY}?"), "");
    assert!(!shown.contains(KEY), "{shown}");
}
