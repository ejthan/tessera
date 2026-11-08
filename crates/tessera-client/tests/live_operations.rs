// SPDX-License-Identifier: GPL-3.0-or-later

//! Operations against a real, throwaway Typesense server. Snapshot and compact are not for a
//! server with data worth keeping, so the dev port and 8108 are refused.
//!
//! Start one with `docker run --rm -d --name t302 -p 18208:8108 typesense/typesense:30.2
//! --data-dir /tmp --api-key=xyz --enable-cors=false`, then run
//! `TESSERA_IT_URL=http://localhost:18208 TESSERA_IT_KEY=xyz cargo test -p tessera-client
//! --test live_operations -- --ignored`.

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{Client, ClientOptions, Error, NodeClient};
use url::Url;

/// The node under test, or a panic on a server these tests must not touch.
fn node() -> NodeClient {
    let url = std::env::var("TESSERA_IT_URL").expect("TESSERA_IT_URL names a throwaway server");
    let parsed = Url::parse(&url).expect("TESSERA_IT_URL must be a URL");
    let port = parsed.port_or_known_default();
    assert!(
        !matches!(port, Some(18108 | 8108)),
        "refusing {url}: these tests snapshot and compact, use a throwaway server"
    );
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into());
    Client::new(&[&url], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
        .node(0)
}

fn status(error: &Error) -> Option<u16> {
    match error {
        Error::Http { status, .. } => Some(*status),
        _ => None,
    }
}

/// One test, because the server refuses a snapshot while another one saves.
#[tokio::test]
#[ignore = "needs a throwaway Typesense, see the file header"]
async fn live_snapshot_succeeds_and_keeps_a_copy_error() {
    let node = node();
    let path = format!("/tmp/tessera-it-{}", std::process::id());
    node.snapshot(&path).await.expect("snapshot");

    let error = node.snapshot("/proc/tessera-it").await.unwrap_err();
    assert_eq!(status(&error), Some(500));
    assert!(error.to_string().contains("Copy failed."), "{error}");
}

#[tokio::test]
#[ignore = "needs a throwaway Typesense, see the file header"]
async fn live_compact_and_clear_cache() {
    let node = node();
    node.compact_db().await.expect("compact");
    node.clear_cache().await.expect("clear cache");
}

#[tokio::test]
#[ignore = "needs a throwaway Typesense, see the file header"]
async fn live_vote_on_a_single_node_is_false() {
    assert!(!node().vote().await.expect("vote"));
}

/// Each key, set to the server's start value, which there is no way to read.
#[tokio::test]
#[ignore = "needs a throwaway Typesense, see the file header"]
async fn live_config_keys_are_accepted() {
    let node = node();
    let keys: [(&str, Value); 7] = [
        ("log-slow-requests-time-ms", json!(-1)),
        ("log-slow-searches-time-ms", json!(30000)),
        ("enable-search-logging", json!(false)),
        ("cache-num-entries", json!(1000)),
        ("embedding-cache-num-entries", json!(100)),
        ("healthy-read-lag", json!(1000)),
        ("healthy-write-lag", json!(500)),
    ];
    for (key, value) in &keys {
        node.set_config(key, value).await.expect(key);
    }
}

#[tokio::test]
#[ignore = "needs a throwaway Typesense, see the file header"]
async fn live_config_refuses_a_bad_value_and_ignores_an_unknown_key() {
    let node = node();
    let error = node
        .set_config("cache-num-entries", &json!(0))
        .await
        .unwrap_err();
    assert_eq!(status(&error), Some(400));
    assert_eq!(
        error.typesense_message(),
        Some("Configuration `cache-num-entries` must be a positive integer.")
    );
    node.set_config("tessera-no-such-key", &json!(1))
        .await
        .expect("an unknown key answers success, which is why the list is closed");
}
