// SPDX-License-Identifier: GPL-3.0-or-later

//! The streaming export against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, ExportQuery, RequestEvent, RequestHook};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(url: &str) -> Client {
    Client::new(&[url], SecretString::from("k"), ClientOptions::default()).unwrap()
}

fn recorded(client: Client) -> (Client, Arc<Mutex<Vec<RequestEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    (client.with_hook(hook), events)
}

fn query() -> ExportQuery {
    ExportQuery {
        filter_by: None,
        include_fields: None,
        exclude_fields: None,
    }
}

async fn serve(body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/export"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn export_reads_lines() {
    let server = serve("{\"id\":\"1\"}\n{\"id\":\"2\"}").await;
    let mut stream = client(&server.uri())
        .export_stream("c", &query())
        .await
        .unwrap();
    assert_eq!(stream.next_line().await.unwrap().unwrap(), "{\"id\":\"1\"}");
    assert_eq!(stream.next_line().await.unwrap().unwrap(), "{\"id\":\"2\"}");
    assert_eq!(stream.next_line().await.unwrap(), None);
    assert_eq!(stream.next_line().await.unwrap(), None);
}

#[tokio::test]
async fn empty_export_has_no_lines() {
    let server = serve("").await;
    let mut stream = client(&server.uri())
        .export_stream("c", &query())
        .await
        .unwrap();
    assert_eq!(stream.next_line().await.unwrap(), None);
}

#[tokio::test]
async fn export_sends_its_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/export"))
        .and(query_param("filter_by", "a:=1 && b:=2"))
        .and(query_param("include_fields", "a,b"))
        .and(query_param("exclude_fields", "c"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let q = ExportQuery {
        filter_by: Some("a:=1 && b:=2".into()),
        include_fields: Some("a,b".into()),
        exclude_fields: Some("c".into()),
    };
    client(&server.uri()).export_stream("c", &q).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let raw = requests[0].url.query().unwrap();
    assert!(
        raw.contains("filter_by=a%3A%3D1%20%26%26%20b%3A%3D2"),
        "{raw}"
    );
}

#[tokio::test]
async fn export_error_comes_before_the_first_line() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            "{\"message\":\"Could not find a filter field named `bad` in the schema.\"}",
        ))
        .mount(&server)
        .await;
    let e = client(&server.uri())
        .export_stream("c", &query())
        .await
        .err()
        .unwrap();
    assert_eq!(
        e.typesense_message(),
        Some("Could not find a filter field named `bad` in the schema.")
    );
}

#[tokio::test]
async fn slow_body_is_not_cut_by_the_client_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("{\"id\":\"1\"}\n")
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    let c = client(&server.uri()).with_timeout(Duration::from_millis(100));
    let mut stream = c.export_stream("c", &query()).await.unwrap();
    assert!(stream.next_line().await.unwrap().is_some());
}

#[tokio::test]
async fn export_passes_a_read_only_client() {
    let server = serve("{\"id\":\"1\"}").await;
    let c = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            read_only: true,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let mut stream = c.export_stream("c", &query()).await.unwrap();
    assert!(stream.next_line().await.unwrap().is_some());
}

#[tokio::test]
async fn one_event_per_stream_with_the_total_size() {
    let body = "{\"id\":\"1\"}\n{\"id\":\"22\"}\n";
    let server = serve(body).await;
    let (c, events) = recorded(client(&server.uri()));
    let mut stream = c.export_stream("c", &query()).await.unwrap();
    while stream.next_line().await.unwrap().is_some() {}
    assert!(events.lock().unwrap().is_empty(), "reported before the end");
    drop(stream);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.size, Some(body.len() as u64));
    assert_eq!(e.status, Some(200));
    assert!(e.request_body.is_none() && e.response_body.is_none());
    assert_eq!(e.error, None);
    assert_eq!(e.path, "/collections/c/documents/export");
}

#[tokio::test]
async fn dropped_stream_logs_cancelled() {
    let server = serve("{\"id\":\"1\"}\n{\"id\":\"2\"}\n").await;
    let (c, events) = recorded(client(&server.uri()));
    let mut stream = c.export_stream("c", &query()).await.unwrap();
    assert!(stream.next_line().await.unwrap().is_some());
    drop(stream);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].error.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn a_failed_start_is_one_event_with_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400).set_body_string("{\"message\":\"no\"}"))
        .mount(&server)
        .await;
    let (c, events) = recorded(client(&server.uri()));
    assert!(matches!(
        c.export_stream("c", &query()).await.err(),
        Some(Error::Http { status: 400, .. })
    ));
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].status, events[0].error.clone()),
        (Some(400), None)
    );
}
