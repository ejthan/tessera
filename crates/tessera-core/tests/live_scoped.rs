// SPDX-License-Identifier: GPL-3.0-or-later

//! Scoped keys against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-core --test live_scoped -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use tessera_client::{Client, ClientOptions, Error, NewKey};
use tessera_core::keys::scoped::generate;

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn client() -> Client {
    Client::new(
        &[&env("TESSERA_IT_URL", "http://localhost:18108")],
        SecretString::from(env("TESSERA_IT_KEY", "xyz")),
        ClientOptions::default(),
    )
    .unwrap()
}

/// A collection with `o` values 1, 1, 2.
async fn collection(c: &Client, name: &str) {
    let schema = json!({"name": name, "fields": [{"name": "o", "type": "int32"}]});
    c.create_collection(&schema).await.unwrap();
    for o in [1, 1, 2] {
        c.create_document(name, &json!({ "o": o })).await.unwrap();
    }
}

async fn parent(c: &Client, actions: &[&str]) -> (u64, String) {
    let key = NewKey {
        description: "tessera_it_scoped".into(),
        actions: actions.iter().map(ToString::to_string).collect(),
        collections: vec!["*".into()],
        expires_at: None,
    };
    let created = c.create_key(&key).await.unwrap();
    (created.key.id, created.value.expose_secret().to_string())
}

async fn search(c: &Client, collection: &str, scoped: String) -> Result<Value, Error> {
    let params = [("q".to_string(), "*".to_string())];
    let scoped = c.with_key(SecretString::from(scoped));
    let body = scoped.search(collection, &params).await?;
    Ok(serde_json::from_str(&body).unwrap())
}

fn is_401(answer: &Result<Value, Error>) -> bool {
    matches!(answer, Err(Error::Http { status: 401, .. }))
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_scoped_key_limits_what_is_visible() {
    let (c, name) = (client(), "tessera_it_scoped_visible");
    collection(&c, name).await;
    let (id, value) = parent(&c, &["documents:search"]).await;
    let scoped = generate(&value, &json!({"filter_by": "o:=1"})).unwrap();
    let answer = search(&c, name, scoped).await;
    c.delete_key(id).await.unwrap();
    c.delete_collection(name).await.unwrap();
    assert_eq!(answer.unwrap()["found"], 2);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_parent_with_more_actions_is_401() {
    let (c, name) = (client(), "tessera_it_scoped_more");
    collection(&c, name).await;
    let (id, value) = parent(&c, &["documents:search", "documents:get"]).await;
    let scoped = generate(&value, &json!({"filter_by": "o:=1"})).unwrap();
    let answer = search(&c, name, scoped).await;
    c.delete_key(id).await.unwrap();
    c.delete_collection(name).await.unwrap();
    assert!(is_401(&answer), "{answer:?}");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_embedded_expiry_past_is_401() {
    let (c, name) = (client(), "tessera_it_scoped_past");
    collection(&c, name).await;
    let (id, value) = parent(&c, &["documents:search"]).await;
    let scoped = generate(&value, &json!({"expires_at": 1000000000})).unwrap();
    let answer = search(&c, name, scoped).await;
    c.delete_key(id).await.unwrap();
    c.delete_collection(name).await.unwrap();
    assert!(is_401(&answer), "{answer:?}");
}
