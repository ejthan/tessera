// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client --test live -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, KeyAccess, RaftState};

fn url() -> String {
    std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into())
}

fn client_with(key: &str) -> Client {
    Client::new(&[&url()], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
}

fn admin_key() -> String {
    std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into())
}

fn client() -> Client {
    client_with(&admin_key())
}

fn admin_request(method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let url = format!("{}/{path}", url().trim_end_matches('/'));
    reqwest::Client::new()
        .request(method, url)
        .header("X-TYPESENSE-API-KEY", admin_key())
}

/// A raw admin request for test setup and cleanup, outside the client under test.
async fn admin(method: reqwest::Method, path: &str, body: Option<&str>) -> serde_json::Value {
    let mut request = admin_request(method, path);
    if let Some(body) = body {
        request = request.body(body.to_string());
    }
    let response = request.send().await.expect("admin request");
    assert!(
        response.status().is_success(),
        "{path}: {}",
        response.status()
    );
    response.json().await.expect("admin response is JSON")
}

async fn wait_until_ready(client: &Client) {
    for _ in 0..60 {
        if matches!(client.health().await, Ok(h) if h.ok) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("Typesense at {} did not become healthy within 30 s", url());
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_health_ok() {
    wait_until_ready(&client()).await;
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_debug_is_leader_and_30x() {
    let client = client();
    wait_until_ready(&client).await;
    let debug = client.debug().await.unwrap();
    assert_eq!(debug.state, RaftState::Leader);
    assert!(
        debug.version.starts_with("30."),
        "version {}",
        debug.version
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_wrong_key_is_401() {
    wait_until_ready(&client()).await;
    match client_with("wrong").debug().await {
        Err(Error::Http {
            status, message, ..
        }) => {
            assert_eq!(status, 401);
            assert!(message.contains("x-typesense-api-key"), "{message}");
        }
        other => panic!("expected 401, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_metrics_parses() {
    let client = client();
    wait_until_ready(&client).await;
    let metrics = client.metrics().await.unwrap();
    assert!(metrics.cpu_percent.is_some(), "{metrics:?}");
    assert!(metrics.memory_total.unwrap_or(0) > 0, "{metrics:?}");
    assert!(metrics.raw.contains_key("typesense_memory_active_bytes"));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_stats_parses() {
    let client = client();
    wait_until_ready(&client).await;
    let stats = client.stats().await.unwrap();
    assert!(stats.total_requests_per_second.is_some(), "{stats:?}");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collections_summary() {
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_summary";
    // Best effort, status ignored: an aborted earlier run may have left the collection behind.
    let _ = admin_request(reqwest::Method::DELETE, &format!("collections/{name}"))
        .send()
        .await;
    let schema = format!(r#"{{"name":"{name}","fields":[{{"name":"title","type":"string"}}]}}"#);
    admin(reqwest::Method::POST, "collections", Some(&schema)).await;
    let summary = client.collections_summary().await;
    // Clean up before asserting, so a failed assertion leaves no collection behind.
    admin(
        reqwest::Method::DELETE,
        &format!("collections/{name}"),
        None,
    )
    .await;
    let summary = summary.unwrap();
    let entry = summary
        .iter()
        .find(|c| c.name == name)
        .expect("collection listed");
    assert_eq!(entry.num_documents, 0);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collections_probe() {
    let client = client();
    wait_until_ready(&client).await;
    client.collections_probe().await.unwrap();
    client.node(0).collections_probe().await.unwrap();
    let wrong = client_with("not-the-key").collections_probe().await;
    assert!(
        matches!(wrong, Err(Error::Http { status: 401, .. })),
        "{wrong:?}"
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_keys_probe_admin() {
    let client = client();
    wait_until_ready(&client).await;
    assert_eq!(client.keys_probe().await.unwrap(), KeyAccess::Admin);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_keys_probe_restricted() {
    wait_until_ready(&client()).await;
    let body = r#"{"description":"tessera-it","actions":["documents:search"],"collections":["*"]}"#;
    let created = admin(reqwest::Method::POST, "keys", Some(body)).await;
    let id = created["id"].as_u64().expect("key id");
    let value = created["value"].as_str().expect("key value").to_string();
    let probe = client_with(&value).keys_probe().await;
    // Delete before asserting, so a failed assertion leaves no key behind.
    admin(reqwest::Method::DELETE, &format!("keys/{id}"), None).await;
    assert_eq!(probe.unwrap(), KeyAccess::Restricted);
}
