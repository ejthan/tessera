// SPDX-License-Identifier: GPL-3.0-or-later

//! `id_batches` against a real Typesense server: what it sends deletes exactly its ids.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-core --test live_ids -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions};
use tessera_core::documents::ids::id_batches;

const BYSTANDERS: [&str; 5] = ["keep", "c", "b", "keep1", "other"];

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_a_delete_leaves_every_bystander() {
    let url = std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into());
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into());
    let client = Client::new(&[&url], SecretString::from(key), ClientOptions::default()).unwrap();
    let name = "tessera_it_bystanders";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[{"name":"t","type":"string"}]}))
        .await
        .unwrap();
    let doomed: Vec<String> = [
        "a\\",
        "b,keep,c",
        "\\",
        "\\\\",
        "a\\b",
        "*",
        " * ",
        "keep1 ",
        " lead",
        "x`y",
        "a b",
        "é",
        "...",
        ".",
        "..",
        "plain",
        "a:b@c+d.e-f_g",
    ]
    .map(String::from)
    .to_vec();
    for id in doomed.iter().map(String::as_str).chain(BYSTANDERS) {
        client
            .create_document(name, &json!({"id":id,"t":"x"}))
            .await
            .unwrap();
    }
    let batches = id_batches(&doomed, 7500);
    let mut deleted = 0;
    for filter in &batches.filters {
        deleted += client.delete_by_filter(name, filter).await.unwrap();
    }
    for id in &batches.single {
        client.delete_document(name, id).await.unwrap();
        deleted += 1;
    }
    let left = client.count_documents(name, None).await;
    let mut survivors = Vec::new();
    for id in BYSTANDERS {
        survivors.push(client.document(name, id).await.is_ok());
    }
    let _ = client.delete_collection(name).await;
    assert_eq!(deleted, doomed.len() as u64);
    assert_eq!(left.unwrap(), BYSTANDERS.len() as u64);
    assert!(survivors.iter().all(|alive| *alive), "{survivors:?}");
}
