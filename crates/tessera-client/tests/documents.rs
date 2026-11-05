// SPDX-License-Identifier: GPL-3.0-or-later

//! Document read endpoints against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, DocumentQuery, Error, RequestEvent, RequestHook};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(url: &str) -> Client {
    Client::new(&[url], SecretString::from("k"), ClientOptions::default()).unwrap()
}

async fn mock(server: &MockServer, p: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

const SEARCH: &str = r#"{"found":2,"out_of":5,"page":3,"search_time_ms":7,"hits":[{"document":{"id":"1","a":1},"highlight":{},"highlights":[]}],"facet_counts":[]}"#;

#[tokio::test]
async fn search_builds_the_query_and_decodes_hits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/search"))
        .and(query_param("q", "*"))
        .and(query_param("per_page", "250"))
        .and(query_param("page", "3"))
        .and(query_param("filter_by", "year:>2000 && a:=b"))
        .and(query_param("sort_by", "price:desc"))
        .and(query_param("exclude_fields", "embedding"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEARCH))
        .expect(1)
        .mount(&server)
        .await;
    let query = DocumentQuery {
        page: 3,
        per_page: 250,
        filter_by: Some("year:>2000 && a:=b".into()),
        sort_by: Some("price:desc".into()),
        exclude_fields: Some("embedding".into()),
    };
    let page = client(&server.uri())
        .search_documents("c", &query)
        .await
        .unwrap();
    assert_eq!((page.found, page.out_of, page.page), (2, 5, 3));
    assert_eq!(page.search_time_ms, 7);
    assert_eq!(page.documents.len(), 1);
    assert_eq!(page.documents[0]["a"], 1);
    let requests = server.received_requests().await.unwrap();
    let raw = requests[0].url.query().unwrap();
    assert!(raw.contains("%26%26"), "{raw}");
    assert!(!raw.contains("&&"), "{raw}");
}

#[tokio::test]
async fn count_uses_per_page_zero() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/search"))
        .and(query_param("per_page", "0"))
        .and(query_param("filter_by", "a:=1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"found":9,"out_of":9,"page":1,"search_time_ms":0,"hits":[]}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    let n = client(&server.uri())
        .count_documents("c", Some("a:=1"))
        .await
        .unwrap();
    assert_eq!(n, 9);
}

const FACETS: &str = r#"{"found":5,"hits":[],"facet_counts":[{"counts":[{"count":1,"highlighted":"crime","value":"crime"}],"field_name":"genre"}]}"#;

#[tokio::test]
async fn facet_values_reads_counts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/search"))
        .and(query_param("per_page", "0"))
        .and(query_param("facet_by", "genre"))
        .and(query_param("max_facet_values", "10"))
        .and(query_param("facet_query", "genre:cr"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FACETS))
        .expect(1)
        .mount(&server)
        .await;
    let values = client(&server.uri())
        .facet_values("c", "genre", "cr", 10)
        .await
        .unwrap();
    assert_eq!(values, ["crime"]);
}

#[tokio::test]
async fn facet_values_without_prefix_sends_no_facet_query() {
    let server = MockServer::start().await;
    mock(&server, "/collections/c/documents/search", FACETS).await;
    client(&server.uri())
        .facet_values("c", "genre", "", 10)
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].url.query().unwrap().contains("facet_query"));
}

#[tokio::test]
async fn document_id_is_one_segment() {
    let server = MockServer::start().await;
    mock(
        &server,
        "/collections/c/documents/a%2Fb%20c%3Fx",
        r#"{"id":"x"}"#,
    )
    .await;
    let doc = client(&server.uri())
        .document("c", "a/b c?x")
        .await
        .unwrap();
    assert_eq!(doc["id"], "x");
}

#[tokio::test]
async fn missing_document_is_http_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string(r#"{"message":"Could not find a document with id: 1"}"#),
        )
        .mount(&server)
        .await;
    let err = client(&server.uri()).document("c", "1").await.unwrap_err();
    assert!(matches!(err, Error::Http { status: 404, .. }), "{err:?}");
    assert_eq!(
        err.typesense_message(),
        Some("Could not find a document with id: 1")
    );
}

#[tokio::test]
async fn export_uses_the_long_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/export"))
        .and(query_param("filter_by", "a:=1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(300))
                .set_body_string("{\"id\":\"1\"}\n{\"id\":\"2\"}"),
        )
        .mount(&server)
        .await;
    let client = client(&server.uri()).with_timeout(Duration::from_millis(100));
    let jsonl = client.export_documents("c", "a:=1").await.unwrap();
    assert_eq!(jsonl.lines().count(), 2);
}

#[tokio::test]
async fn quiet_client_logs_size_but_no_body() {
    let server = MockServer::start().await;
    mock(&server, "/collections/c/documents/search", SEARCH).await;
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let plain = client(&server.uri()).with_hook(hook);
    let query = DocumentQuery {
        page: 1,
        per_page: 10,
        filter_by: None,
        sort_by: None,
        exclude_fields: None,
    };
    plain.search_documents("c", &query).await.unwrap();
    plain
        .without_response_bodies()
        .search_documents("c", &query)
        .await
        .unwrap();
    let events = events.lock().unwrap();
    assert!(events[0].response_body.is_some());
    assert!(events[1].response_body.is_none());
    assert_eq!(events[1].size, Some(SEARCH.len() as u64));
    assert_eq!(events[1].status, Some(200));
}
