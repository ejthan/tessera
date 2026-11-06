// SPDX-License-Identifier: GPL-3.0-or-later

//! Aliases, stopwords and preset delete against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Alias, Client, ClientOptions, Error, SetKind, StopwordSet};
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

const KINDS: [SetKind; 2] = [SetKind::Synonym, SetKind::Curation];

#[tokio::test]
async fn set_calls_encode_name_and_id() {
    for kind in KINDS {
        let server = MockServer::start().await;
        let set = format!("/{}/a%2Fb%2Cc%20d", kind.path());
        let item = format!("{set}/items/i%2F1%2C%20x");
        mount(
            &server,
            "GET",
            &set,
            r#"{"name":"a/b,c d","items":[{"id":"i"}]}"#,
        )
        .await;
        mount(&server, "DELETE", &set, r#"{"name":"a/b,c d"}"#).await;
        mount(&server, "GET", &item, r#"{"id":"i/1, x"}"#).await;
        mount(&server, "DELETE", &item, r#"{"id":"i/1, x"}"#).await;
        Mock::given(method("PUT"))
            .and(path(set.as_str()))
            .and(body_json(json!({"items":[{"id":"i"}]})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"name":"a/b,c d","items":[{"id":"i"}]}"#),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(item.as_str()))
            .and(body_json(json!({"synonyms":["a","b"]})))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let (name, id) = ("a/b,c d", "i/1, x");
        assert_eq!(c.set(kind, name).await.unwrap().items.len(), 1);
        let items = [json!({"id":"i"})];
        assert_eq!(c.upsert_set(kind, name, &items).await.unwrap().name, name);
        assert_eq!(c.set_item(kind, name, id).await.unwrap()["id"], id);
        let body = json!({"id":id,"synonyms":["a","b"]});
        c.upsert_set_item(kind, name, id, &body).await.unwrap();
        c.delete_set_item(kind, name, id).await.unwrap();
        c.delete_set(kind, name).await.unwrap();
    }
}

#[tokio::test]
async fn sets_decodes_a_bare_array() {
    for kind in KINDS {
        let server = MockServer::start().await;
        mount(
            &server,
            "GET",
            &format!("/{}", kind.path()),
            r#"[{"name":"x","items":[{"id":"1"}]},{"name":"y","items":[]}]"#,
        )
        .await;
        let list = client(&server.uri()).sets(kind).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].items, vec![json!({"id":"1"})]);
        assert_eq!(list[1].name, "y");
    }
}

#[tokio::test]
async fn set_dot_segments_are_refused_without_a_request() {
    let server = MockServer::start().await;
    for verb in ["GET", "PUT", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
    }
    let c = client(&server.uri());
    let refused = |e: &Error| matches!(e, Error::UnaddressableId(_));
    for kind in KINDS {
        for bad in [".", ".."] {
            assert!(refused(&c.set(kind, bad).await.unwrap_err()));
            assert!(refused(&c.upsert_set(kind, bad, &[]).await.unwrap_err()));
            assert!(refused(&c.delete_set(kind, bad).await.unwrap_err()));
            assert!(refused(&c.set_item(kind, "s", bad).await.unwrap_err()));
            assert!(refused(&c.set_item(kind, bad, "i").await.unwrap_err()));
            let item = json!({});
            assert!(refused(
                &c.upsert_set_item(kind, "s", bad, &item).await.unwrap_err()
            ));
            assert!(refused(
                &c.delete_set_item(kind, "s", bad).await.unwrap_err()
            ));
        }
    }
}

#[tokio::test]
async fn a_set_404_keeps_the_server_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404).set_body_string(r#"{"message":"Synonym index not found"}"#),
        )
        .mount(&server)
        .await;
    let c = client(&server.uri());
    for error in [
        c.set(SetKind::Synonym, "a").await.unwrap_err(),
        c.set_item(SetKind::Synonym, "a", "i").await.unwrap_err(),
    ] {
        assert_eq!(error.typesense_message(), Some("Synonym index not found"));
    }
}

#[tokio::test]
async fn set_writes_on_a_read_only_client_are_refused() {
    let server = MockServer::start().await;
    for verb in ["PUT", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
    }
    let c = read_only(&server.uri());
    let refused = |e: Error| matches!(e, Error::ReadOnly { .. });
    for kind in KINDS {
        assert!(refused(c.upsert_set(kind, "a", &[]).await.unwrap_err()));
        assert!(refused(c.delete_set(kind, "a").await.unwrap_err()));
        let item = json!({});
        assert!(refused(
            c.upsert_set_item(kind, "a", "i", &item).await.unwrap_err()
        ));
        assert!(refused(
            c.delete_set_item(kind, "a", "i").await.unwrap_err()
        ));
    }
}
