// SPDX-License-Identifier: GPL-3.0-or-later

//! AI models and embedding schema writes against wiremock.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions, Error, ModelKind, RequestEvent, RequestHook};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn logged(uri: &str) -> (Client, Arc<Mutex<Vec<RequestEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let client = Client::new(&[uri], SecretString::from("k"), ClientOptions::default())
        .unwrap()
        .with_hook(hook);
    (client, events)
}

#[tokio::test]
async fn model_events_hold_no_bodies() {
    let server = MockServer::start().await;
    let model = json!({"id": "m", "api_key": "sk-st*********"});
    for (verb, route, answer) in [
        ("GET", "/conversations/models", json!([model])),
        ("GET", "/nl_search_models", json!([model])),
        ("POST", "/conversations/models", model.clone()),
        ("GET", "/conversations/models/m", model.clone()),
        ("PUT", "/conversations/models/m", model.clone()),
        ("DELETE", "/nl_search_models/m", model.clone()),
    ] {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(&server)
            .await;
    }
    let (client, events) = logged(&server.uri());
    let body = json!({"api_key": "sk-secret"});

    assert_eq!(
        client.ai_models(ModelKind::Conversation).await.unwrap()[0]["id"],
        "m"
    );
    assert_eq!(
        client.ai_models(ModelKind::NlSearch).await.unwrap().len(),
        1
    );
    assert_eq!(
        client
            .create_ai_model(ModelKind::Conversation, &body)
            .await
            .unwrap()["id"],
        "m"
    );
    client.ai_model(ModelKind::Conversation, "m").await.unwrap();
    client
        .update_ai_model(ModelKind::Conversation, "m", &body)
        .await
        .unwrap();
    client
        .delete_ai_model(ModelKind::NlSearch, "m")
        .await
        .unwrap();
    let refused = client.ai_model(ModelKind::NlSearch, "..").await;

    assert!(matches!(refused, Err(Error::UnaddressableId(_))));
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 6);
    for event in events.iter() {
        assert!(event.request_body.is_none(), "{}", event.path);
        assert!(event.response_body.is_none(), "{}", event.path);
    }
}

#[tokio::test]
async fn ai_search_is_not_retried_after_a_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/c/documents/search"))
        .and(query_param("q", "hi"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("{}")
                .set_delay(Duration::from_millis(400)),
        )
        .expect(1)
        .mount(&server)
        .await;
    // Longer than the profile timeout: only AI_TIMEOUT lets this answer arrive, and a call with
    // its own timeout is not retried when that runs out.
    let client = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            timeout: Duration::from_millis(100),
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let params = [("q".to_string(), "hi".to_string())];
    assert_eq!(client.ai_search("c", &params).await.unwrap(), "{}");
    assert_eq!(tessera_client::AI_TIMEOUT, Duration::from_secs(120));
}

#[tokio::test]
async fn ai_search_refuses_a_query_too_long_for_a_get() {
    let server = MockServer::start().await;
    let (client, events) = logged(&server.uri());
    let params = [("q".to_string(), "x".repeat(5000))];
    let error = client.ai_search("c", &params).await.unwrap_err();
    assert!(
        error.to_string().contains("the query is too long"),
        "{error}"
    );
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn embed_schema_event_holds_no_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/collections"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"name": "c", "fields": []})))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/collections/c"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"fields": []})))
        .mount(&server)
        .await;
    let (client, events) = logged(&server.uri());
    let embed =
        json!({"from": ["t"], "model_config": {"model_name": "openai/x", "api_key": "sk-secret"}});
    let schema = json!({"name": "c", "fields": [
        {"name": "t", "type": "string"},
        {"name": "e", "type": "float[]", "embed": embed},
    ]});
    client.create_collection(&schema).await.unwrap();
    client.update_collection("c", &schema).await.unwrap();
    client.node(0).create_collection(&schema).await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    for event in events.iter() {
        assert!(event.request_body.is_none(), "{}", event.path);
        assert!(event.response_body.is_none(), "{}", event.path);
    }
}

#[tokio::test]
async fn plain_schema_event_keeps_its_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/collections"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"name": "c", "fields": []})))
        .mount(&server)
        .await;
    let (client, events) = logged(&server.uri());
    let schema = json!({"name": "c", "fields": [{"name": "t", "type": "string"}]});
    client.create_collection(&schema).await.unwrap();
    let events = events.lock().unwrap();
    assert!(events[0].request_body.is_some());
    assert!(events[0].response_body.is_some());
}

#[tokio::test]
async fn provider_key_stays_out_of_the_request_log() {
    const KEY: &str = "sk-very-secret";
    let server = MockServer::start().await;
    let echo = json!({"message": format!("Incorrect API key provided: {KEY}.")});
    Mock::given(method("POST"))
        .and(path("/conversations/models"))
        .respond_with(ResponseTemplate::new(400).set_body_json(echo.clone()))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/nl_search_models/m"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "m"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/collections"))
        .respond_with(ResponseTemplate::new(400).set_body_json(echo))
        .mount(&server)
        .await;
    let (client, events) = logged(&server.uri());
    let body = json!({"api_key": KEY});
    let schema = json!({"name": "c", "fields": [{"name": "e", "type": "float[]", "embed": {
        "from": ["t"], "model_config": {"model_name": "openai/x", "api_key": KEY}
    }}]});

    client
        .create_ai_model(ModelKind::Conversation, &body)
        .await
        .unwrap_err();
    client
        .update_ai_model(ModelKind::NlSearch, "m", &body)
        .await
        .unwrap();
    client.create_collection(&schema).await.unwrap_err();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    for event in events.iter() {
        assert!(!format!("{event:?}").contains(KEY), "{}", event.path);
    }
}
