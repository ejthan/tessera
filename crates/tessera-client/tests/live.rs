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

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collection_reads() {
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_reads";
    let _ = admin_request(reqwest::Method::DELETE, &format!("collections/{name}"))
        .send()
        .await;
    let schema = format!(
        r#"{{"name":"{name}","fields":[{{"name":"title","type":"string"}},{{"name":"n","type":"int32"}}],"default_sorting_field":"n"}}"#
    );
    admin(reqwest::Method::POST, "collections", Some(&schema)).await;
    let one = client.collection(name).await;
    let all = client.collections().await;
    let aliases = client.aliases().await;
    // Clean up before asserting, so a failed assertion leaves no collection behind.
    admin(
        reqwest::Method::DELETE,
        &format!("collections/{name}"),
        None,
    )
    .await;
    let one = one.unwrap();
    assert_eq!(one.name, name);
    assert_eq!(one.fields.len(), 2);
    assert_eq!(one.default_sorting_field, "n");
    assert!(one.fields[0].flag("index"));
    assert!(all.unwrap().iter().any(|c| c.name == name));
    aliases.unwrap();
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collection_lifecycle() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_lifecycle";
    let _ = client.delete_collection(name).await;
    let schema =
        json!({"name":name,"fields":[{"name":"n","type":"int32"},{"name":"t","type":"string"}]});
    let created = client.create_collection(&schema).await;
    let duplicate = client.create_collection(&schema).await;
    let invalid = client
        .create_collection(&json!({"name":"x","fields":[]}))
        .await;
    admin(
        reqwest::Method::POST,
        &format!("collections/{name}/documents"),
        Some(r#"{"n":1,"t":"a"}"#),
    )
    .await;
    // Add, drop and change a type (drop + add in one request).
    let patch = client
        .update_collection(
            name,
            &json!({"fields":[
                {"name":"extra","type":"string","optional":true},
                {"name":"t","drop":true},
                {"name":"n","drop":true},{"name":"n","type":"int64"}
            ]}),
        )
        .await;
    let changed = client.collection(name).await;
    let without_drop = client
        .update_collection(name, &json!({"fields":[{"name":"n","type":"int32"}]}))
        .await;
    let changes = client.schema_changes().await;
    let truncated = client.truncate_collection(name).await;
    let deleted = client.delete_collection(name).await;
    let gone = client.collection(name).await;
    // Everything ran; now assert.
    assert_eq!(created.unwrap().name, name);
    match duplicate {
        Err(e @ Error::Http { status: 409, .. }) => assert!(e.typesense_message().is_some()),
        other => panic!("duplicate: {other:?}"),
    }
    match invalid {
        Err(e @ Error::Http { status: 400, .. }) => assert!(e.typesense_message().is_some()),
        other => panic!("invalid: {other:?}"),
    }
    patch.unwrap();
    let fields = changed.unwrap().fields;
    assert!(fields.iter().any(|f| f.name == "extra"));
    assert!(!fields.iter().any(|f| f.name == "t"));
    assert_eq!(
        fields.iter().find(|f| f.name == "n").unwrap().r#type,
        "int64"
    );
    match without_drop {
        Err(e @ Error::Http { status: 400, .. }) => {
            assert!(
                e.typesense_message()
                    .unwrap()
                    .contains("already part of the schema")
            );
        }
        other => panic!("change without drop: {other:?}"),
    }
    assert!(changes.unwrap().is_empty());
    assert_eq!(truncated.unwrap(), 1);
    deleted.unwrap();
    assert!(matches!(gone, Err(Error::Http { status: 404, .. })));
}
