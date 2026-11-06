// SPDX-License-Identifier: GPL-3.0-or-later

//! Admin resources against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client --test live_admin -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, DocumentQuery, Error, NewKey};

/// Sat, 01 Jan 2100.
const EXPIRES: i64 = 4102444800;

fn url() -> String {
    std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into())
}

fn client() -> Client {
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into());
    Client::new(&[&url()], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
}

fn status(error: &Error) -> Option<u16> {
    match error {
        Error::Http { status, .. } => Some(*status),
        _ => None,
    }
}

fn search_only(description: &str) -> NewKey {
    NewKey {
        description: description.into(),
        actions: vec!["documents:search".into()],
        collections: vec!["*".into()],
        expires_at: Some(EXPIRES),
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_key_crud() {
    let c = client();
    let created = c
        .create_key(&search_only("tessera_it_crud"))
        .await
        .expect("create");
    let id = created.key.id;
    let listed = c.keys().await;
    let read = c.key(id).await;
    let deleted = c.delete_key(id).await;
    let gone = c.key(id).await;

    assert_eq!(created.key.expires_at, Some(EXPIRES));
    assert!(!created.key.extra.contains_key("value"));
    let listed = listed.expect("list");
    assert!(listed.iter().any(|k| k.id == id));
    let read = read.expect("get");
    assert_eq!(read.description, "tessera_it_crud");
    assert_eq!(read.value_prefix.as_ref().map(String::len), Some(4));
    assert!(!read.extra.contains_key("value"));
    deleted.expect("delete");
    assert_eq!(status(&gone.expect_err("gone")), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_with_key_uses_the_other_key() {
    let c = client();
    let created = c
        .create_key(&search_only("tessera_it_with_key"))
        .await
        .expect("create");
    let answer = c.with_key(created.value).keys().await;
    c.delete_key(created.key.id).await.expect("delete");
    assert_eq!(status(&answer.expect_err("refused")), Some(401));
}

// Analytics. Rules and events answer on any server, but the aggregates reach a destination only
// when it runs with `--enable-search-analytics=true` and `--analytics-dir`: the dev compose file
// has them, and `TESSERA_IT_ANALYTICS=1` says so. Without it a test sends at most 2 events,
// because the default limit is 5 a minute and refused requests count.

fn analytics_server() -> bool {
    std::env::var("TESSERA_IT_ANALYTICS").is_ok_and(|v| v == "1")
}

async fn make_collection(c: &Client, name: &str, fields: serde_json::Value) {
    let _ = c.delete_collection(name).await;
    let schema = serde_json::json!({ "name": name, "fields": fields });
    c.create_collection(&schema).await.expect("collection");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_rule_crud() {
    use serde_json::json;
    let c = client();
    let (source, dest, rule) = (
        "tessera_it_ar_src",
        "tessera_it_ar_dst",
        "tessera_it_ar_rule",
    );
    let _ = c.delete_analytics_rule(rule).await;
    make_collection(&c, source, json!([{"name": "title", "type": "string"}])).await;
    make_collection(
        &c,
        dest,
        json!([{"name": "q", "type": "string"}, {"name": "count", "type": "int32"}]),
    )
    .await;
    let body = |limit: u32| {
        json!({
            "name": rule, "type": "popular_queries", "collection": source, "event_type": "search",
            "params": {"destination_collection": dest, "limit": limit}
        })
    };
    let created = c.create_analytics_rule(&body(100)).await;
    let duplicate = c.create_analytics_rule(&body(100)).await;
    let listed = c.analytics_rules().await;
    let put = c
        .update_analytics_rule(rule, &json!({"params": {"limit": 7}}))
        .await;
    let read = c.analytics_rule(rule).await;
    let retyped = c.update_analytics_rule(rule, &json!({"type": "log"})).await;
    let deleted = c.delete_analytics_rule(rule).await;
    let gone = c.analytics_rule(rule).await;
    let _ = c.delete_collection(source).await;
    let _ = c.delete_collection(dest).await;

    created.expect("create");
    assert_eq!(status(&duplicate.expect_err("duplicate")), Some(400));
    assert!(listed.expect("list").iter().any(|r| r["name"] == rule));
    put.expect("put");
    let read = read.expect("get");
    assert_eq!(read["params"]["limit"], 7);
    assert_eq!(
        read["params"]["destination_collection"], dest,
        "a partial body merges"
    );
    assert_eq!(status(&retyped.expect_err("retype")), Some(400));
    deleted.expect("delete");
    assert_eq!(status(&gone.expect_err("gone")), Some(400));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_put_creates_a_missing_rule() {
    use serde_json::json;
    let c = client();
    let (source, rule) = ("tessera_it_ap_src", "tessera_it_ap_rule");
    let _ = c.delete_analytics_rule(rule).await;
    make_collection(&c, source, json!([{"name": "title", "type": "string"}])).await;
    let put = c
        .update_analytics_rule(
            rule,
            &json!({"type": "log", "collection": source, "event_type": "click"}),
        )
        .await;
    let read = c.analytics_rule(rule).await;
    let _ = c.delete_analytics_rule(rule).await;
    let _ = c.delete_collection(source).await;
    put.expect("an unknown name is created, not refused");
    assert_eq!(read.expect("exists")["type"], "log");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_event_is_accepted() {
    use serde_json::json;
    let c = client();
    let (source, rule) = ("tessera_it_ae_src", "tessera_it_ae_rule");
    let _ = c.delete_analytics_rule(rule).await;
    make_collection(&c, source, json!([{"name": "title", "type": "string"}])).await;
    c.create_analytics_rule(&json!({
        "name": rule, "type": "log", "collection": source, "event_type": "click"
    }))
    .await
    .expect("rule");
    let sent = c
        .analytics_event(&json!({
            "name": rule, "event_type": "click", "data": {"user_id": "u", "doc_id": "1"}
        }))
        .await;
    let _ = c.delete_analytics_rule(rule).await;
    let _ = c.delete_collection(source).await;
    sent.expect("event");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_flush_answers() {
    client().flush_analytics().await.expect("flush");
}

#[tokio::test]
#[ignore = "needs Typesense with the analytics flags, see dev/docker-compose.yml"]
async fn live_aggregate_reaches_the_destination() {
    use serde_json::json;
    if !analytics_server() {
        eprintln!("skipped: set TESSERA_IT_ANALYTICS=1 on a server with the analytics flags");
        return;
    }
    let c = client();
    let (source, dest, rule) = (
        "tessera_it_ag_src",
        "tessera_it_ag_dst",
        "tessera_it_ag_rule",
    );
    let _ = c.delete_analytics_rule(rule).await;
    make_collection(&c, source, json!([{"name": "title", "type": "string"}])).await;
    make_collection(
        &c,
        dest,
        json!([{"name": "q", "type": "string"}, {"name": "count", "type": "int32"}]),
    )
    .await;
    c.create_analytics_rule(&json!({
        "name": rule, "type": "popular_queries", "collection": source, "event_type": "search",
        "params": {"destination_collection": dest, "limit": 100}
    }))
    .await
    .expect("rule");
    c.analytics_event(&json!({
        "name": rule, "event_type": "search", "data": {"user_id": "u", "q": "tessera shoes"}
    }))
    .await
    .expect("event");
    c.flush_analytics().await.expect("flush");
    let mut hits = Ok(0);
    for _ in 0..20 {
        let all = DocumentQuery {
            page: 1,
            per_page: 10,
            filter_by: None,
            sort_by: None,
            exclude_fields: None,
        };
        hits = c.search_documents(dest, &all).await.map(|p| p.found);
        if !matches!(hits, Ok(0)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let _ = c.delete_analytics_rule(rule).await;
    let _ = c.delete_collection(source).await;
    let _ = c.delete_collection(dest).await;
    let hits = hits.expect("search of the destination");
    assert!(hits > 0, "no aggregate reached the destination within 10 s");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_put_clears_the_tag_and_the_meta_fields() {
    use serde_json::json;
    let c = client();
    let (source, dest, rule) = (
        "tessera_it_clr_src",
        "tessera_it_clr_dst",
        "tessera_it_clr_rule",
    );
    let _ = c.delete_analytics_rule(rule).await;
    make_collection(&c, source, json!([{"name": "title", "type": "string"}])).await;
    make_collection(
        &c,
        dest,
        json!([{"name": "q", "type": "string"}, {"name": "count", "type": "int32"}]),
    )
    .await;
    let created = c
        .create_analytics_rule(&json!({
            "name": rule, "type": "popular_queries", "collection": source,
            "event_type": "search", "rule_tag": "web",
            "params": {"destination_collection": dest, "limit": 10, "meta_fields": ["filter_by"]}
        }))
        .await;
    let updated = c
        .update_analytics_rule(
            rule,
            &json!({
                "collection": source, "event_type": "search", "rule_tag": "",
                "params": {"destination_collection": dest, "limit": 10, "meta_fields": []}
            }),
        )
        .await;
    let stored = c.analytics_rule(rule).await;
    let _ = c.delete_analytics_rule(rule).await;
    let _ = c.delete_collection(source).await;
    let _ = c.delete_collection(dest).await;
    created.expect("create");
    updated.expect("update");
    let stored = stored.expect("get");
    assert_eq!(stored["rule_tag"], "");
    assert!(
        stored["params"]
            .get("meta_fields")
            .is_none_or(|f| f == &json!([]))
    );
}

// Stemming dictionaries.

fn lines(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(w, r)| format!("{{\"word\":\"{w}\",\"root\":\"{r}\"}}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn roots(d: &tessera_client::StemmingDictionary) -> Vec<(String, String)> {
    let mut v: Vec<_> = d
        .words
        .iter()
        .map(|w| (w.word.clone(), w.root.clone()))
        .collect();
    v.sort();
    v
}

fn pair(w: &str, r: &str) -> (String, String) {
    (w.to_string(), r.to_string())
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_first_duplicate_of_an_import_wins() {
    let c = client();
    let id = "tessera_it_stem_dup";
    let _ = c.delete_stemming(id).await;
    let imported = c
        .import_stemming(id, &lines(&[("a", "x"), ("a", "y")]))
        .await;
    let stored = c.stemming_dictionary(id).await;
    let _ = c.delete_stemming(id).await;
    imported.expect("import");
    assert_eq!(roots(&stored.expect("get")), [pair("a", "x")]);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_stemming_crud() {
    let c = client();
    let id = "tessera_it_stem_crud";
    let _ = c.delete_stemming(id).await;
    c.import_stemming(id, &lines(&[("people", "person"), ("mice", "mouse")]))
        .await
        .expect("import");
    let first = c.stemming_dictionary(id).await;
    c.import_stemming(id, &lines(&[("people", "human"), ("geese", "goose")]))
        .await
        .expect("merge");
    let merged = c.stemming_dictionary(id).await;
    let listed = c.stemming_dictionaries().await;
    let deleted = c.delete_stemming(id).await;
    let after = c.stemming_dictionaries().await;
    let gone = c.stemming_dictionary(id).await;

    assert_eq!(first.expect("get").id, id);
    // Measured on 30.0 and 30.2: an import adds new words to an existing dictionary but keeps
    // the root a word already has, whatever the docs say. A new root needs a delete first.
    assert_eq!(
        roots(&merged.expect("merged")),
        [
            pair("geese", "goose"),
            pair("mice", "mouse"),
            pair("people", "person")
        ]
    );
    assert!(listed.expect("list").iter().any(|d| d == id));
    deleted.expect("delete");
    assert!(!after.expect("list").iter().any(|d| d == id));
    assert_eq!(status(&gone.expect_err("gone")), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_bad_line_keeps_the_lines_before_it() {
    let c = client();
    let id = "tessera_it_stem_bad";
    let _ = c.delete_stemming(id).await;
    let refused = c
        .import_stemming(id, &format!("{}\nnot json", lines(&[("a", "b")])))
        .await;
    let stored = c.stemming_dictionary(id).await;
    let _ = c.delete_stemming(id).await;
    assert_eq!(status(&refused.expect_err("refused")), Some(400));
    assert_eq!(
        roots(&stored.expect("the first line was stored")),
        [pair("a", "b")]
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_delete_of_an_unknown_id_answers_200() {
    client()
        .delete_stemming("tessera_it_stem_unknown")
        .await
        .expect("200 also for an unknown id");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_odd_id_round_trips() {
    let c = client();
    let id = "a/b c";
    let _ = c.delete_stemming(id).await;
    let imported = c.import_stemming(id, &lines(&[("x", "y")])).await;
    let read = c.stemming_dictionary(id).await;
    let listed = c.stemming_dictionaries().await;
    let deleted = c.delete_stemming(id).await;
    imported.expect("import");
    let read = read.expect("get");
    assert_eq!(read.id, id);
    assert_eq!(roots(&read), [pair("x", "y")]);
    assert!(listed.expect("list").iter().any(|d| d == id));
    deleted.expect("delete");
}
