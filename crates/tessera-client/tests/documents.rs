// SPDX-License-Identifier: GPL-3.0-or-later

//! Document read endpoints against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{
    BODY_LIMIT, Client, ClientOptions, DirtyValues, DocumentQuery, Error, ImportAction,
    ImportParams, RequestEvent, RequestHook,
};
use wiremock::matchers::{body_string, header, method, path, query_param};
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
        .and(query_param("enable_curations", "false"))
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
        .and(query_param("enable_curations", "false"))
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

#[tokio::test]
async fn quiet_client_keeps_no_full_body() {
    let server = MockServer::start().await;
    let long = SEARCH.replacen(
        "\"found\":2",
        &format!("\"pad\":\"{}\",\"found\":2", "a".repeat(BODY_LIMIT)),
        1,
    );
    mock(&server, "/collections/c/documents/search", &long).await;
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
    let kept = events[0].response_body.as_ref().unwrap();
    assert_eq!(kept.full.as_deref(), Some(long.as_str()));
    assert!(events[1].response_body.is_none());
}

#[tokio::test]
async fn document_writes_send_method_path_query_and_body() {
    let server = MockServer::start().await;
    let doc = serde_json::json!({"id":"a b","n":1});
    Mock::given(method("POST"))
        .and(path("/collections/c/documents"))
        .and(wiremock::matchers::body_json(&doc))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"id":"a b","n":1}"#))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/collections/c/documents"))
        .and(query_param("action", "upsert"))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"id":"u"}"#))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/collections/c/documents/a%20b"))
        .and(wiremock::matchers::body_json(
            serde_json::json!({"a":{"b":null}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"a b"}"#))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/collections/c/documents/a%20b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"a b"}"#))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    assert_eq!(c.create_document("c", &doc).await.unwrap()["id"], "a b");
    // Plain create must not carry the upsert action.
    let first = &server.received_requests().await.unwrap()[0];
    assert_eq!(first.url.query(), None);
    assert_eq!(
        c.upsert_document("c", &serde_json::json!({"id":"u"}))
            .await
            .unwrap()["id"],
        "u"
    );
    c.patch_document("c", "a b", &serde_json::json!({"a":{"b":null}}))
        .await
        .unwrap();
    c.delete_document("c", "a b").await.unwrap();
}

#[tokio::test]
async fn document_write_errors_surface_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(409).set_body_string(r#"{"message":"exists"}"#))
        .mount(&server)
        .await;
    let err = client(&server.uri())
        .create_document("c", &serde_json::json!({"id":"a"}))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 409, .. }));
}

#[tokio::test]
async fn read_only_client_refuses_document_writes() {
    let server = MockServer::start().await;
    let locked = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            read_only: true,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let v = serde_json::json!({"id":"a"});
    assert!(matches!(
        locked.create_document("c", &v).await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(matches!(
        locked.upsert_document("c", &v).await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(matches!(
        locked.patch_document("c", "a", &v).await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(matches!(
        locked.delete_document("c", "a").await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_by_filter_sends_the_filter_and_reads_the_count() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/collections/c/documents"))
        .and(query_param("filter_by", "a:=1 && b:=2"))
        .and(query_param("batch_size", "1000"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(300))
                .set_body_string(r#"{"num_deleted":42}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    // The delay exceeds the profile timeout: the long timeout applies.
    let client = client(&server.uri()).with_timeout(Duration::from_millis(100));
    assert_eq!(
        client.delete_by_filter("c", "a:=1 && b:=2").await.unwrap(),
        42
    );
}

#[tokio::test]
async fn update_by_filter_sends_the_partial_and_reads_the_count() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/collections/c/documents"))
        .and(query_param("filter_by", "a:=1"))
        .and(wiremock::matchers::body_json(serde_json::json!({"x":2})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(300))
                .set_body_string(r#"{"num_updated":7}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server.uri()).with_timeout(Duration::from_millis(100));
    let n = client
        .update_by_filter("c", "a:=1", &serde_json::json!({"x":2}))
        .await
        .unwrap();
    assert_eq!(n, 7);
}

#[tokio::test]
async fn read_only_client_refuses_bulk_writes() {
    let server = MockServer::start().await;
    let locked = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            read_only: true,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    assert!(matches!(
        locked.delete_by_filter("c", "a:=1").await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(matches!(
        locked
            .update_by_filter("c", "a:=1", &serde_json::json!({}))
            .await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_or_non_numeric_count_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"num_updated":3}"#))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"num_updated":"x"}"#))
        .mount(&server)
        .await;
    let c = client(&server.uri());
    assert!(matches!(
        c.delete_by_filter("c", "a:=1").await,
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        c.update_by_filter("c", "a:=1", &serde_json::json!({}))
            .await,
        Err(Error::Decode(_))
    ));
}

#[tokio::test]
async fn dot_collections_never_reach_the_server() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::new(&[&server.uri()], "k".to_string().into(), Default::default()).unwrap();
    for name in ["", ".", ".."] {
        let refused = |e: &Error| matches!(e, Error::UnaddressableId(n) if n == name);
        assert!(refused(&client.delete_collection(name).await.unwrap_err()));
        assert!(refused(&client.document(name, "a").await.unwrap_err()));
        let filtered = client.delete_by_filter(name, "id:[`a`]").await;
        assert!(refused(&filtered.unwrap_err()));
    }
}

#[tokio::test]
async fn dot_ids_never_reach_the_server() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::new(&[&server.uri()], "k".to_string().into(), Default::default()).unwrap();
    for id in ["", ".", ".."] {
        let refused = |e: &Error| matches!(e, Error::UnaddressableId(i) if i == id);
        assert!(refused(&client.document("c", id).await.unwrap_err()));
        assert!(refused(&client.delete_document("c", id).await.unwrap_err()));
        let patched = client.patch_document("c", id, &serde_json::json!({})).await;
        assert!(refused(&patched.unwrap_err()));
    }
}

fn params(action: ImportAction, dirty_values: DirtyValues) -> ImportParams {
    ImportParams {
        action,
        batch_size: 500,
        dirty_values,
    }
}

#[tokio::test]
async fn import_sends_action_and_options() {
    let actions = [
        (ImportAction::Create, "create"),
        (ImportAction::Upsert, "upsert"),
        (ImportAction::Update, "update"),
        (ImportAction::Emplace, "emplace"),
    ];
    let dirty = [
        (DirtyValues::CoerceOrReject, "coerce_or_reject"),
        (DirtyValues::CoerceOrDrop, "coerce_or_drop"),
        (DirtyValues::Drop, "drop"),
        (DirtyValues::Reject, "reject"),
    ];
    for (action, a) in actions {
        for (value, d) in dirty {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/collections/c/documents/import"))
                .and(query_param("action", a))
                .and(query_param("batch_size", "500"))
                .and(query_param("dirty_values", d))
                .and(query_param("return_id", "true"))
                .and(header("content-type", "text/plain"))
                .and(body_string("{\"id\":\"1\"}"))
                .respond_with(ResponseTemplate::new(200).set_body_string("{\"success\":true}\n"))
                .expect(1)
                .mount(&server)
                .await;
            let lines = client(&server.uri())
                .import_batch("c", &params(action, value), "{\"id\":\"1\"}")
                .await
                .unwrap();
            assert_eq!(lines.len(), 1);
        }
    }
}

async fn import_answer(answer: &str) -> Vec<tessera_client::ImportLine> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/collections/c/documents/import"))
        .respond_with(ResponseTemplate::new(200).set_body_string(answer))
        .mount(&server)
        .await;
    client(&server.uri())
        .import_batch(
            "c",
            &params(ImportAction::Create, DirtyValues::default()),
            "x",
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn import_decodes_lines_in_order() {
    let lines = import_answer(
        "{\"success\":true,\"id\":\"1\"}\n{\"code\":400,\"error\":\"Field `n` must be an int32.\",\"id\":\"2\",\"success\":false}\n",
    )
    .await;
    assert_eq!(lines.len(), 2);
    assert!(lines[0].success);
    assert_eq!(lines[0].id.as_deref(), Some("1"));
    assert_eq!((lines[0].code, lines[0].error.as_deref()), (None, None));
    assert!(!lines[1].success);
    assert_eq!(lines[1].code, Some(400));
    assert_eq!(
        lines[1].error.as_deref(),
        Some("Field `n` must be an int32.")
    );
    assert_eq!(lines[1].id.as_deref(), Some("2"));
}

#[tokio::test]
async fn import_keeps_an_undecodable_line() {
    let lines = import_answer("{\"success\":true}\nnot json\n").await;
    assert_eq!(lines.len(), 2);
    assert!(!lines[1].success);
    assert_eq!(lines[1].error.as_deref(), Some("not json"));
    assert_eq!((lines[1].code, lines[1].id.as_deref()), (None, None));
}

#[tokio::test]
async fn import_empty_answer_is_empty() {
    assert!(import_answer("").await.is_empty());
}

#[tokio::test]
async fn import_error_status_is_http() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/collections/gone/documents/import"))
        .respond_with(
            ResponseTemplate::new(404).set_body_string("{\"message\":\"Collection not found\"}"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/collections/busy/documents/import"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Not Ready"))
        .mount(&server)
        .await;
    let c = client(&server.uri());
    let p = params(ImportAction::Create, DirtyValues::default());
    let e = c.import_batch("gone", &p, "x").await.unwrap_err();
    assert_eq!(e.typesense_message(), Some("Collection not found"));
    let e = c.import_batch("busy", &p, "x").await.unwrap_err();
    assert!(matches!(e, Error::Http { status: 503, .. }), "{e:?}");
}

#[tokio::test]
async fn import_is_refused_when_read_only() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let c = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            read_only: true,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let e = c
        .import_batch(
            "c",
            &params(ImportAction::Create, DirtyValues::default()),
            "x",
        )
        .await;
    assert!(matches!(e, Err(Error::ReadOnly { .. })));
}

#[tokio::test]
async fn import_event_has_no_bodies() {
    let server = MockServer::start().await;
    let answer = "{\"success\":true}\n";
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(answer))
        .mount(&server)
        .await;
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let c = client(&server.uri()).with_hook(hook);
    let p = params(ImportAction::Create, DirtyValues::default());
    let big = "a".repeat(BODY_LIMIT + 10);
    c.import_batch("c", &p, "{\"id\":\"1\"}").await.unwrap();
    c.import_batch("c", &p, &big).await.unwrap();
    let events = events.lock().unwrap();
    for e in events.iter() {
        assert!(e.request_body.is_none() && e.response_body.is_none());
        assert_eq!((e.status, e.size), (Some(200), Some(answer.len() as u64)));
    }
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn without_bodies_keeps_other_clients_logging() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"success\":true}\n"))
        .mount(&server)
        .await;
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let c = client(&server.uri()).with_hook(hook);
    let _quiet = c.without_bodies();
    c.create_document("c", &serde_json::json!({"id":"1"}))
        .await
        .unwrap();
    let events = events.lock().unwrap();
    assert!(events[0].request_body.is_some());
    assert!(events[0].response_body.is_some());
}
