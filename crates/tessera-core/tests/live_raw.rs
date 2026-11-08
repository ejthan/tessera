// SPDX-License-Identifier: GPL-3.0-or-later

//! The console's path from typed text to answer against a real Typesense server. It lives here,
//! not in tessera-client's `live.rs`, because it needs core's `parse_path` and `outcome_note`.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-core --test live_raw -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::Value;
use tessera_client::{Client, ClientOptions, Method, RawBody, RawRequest};
use tessera_core::raw::{OutcomeNote, outcome_note, parse_path, raw_query};

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

async fn send(client: &Client, method: Method, path: &str, body: Option<RawBody>) -> (u16, String) {
    let response = client
        .raw(RawRequest {
            method,
            path: parse_path(path).unwrap(),
            query: None,
            body,
        })
        .await
        .unwrap();
    (response.status, response.body)
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_raw_requests() {
    let client = client();
    let name = "tessera_it_raw";
    let path = format!("collections/{name}");
    let _ = send(&client, Method::DELETE, &path, None).await;

    let (status, body) = send(&client, Method::GET, "/debug", None).await;
    assert_eq!(status, 200);
    assert!(serde_json::from_str::<Value>(&body).unwrap()["version"].is_string());

    let schema = format!(r#"{{"name":"{name}","fields":[{{"name":"n","type":"int32"}}]}}"#);
    let created = send(
        &client,
        Method::POST,
        "collections",
        Some(RawBody::Json(schema)),
    )
    .await;
    assert_eq!(created.0, 201, "{}", created.1);

    let import = format!("{path}/documents/import");
    let lines = "{\"n\":1}\n{\"n\":\"bad\"}\n{\"n\":3}";
    let response = client
        .raw(RawRequest {
            method: Method::POST,
            path: parse_path(&import).unwrap(),
            query: raw_query(&[("action".into(), "create".into())]).unwrap(),
            body: Some(RawBody::Text(lines.into())),
        })
        .await
        .unwrap();
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        outcome_note(&import, response.status, &response.body),
        Some(OutcomeNote {
            failed: 1,
            total: 3
        })
    );

    let (status, _) = send(&client, Method::GET, "collections/does_not_exist", None).await;
    assert_eq!(status, 404);

    let (status, body) = send(&client, Method::DELETE, &path, None).await;
    assert_eq!(status, 200, "{body}");
}
