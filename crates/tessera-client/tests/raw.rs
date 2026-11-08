// SPDX-License-Identifier: GPL-3.0-or-later

//! The console's raw requests against wiremock. The node check is a security boundary: a
//! second server stands in for the foreign host, so a leak would reach it with the key.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, Method, RawBody, RawRequest};
use wiremock::matchers::{any, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_with(url: &str, read_only: bool) -> Client {
    let options = ClientOptions {
        read_only,
        ..ClientOptions::default()
    };
    Client::new(&[url], SecretString::from("secret-key"), options).unwrap()
}

fn request(method: Method, path: &str) -> RawRequest {
    RawRequest {
        method,
        path: path.into(),
        query: None,
        body: None,
    }
}

async fn answer_anything(server: &MockServer) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}

async fn received(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn raw_returns_status_headers_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections"))
        .and(header("X-TYPESENSE-API-KEY", "secret-key"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("x-demo", "yes")
                .set_body_string("[]"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let response = client_with(&server.uri(), false)
        .raw(RawRequest {
            query: Some("limit=1".into()),
            ..request(Method::GET, "collections")
        })
        .await
        .unwrap();
    assert_eq!((response.status, response.body.as_str()), (201, "[]"));
    assert!(
        response
            .headers
            .contains(&("x-demo".to_string(), "yes".to_string()))
    );
    let seen = &server.received_requests().await.unwrap()[0];
    assert_eq!(seen.url.query(), Some("limit=1"));
}

#[tokio::test]
async fn raw_header_that_is_not_utf8_shows_lossily() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).append_header(
            "x-latin",
            wiremock::http::HeaderValue::from_bytes(b"caf\xE9").unwrap(),
        ))
        .mount(&server)
        .await;
    let response = client_with(&server.uri(), false)
        .raw(request(Method::GET, "health"))
        .await
        .unwrap();
    assert!(
        response
            .headers
            .contains(&("x-latin".to_string(), "caf\u{FFFD}".to_string()))
    );
}

#[tokio::test]
async fn raw_refuses_write_on_read_only() {
    let server = MockServer::start().await;
    answer_anything(&server).await;
    let client = client_with(&server.uri(), true);
    for (verb, target) in [
        (Method::POST, "collections"),
        (Method::PUT, "collections/a"),
        (Method::PATCH, "collections/a"),
        (Method::DELETE, "collections/a"),
        (Method::POST, "multi_search"),
    ] {
        let result = client.raw(request(verb.clone(), target)).await;
        assert!(
            matches!(&result, Err(Error::ReadOnly { method }) if *method == verb),
            "{verb} {target}: {result:?}"
        );
    }
    assert_eq!(received(&server).await, 0);
}

#[tokio::test]
async fn raw_passes_get_and_head_on_read_only() {
    let server = MockServer::start().await;
    answer_anything(&server).await;
    let client = client_with(&server.uri(), true);
    for verb in [Method::GET, Method::HEAD] {
        let response = client.raw(request(verb, "health")).await.unwrap();
        assert_eq!(response.status, 200);
    }
    assert_eq!(received(&server).await, 2);
}

#[tokio::test]
async fn raw_404_is_ok() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"message":"Not Found"}"#))
        .mount(&server)
        .await;
    let response = client_with(&server.uri(), false)
        .raw(request(Method::GET, "nope"))
        .await
        .unwrap();
    assert_eq!(response.status, 404);
    assert!(response.body.contains("Not Found"));
}

/// Every spelling that would leave the node if it were joined blindly. `{evil}` is the
/// `host:port` of a server that must never see a request.
const FOREIGN: [&str; 22] = [
    "//{evil}/x",
    "///{evil}/x",
    "/\\{evil}",
    "/\\{evil}/x",
    "\\\\{evil}/x",
    "\\/{evil}/x",
    "/\t/{evil}/x",
    "/\n/{evil}/x",
    " //{evil}/x",
    "http://{evil}/",
    "HTTP://{evil}/",
    "https://{evil}/",
    "ht\ttp://{evil}/",
    "http:\\\\{evil}/",
    "http:/{evil}/",
    "http:{evil}/x",
    "http:x",
    "https:x",
    "ftp://{evil}/",
    "//user:pw@{evil}/x",
    "//{evil}%2Fx",
    "//127.0.0.1%3A1/x",
];

#[tokio::test]
async fn raw_never_leaves_the_node() {
    let node = MockServer::start().await;
    let evil = MockServer::start().await;
    answer_anything(&node).await;
    answer_anything(&evil).await;
    let host = evil.address().to_string();
    let client = client_with(&node.uri(), false);
    for spelling in FOREIGN {
        let path = spelling.replace("{evil}", &host);
        for verb in [Method::GET, Method::DELETE] {
            let result = client.raw(request(verb, &path)).await;
            assert!(
                matches!(result, Err(Error::InvalidUrl(_))),
                "{path:?}: {result:?}"
            );
        }
    }
    assert_eq!(received(&evil).await, 0, "the key reached a foreign host");
    assert_eq!(received(&node).await, 0);
}

/// `Url::join` collapses these, on any base path, so they would address another resource.
const TRAVERSAL: [&str; 17] = [
    "..",
    " ../keys",
    "../keys ",
    " %2e%2e/keys",
    "../keys",
    "./keys",
    "a/../keys",
    "%2e%2e/keys",
    "%2E%2e/keys",
    ".%2e/keys",
    "%2e./keys",
    "a/%2e%2e/keys",
    "a/%2E/keys",
    "collections/..%2f..%2fkeys",
    "collections/%2e%2e%2f%2e%2e%2fkeys",
    "collections/..%5C..%5Ckeys",
    "collections\\..\\..\\keys",
];

#[tokio::test]
async fn raw_refuses_dot_segments_on_any_base() {
    let server = MockServer::start().await;
    answer_anything(&server).await;
    for base in [server.uri(), format!("{}/ts", server.uri())] {
        let client = client_with(&base, false);
        for path in TRAVERSAL {
            let result = client.raw(request(Method::DELETE, path)).await;
            assert!(
                matches!(result, Err(Error::InvalidUrl(_))),
                "{base} {path:?}: {result:?}"
            );
        }
    }
    assert_eq!(received(&server).await, 0);
}

#[tokio::test]
async fn base_path_prefix_is_kept() {
    let server = MockServer::start().await;
    Mock::given(path("/ts/collections"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_with(&format!("{}/ts/", server.uri()), false);
    let response = client.raw(request(Method::GET, "collections")).await;
    assert_eq!(response.unwrap().status, 200);
    // A rooted path would drop the prefix, and a sibling must not pass for it.
    for path in ["/collections", "/tsx/collections", "/keys"] {
        let result = client.raw(request(Method::GET, path)).await;
        assert!(
            matches!(result, Err(Error::InvalidUrl(_))),
            "{path:?}: {result:?}"
        );
    }
}

#[tokio::test]
async fn an_at_sign_stays_in_the_path() {
    let server = MockServer::start().await;
    answer_anything(&server).await;
    let client = client_with(&server.uri(), false);
    client
        .raw(request(Method::GET, "@evil.example/x"))
        .await
        .unwrap();
    let seen = &server.received_requests().await.unwrap()[0];
    assert_eq!(seen.url.path(), "/@evil.example/x");
}

#[tokio::test]
async fn raw_body_kinds_set_the_content_type() {
    let server = MockServer::start().await;
    answer_anything(&server).await;
    let client = client_with(&server.uri(), false);
    for (body, kind) in [
        (RawBody::Json("{}".into()), "application/json"),
        (RawBody::Text("{}\n{}".into()), "text/plain"),
    ] {
        client
            .raw(RawRequest {
                body: Some(body),
                ..request(Method::POST, "collections")
            })
            .await
            .unwrap();
        let seen = server.received_requests().await.unwrap();
        let seen = seen.last().unwrap();
        assert_eq!(seen.headers.get("content-type").unwrap(), kind);
    }
}

#[tokio::test]
async fn the_event_carries_the_real_content_type_and_no_key() {
    use std::sync::{Arc, Mutex};
    let server = MockServer::start().await;
    answer_anything(&server).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let client = client_with(&server.uri(), false).with_hook(Arc::new(move |e| {
        sink.lock().unwrap().push(e.clone());
    }));
    client
        .raw(RawRequest {
            body: Some(RawBody::Text("a".into())),
            ..request(Method::POST, "collections/c/documents/import")
        })
        .await
        .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(
        events[0].request_headers,
        [("content-type".to_string(), "text/plain".to_string())]
    );
    assert!(!format!("{:?}", events[0]).contains("secret-key"));
}

#[test]
fn debug_prints_no_body() {
    let body = RawBody::Text("hidden".into());
    assert!(!format!("{body:?}").contains("hidden"));
    let request = RawRequest {
        body: Some(body),
        ..request(Method::POST, "keys")
    };
    assert!(!format!("{request:?}").contains("hidden"));
    let response = tessera_client::RawResponse {
        status: 200,
        headers: vec![],
        body: "hidden".into(),
        duration: std::time::Duration::ZERO,
    };
    assert!(!format!("{response:?}").contains("hidden"));
}
