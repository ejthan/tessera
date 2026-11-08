// SPDX-License-Identifier: GPL-3.0-or-later

//! The update check against wiremock.

#![allow(clippy::unwrap_used)]

use tessera_client::{Error, latest_release};
use wiremock::matchers::{header_exists, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn serve(answer: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(answer)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn reads_the_tag_only() {
    let answer = ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "tag_name": "v1.2.3",
        "html_url": "https://elsewhere.example/",
    }));
    let server = serve(answer).await;
    assert_eq!(latest_release(&server.uri()).await.unwrap(), "v1.2.3");
}

#[tokio::test]
async fn a_404_is_an_http_error() {
    let server = serve(ResponseTemplate::new(404)).await;
    let got = latest_release(&server.uri()).await;
    assert!(
        matches!(got, Err(Error::Http { status: 404, .. })),
        "{got:?}"
    );
}

#[tokio::test]
async fn a_body_over_the_limit_stops() {
    let body = format!("{{\"tag_name\":\"{}\"}}", "x".repeat(1 << 21));
    let server = serve(ResponseTemplate::new(200).set_body_string(body)).await;
    let got = latest_release(&server.uri()).await;
    assert!(matches!(got, Err(Error::LineTooLong { .. })), "{got:?}");
}

#[tokio::test]
async fn sends_a_user_agent_and_no_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("user-agent"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"tag_name\":\"v1\"}"))
        .mount(&server)
        .await;
    latest_release(&server.uri()).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let headers = &requests[0].headers;
    assert!(
        headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("tessera/")
    );
    assert!(!headers.contains_key("x-typesense-api-key"));
}

#[tokio::test]
async fn follows_no_redirect() {
    let target = serve(ResponseTemplate::new(200).set_body_string("{\"tag_name\":\"v9\"}")).await;
    let moved = ResponseTemplate::new(302).insert_header("location", target.uri());
    let server = serve(moved).await;
    let got = latest_release(&server.uri()).await;
    assert!(
        matches!(got, Err(Error::Http { status: 302, .. })),
        "{got:?}"
    );
    assert!(target.received_requests().await.unwrap().is_empty());
}
