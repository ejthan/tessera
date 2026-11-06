// SPDX-License-Identifier: GPL-3.0-or-later

//! Admin resources against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client --test live_admin -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, NewKey};

/// Sat, 01 Jan 2100.
const EXPIRES: i64 = 4102444800;

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

fn search_only(description: &str) -> NewKey {
    NewKey {
        description: description.into(),
        actions: vec!["documents:search".into()],
        collections: vec!["*".into()],
        expires_at: Some(EXPIRES),
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_key_crud() {
    let c = client();
    let created = c
        .create_key(&search_only("tessera_it_crud"))
        .await
        .expect("create");
    let id = created.key.id;
    let listed = c.keys().await;
    let read = c.key(id).await;
    let deleted = c.delete_key(id).await;
    let gone = c.key(id).await;

    assert_eq!(created.key.expires_at, Some(EXPIRES));
    assert!(!created.key.extra.contains_key("value"));
    let listed = listed.expect("list");
    assert!(listed.iter().any(|k| k.id == id));
    let read = read.expect("get");
    assert_eq!(read.description, "tessera_it_crud");
    assert_eq!(read.value_prefix.as_ref().map(String::len), Some(4));
    assert!(!read.extra.contains_key("value"));
    deleted.expect("delete");
    assert_eq!(status(&gone.expect_err("gone")), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_with_key_uses_the_other_key() {
    let c = client();
    let created = c
        .create_key(&search_only("tessera_it_with_key"))
        .await
        .expect("create");
    let answer = c.with_key(created.value).keys().await;
    c.delete_key(created.key.id).await.expect("delete");
    assert_eq!(status(&answer.expect_err("refused")), Some(401));
}
