// SPDX-License-Identifier: GPL-3.0-or-later

//! Client tests against a wiremock server.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{
    BODY_LIMIT, Client, ClientOptions, Error, Health, Method, Origin, RaftState, RequestEvent,
    RequestHook,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-key-123";

fn key() -> SecretString {
    SecretString::from(KEY)
}

fn client(url: &str) -> Result<Client, Error> {
    Client::new(&[url], key(), ClientOptions::default())
}

fn recorder() -> (RequestHook, Arc<Mutex<Vec<RequestEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let hook: RequestHook =
        Arc::new(move |e: &RequestEvent| sink.lock().expect("event log lock").push(e.clone()));
    (hook, events)
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
    let client = client(&server.uri()).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: true });
}

#[tokio::test]
async fn health_503_not_ok_is_not_an_error() {
    let server = MockServer::start().await;
    mock(&server, "/health", 503, r#"{"ok":false}"#).await;
    let client = client(&server.uri()).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: false });
}

#[tokio::test]
async fn debug_parses_version_and_leader() {
    let server = MockServer::start().await;
    mock(&server, "/debug", 200, r#"{"state":1,"version":"30.2"}"#).await;
    let client = client(&server.uri()).unwrap();
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
        let client = client(&server.uri()).unwrap();
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
    let client = client(&server.uri()).unwrap();
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
    let client = client(&server.uri()).unwrap();
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
    let client = client(&server.uri()).unwrap();
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
    let client = client(&format!("{}/typesense", server.uri())).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: true });
}

#[tokio::test]
async fn trailing_slash_base_works() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let client = client(&format!("{}/", server.uri())).unwrap();
    assert_eq!(client.health().await.unwrap(), Health { ok: true });
}

#[test]
fn rejects_non_http_scheme() {
    for input in ["localhost:8108", "ftp://example.com", "not a url"] {
        assert!(
            matches!(client(input), Err(Error::InvalidUrl(_))),
            "{input} should be rejected"
        );
    }
}

#[tokio::test]
async fn closed_port_is_network_error() {
    let client = client("http://127.0.0.1:9").unwrap();
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
    let client = client(&server.uri())
        .unwrap()
        .with_timeout(Duration::from_millis(100));
    assert!(matches!(client.health().await, Err(Error::Timeout)));
}

#[tokio::test]
async fn hook_fires_once_per_request() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::<(String, Option<u16>)>::new()));
    let (c, s) = (count.clone(), seen.clone());
    let client = client(&server.uri())
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
    let client = client(&server.uri()).unwrap();
    assert!(!format!("{client:?}").contains(KEY));
    // The server may echo text, but our own wrappers must not add the key.
    let err = client.debug().await.unwrap_err();
    let shown = format!("{err} {err:?}").replace(&format!("bad key {KEY}?"), "");
    assert!(!shown.contains(KEY), "{shown}");
}

#[tokio::test]
async fn redirect_is_not_followed_and_key_stays_home() {
    let elsewhere = MockServer::start().await;
    mock(&elsewhere, "/debug", 200, r#"{"state":1,"version":"30.2"}"#).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/debug"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/debug", elsewhere.uri())),
        )
        .mount(&server)
        .await;
    let client = client(&server.uri()).unwrap();
    assert!(matches!(
        client.debug().await,
        Err(Error::Http { status: 302, .. })
    ));
    let leaked = elsewhere.received_requests().await.unwrap_or_default();
    assert!(
        leaked.is_empty(),
        "redirect target received {} request(s)",
        leaked.len()
    );
}

#[test]
fn rejects_empty_node_list_and_any_bad_node() {
    let none: [&str; 0] = [];
    assert!(matches!(
        Client::new(&none, key(), ClientOptions::default()),
        Err(Error::InvalidUrl(_))
    ));
    assert!(matches!(
        Client::new(
            &["http://localhost:8108", "ftp://example.com"],
            key(),
            ClientOptions::default()
        ),
        Err(Error::InvalidUrl(_))
    ));
}

#[tokio::test]
async fn event_carries_node_origin_and_bodies() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let (hook, events) = recorder();
    let client = client(&server.uri()).unwrap().with_hook(hook);
    client.health().await.unwrap();
    client.health().await.unwrap();
    let events = events.lock().unwrap();
    let e = &events[0];
    assert_eq!(&e.node, client.base_url());
    assert_eq!(e.origin, Origin::User);
    assert_eq!(e.method, Method::GET);
    assert_eq!(e.path, "/health");
    assert_eq!(e.query, None);
    assert_eq!(e.status, Some(200));
    assert_eq!(e.size, Some(11));
    assert!(e.request_body.is_none());
    let body = e.response_body.as_ref().unwrap();
    assert_eq!(&*body.text, r#"{"ok":true}"#);
    assert_eq!(body.original_len, 11);
    assert_eq!(e.key_prefix, &KEY[..4]);
    assert_eq!(e.error, None);
    assert!(events[1].id > e.id, "ids increase");
}

#[tokio::test]
async fn event_headers_never_contain_key() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let (hook, events) = recorder();
    client(&server.uri())
        .unwrap()
        .with_hook(hook.clone())
        .health()
        .await
        .unwrap();
    // A failed request must not leak the key either.
    let _ = client("http://127.0.0.1:9")
        .unwrap()
        .with_hook(hook)
        .health()
        .await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    for event in events.iter() {
        for (name, value) in &event.request_headers {
            assert!(!name.eq_ignore_ascii_case("x-typesense-api-key"), "{name}");
            assert!(!value.contains(KEY));
        }
        assert!(!format!("{event:?}").contains(KEY));
    }
    assert!(events[1].error.is_some());
    assert_eq!(events[1].status, None);
}

#[tokio::test]
async fn body_is_cut_at_limit_on_char_boundary() {
    let full = format!("{}é…", "a".repeat(BODY_LIMIT - 1));
    let server = MockServer::start().await;
    mock(&server, "/debug", 200, &full).await;
    let (hook, events) = recorder();
    let client = client(&server.uri()).unwrap().with_hook(hook);
    let _ = client.debug().await;
    let events = events.lock().unwrap();
    let body = events[0].response_body.as_ref().unwrap();
    assert!(body.text.len() <= BODY_LIMIT);
    assert!(full.starts_with(&*body.text));
    assert_eq!(body.text.len(), BODY_LIMIT - 1, "é must not be split");
    assert_eq!(body.original_len, full.len());
    assert_eq!(events[0].size, Some(full.len() as u64));
}

#[tokio::test]
async fn polling_clone_tags_events() {
    let server = MockServer::start().await;
    mock(&server, "/health", 200, r#"{"ok":true}"#).await;
    let (hook, events) = recorder();
    let client = client(&server.uri()).unwrap().with_hook(hook);
    client.polling().health().await.unwrap();
    client.health().await.unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events[0].origin, Origin::Polling);
    assert_eq!(events[1].origin, Origin::User);
}

#[tokio::test]
async fn node_client_targets_one_node() {
    let (a, b) = (MockServer::start().await, MockServer::start().await);
    mock(&a, "/health", 200, r#"{"ok":true}"#).await;
    mock(&b, "/health", 200, r#"{"ok":true}"#).await;
    let client = Client::new(&[&a.uri(), &b.uri()], key(), ClientOptions::default()).unwrap();
    assert!(client.node(1).health().await.unwrap().ok);
    assert!(
        client.node(1).debug().await.is_err(),
        "no failover to node 0"
    );
    assert_eq!(a.received_requests().await.unwrap().len(), 0);
    assert_eq!(b.received_requests().await.unwrap().len(), 2);
    assert_eq!(client.active_node(), 0);
}

#[tokio::test]
async fn active_node_starts_at_nearest() {
    let (a, b) = (MockServer::start().await, MockServer::start().await);
    mock(&b, "/health", 200, r#"{"ok":true}"#).await;
    let nearest = |url: &str| ClientOptions {
        nearest_node: Some(url.to_string()),
        ..ClientOptions::default()
    };

    // Nearest is one of the nodes.
    let client = Client::new(&[&a.uri(), &b.uri()], key(), nearest(&b.uri())).unwrap();
    assert_eq!(client.nodes().len(), 2);
    assert_eq!(client.active_node(), 1);
    assert!(client.health().await.unwrap().ok);
    assert_eq!(a.received_requests().await.unwrap().len(), 0);

    // Nearest is a separate node: it is listed first and starts active.
    let client = Client::new(&[&a.uri()], key(), nearest(&format!("{}/", b.uri()))).unwrap();
    assert_eq!(client.nodes().len(), 2);
    assert_eq!(client.active_node(), 0);
    assert_eq!(client.base_url().as_str(), format!("{}/", b.uri()));

    // Without nearest, the first node starts active.
    let client = Client::new(&[&a.uri(), &b.uri()], key(), ClientOptions::default()).unwrap();
    assert_eq!(client.active_node(), 0);
}

#[tokio::test]
async fn active_node_is_shared_by_clones() {
    let client = Client::new(
        &["http://127.0.0.1:1", "http://127.0.0.1:2"],
        key(),
        ClientOptions::default(),
    )
    .unwrap();
    let polling = client.polling();
    polling.set_active_node(1);
    assert_eq!(client.active_node(), 1);
    assert_eq!(client.base_url().as_str(), "http://127.0.0.1:2/");
}

#[test]
fn invalid_nearest_node_is_rejected() {
    let options = ClientOptions {
        nearest_node: Some("ftp://example.com".into()),
        ..ClientOptions::default()
    };
    assert!(matches!(
        Client::new(&["http://localhost:8108"], key(), options),
        Err(Error::InvalidUrl(_))
    ));
}

/// A one-shot HTTPS server on 127.0.0.1 with a self-signed certificate for `localhost`.
fn self_signed_server() -> String {
    use std::io::{Read, Write};
    let cert = include_bytes!("fixtures/self-signed.crt.der").to_vec();
    let key = include_bytes!("fixtures/self-signed.key.der").to_vec();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert.into()], key.try_into().expect("PKCS#8 key"))
        .expect("certificate and key");
    let config = Arc::new(config);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local address").port();
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let conn = rustls::ServerConnection::new(config.clone()).expect("TLS session");
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut buf = [0u8; 4096];
            // A rejected handshake ends here with an error; the client sees the TLS failure.
            if tls.read(&mut buf).is_ok() {
                let _ = tls.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
                );
                let _ = tls.flush();
            }
        }
    });
    format!("https://localhost:{port}")
}

#[tokio::test]
async fn self_signed_certificate_is_tls_error() {
    let url = self_signed_server();
    match client(&url).unwrap().health().await {
        Err(Error::Tls(detail)) => assert!(!detail.is_empty()),
        other => panic!("expected Tls error, got {other:?}"),
    }
}

#[tokio::test]
async fn allow_insecure_accepts_self_signed_certificate() {
    let url = self_signed_server();
    let options = ClientOptions {
        allow_insecure: true,
        ..ClientOptions::default()
    };
    let client = Client::new(&[&url], key(), options).unwrap();
    assert!(client.health().await.unwrap().ok);
}

#[test]
fn ca_file_must_exist_and_hold_a_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "no certificate here\n").unwrap();
    for path in [dir.path().join("missing.pem"), empty] {
        let options = ClientOptions {
            ca_file: Some(path.clone()),
            ..ClientOptions::default()
        };
        assert!(
            matches!(
                Client::new(&["https://localhost"], key(), options),
                Err(Error::Tls(_))
            ),
            "{}",
            path.display()
        );
    }
}
