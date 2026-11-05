// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection read endpoints against a wiremock server.

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions, Error};
use wiremock::matchers::{method, path};
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
