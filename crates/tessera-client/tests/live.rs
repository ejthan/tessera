// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:8108` and `xyz`.

use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, RaftState};

fn url() -> String {
    std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:8108".into())
}

fn client_with(key: &str) -> Client {
    Client::new(&[&url()], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
}

fn client() -> Client {
    client_with(&std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into()))
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
