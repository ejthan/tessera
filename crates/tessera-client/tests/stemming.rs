// SPDX-License-Identifier: GPL-3.0-or-later

//! Stemming dictionaries against wiremock.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, Error, RequestEvent, RequestHook};
use wiremock::matchers::{body_string, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn import_sends_text_with_an_encoded_id_and_keeps_no_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/stemming/dictionaries/import"))
        .and(query_param("id", "a/b c"))
        .and(header("content-type", "text/plain"))
        .and(body_string("{\"word\":\"x\",\"root\":\"y\"}"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"word\":\"x\"}"))
        .expect(1)
        .mount(&server)
        .await;
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let client = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions::default(),
    )
    .unwrap()
    .with_hook(hook);
    client
        .import_stemming("a/b c", "{\"word\":\"x\",\"root\":\"y\"}")
        .await
        .unwrap();
    let refused = client.import_stemming("", "").await;
    assert!(matches!(refused, Err(Error::UnaddressableId(_))));
    let events = events.lock().unwrap();
    assert!(events[0].request_body.is_none() && events[0].response_body.is_none());
}

#[tokio::test]
async fn a_failed_import_is_an_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string("{\"message\":\"Invalid dictionary format.\"}"),
        )
        .mount(&server)
        .await;
    let client = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions::default(),
    )
    .unwrap();
    let error = client.import_stemming("d", "not json").await.unwrap_err();
    assert!(
        matches!(error, Error::Http { status: 400, .. }),
        "{error:?}"
    );
}
