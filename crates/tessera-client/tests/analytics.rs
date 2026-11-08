// SPDX-License-Identifier: GPL-3.0-or-later

//! Analytics rules against wiremock.

#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{Client, ClientOptions, Error};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(url: &str) -> Client {
    Client::new(&[url], SecretString::from("k"), ClientOptions::default()).unwrap()
}

#[tokio::test]
async fn rule_name_is_one_path_segment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/analytics/rules/a%2Fb%20c"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "a/b c"})))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    c.analytics_rule("a/b c").await.unwrap();
    for bad in ["", ".", ".."] {
        assert!(matches!(
            c.analytics_rule(bad).await,
            Err(Error::UnaddressableId(_))
        ));
        assert!(c.delete_analytics_rule(bad).await.is_err());
        assert!(c.update_analytics_rule(bad, &json!({})).await.is_err());
    }
}

#[tokio::test]
async fn create_posts_an_object() {
    let server = MockServer::start().await;
    let rule = json!({"name": "r", "type": "log", "collection": "c", "event_type": "click"});
    Mock::given(method("POST"))
        .and(path("/analytics/rules"))
        .and(body_json(&rule))
        .respond_with(ResponseTemplate::new(200).set_body_json(&rule))
        .expect(1)
        .mount(&server)
        .await;
    let made = client(&server.uri())
        .create_analytics_rule(&rule)
        .await
        .unwrap();
    assert_eq!(made["name"], "r");
}

#[tokio::test]
async fn list_reads_a_bare_array_and_a_null_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/analytics/rules"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"name": "r", "type": "log", "params": null, "rule_tag": ""}
        ])))
        .mount(&server)
        .await;
    let rules = client(&server.uri()).analytics_rules().await.unwrap();
    assert_eq!(rules.len(), 1);
    assert!(rules[0]["params"].is_null());
}
