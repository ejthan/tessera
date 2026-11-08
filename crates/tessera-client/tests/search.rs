// SPDX-License-Identifier: GPL-3.0-or-later

//! Search, multi-search and presets against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{
    Client, ClientOptions, Error, GET_QUERY_LIMIT, Method, MultiSearch, SearchRoute, multi_route,
    search_route,
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HITS: &str = r#"{"found":1,"hits":[]}"#;

fn client_with(urls: &[&str], options: ClientOptions) -> Client {
    Client::new(urls, SecretString::from("k"), options).unwrap()
}

fn client(url: &str) -> Client {
    client_with(&[url], ClientOptions::default())
}

fn pair(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

/// Parameters whose encoded query is exactly `len` bytes (`q=` plus letters).
fn params_of(len: usize) -> Vec<(String, String)> {
    vec![pair("q", &"a".repeat(len - 2))]
}

async fn mount_get(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HITS))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_post(server: &MockServer, body: &str) {
    Mock::given(method("POST"))
        .and(path("/multi_search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn short_query_goes_by_get() {
    let server = MockServer::start().await;
    mount_get(&server).await;
    let text = client(&server.uri())
        .search("c", &[pair("q", "dune")])
        .await
        .unwrap();
    assert_eq!(text, HITS);
}

#[tokio::test]
async fn query_of_3999_bytes_stays_get() {
    assert_eq!(GET_QUERY_LIMIT, 4000);
    let params = params_of(3999);
    assert!(matches!(
        search_route("c", &params).unwrap(),
        SearchRoute::Get { .. }
    ));
    let server = MockServer::start().await;
    mount_get(&server).await;
    client(&server.uri()).search("c", &params).await.unwrap();
}

#[tokio::test]
async fn query_of_4000_bytes_goes_by_post() {
    // "q=" + letters + "&filter_by=" is 4000 bytes.
    let q_len = 4000 - 2 - 11;
    let params = vec![pair("q", &"a".repeat(q_len)), pair("filter_by", "")];
    let server = MockServer::start().await;
    mount_post(&server, r#"{"results":[{"found":1,"hits":[]}]}"#).await;
    let text = client(&server.uri()).search("c", &params).await.unwrap();
    assert_eq!(text, r#"{"found":1,"hits":[]}"#);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, wiremock::http::Method::POST);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let searches = body["searches"].as_array().unwrap();
    assert_eq!(searches.len(), 1);
    assert_eq!(searches[0]["collection"], "c");
    assert_eq!(searches[0]["q"].as_str().unwrap().len(), q_len);
    assert_eq!(searches[0]["filter_by"], "");
    assert!(requests[0].url.query().is_none());
}

#[tokio::test]
async fn long_filter_reaches_the_server_whole() {
    let filter = format!("genre:=[{}]", "`x`,".repeat(2000));
    let params = vec![pair("q", "*"), pair("filter_by", &filter)];
    let SearchRoute::Post { body, .. } = search_route("c", &params).unwrap() else {
        panic!("expected POST");
    };
    assert_eq!(body["searches"][0]["filter_by"], filter.as_str());
}

#[tokio::test]
async fn read_only_client_searches_and_multi_searches() {
    let server = MockServer::start().await;
    mount_post(&server, r#"{"results":[{"found":1}]}"#).await;
    let options = ClientOptions {
        read_only: true,
        ..ClientOptions::default()
    };
    let client = client_with(&[&server.uri()], options);
    let long = params_of(5000);
    assert!(client.search("c", &long).await.is_ok());
    let request = MultiSearch {
        union: false,
        searches: vec![
            json!({"collection":"c","q":"x"})
                .as_object()
                .unwrap()
                .clone(),
        ],
        page: None,
        per_page: None,
    };
    assert!(client.multi_search(&request).await.is_ok());
    let raw = client.request(Method::POST, "multi_search").await;
    assert!(matches!(raw, Err(Error::ReadOnly { .. })), "{raw:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn every_search_reaches_the_request_log_once() {
    let server = MockServer::start().await;
    mount_post(&server, r#"{"results":[{"found":1}]}"#).await;
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = events.clone();
    let client = client(&server.uri()).with_hook(std::sync::Arc::new(move |e| {
        sink.lock().unwrap().push(e.path.clone());
    }));
    client.search("c", &params_of(5000)).await.unwrap();
    let request = MultiSearch {
        union: false,
        searches: vec![json!({"collection":"c"}).as_object().unwrap().clone()],
        page: None,
        per_page: None,
    };
    client.multi_search(&request).await.unwrap();
    assert_eq!(events.lock().unwrap().len(), 2, "{events:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn multi_search_fails_over() {
    let server = MockServer::start().await;
    mount_post(&server, r#"{"results":[{"found":1}]}"#).await;
    let client = client_with(
        &["http://127.0.0.1:9", &server.uri()],
        ClientOptions::default(),
    );
    let request = MultiSearch {
        union: false,
        searches: vec![json!({"collection":"c"}).as_object().unwrap().clone()],
        page: None,
        per_page: None,
    };
    let text = client.multi_search(&request).await.unwrap();
    assert!(text.contains("\"found\":1"));
    assert_eq!(client.active_node(), 1);
}

#[tokio::test]
async fn union_error_is_http() {
    let server = MockServer::start().await;
    mount_post(&server, r#"{"code":400,"error":"Expected size"}"#).await;
    let request = MultiSearch {
        union: true,
        searches: vec![json!({"collection":"c"}).as_object().unwrap().clone()],
        page: None,
        per_page: None,
    };
    let err = client(&server.uri())
        .multi_search(&request)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Http { status: 400, message, .. } if message == "Expected size"),
        "{err:?}"
    );
    assert_eq!(err.typesense_message(), Some("Expected size"));
}

#[tokio::test]
async fn single_search_fallback_maps_its_error() {
    let server = MockServer::start().await;
    mount_post(&server, r#"{"results":[{"code":404,"error":"x"}]}"#).await;
    let err = client(&server.uri())
        .search("c", &params_of(5000))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Http { status: 404, message, .. } if message == "x"),
        "{err:?}"
    );
    assert_eq!(err.typesense_message(), Some("x"));
}

#[tokio::test]
async fn federated_error_stays_in_the_text() {
    let server = MockServer::start().await;
    let body = r#"{"results":[{"found":1},{"code":404,"error":"Collection not found"}]}"#;
    mount_post(&server, body).await;
    let request = MultiSearch {
        union: false,
        searches: vec![
            json!({"collection":"a"}).as_object().unwrap().clone(),
            json!({"collection":"b"}).as_object().unwrap().clone(),
        ],
        page: None,
        per_page: None,
    };
    let text = client(&server.uri()).multi_search(&request).await.unwrap();
    assert_eq!(text, body);
}

#[tokio::test]
async fn union_page_goes_in_the_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/multi_search"))
        .and(query_param("page", "2"))
        .and(query_param("per_page", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HITS))
        .expect(1)
        .mount(&server)
        .await;
    let request = MultiSearch {
        union: true,
        searches: vec![
            json!({"collection":"c","q":"x"})
                .as_object()
                .unwrap()
                .clone(),
        ],
        page: Some(2),
        per_page: Some(5),
    };
    client(&server.uri()).multi_search(&request).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["union"], true);
    assert!(body.get("page").is_none() && body["searches"][0].get("page").is_none());
}

#[test]
fn union_page_defaults_to_one_and_federated_has_no_query() {
    let search = json!({"collection":"c"}).as_object().unwrap().clone();
    let mut request = MultiSearch {
        union: true,
        searches: vec![search],
        page: None,
        per_page: None,
    };
    let SearchRoute::Post { query, body } = multi_route(&request) else {
        panic!("expected POST");
    };
    assert_eq!(query.as_deref(), Some("page=1"));
    assert_eq!(body["union"], true);
    request.union = false;
    let SearchRoute::Post { query, body } = multi_route(&request) else {
        panic!("expected POST");
    };
    assert_eq!(query, None);
    assert!(body.get("union").is_none());
}

#[tokio::test]
async fn preset_name_is_one_segment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/presets/a%2Fb"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"name":"a/b","value":{}}"#))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    c.preset("a/b").await.unwrap();
    for bad in [".", "..", ""] {
        assert!(
            matches!(c.preset(bad).await, Err(Error::UnaddressableId(_))),
            "{bad}"
        );
        assert!(matches!(
            c.upsert_preset(bad, &json!({})).await,
            Err(Error::UnaddressableId(_))
        ));
    }
}

#[tokio::test]
async fn presets_list_and_upsert() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/presets"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"presets":[{"name":"p","value":{"q":"x"}}]}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/presets/p"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"name":"p","value":{}}"#))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    let list = c.presets().await.unwrap();
    assert_eq!(
        (list[0].name.as_str(), &list[0].value),
        ("p", &json!({"q":"x"}))
    );
    c.upsert_preset("p", &json!({"searches":[]})).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let put: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(put, json!({"value":{"searches":[]}}));
}
