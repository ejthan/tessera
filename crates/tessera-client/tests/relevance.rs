// SPDX-License-Identifier: GPL-3.0-or-later

//! Aliases, stopwords and preset delete against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Alias, Client, ClientOptions, Error, StopwordSet};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_with(url: &str, options: ClientOptions) -> Client {
    Client::new(&[url], SecretString::from("k"), options).unwrap()
}

fn client(url: &str) -> Client {
    client_with(url, ClientOptions::default())
}

fn read_only(url: &str) -> Client {
    client_with(
        url,
        ClientOptions {
            read_only: true,
            ..ClientOptions::default()
        },
    )
}

/// Wiremock matches the percent-encoded path as it was sent.
async fn mount(server: &MockServer, verb: &str, at: &str, body: &str) {
    Mock::given(method(verb))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(server)
        .await;
}

fn alias_json() -> &'static str {
    r#"{"name":"a/b c","collection_name":"c"}"#
}

#[tokio::test]
async fn alias_calls_encode_the_name() {
    let server = MockServer::start().await;
    let at = "/aliases/a%2Fb%20c";
    mount(&server, "GET", at, alias_json()).await;
    mount(&server, "DELETE", at, alias_json()).await;
    Mock::given(method("PUT"))
        .and(path(at))
        .and(body_json(json!({"collection_name":"c"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(alias_json()))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    let expected = Alias {
        name: "a/b c".into(),
        collection_name: "c".into(),
    };
    assert_eq!(c.alias("a/b c").await.unwrap(), expected);
    assert_eq!(c.upsert_alias("a/b c", "c").await.unwrap(), expected);
    c.delete_alias("a/b c").await.unwrap();
}

#[tokio::test]
async fn preset_delete_encodes_the_name() {
    let server = MockServer::start().await;
    mount(&server, "DELETE", "/presets/a%2Fb%20c", "{}").await;
    client(&server.uri()).delete_preset("a/b c").await.unwrap();
}

#[tokio::test]
async fn stopword_calls_encode_the_name() {
    let server = MockServer::start().await;
    let at = "/stopwords/a%2Fb%20c";
    mount(
        &server,
        "GET",
        at,
        r#"{"stopwords":{"id":"a/b c","stopwords":["a"]}}"#,
    )
    .await;
    mount(&server, "DELETE", at, r#"{"id":"a/b c"}"#).await;
    let c = client(&server.uri());
    assert_eq!(
        c.stopword_set("a/b c").await.unwrap(),
        StopwordSet {
            id: "a/b c".into(),
            stopwords: vec!["a".into()],
            locale: None,
        }
    );
    c.delete_stopwords("a/b c").await.unwrap();
}

#[tokio::test]
async fn dot_segments_are_refused_without_a_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    for bad in [".", ".."] {
        let refused = |e: &Error| matches!(e, Error::UnaddressableId(_));
        assert!(refused(&c.alias(bad).await.unwrap_err()));
        assert!(refused(&c.upsert_alias(bad, "c").await.unwrap_err()));
        assert!(refused(&c.delete_alias(bad).await.unwrap_err()));
        assert!(refused(&c.delete_preset(bad).await.unwrap_err()));
        assert!(refused(&c.stopword_set(bad).await.unwrap_err()));
        assert!(refused(
            &c.upsert_stopwords(bad, &[], None).await.unwrap_err()
        ));
        assert!(refused(&c.delete_stopwords(bad).await.unwrap_err()));
    }
}

#[tokio::test]
async fn upsert_stopwords_leaves_out_a_missing_locale() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/stopwords/s"))
        .and(body_json(json!({"stopwords":["a"]})))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/stopwords/s"))
        .and(body_json(json!({"stopwords":["a"],"locale":"en"})))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    let words = ["a".to_string()];
    c.upsert_stopwords("s", &words, None).await.unwrap();
    c.upsert_stopwords("s", &words, Some("en")).await.unwrap();
}

#[tokio::test]
async fn stopwords_reads_the_list() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/stopwords",
        r#"{"stopwords":[{"id":"x","stopwords":["a"],"locale":"en"},{"id":"y","stopwords":[]}]}"#,
    )
    .await;
    let list = client(&server.uri()).stopwords().await.unwrap();
    assert_eq!(list[0].locale.as_deref(), Some("en"));
    assert_eq!(list[1].id, "y");
    assert_eq!(list[1].locale, None);
}

#[tokio::test]
async fn a_404_keeps_the_server_message() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"message":"Not Found"}"#))
        .mount(&server)
        .await;
    let c = client(&server.uri());
    for error in [
        c.delete_alias("a").await.unwrap_err(),
        c.delete_preset("a").await.unwrap_err(),
        c.delete_stopwords("a").await.unwrap_err(),
    ] {
        assert_eq!(error.typesense_message(), Some("Not Found"));
    }
}

#[tokio::test]
async fn writes_on_a_read_only_client_are_refused() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let c = read_only(&server.uri());
    let refused = |e: Error| matches!(e, Error::ReadOnly { .. });
    assert!(refused(c.upsert_alias("a", "c").await.unwrap_err()));
    assert!(refused(c.delete_alias("a").await.unwrap_err()));
    assert!(refused(c.delete_preset("a").await.unwrap_err()));
    assert!(refused(
        c.upsert_stopwords("a", &[], None).await.unwrap_err()
    ));
    assert!(refused(c.delete_stopwords("a").await.unwrap_err()));
}
