// SPDX-License-Identifier: GPL-3.0-or-later

//! The action catalog against a real Typesense server: each name grants what the form says.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-core --test live_actions -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Method, NewKey, RawBody, RawRequest};
use tessera_core::keys::ACTION_GROUPS;

const NAME: &str = "tessera_it_w";

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

fn request(method: Method, path: &str, query: Option<&str>, body: Option<RawBody>) -> RawRequest {
    RawRequest {
        method,
        path: path.into(),
        query: query.map(str::to_string),
        body,
    }
}

/// The status of `request` with a key that holds only `action`.
async fn status_with(c: &Client, action: &str, request: RawRequest) -> u16 {
    let key = NewKey {
        description: "tessera_it_action".into(),
        actions: vec![action.into()],
        collections: vec!["*".into()],
        expires_at: None,
    };
    let created = c.create_key(&key).await.unwrap();
    let answer = c.with_key(created.value).raw(request).await;
    c.delete_key(created.key.id).await.unwrap();
    answer.unwrap().status
}

fn json(text: &str) -> Option<RawBody> {
    Some(RawBody::Json(text.into()))
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn actions_in_the_catalog_grant_what_they_say() {
    let c = client();
    let stopwords = format!("stopwords/{NAME}");
    let synonyms = format!("synonym_sets/{NAME}");
    let presets = format!("presets/{NAME}");
    let cases = [
        ("aliases:list", request(Method::GET, "aliases", None, None)),
        (
            "stopwords:upsert",
            request(
                Method::PUT,
                &stopwords,
                None,
                json(r#"{"stopwords":["a"]}"#),
            ),
        ),
        (
            "synonym_sets:upsert",
            request(Method::PUT, &synonyms, None, json(r#"{"items":[]}"#)),
        ),
        (
            "presets:upsert",
            request(Method::PUT, &presets, None, json(r#"{"value":{"q":"*"}}"#)),
        ),
        (
            "analytics/rules:list",
            request(Method::GET, "analytics/rules", None, None),
        ),
        (
            "analytics/status:list",
            request(Method::GET, "analytics/status", None, None),
        ),
        (
            "stemming/dictionaries:list",
            request(Method::GET, "stemming/dictionaries", None, None),
        ),
        (
            "stemming/dictionaries/import:create",
            request(
                Method::POST,
                "stemming/dictionaries/import",
                Some("id=tessera_it_w"),
                Some(RawBody::Text(r#"{"word":"a","root":"b"}"#.into())),
            ),
        ),
        (
            "conversations/models:list",
            request(Method::GET, "conversations/models", None, None),
        ),
        (
            "nl_search_models:list",
            request(Method::GET, "nl_search_models", None, None),
        ),
        ("debug:list", request(Method::GET, "debug", None, None)),
    ];
    let mut refused = Vec::new();
    for (action, request) in cases {
        let group = ACTION_GROUPS.iter().any(|g| g.actions.contains(&action));
        assert!(group, "{action} is in no group");
        let status = status_with(&c, action, request).await;
        if status == 401 {
            refused.push(action);
        }
    }
    for path in [
        stopwords,
        synonyms,
        presets,
        format!("stemming/dictionaries/{NAME}"),
    ] {
        c.raw(request(Method::DELETE, &path, None, None))
            .await
            .unwrap();
    }
    assert_eq!(refused, Vec::<&str>::new());
}
