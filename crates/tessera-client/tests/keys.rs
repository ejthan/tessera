// SPDX-License-Identifier: GPL-3.0-or-later

//! API keys against wiremock. The value of a new key must reach the caller and nothing else.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use tessera_client::{
    Client, ClientOptions, Error, Method, NewKey, RawBody, RawRequest, RequestEvent, RequestHook,
};
use wiremock::matchers::{any, body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const VALUE: &str = "k3yV4lueThatMustStaySecret";

fn logged(url: &str) -> (Client, Arc<Mutex<Vec<RequestEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let client = Client::new(
        &[url],
        SecretString::from("secret-key"),
        ClientOptions::default(),
    )
    .unwrap()
    .with_hook(hook);
    (client, events)
}

fn draft(expires_at: Option<i64>) -> NewKey {
    NewKey {
        description: "search".into(),
        actions: vec!["documents:search".into()],
        collections: vec!["*".into()],
        expires_at,
    }
}

fn created() -> Value {
    json!({
        "id": 7,
        "description": "search",
        "actions": ["documents:search"],
        "collections": ["*"],
        "expires_at": 64723363199_i64,
        "autodelete": false,
        "value": VALUE,
    })
}

async fn answer_create(server: &MockServer, answer: Value) {
    Mock::given(method("POST"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(201).set_body_json(answer))
        .mount(server)
        .await;
}

/// Nothing an event holds or prints shows the value.
fn assert_clean(events: &[RequestEvent], value: &str) {
    for event in events {
        assert!(event.response_body.is_none(), "{event:?}");
        let held = event.request_body.as_ref().map(|b| b.text.to_string());
        assert!(!held.unwrap_or_default().contains(value));
        assert!(!format!("{event:?}").contains(value));
        assert!(!event.error.clone().unwrap_or_default().contains(value));
    }
}

fn post_keys(path: &str) -> RawRequest {
    RawRequest {
        method: Method::POST,
        path: path.into(),
        query: None,
        body: Some(RawBody::Json("{}".into())),
    }
}

#[tokio::test]
async fn create_key_body_and_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/keys"))
        .and(body_json(json!({
            "description": "search",
            "actions": ["documents:search"],
            "collections": ["*"],
            "expires_at": 4102444800_i64,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(created()))
        .expect(1)
        .mount(&server)
        .await;
    let (client, _) = logged(&server.uri());
    let created = client.create_key(&draft(Some(4102444800))).await.unwrap();
    assert_eq!(created.value.expose_secret(), VALUE);
    assert_eq!(created.key.id, 7);
    assert_eq!(created.key.actions, ["documents:search"]);
    assert!(!created.key.extra.contains_key("value"));
}

#[tokio::test]
async fn create_without_expiry_leaves_it_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/keys"))
        .and(body_json(json!({
            "description": "search",
            "actions": ["documents:search"],
            "collections": ["*"],
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(created()))
        .expect(1)
        .mount(&server)
        .await;
    let (client, _) = logged(&server.uri());
    client.create_key(&draft(None)).await.unwrap();
}

#[tokio::test]
async fn create_event_holds_no_value() {
    let server = MockServer::start().await;
    answer_create(&server, created()).await;
    let (client, events) = logged(&server.uri());
    let created = client.create_key(&draft(None)).await.unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_clean(&events, VALUE);
    assert!(!format!("{created:?}").contains(VALUE));
    assert!(!format!("{:?}", created.key).contains(VALUE));
    let json = serde_json::to_string(&created.key).unwrap();
    assert!(!json.contains(VALUE));
}

#[tokio::test]
async fn raw_post_keys_event_holds_no_value() {
    for spelling in ["keys", "/keys", "keys/", "keys//", "keys?a=1", "keys/#x"] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(201).set_body_json(created()))
            .mount(&server)
            .await;
        let (client, events) = logged(&server.uri());
        let answer = client.raw(post_keys(spelling)).await.unwrap();
        assert!(answer.body.contains(VALUE), "the console shows it");
        assert_clean(&events.lock().unwrap(), VALUE);
    }
}

#[tokio::test]
async fn raw_post_keys_below_a_path_prefix_holds_no_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ts/keys"))
        .respond_with(ResponseTemplate::new(201).set_body_json(created()))
        .mount(&server)
        .await;
    let (client, events) = logged(&format!("{}/ts", server.uri()));
    client.raw(post_keys("keys")).await.unwrap();
    assert_clean(&events.lock().unwrap(), VALUE);
}

/// The console can choose the value: the request body holds the same secret.
#[tokio::test]
async fn raw_post_keys_event_holds_no_chosen_value() {
    let server = MockServer::start().await;
    answer_create(&server, created()).await;
    let (client, events) = logged(&server.uri());
    let mut request = post_keys("keys");
    request.body = Some(RawBody::Json(format!(r#"{{"value":"{VALUE}"}}"#)));
    client.raw(request).await.unwrap();
    let events = events.lock().unwrap();
    assert!(events[0].request_body.is_none());
    assert_clean(&events, VALUE);
}

#[tokio::test]
async fn raw_get_keys_keeps_its_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"keys":[]}"#))
        .mount(&server)
        .await;
    let (client, events) = logged(&server.uri());
    let mut request = post_keys("keys");
    request.method = Method::GET;
    request.body = None;
    client.raw(request).await.unwrap();
    let events = events.lock().unwrap();
    let body = events[0].response_body.as_ref().unwrap();
    assert_eq!(&*body.text, r#"{"keys":[]}"#);
}

#[tokio::test]
async fn create_answer_without_value_is_an_error() {
    let mut answer = created();
    answer["value_prefix"] = json!("k3yV");
    answer.as_object_mut().unwrap().remove("value");
    let server = MockServer::start().await;
    answer_create(&server, answer).await;
    let (client, _) = logged(&server.uri());
    let error = client.create_key(&draft(None)).await.unwrap_err();
    assert!(matches!(error, Error::Decode(_)), "{error:?}");
    assert!(error.to_string().contains("no value"), "{error}");
    assert!(!error.to_string().contains("k3yV"), "{error}");
}

/// A shape the client does not expect still keeps the value out of the error.
#[tokio::test]
async fn an_unreadable_create_answer_prints_no_value() {
    let server = MockServer::start().await;
    let mut answer = created();
    answer["id"] = json!("not a number");
    answer_create(&server, answer).await;
    let (client, events) = logged(&server.uri());
    let error = client.create_key(&draft(None)).await.unwrap_err();
    assert!(!format!("{error} {error:?}").contains(VALUE), "{error:?}");
    assert_clean(&events.lock().unwrap(), VALUE);
}

#[tokio::test]
async fn keys_decodes_unknown_fields_into_extra() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": [{
            "id": 1,
            "actions": ["*"],
            "collections": ["*"],
            "expires_at": 64723363199_i64,
            "value_prefix": "abcd",
            "autodelete": false,
        }]})))
        .mount(&server)
        .await;
    let (client, _) = logged(&server.uri());
    let keys = client.keys().await.unwrap();
    assert_eq!(keys[0].description, "");
    assert_eq!(keys[0].value_prefix.as_deref(), Some("abcd"));
    assert_eq!(keys[0].extra["autodelete"], json!(false));
}

#[tokio::test]
async fn delete_goes_to_the_id() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/keys/9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":9}"#))
        .expect(1)
        .mount(&server)
        .await;
    let (client, _) = logged(&server.uri());
    client.delete_key(9).await.unwrap();
}
