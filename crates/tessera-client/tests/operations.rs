// SPDX-License-Identifier: GPL-3.0-or-later

//! Server operations against wiremock.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions, Error};
use wiremock::matchers::{any, body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(urls: &[&str], read_only: bool) -> Client {
    let options = ClientOptions {
        read_only,
        ..ClientOptions::default()
    };
    Client::new(urls, SecretString::from("secret-key"), options).unwrap()
}

async fn answer(server: &MockServer, status: u16, body: &str) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

async fn received(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn snapshot_encodes_its_path_and_accepts_201() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/operations/snapshot"))
        .and(query_param("snapshot_path", "/var/my backups/ü&x=1"))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"success":true}"#))
        .expect(1)
        .mount(&server)
        .await;
    client(&[&server.uri()], false)
        .node(0)
        .snapshot("/var/my backups/ü&x=1")
        .await
        .unwrap();
}

#[tokio::test]
async fn compact_and_clear_cache_post_to_their_paths() {
    let server = MockServer::start().await;
    for p in ["/operations/db/compact", "/operations/cache/clear"] {
        Mock::given(method("POST"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"success":true}"#))
            .expect(1)
            .mount(&server)
            .await;
    }
    let node = client(&[&server.uri()], false).node(0);
    node.compact_db().await.unwrap();
    node.clear_cache().await.unwrap();
}

#[tokio::test]
async fn set_config_sends_exactly_the_one_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/config"))
        .and(body_json(json!({"cache-num-entries": 5})))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"success":true}"#))
        .expect(1)
        .mount(&server)
        .await;
    client(&[&server.uri()], false)
        .node(0)
        .set_config("cache-num-entries", &json!(5))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_refused_2xx_is_an_error_with_its_body() {
    let server = MockServer::start().await;
    let body = r#"{"error":"nope","success":false}"#;
    answer(&server, 200, body).await;
    let node = client(&[&server.uri()], false).node(0);
    for result in [
        node.snapshot("/x").await,
        node.compact_db().await,
        node.clear_cache().await,
        node.set_config("cache-num-entries", &json!(5)).await,
    ] {
        match result {
            Err(Error::Http {
                status: 200,
                body: b,
                ..
            }) => assert_eq!(b, body),
            other => panic!("expected Http, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_body_without_success_passes() {
    let server = MockServer::start().await;
    answer(&server, 200, "").await;
    let node = client(&[&server.uri()], false).node(0);
    node.clear_cache().await.unwrap();
    assert!(node.vote().await.unwrap());
}

#[tokio::test]
async fn vote_returns_what_the_server_says() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/operations/vote"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"success":false}"#))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/operations/vote"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"success":true}"#))
        .mount(&server)
        .await;
    let node = client(&[&server.uri()], false).node(0);
    assert!(!node.vote().await.unwrap());
    assert!(node.vote().await.unwrap());
}

#[tokio::test]
async fn a_conflict_keeps_its_message() {
    let server = MockServer::start().await;
    answer(
        &server,
        409,
        r#"{"message":"Another snapshot is in progress."}"#,
    )
    .await;
    let error = client(&[&server.uri()], false)
        .node(0)
        .snapshot("/x")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Http { status: 409, .. }));
    assert_eq!(
        error.typesense_message(),
        Some("Another snapshot is in progress.")
    );
}

#[tokio::test]
async fn a_failed_copy_keeps_its_body() {
    let server = MockServer::start().await;
    let body = r#"{"error":"Copy failed.","success":false}"#;
    answer(&server, 500, body).await;
    let error = client(&[&server.uri()], false)
        .node(0)
        .snapshot("/x")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Http { status: 500, body: b, .. } if b == body));
}

#[tokio::test]
async fn read_only_sends_nothing() {
    let server = MockServer::start().await;
    answer(&server, 200, r#"{"success":true}"#).await;
    let node = client(&[&server.uri()], true).node(0);
    let refused = |r: Result<(), Error>| matches!(r, Err(Error::ReadOnly { .. }));
    assert!(refused(node.snapshot("/x").await));
    assert!(refused(node.compact_db().await));
    assert!(refused(node.clear_cache().await));
    assert!(refused(node.set_config("k", &json!(1)).await));
    assert!(refused(node.vote().await.map(drop)));
    assert_eq!(received(&server).await, 0);
}

#[tokio::test]
async fn an_operation_never_goes_to_another_node() {
    let (dead, up) = (MockServer::start().await, MockServer::start().await);
    answer(&up, 200, r#"{"success":true}"#).await;
    let dead_url = dead.uri();
    drop(dead);
    let c = client(&[&dead_url, &up.uri()], false);
    let node = c.node(0);
    assert!(node.vote().await.is_err());
    assert!(node.snapshot("/x").await.is_err());
    assert!(node.clear_cache().await.is_err());
    assert_eq!(received(&up).await, 0);
}
