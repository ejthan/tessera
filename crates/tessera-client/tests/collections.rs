// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection read endpoints against a wiremock server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions, Error, RequestEvent, RequestHook};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(url: &str) -> Result<Client, Error> {
    Client::new(&[url], SecretString::from("k"), ClientOptions::default())
}

async fn mock(server: &MockServer, p: &str, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

const FULL: &str = r#"[{"created_at":1793884719,"curation_sets":[],"default_sorting_field":"n","enable_nested_fields":false,"fields":[{"facet":false,"index":true,"infix":false,"locale":"","name":"title","optional":false,"sort":false,"stem":false,"stem_dictionary":"","store":true,"truncate_len":100,"type":"string"}],"metadata":{"owner":"me"},"name":"m2probe","num_documents":0,"symbols_to_index":[],"synonym_sets":["m2probe_syn"],"token_separators":[]}]"#;

#[tokio::test]
async fn collections_decodes_full_schemas() {
    let server = MockServer::start().await;
    mock(&server, "/collections", 200, FULL).await;
    let all = client(&server.uri()).unwrap().collections().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].num_documents, 0);
    assert_eq!(all[0].fields.len(), 1);
    assert_eq!(all[0].synonym_sets, ["m2probe_syn"]);
    assert_eq!(all[0].metadata, json!({"owner":"me"}).as_object().cloned());
    assert!(all[0].extra.is_empty(), "{:?}", all[0].extra);
    assert!(all[0].fields[0].flag("index"));
    assert!(!all[0].fields[0].flag("facet"));
}

#[tokio::test]
async fn unknown_attributes_survive() {
    let server = MockServer::start().await;
    let body = r#"{"name":"c","future_option":1,"fields":[{"name":"f","type":"int8[]","future_flag":true}]}"#;
    mock(&server, "/collections/c", 200, body).await;
    let c = client(&server.uri())
        .unwrap()
        .collection("c")
        .await
        .unwrap();
    assert_eq!(c.extra["future_option"], json!(1));
    assert_eq!(c.fields[0].attr("future_flag"), Some(&json!(true)));
    assert!(serde_json::to_value(&c).unwrap().get("metadata").is_none());
    assert!(c.fields[0].flag("index") && c.fields[0].flag("store"));
}

#[tokio::test]
async fn name_is_one_path_segment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"name":"x"}"#))
        .mount(&server)
        .await;
    client(&server.uri())
        .unwrap()
        .collection("a/b c?x")
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), "/collections/a%2Fb%20c%3Fx");
    assert_eq!(requests[0].url.query(), None);
}

#[tokio::test]
async fn aliases_unwraps_the_list() {
    let server = MockServer::start().await;
    mock(
        &server,
        "/aliases",
        200,
        r#"{"aliases":[{"name":"cur","collection_name":"books"}]}"#,
    )
    .await;
    let aliases = client(&server.uri()).unwrap().aliases().await.unwrap();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].name, "cur");
    assert_eq!(aliases[0].collection_name, "books");
}

#[tokio::test]
async fn missing_collection_is_http_404() {
    let server = MockServer::start().await;
    mock(
        &server,
        "/collections/nope",
        404,
        r#"{"message":"Not Found"}"#,
    )
    .await;
    let err = client(&server.uri())
        .unwrap()
        .collection("nope")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 404, .. }), "{err:?}");
    assert_eq!(err.typesense_message(), Some("Not Found"));
}

#[tokio::test]
async fn create_sends_the_body_verbatim() {
    let server = MockServer::start().await;
    let schema = json!({"name":"c","future":1,"fields":[{"name":"f","type":"int8[]","x":true}]});
    Mock::given(method("POST"))
        .and(path("/collections"))
        .and(body_json(&schema))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"name":"c","fields":[]}"#))
        .expect(1)
        .mount(&server)
        .await;
    let created = client(&server.uri())
        .unwrap()
        .create_collection(&schema)
        .await
        .unwrap();
    assert_eq!(created.name, "c");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["content-type"], "application/json");
}

#[tokio::test]
async fn patch_outlives_the_profile_timeout() {
    let server = MockServer::start().await;
    let slow = || ResponseTemplate::new(200).set_delay(Duration::from_secs(2));
    Mock::given(method("PATCH"))
        .and(path("/collections/c"))
        .and(body_json(json!({"fields":[]})))
        .respond_with(slow().set_body_string("{}"))
        .mount(&server)
        .await;
    mock(&server, "/collections/c", 200, "{}").await;
    let client = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            timeout: Duration::from_secs(1),
            retries: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    client
        .update_collection("c", &json!({"fields":[]}))
        .await
        .unwrap();
    // Same delay on a GET: the profile timeout still applies.
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(slow().set_body_string("{}"))
        .mount(&server)
        .await;
    assert!(matches!(client.collection("c").await, Err(Error::Timeout)));
}

#[tokio::test]
async fn truncate_returns_num_deleted() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/collections/c/documents"))
        .and(query_param("truncate", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"num_deleted":7}"#))
        .expect(1)
        .mount(&server)
        .await;
    let n = client(&server.uri())
        .unwrap()
        .truncate_collection("c")
        .await
        .unwrap();
    assert_eq!(n, 7);
}

#[tokio::test]
async fn delete_and_schema_changes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/collections/c"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"name":"c"}"#))
        .expect(1)
        .mount(&server)
        .await;
    mock(
        &server,
        "/operations/schema_changes",
        200,
        r#"[{"collection":"c","validated_docs":3,"altered_docs":1}]"#,
    )
    .await;
    let client = client(&server.uri()).unwrap();
    client.delete_collection("c").await.unwrap();
    let changes = client.node(0).schema_changes().await.unwrap();
    assert_eq!(changes[0].collection, "c");
    assert_eq!(changes[0].altered_docs, 1);
}

#[tokio::test]
async fn post_and_patch_are_never_retried() {
    // A port nothing listens on.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let client = client(&dead).unwrap().with_hook(hook);
    let schema = json!({"name":"c","fields":[]});
    assert!(matches!(
        client.create_collection(&schema).await,
        Err(Error::Network(_))
    ));
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(matches!(
        client.update_collection("c", &schema).await,
        Err(Error::Network(_))
    ));
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unlock_then_lock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"name":"c"}"#))
        .mount(&server)
        .await;
    let events = Arc::new(Mutex::new(Vec::<RequestEvent>::new()));
    let sink = events.clone();
    let hook: RequestHook = Arc::new(move |e: &RequestEvent| sink.lock().unwrap().push(e.clone()));
    let locked = Client::new(
        &[&server.uri()],
        SecretString::from("k"),
        ClientOptions {
            read_only: true,
            ..ClientOptions::default()
        },
    )
    .unwrap()
    .with_hook(hook);
    let schema = json!({"name":"c","fields":[]});
    assert!(locked.is_read_only());
    assert!(matches!(
        locked.create_collection(&schema).await,
        Err(Error::ReadOnly { .. })
    ));
    assert!(events.lock().unwrap().is_empty());
    let clone = locked.clone();
    clone.set_read_only(false);
    assert!(!locked.is_read_only());
    locked.create_collection(&schema).await.unwrap();
    assert_eq!(events.lock().unwrap().len(), 1);
    clone.set_read_only(true);
    assert!(matches!(
        locked.create_collection(&schema).await,
        Err(Error::ReadOnly { .. })
    ));
    assert_eq!(events.lock().unwrap().len(), 1);
}
