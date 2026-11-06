// SPDX-License-Identifier: GPL-3.0-or-later

//! Relevance resources against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client --test live_relevance -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

use std::collections::BTreeSet;

use secrecy::SecretString;
use serde_json::json;
use tessera_client::{
    Client, ClientOptions, Error, ExportQuery, MultiSearch, RawBody, RawRequest, SetKind,
};

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

async fn create(client: &Client, name: &str) {
    let schema = json!({"name": name, "fields": [{"name": "title", "type": "string"}]});
    client.create_collection(&schema).await.expect("create");
}

fn search_all() -> Vec<(String, String)> {
    vec![
        ("q".to_string(), "*".to_string()),
        ("query_by".to_string(), "title".to_string()),
    ]
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_round_trip() {
    let c = client();
    let (one, two, alias) = (
        "tessera_it_alias_one",
        "tessera_it_alias_two",
        "tessera_it_alias",
    );
    create(&c, one).await;
    create(&c, two).await;

    let created = c.upsert_alias(alias, one).await;
    let read = c.alias(alias).await;
    let moved = c.upsert_alias(alias, two).await;
    let listed = c.aliases().await;
    let removed = c.delete_alias(alias).await;
    let gone = c.alias(alias).await;
    c.delete_collection(one).await.expect("cleanup");
    c.delete_collection(two).await.expect("cleanup");

    assert_eq!(created.unwrap().collection_name, one);
    assert_eq!(read.unwrap().collection_name, one);
    assert_eq!(moved.unwrap().collection_name, two);
    let listed = listed.unwrap();
    let found = listed.iter().find(|a| a.name == alias).unwrap();
    assert_eq!(found.collection_name, two);
    removed.unwrap();
    assert_eq!(status(&gone.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_accepts_a_missing_target() {
    let c = client();
    let alias = "tessera_it_alias_missing";
    let saved = c.upsert_alias(alias, "tessera_it_no_such_target").await;
    let searched = c.search(alias, &search_all()).await;
    c.delete_alias(alias).await.expect("cleanup");

    saved.unwrap();
    assert_eq!(status(&searched.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_of_an_alias_does_not_resolve() {
    let c = client();
    let (target, first, chain) = (
        "tessera_it_chain_target",
        "tessera_it_chain_first",
        "tessera_it_chain",
    );
    create(&c, target).await;
    c.upsert_alias(first, target).await.expect("alias");
    let saved = c.upsert_alias(chain, first).await;
    let resolved = c.collection(chain).await;
    c.delete_alias(chain).await.expect("cleanup");
    c.delete_alias(first).await.expect("cleanup");
    c.delete_collection(target).await.expect("cleanup");

    saved.unwrap();
    assert_eq!(status(&resolved.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_alias_cannot_take_a_collection_name() {
    let c = client();
    let name = "tessera_it_alias_taken";
    create(&c, name).await;
    let refused = c.upsert_alias(name, name).await;
    c.delete_collection(name).await.expect("cleanup");

    let error = refused.unwrap_err();
    assert_eq!(status(&error), Some(500));
    assert!(error.typesense_message().unwrap().contains("conflicts"));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_delete_missing_alias_is_404() {
    let error = client()
        .delete_alias("tessera_it_no_such_alias")
        .await
        .unwrap_err();
    assert_eq!(status(&error), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_stopwords_round_trip() {
    let c = client();
    let (set, name) = ("tessera_it_stopwords", "tessera_it_stopwords_c");
    create(&c, name).await;
    let words = ["The".to_string(), "the".to_string(), "  x y ".to_string()];
    let saved = c.upsert_stopwords(set, &words, Some("en")).await;
    let read = c.stopword_set(set).await;
    let listed = c.stopwords().await;
    let empty = c.upsert_stopwords(set, &[], None).await;
    let removed = c.delete_stopwords(set).await;
    let gone = c.stopword_set(set).await;
    let mut params = search_all();
    params.push(("stopwords".to_string(), set.to_string()));
    let searched = c.search(name, &params).await;
    c.delete_collection(name).await.expect("cleanup");

    saved.unwrap();
    let read = read.unwrap();
    let stored: BTreeSet<&str> = read.stopwords.iter().map(String::as_str).collect();
    assert_eq!(stored, BTreeSet::from(["the", "x", "y"]));
    assert_eq!(read.locale.as_deref(), Some("en"));
    assert!(listed.unwrap().iter().any(|s| s.id == set));
    assert_eq!(status(&empty.unwrap_err()), Some(400));
    removed.unwrap();
    assert_eq!(status(&gone.unwrap_err()), Some(404));
    assert_eq!(status(&searched.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_preset_delete() {
    let c = client();
    let (preset, name) = ("tessera_it_preset_delete", "tessera_it_preset_delete_c");
    create(&c, name).await;
    let saved = c.upsert_preset(preset, &json!("str")).await;
    let listed = c.presets().await;
    let removed = c.delete_preset(preset).await;
    let gone = c.preset(preset).await;
    let mut params = search_all();
    params.push(("preset".to_string(), preset.to_string()));
    let searched = c.search(name, &params).await;
    c.delete_collection(name).await.expect("cleanup");

    saved.unwrap();
    let listed = listed.unwrap();
    let found = listed.iter().find(|p| p.name == preset).unwrap();
    assert_eq!(found.value, json!("str"));
    removed.unwrap();
    assert_eq!(status(&gone.unwrap_err()), Some(404));
    searched.unwrap();
}

const KINDS: [SetKind; 2] = [SetKind::Synonym, SetKind::Curation];

fn ids(text: &str) -> Vec<String> {
    let answer: serde_json::Value = serde_json::from_str(text).expect("json");
    answer["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["document"]["id"].as_str().expect("id").to_string())
        .collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// A search on `collection` for `q` over `title` with extra parameters.
async fn find(client: &Client, collection: &str, q: &str, extra: &[(&str, &str)]) -> Vec<String> {
    let mut params = vec![
        ("q".to_string(), q.to_string()),
        ("query_by".to_string(), "title".to_string()),
    ];
    params.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    ids(&client.search(collection, &params).await.expect("search"))
}

async fn add_docs(client: &Client, collection: &str, titles: &[&str]) {
    for (i, title) in titles.iter().enumerate() {
        let document = json!({"id": (i + 1).to_string(), "title": title});
        client
            .upsert_document(collection, &document)
            .await
            .expect("doc");
    }
}

fn message(error: &Error) -> String {
    error.typesense_message().unwrap_or_default().to_string()
}

/// The server's own text, whether it came as an error or inside a multi-search answer.
fn failure_text<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
    match result {
        Err(error) => format!("{error} {}", message(&error)),
        Ok(text) => format!("{text:?}"),
    }
}

fn syn(id: &str, words: &[&str]) -> serde_json::Value {
    json!({"id": id, "synonyms": words})
}

fn pin(id: &str, query: &str, doc: &str) -> serde_json::Value {
    json!({"id": id, "rule": {"query": query, "match": "exact"},
        "includes": [{"id": doc, "position": 1}]})
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_sets_round_trip() {
    let c = client();
    for kind in KINDS {
        let name = format!("tessera_it_rt_{}", kind.path());
        let first = if kind == SetKind::Synonym {
            syn("one", &["a", "b"])
        } else {
            pin("one", "a", "1")
        };
        let second = if kind == SetKind::Synonym {
            syn("two", &["c", "d"])
        } else {
            pin("two", "c", "2")
        };
        let created = c
            .upsert_set(kind, &name, std::slice::from_ref(&first))
            .await;
        let listed = c.sets(kind).await;
        let read = c.set(kind, &name).await;
        let added = c.upsert_set_item(kind, &name, "two", &second).await;
        let item = c.set_item(kind, &name, "two").await;
        let removed_item = c.delete_set_item(kind, &name, "two").await;
        let after_item = c.set(kind, &name).await;
        let removed = c.delete_set(kind, &name).await;
        let gone = c.set(kind, &name).await;

        assert_eq!(created.unwrap().name, name);
        assert!(listed.unwrap().iter().any(|s| s.name == name));
        assert_eq!(read.unwrap().items[0]["id"], "one");
        added.unwrap();
        assert_eq!(item.unwrap()["id"], "two");
        removed_item.unwrap();
        assert_eq!(after_item.unwrap().items.len(), 1);
        removed.unwrap();
        assert_eq!(status(&gone.unwrap_err()), Some(404));
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_set_duplicate_ids_are_reduced() {
    let c = client();
    let name = "tessera_it_dup";
    let items = [syn("x", &["a", "b"]), syn("x", &["c", "d"])];
    let sent = c.upsert_set(SetKind::Synonym, name, &items).await;
    let read = c.set(SetKind::Synonym, name).await;
    c.delete_set(SetKind::Synonym, name).await.expect("cleanup");

    sent.unwrap();
    let stored = read.unwrap().items;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["synonyms"], json!(["c", "d"]));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_item_put_echoes_but_stores_less() {
    let c = client();
    let name = "tessera_it_echo";
    c.upsert_set(SetKind::Synonym, name, &[syn("x", &["a", "b"])])
        .await
        .expect("set");
    let body = json!({"synonyms": ["a", "b"], "tessera_unknown": 1});
    let raw = c
        .raw(RawRequest {
            method: tessera_client::Method::PUT,
            path: format!("synonym_sets/{name}/items/x"),
            query: None,
            body: Some(RawBody::Json(body.to_string())),
        })
        .await
        .expect("raw")
        .body;
    let stored = c.set_item(SetKind::Synonym, name, "x").await;
    c.delete_set(SetKind::Synonym, name).await.expect("cleanup");

    assert!(raw.contains("tessera_unknown"), "the answer echoes: {raw}");
    assert!(stored.unwrap().get("tessera_unknown").is_none());
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_item_of_a_missing_set_is_404() {
    let c = client();
    for kind in KINDS {
        let name = "tessera_it_no_such_set";
        let item = if kind == SetKind::Synonym {
            syn("x", &["a", "b"])
        } else {
            pin("x", "a", "1")
        };
        let put = c.upsert_set_item(kind, name, "x", &item).await;
        let read = c.set_item(kind, name, "x").await;
        let removed = c.delete_set_item(kind, name, "x").await;
        for error in [put.unwrap_err(), read.unwrap_err(), removed.unwrap_err()] {
            assert_eq!(status(&error), Some(404), "{kind:?} {error}");
        }
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_attachment_by_patch() {
    let c = client();
    for kind in KINDS {
        let (coll, set) = (
            format!("tessera_it_att_{}", kind.path()),
            format!("tessera_it_att_set_{}", kind.path()),
        );
        create(&c, &coll).await;
        c.upsert_set(kind, &set, &[]).await.expect("set");
        let key = kind.path();
        let attached = c.update_collection(&coll, &json!({ key: [set] })).await;
        let read = c.collection(&coll).await;
        let missing = c
            .update_collection(&coll, &json!({ key: [set, "tessera_it_never_was"] }))
            .await;
        let after_missing = c.collection(&coll).await;
        let detached = c.update_collection(&coll, &json!({ key: [] })).await;
        let after_detach = c.collection(&coll).await;
        c.delete_collection(&coll).await.expect("cleanup");
        c.delete_set(kind, &set).await.expect("cleanup");

        attached.unwrap();
        let list = |c: &tessera_client::Collection| match kind {
            SetKind::Synonym => c.synonym_sets.clone(),
            SetKind::Curation => c.curation_sets.clone(),
        };
        assert_eq!(list(&read.unwrap()), vec![set.clone()]);
        assert_eq!(status(&missing.unwrap_err()), Some(404));
        assert_eq!(list(&after_missing.unwrap()), vec![set]);
        detached.unwrap();
        assert!(list(&after_detach.unwrap()).is_empty());
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_deleted_attached_synonym_set_breaks_search() {
    let c = client();
    let (coll, set) = ("tessera_it_broken_syn", "tessera_it_broken_syn_set");
    create(&c, coll).await;
    add_docs(&c, coll, &["sofa"]).await;
    c.upsert_set(SetKind::Synonym, set, &[syn("s", &["sofa", "couch"])])
        .await
        .expect("set");
    c.update_collection(coll, &json!({"synonym_sets": [set]}))
        .await
        .expect("attach");
    c.delete_set(SetKind::Synonym, set).await.expect("delete");

    let star = c.search(coll, &search_all()).await;
    let mut off = search_all();
    off.push(("enable_synonyms".into(), "false".into()));
    let off = c.search(coll, &off).await;
    let multi = c
        .multi_search(&MultiSearch {
            union: false,
            searches: vec![
                serde_json::from_value(json!({"collection": coll, "q": "*", "query_by": "title"}))
                    .unwrap(),
            ],
            page: None,
            per_page: None,
        })
        .await;
    let exported = async {
        let mut stream = c.export_stream(coll, &ExportQuery::default()).await?;
        stream.next_line().await
    }
    .await;
    let healed_by_patch = c
        .update_collection(coll, &json!({"synonym_sets": []}))
        .await;
    let healthy = c.search(coll, &search_all()).await;
    c.delete_collection(coll).await.expect("cleanup");

    for text in [failure_text(star), failure_text(off), failure_text(multi)] {
        assert!(text.contains("Synonym index not found"), "{text}");
    }
    assert!(exported.unwrap().is_some(), "export still works");
    healed_by_patch.unwrap();
    healthy.unwrap();
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_recreating_an_attached_synonym_set_heals_search() {
    let c = client();
    let (coll, set) = ("tessera_it_heal_syn", "tessera_it_heal_syn_set");
    create(&c, coll).await;
    c.upsert_set(SetKind::Synonym, set, &[syn("s", &["a", "b"])])
        .await
        .expect("set");
    c.update_collection(coll, &json!({"synonym_sets": [set]}))
        .await
        .expect("attach");
    c.delete_set(SetKind::Synonym, set).await.expect("delete");
    let broken = c.search(coll, &search_all()).await;
    c.upsert_set(SetKind::Synonym, set, &[syn("s", &["a", "b"])])
        .await
        .expect("recreate");
    let healed = c.search(coll, &search_all()).await;
    c.delete_collection(coll).await.expect("cleanup");
    c.delete_set(SetKind::Synonym, set).await.expect("cleanup");

    assert!(broken.is_err());
    healed.unwrap();
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_deleted_attached_curation_set_leaves_search() {
    let c = client();
    let (coll, set) = ("tessera_it_broken_cur", "tessera_it_broken_cur_set");
    let (other, other_name) = ("tessera_it_broken_cur_other", "tessera_it_broken_cur_o");
    create(&c, coll).await;
    c.upsert_set(SetKind::Curation, set, &[pin("p", "a", "1")])
        .await
        .expect("set");
    c.upsert_set(SetKind::Curation, other_name, &[pin("p", "a", "1")])
        .await
        .expect("other");
    c.update_collection(coll, &json!({"curation_sets": [set]}))
        .await
        .expect("attach");
    c.delete_set(SetKind::Curation, set).await.expect("delete");

    let searched = c.search(coll, &search_all()).await;
    let stale = c
        .update_collection(coll, &json!({"curation_sets": [set, other_name]}))
        .await;
    let created = c
        .create_collection(&json!({"name": other, "curation_sets": [set],
            "fields": [{"name": "title", "type": "string"}]}))
        .await;
    c.delete_collection(coll).await.expect("cleanup");
    c.delete_collection(other).await.ok();
    c.delete_set(SetKind::Curation, other_name)
        .await
        .expect("cleanup");

    searched.unwrap();
    assert_eq!(status(&stale.unwrap_err()), Some(404));
    assert_eq!(status(&created.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_synonym_sets_parameter_adds() {
    let c = client();
    let (coll, attached, named) = (
        "tessera_it_param",
        "tessera_it_param_a",
        "tessera_it_param_n",
    );
    create(&c, coll).await;
    add_docs(&c, coll, &["sofa", "couch", "chair"]).await;
    c.upsert_set(SetKind::Synonym, attached, &[syn("a", &["sofa", "couch"])])
        .await
        .expect("attached");
    c.upsert_set(SetKind::Synonym, named, &[syn("n", &["sofa", "chair"])])
        .await
        .expect("named");
    c.update_collection(coll, &json!({"synonym_sets": [attached]}))
        .await
        .expect("attach");

    let plain = find(&c, coll, "sofa", &[]).await;
    let with_named = find(&c, coll, "sofa", &[("synonym_sets", named)]).await;
    let off = find(&c, coll, "sofa", &[("enable_synonyms", "false")]).await;
    let off_named = find(
        &c,
        coll,
        "sofa",
        &[("enable_synonyms", "false"), ("synonym_sets", named)],
    )
    .await;
    let mut missing = search_all();
    missing.push(("synonym_sets".into(), "tessera_it_param_missing".into()));
    let missing = c.search(coll, &missing).await;
    c.update_collection(coll, &json!({"synonym_sets": []}))
        .await
        .ok();
    c.delete_collection(coll).await.expect("cleanup");
    c.delete_set(SetKind::Synonym, attached)
        .await
        .expect("cleanup");
    c.delete_set(SetKind::Synonym, named)
        .await
        .expect("cleanup");

    assert_eq!(sorted(plain), ["1", "2"]);
    assert_eq!(sorted(with_named), ["1", "2", "3"]);
    assert_eq!(off, ["1"]);
    assert_eq!(off_named, ["1"], "enable_synonyms=false beats every set");
    assert_eq!(status(&missing.unwrap_err()), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_curation_sets_is_no_parameter() {
    let c = client();
    let (coll, set) = ("tessera_it_cur_param", "tessera_it_cur_param_set");
    create(&c, coll).await;
    add_docs(&c, coll, &["shoe", "shoe boot", "shoe sandal"]).await;
    c.upsert_set(SetKind::Curation, set, &[pin("p", "shoe", "3")])
        .await
        .expect("set");

    let plain = find(&c, coll, "shoe", &[]).await;
    let named = find(&c, coll, "shoe", &[("curation_sets", set)]).await;
    let missing = find(
        &c,
        coll,
        "shoe",
        &[("curation_sets", "tessera_it_cur_none")],
    )
    .await;
    c.delete_collection(coll).await.expect("cleanup");
    c.delete_set(SetKind::Curation, set).await.expect("cleanup");

    assert_eq!(named, plain);
    assert_eq!(missing, plain);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_curation_tags_gate_both_ways() {
    let c = client();
    let (coll, set) = ("tessera_it_tags", "tessera_it_tags_set");
    create(&c, coll).await;
    add_docs(&c, coll, &["shoe", "shoe boot", "shoe sandal"]).await;
    // Pins document 3 to the front when it fires.
    let rule = |id: &str, rule: serde_json::Value| json!({"id": id, "rule": rule, "includes": [{"id": "3", "position": 1}]});
    let first = |ids: Vec<String>| ids.first().cloned().unwrap_or_default();
    let cases: [(&str, serde_json::Value); 3] = [
        ("untagged", json!({"query": "shoe", "match": "exact"})),
        ("tagged", json!({"tags": ["vip"]})),
        (
            "both",
            json!({"query": "shoe", "match": "exact", "tags": ["vip"]}),
        ),
    ];
    let mut fired = Vec::new();
    for (id, r) in cases {
        c.upsert_set(SetKind::Curation, set, &[rule(id, r)])
            .await
            .expect("set");
        c.update_collection(coll, &json!({"curation_sets": [set]}))
            .await
            .expect("attach");
        let no_tags = first(find(&c, coll, "shoe", &[]).await) == "3";
        let vip = first(find(&c, coll, "shoe", &[("curation_tags", "vip")]).await) == "3";
        let other = first(find(&c, coll, "shoe", &[("curation_tags", "other")]).await) == "3";
        let off = first(find(&c, coll, "shoe", &[("enable_curations", "false")]).await) == "3";
        fired.push((id, no_tags, vip, other, off));
        c.update_collection(coll, &json!({"curation_sets": []}))
            .await
            .expect("detach");
    }
    c.delete_collection(coll).await.expect("cleanup");
    c.delete_set(SetKind::Curation, set).await.expect("cleanup");

    // (case, no tags, tag vip, tag other, curations off)
    assert_eq!(
        fired,
        [
            ("untagged", true, false, false, false),
            ("tagged", false, true, false, false),
            ("both", false, true, false, false),
        ]
    );
}

fn version_at_least(version: &str, major: u32, minor: u32) -> bool {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0)) >= (major, minor)
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_rule_flags_by_version() {
    let c = client();
    let set = "tessera_it_flags";
    let version = c.debug().await.expect("debug").version;
    let item = json!({"id": "r", "rule": {"query": "a", "match": "exact", "stem": true},
        "includes": [{"id": "1", "position": 1}],
        "diversity": {"similarity_metric": [{"field": "title", "method": "equality", "weight": 1.0}]}});
    let saved = c.upsert_set(SetKind::Curation, set, &[item]).await;
    let read = c.set_item(SetKind::Curation, set, "r").await;
    c.delete_set(SetKind::Curation, set).await.expect("cleanup");

    saved.unwrap();
    let read = read.unwrap();
    if version_at_least(&version, 30, 2) {
        assert_eq!(read["rule"]["stem"], json!(true), "{version}");
    } else {
        assert!(read["rule"].get("stem").is_none(), "{version}: {read}");
    }
    assert_eq!(
        read["diversity"]["similarity_metric"][0]["weight"],
        json!(1.0),
        "{version}"
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_bad_sort_breaks_matching_search() {
    let c = client();
    let (coll, set) = ("tessera_it_bad_sort", "tessera_it_bad_sort_set");
    create(&c, coll).await;
    add_docs(&c, coll, &["shoe"]).await;
    let item = json!({"id": "s", "rule": {"query": "shoe", "match": "exact"},
        "sort_by": "tessera_no_such_field:asc"});
    c.upsert_set(SetKind::Curation, set, &[item])
        .await
        .expect("set");
    c.update_collection(coll, &json!({"curation_sets": [set]}))
        .await
        .expect("attach");

    let matching = {
        let mut p = search_all();
        p[0].1 = "shoe".into();
        c.search(coll, &p).await
    };
    let other = {
        let mut p = search_all();
        p[0].1 = "boot".into();
        c.search(coll, &p).await
    };
    let star = c.search(coll, &search_all()).await;
    c.update_collection(coll, &json!({"curation_sets": []}))
        .await
        .ok();
    c.delete_collection(coll).await.expect("cleanup");
    c.delete_set(SetKind::Curation, set).await.expect("cleanup");

    assert_eq!(status(&matching.unwrap_err()), Some(404));
    other.unwrap();
    star.unwrap();
}
