// SPDX-License-Identifier: GPL-3.0-or-later

//! Relevance resources against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client --test live_relevance -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

use std::collections::BTreeSet;

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions, Error};

fn url() -> String {
    std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into())
}

fn client() -> Client {
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into());
    Client::new(&[&url()], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
}

fn status(error: &Error) -> Option<u16> {
    match error {
        Error::Http { status, .. } => Some(*status),
        _ => None,
    }
}

async fn create(client: &Client, name: &str) {
    let schema = json!({"name": name, "fields": [{"name": "title", "type": "string"}]});
    client.create_collection(&schema).await.expect("create");
}

fn search_all() -> Vec<(String, String)> {
    vec![
        ("q".to_string(), "*".to_string()),
        ("query_by".to_string(), "title".to_string()),
    ]
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_round_trip() {
    let c = client();
    let (one, two, alias) = (
        "tessera_it_alias_one",
        "tessera_it_alias_two",
        "tessera_it_alias",
    );
    create(&c, one).await;
    create(&c, two).await;

    let created = c.upsert_alias(alias, one).await;
    let read = c.alias(alias).await;
    let moved = c.upsert_alias(alias, two).await;
    let listed = c.aliases().await;
    let removed = c.delete_alias(alias).await;
    let gone = c.alias(alias).await;
    c.delete_collection(one).await.expect("cleanup");
    c.delete_collection(two).await.expect("cleanup");

    assert_eq!(created.unwrap().collection_name, one);
    assert_eq!(read.unwrap().collection_name, one);
    assert_eq!(moved.unwrap().collection_name, two);
    let listed = listed.unwrap();
    let found = listed.iter().find(|a| a.name == alias).unwrap();
    assert_eq!(found.collection_name, two);
    removed.unwrap();
    assert_eq!(status(&gone.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_accepts_a_missing_target() {
    let c = client();
    let alias = "tessera_it_alias_missing";
    let saved = c.upsert_alias(alias, "tessera_it_no_such_target").await;
    let searched = c.search(alias, &search_all()).await;
    c.delete_alias(alias).await.expect("cleanup");

    saved.unwrap();
    assert_eq!(status(&searched.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_of_an_alias_does_not_resolve() {
    let c = client();
    let (target, first, chain) = (
        "tessera_it_chain_target",
        "tessera_it_chain_first",
        "tessera_it_chain",
    );
    create(&c, target).await;
    c.upsert_alias(first, target).await.expect("alias");
    let saved = c.upsert_alias(chain, first).await;
    let resolved = c.collection(chain).await;
    c.delete_alias(chain).await.expect("cleanup");
    c.delete_alias(first).await.expect("cleanup");
    c.delete_collection(target).await.expect("cleanup");

    saved.unwrap();
    assert_eq!(status(&resolved.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_cannot_take_a_collection_name() {
    let c = client();
    let name = "tessera_it_alias_taken";
    create(&c, name).await;
    let refused = c.upsert_alias(name, name).await;
    c.delete_collection(name).await.expect("cleanup");

    let error = refused.unwrap_err();
    assert_eq!(status(&error), Some(500));
    assert!(error.typesense_message().unwrap().contains("conflicts"));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_delete_missing_alias_is_404() {
    let error = client()
        .delete_alias("tessera_it_no_such_alias")
        .await
        .unwrap_err();
    assert_eq!(status(&error), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_stopwords_round_trip() {
    let c = client();
    let (set, name) = ("tessera_it_stopwords", "tessera_it_stopwords_c");
    create(&c, name).await;
    let words = ["The".to_string(), "the".to_string(), "  x y ".to_string()];
    let saved = c.upsert_stopwords(set, &words, Some("en")).await;
    let read = c.stopword_set(set).await;
    let listed = c.stopwords().await;
    let empty = c.upsert_stopwords(set, &[], None).await;
    let removed = c.delete_stopwords(set).await;
    let gone = c.stopword_set(set).await;
    let mut params = search_all();
    params.push(("stopwords".to_string(), set.to_string()));
    let searched = c.search(name, &params).await;
    c.delete_collection(name).await.expect("cleanup");

    saved.unwrap();
    let read = read.unwrap();
    let stored: BTreeSet<&str> = read.stopwords.iter().map(String::as_str).collect();
    assert_eq!(stored, BTreeSet::from(["the", "x", "y"]));
    assert_eq!(read.locale.as_deref(), Some("en"));
    assert!(listed.unwrap().iter().any(|s| s.id == set));
    assert_eq!(status(&empty.unwrap_err()), Some(400));
    removed.unwrap();
    assert_eq!(status(&gone.unwrap_err()), Some(404));
    assert_eq!(status(&searched.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_preset_delete() {
    let c = client();
    let (preset, name) = ("tessera_it_preset_delete", "tessera_it_preset_delete_c");
    create(&c, name).await;
    let saved = c.upsert_preset(preset, &json!("str")).await;
    let listed = c.presets().await;
    let removed = c.delete_preset(preset).await;
    let gone = c.preset(preset).await;
    let mut params = search_all();
    params.push(("preset".to_string(), preset.to_string()));
    let searched = c.search(name, &params).await;
    c.delete_collection(name).await.expect("cleanup");

    saved.unwrap();
    let listed = listed.unwrap();
    let found = listed.iter().find(|p| p.name == preset).unwrap();
    assert_eq!(found.value, json!("str"));
    removed.unwrap();
    assert_eq!(status(&gone.unwrap_err()), Some(404));
    searched.unwrap();
}
