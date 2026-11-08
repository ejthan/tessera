// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests against a real Typesense server.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-client --test live -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{
    Client, ClientOptions, DirtyValues, DocumentQuery, Error, ExportQuery, ImportAction,
    ImportParams, KeyAccess, RaftState,
};

fn url() -> String {
    std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into())
}

fn client_with(key: &str) -> Client {
    Client::new(&[&url()], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
}

fn admin_key() -> String {
    std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into())
}

fn client() -> Client {
    client_with(&admin_key())
}

fn admin_request(method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let url = format!("{}/{path}", url().trim_end_matches('/'));
    reqwest::Client::new()
        .request(method, url)
        .header("X-TYPESENSE-API-KEY", admin_key())
}

/// A raw admin request for test setup and cleanup, outside the client under test.
async fn admin(method: reqwest::Method, path: &str, body: Option<&str>) -> serde_json::Value {
    let mut request = admin_request(method, path);
    if let Some(body) = body {
        request = request.body(body.to_string());
    }
    let response = request.send().await.expect("admin request");
    assert!(
        response.status().is_success(),
        "{path}: {}",
        response.status()
    );
    response.json().await.expect("admin response is JSON")
}

async fn wait_until_ready(client: &Client) {
    for _ in 0..60 {
        if matches!(client.health().await, Ok(h) if h.ok) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("Typesense at {} did not become healthy within 30 s", url());
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_health_ok() {
    wait_until_ready(&client()).await;
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_debug_is_leader_and_30x() {
    let client = client();
    wait_until_ready(&client).await;
    let debug = client.debug().await.unwrap();
    assert_eq!(debug.state, RaftState::Leader);
    assert!(
        debug.version.starts_with("30."),
        "version {}",
        debug.version
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_wrong_key_is_401() {
    wait_until_ready(&client()).await;
    match client_with("wrong").debug().await {
        Err(Error::Http {
            status, message, ..
        }) => {
            assert_eq!(status, 401);
            assert!(message.contains("x-typesense-api-key"), "{message}");
        }
        other => panic!("expected 401, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_metrics_parses() {
    let client = client();
    wait_until_ready(&client).await;
    let metrics = client.metrics().await.unwrap();
    assert!(metrics.cpu_percent.is_some(), "{metrics:?}");
    assert!(metrics.memory_total.unwrap_or(0) > 0, "{metrics:?}");
    assert!(metrics.raw.contains_key("typesense_memory_active_bytes"));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_stats_parses() {
    let client = client();
    wait_until_ready(&client).await;
    let stats = client.stats().await.unwrap();
    assert!(stats.total_requests_per_second.is_some(), "{stats:?}");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collections_summary() {
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_summary";
    // Best effort, status ignored: an aborted earlier run may have left the collection behind.
    let _ = admin_request(reqwest::Method::DELETE, &format!("collections/{name}"))
        .send()
        .await;
    let schema = format!(r#"{{"name":"{name}","fields":[{{"name":"title","type":"string"}}]}}"#);
    admin(reqwest::Method::POST, "collections", Some(&schema)).await;
    let summary = client.collections_summary().await;
    // Clean up before asserting, so a failed assertion leaves no collection behind.
    admin(
        reqwest::Method::DELETE,
        &format!("collections/{name}"),
        None,
    )
    .await;
    let summary = summary.unwrap();
    let entry = summary
        .iter()
        .find(|c| c.name == name)
        .expect("collection listed");
    assert_eq!(entry.num_documents, 0);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collections_probe() {
    let client = client();
    wait_until_ready(&client).await;
    client.collections_probe().await.unwrap();
    client.node(0).collections_probe().await.unwrap();
    let wrong = client_with("not-the-key").collections_probe().await;
    assert!(
        matches!(wrong, Err(Error::Http { status: 401, .. })),
        "{wrong:?}"
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_keys_probe_admin() {
    let client = client();
    wait_until_ready(&client).await;
    assert_eq!(client.keys_probe().await.unwrap(), KeyAccess::Admin);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_keys_probe_restricted() {
    wait_until_ready(&client()).await;
    let body = r#"{"description":"tessera-it","actions":["documents:search"],"collections":["*"]}"#;
    let created = admin(reqwest::Method::POST, "keys", Some(body)).await;
    let id = created["id"].as_u64().expect("key id");
    let value = created["value"].as_str().expect("key value").to_string();
    let probe = client_with(&value).keys_probe().await;
    // Delete before asserting, so a failed assertion leaves no key behind.
    admin(reqwest::Method::DELETE, &format!("keys/{id}"), None).await;
    assert_eq!(probe.unwrap(), KeyAccess::Restricted);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collections_only_key_cannot_read_aliases() {
    wait_until_ready(&client()).await;
    let body = r#"{"description":"tessera-it-collections","actions":["collections:*"],"collections":["*"]}"#;
    let created = admin(reqwest::Method::POST, "keys", Some(body)).await;
    let id = created["id"].as_u64().expect("key id");
    let value = created["value"].as_str().expect("key value").to_string();
    let scoped = client_with(&value);
    let collections = scoped.collections().await;
    let aliases = scoped.aliases().await;
    // Delete before asserting, so a failed assertion leaves no key behind.
    admin(reqwest::Method::DELETE, &format!("keys/{id}"), None).await;
    collections.unwrap();
    assert!(
        matches!(
            aliases,
            Err(Error::Http {
                status: 401 | 403,
                ..
            })
        ),
        "{aliases:?}"
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collection_reads() {
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_reads";
    let _ = admin_request(reqwest::Method::DELETE, &format!("collections/{name}"))
        .send()
        .await;
    let schema = format!(
        r#"{{"name":"{name}","fields":[{{"name":"title","type":"string"}},{{"name":"n","type":"int32"}}],"default_sorting_field":"n"}}"#
    );
    admin(reqwest::Method::POST, "collections", Some(&schema)).await;
    let one = client.collection(name).await;
    let all = client.collections().await;
    let aliases = client.aliases().await;
    // Clean up before asserting, so a failed assertion leaves no collection behind.
    admin(
        reqwest::Method::DELETE,
        &format!("collections/{name}"),
        None,
    )
    .await;
    let one = one.unwrap();
    assert_eq!(one.name, name);
    assert_eq!(one.fields.len(), 2);
    assert_eq!(one.default_sorting_field, "n");
    assert!(one.fields[0].flag("index"));
    assert!(all.unwrap().iter().any(|c| c.name == name));
    aliases.unwrap();
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_collection_lifecycle() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_lifecycle";
    let _ = client.delete_collection(name).await;
    let schema =
        json!({"name":name,"fields":[{"name":"n","type":"int32"},{"name":"t","type":"string"}]});
    let created = client.create_collection(&schema).await;
    let duplicate = client.create_collection(&schema).await;
    let invalid = client
        .create_collection(&json!({"name":"x","fields":[]}))
        .await;
    admin(
        reqwest::Method::POST,
        &format!("collections/{name}/documents"),
        Some(r#"{"n":1,"t":"a"}"#),
    )
    .await;
    // Add, drop and change a type (drop + add in one request).
    let patch = client
        .update_collection(
            name,
            &json!({"fields":[
                {"name":"extra","type":"string","optional":true},
                {"name":"t","drop":true},
                {"name":"n","drop":true},{"name":"n","type":"int64"}
            ]}),
        )
        .await;
    let changed = client.collection(name).await;
    let without_drop = client
        .update_collection(name, &json!({"fields":[{"name":"n","type":"int32"}]}))
        .await;
    let changes = client.schema_changes().await;
    // Metadata and synonym sets, then removing the metadata with an empty object.
    let synonyms = "tessera_it_lifecycle_synonyms";
    admin(
        reqwest::Method::PUT,
        &format!("synonym_sets/{synonyms}"),
        Some(r#"{"items":[{"id":"s1","synonyms":["a","b"]}]}"#),
    )
    .await;
    let with_meta = client
        .update_collection(
            name,
            &json!({"metadata":{"owner":"it"},"synonym_sets":[synonyms]}),
        )
        .await;
    let meta = client.collection(name).await;
    let cleared = client
        .update_collection(name, &json!({"metadata":{}}))
        .await;
    let no_meta = client.collection(name).await;
    let unknown_set = client
        .update_collection(name, &json!({"synonym_sets":["tessera_it_no_such_set"]}))
        .await;
    let truncated = client.truncate_collection(name).await;
    let deleted = client.delete_collection(name).await;
    let gone = client.collection(name).await;
    admin(
        reqwest::Method::DELETE,
        &format!("synonym_sets/{synonyms}"),
        None,
    )
    .await;
    // Everything ran; now assert.
    assert_eq!(created.unwrap().name, name);
    match duplicate {
        Err(e @ Error::Http { status: 409, .. }) => assert!(e.typesense_message().is_some()),
        other => panic!("duplicate: {other:?}"),
    }
    match invalid {
        Err(e @ Error::Http { status: 400, .. }) => assert!(e.typesense_message().is_some()),
        other => panic!("invalid: {other:?}"),
    }
    patch.unwrap();
    let fields = changed.unwrap().fields;
    assert!(fields.iter().any(|f| f.name == "extra"));
    assert!(!fields.iter().any(|f| f.name == "t"));
    assert_eq!(
        fields.iter().find(|f| f.name == "n").unwrap().r#type,
        "int64"
    );
    match without_drop {
        Err(e @ Error::Http { status: 400, .. }) => {
            assert!(
                e.typesense_message()
                    .unwrap()
                    .contains("already part of the schema")
            );
        }
        other => panic!("change without drop: {other:?}"),
    }
    assert!(changes.unwrap().is_empty());
    with_meta.unwrap();
    let meta = meta.unwrap();
    assert_eq!(
        meta.metadata,
        Some(json!({"owner":"it"}).as_object().unwrap().clone())
    );
    assert_eq!(meta.synonym_sets, [synonyms]);
    cleared.unwrap();
    let no_meta = no_meta.unwrap();
    assert_eq!(no_meta.metadata, None, "{{}} removes the metadata");
    assert_eq!(no_meta.synonym_sets, [synonyms], "untouched");
    // 30.2 answers 404, not 400, for a synonym set that does not exist.
    match unknown_set {
        Err(e @ Error::Http { status: 404, .. }) => {
            assert!(
                e.typesense_message().unwrap().contains("not found"),
                "{e:?}"
            );
        }
        other => panic!("unknown synonym set: {other:?}"),
    }
    assert_eq!(truncated.unwrap(), 1);
    deleted.unwrap();
    assert!(matches!(gone, Err(Error::Http { status: 404, .. })));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_document_reads() {
    use serde_json::json;
    use tessera_client::DocumentQuery;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_documents";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[
            {"name":"n","type":"int32"},{"name":"genre","type":"string","facet":true}
        ]}))
        .await
        .unwrap();
    let jsonl = [
        r#"{"id":"a","n":1,"genre":"crime"}"#,
        r#"{"id":"b","n":2,"genre":"crime"}"#,
        r#"{"id":"c","n":3,"genre":"drama"}"#,
    ]
    .join("\n");
    let url = format!(
        "{}/collections/{name}/documents/import?action=create",
        url().trim_end_matches('/')
    );
    let imported = reqwest::Client::new()
        .post(url)
        .header("X-TYPESENSE-API-KEY", admin_key())
        .body(jsonl)
        .send()
        .await
        .unwrap();
    assert!(imported.status().is_success());
    let query = DocumentQuery {
        page: 2,
        per_page: 2,
        filter_by: None,
        sort_by: Some("n:asc".into()),
        exclude_fields: None,
    };
    let page = client.search_documents(name, &query).await;
    let all = client.count_documents(name, None).await;
    let crime = client.count_documents(name, Some("genre:=crime")).await;
    let facets = client.facet_values(name, "genre", "cr", 10).await;
    let doc = client.document(name, "b").await;
    let export = client.export_documents(name, "genre:=crime").await;
    let bad = client.count_documents(name, Some("nope:=1")).await;
    let _ = client.delete_collection(name).await;
    let page = page.unwrap();
    assert_eq!((page.found, page.out_of, page.page), (3, 3, 2));
    assert_eq!(page.documents.len(), 1);
    assert_eq!(page.documents[0]["id"], "c");
    assert_eq!(all.unwrap(), 3);
    assert_eq!(crime.unwrap(), 2);
    assert_eq!(facets.unwrap(), ["crime"]);
    assert_eq!(doc.unwrap()["n"], 2);
    assert_eq!(export.unwrap().lines().count(), 2);
    match bad {
        Err(e @ Error::Http { status: 400, .. }) => assert!(e.typesense_message().is_some()),
        other => panic!("bad filter: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_document_writes() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_doc_writes";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"enable_nested_fields":true,"fields":[
            {"name":"title","type":"string"},
            {"name":"author","type":"object","optional":true},
            {"name":"author.name","type":"string","optional":true},
            {"name":"author.age","type":"int32","optional":true}
        ]}))
        .await
        .unwrap();
    let doc = json!({"id":"d","title":"t","author":{"name":"n","age":3}});
    let created = client.create_document(name, &doc).await;
    let duplicate = client.create_document(name, &doc).await;
    let merged = client
        .patch_document(name, "d", &json!({"author":{"age":4}}))
        .await;
    let after_merge = client.document(name, "d").await;
    let removed = client
        .patch_document(name, "d", &json!({"author":{"age":null}}))
        .await;
    let after_remove = client.document(name, "d").await;
    let required = client
        .patch_document(name, "d", &json!({"title":null}))
        .await;
    let upserted = client
        .upsert_document(name, &json!({"id":"d","title":"new"}))
        .await;
    let after_upsert = client.document(name, "d").await;
    let missing_required = client.upsert_document(name, &json!({"id":"d"})).await;
    let deleted = client.delete_document(name, "d").await;
    let again = client.delete_document(name, "d").await;
    let _ = client.delete_collection(name).await;
    assert_eq!(created.unwrap()["id"], "d");
    assert!(matches!(duplicate, Err(Error::Http { status: 409, .. })));
    merged.unwrap();
    assert_eq!(after_merge.unwrap()["author"], json!({"name":"n","age":4}));
    removed.unwrap();
    assert_eq!(after_remove.unwrap()["author"], json!({"name":"n"}));
    assert!(matches!(required, Err(Error::Http { status: 400, .. })));
    upserted.unwrap();
    let replaced = after_upsert.unwrap();
    assert_eq!(replaced["title"], "new");
    assert!(replaced.get("author").is_none());
    assert!(matches!(
        missing_required,
        Err(Error::Http { status: 400, .. })
    ));
    deleted.unwrap();
    assert!(matches!(again, Err(Error::Http { status: 404, .. })));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_bulk_by_filter() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_bulk";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[
            {"name":"n","type":"int32"},{"name":"tag","type":"string"}
        ]}))
        .await
        .unwrap();
    let docs: String = (0..2500)
        .map(|i| json!({"id":format!("doc-{i:05}"),"n":i,"tag":"t"}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let imported = admin_request(
        reqwest::Method::POST,
        &format!("/collections/{name}/documents/import?action=create"),
    )
    .body(docs)
    .send()
    .await
    .expect("import");
    assert!(imported.status().is_success());
    let updated = client
        .update_by_filter(name, "n:<10", &json!({"tag":"u"}))
        .await;
    let wrong_type = client
        .update_by_filter(name, "n:<10", &json!({"n":"text"}))
        .await;
    let unknown = client.delete_by_filter(name, "nope:=1").await;
    let deleted = client.delete_by_filter(name, "n:<5").await;
    // 2,000 ids: the filter is well over 4,000 bytes, which GET search refuses.
    let ids: Vec<String> = (10..2010).map(|i| format!("`doc-{i:05}`")).collect();
    let filter = format!("id:=[{}]", ids.join(","));
    assert!(filter.len() > 4000);
    let many = client.delete_by_filter(name, &filter).await;
    let left = client.count_documents(name, None).await;
    let _ = client.delete_collection(name).await;
    assert_eq!(updated.unwrap(), 10);
    assert_eq!(wrong_type.unwrap(), 0);
    assert!(matches!(unknown, Err(Error::Http { status: 400, .. })));
    assert_eq!(deleted.unwrap(), 5);
    assert_eq!(many.unwrap(), 2000);
    assert_eq!(left.unwrap(), 2500 - 5 - 2000);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_odd_ids_delete_only_themselves() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_odd_ids";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[{"name":"t","type":"string"}]}))
        .await
        .unwrap();
    let odd = ["*", "keep1 ", " lead", "trail ", "  "];
    let others = ["keep1", "other", ".", ".."];
    for id in odd.iter().chain(&others) {
        client
            .create_document(name, &json!({"id":id,"t":"x"}))
            .await
            .unwrap();
    }
    for id in odd {
        client.delete_document(name, id).await.unwrap();
    }
    let left = client.count_documents(name, None).await;
    let bystander = client.document(name, "keep1").await;
    // A URL cannot hold these two: `..` would reach the collection itself.
    let dots = [
        client.delete_document(name, "..").await,
        client.delete_document(name, ".").await,
    ];
    let by_filter = client.delete_by_filter(name, "id:[`.`,`..`]").await;
    let after = client.count_documents(name, None).await;
    let _ = client.delete_collection(name).await;
    assert_eq!(left.unwrap(), others.len() as u64);
    assert_eq!(bystander.unwrap()["id"], "keep1");
    for dot in dots {
        assert!(matches!(dot, Err(Error::UnaddressableId(_))), "{dot:?}");
    }
    assert_eq!(by_filter.unwrap(), 2);
    assert_eq!(after.unwrap(), 2);
}

/// What "Copy selection" exports for the ids it puts in a filter, `.` and `..` included.
#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_export_by_id_filter_returns_only_those() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_export_ids";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[{"name":"t","type":"string"}]}))
        .await
        .unwrap();
    for id in [".", "..", "a", "b"] {
        client
            .create_document(name, &json!({"id":id,"t":"x"}))
            .await
            .unwrap();
    }
    let dot = client.export_documents(name, "id:[`.`]").await;
    let dots = client.export_documents(name, "id:[`..`]").await;
    let some = client.export_documents(name, "id:[`a`,`b`]").await;
    let _ = client.delete_collection(name).await;
    let ids = |text: String| -> Vec<String> {
        let mut ids: Vec<String> = text
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["id"].to_string())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(dot.unwrap()), ["\".\""]);
    assert_eq!(ids(dots.unwrap()), ["\"..\""]);
    assert_eq!(ids(some.unwrap()), ["\"a\"", "\"b\""]);
}

/// What the bulk dialog refuses and why: the count and the write disagree on these filters.
#[tokio::test]
#[ignore = "needs a Typesense server"]
async fn live_filters_that_name_everything() {
    let client = client();
    let name = "tessera_live_all_filters";
    let _ = client.delete_collection(name).await;
    let schema = serde_json::json!({"name": name, "fields": [
        {"name": "n", "type": "int32"},
        {"name": "t", "type": "string", "optional": true},
    ]});
    client.create_collection(&schema).await.unwrap();
    let import = "{\"id\":\"1\",\"n\":1}\n{\"id\":\"2\",\"n\":2}\n{\"id\":\"3\",\"n\":3}";
    let imported = admin_request(
        reqwest::Method::POST,
        &format!("/collections/{name}/documents/import?action=create"),
    )
    .body(import)
    .send()
    .await
    .expect("import");
    assert!(imported.status().is_success());
    let partial = serde_json::json!({"t": "u"});

    let blank_count = client.count_documents(name, Some("  ")).await;
    let blank_delete = client.delete_by_filter(name, "  ").await;
    let blank_update = client.update_by_filter(name, "  ", &partial).await;
    let star_count = client.count_documents(name, Some("*")).await;
    let star_delete = client.delete_by_filter(name, "*").await;
    let star_update = client.update_by_filter(name, " * ", &partial).await;
    let id_count = client.count_documents(name, Some("id:[a,*]")).await;
    let id_delete = client.delete_by_filter(name, "id:[a,*]").await;
    let left = client.count_documents(name, None).await;
    let _ = client.delete_collection(name).await;

    assert_eq!(blank_count.unwrap(), 3);
    assert_eq!(blank_delete.unwrap(), 0);
    assert_eq!(blank_update.unwrap(), 0);
    assert!(matches!(star_count, Err(Error::Http { status: 400, .. })));
    assert!(matches!(star_delete, Err(Error::Http { status: 400, .. })));
    assert_eq!(star_update.unwrap(), 3);
    assert_eq!(id_count.unwrap(), 3);
    assert_eq!(id_delete.unwrap(), 3);
    assert_eq!(left.unwrap(), 0);
}

/// The bulk dialog's preview must be what the write takes, whatever is attached.
#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_count_is_what_a_delete_removes_under_a_curation_set() {
    let client = client();
    let (name, set) = ("tessera_it_curated", "tessera_it_curated_set");
    let _ = client.delete_collection(name).await;
    let rules = [
        r#"{"items":[{"id":"r","rule":{"query":"*","match":"exact"},"filter_by":"n:<3"}]}"#,
        r#"{"items":[{"id":"r","rule":{"query":"*","match":"exact"},
            "excludes":[{"id":"1"},{"id":"2"}]}]}"#,
        r#"{"items":[{"id":"r","rule":{"query":"*","match":"exact"},
            "includes":[{"id":"1","position":1}]}]}"#,
    ];
    let mut seen = Vec::new();
    for rule in rules {
        admin(
            reqwest::Method::PUT,
            &format!("/curation_sets/{set}"),
            Some(rule),
        )
        .await;
        client
            .create_collection(&serde_json::json!({
                "name": name,
                "fields": [{"name": "n", "type": "int32"}],
                "curation_sets": [set],
            }))
            .await
            .unwrap();
        let docs: Vec<String> = (1..=12)
            .map(|i| serde_json::json!({"id": i.to_string(), "n": i}).to_string())
            .collect();
        let imported = admin_request(
            reqwest::Method::POST,
            &format!("/collections/{name}/documents/import?action=create"),
        )
        .body(docs.join("\n"))
        .send()
        .await
        .expect("import");
        assert!(imported.status().is_success());
        let counted = client.count_documents(name, Some("n:>5")).await;
        // The table's page must show the same figure.
        let query = DocumentQuery {
            page: 1,
            per_page: 250,
            filter_by: Some("n:>5".into()),
            sort_by: None,
            exclude_fields: None,
        };
        let paged = client.search_documents(name, &query).await;
        let deleted = client.delete_by_filter(name, "n:>5").await;
        let _ = client.delete_collection(name).await;
        let paged = paged.unwrap();
        assert_eq!(paged.documents.len() as u64, paged.found);
        seen.push((counted.unwrap(), paged.found, deleted.unwrap()));
    }
    admin(
        reqwest::Method::DELETE,
        &format!("/curation_sets/{set}"),
        None,
    )
    .await;
    assert_eq!(seen, [(7, 7, 7), (7, 7, 7), (7, 7, 7)]);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_search_multi_search_and_presets() {
    use serde_json::{Value, json};
    use tessera_client::MultiSearch;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_search";
    let preset = "tessera_it_preset";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[
            {"name":"title","type":"string"},{"name":"n","type":"int32"}
        ]}))
        .await
        .unwrap();
    let docs: String = (0..20)
        .map(|i| json!({"id":format!("d{i}"),"title":format!("harbor {i}"),"n":i}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let imported = admin_request(
        reqwest::Method::POST,
        &format!("collections/{name}/documents/import?action=create"),
    )
    .body(docs)
    .send()
    .await
    .expect("import");
    assert!(imported.status().is_success());
    let pair = |k: &str, v: &str| (k.to_string(), v.to_string());

    let plain = client
        .search(name, &[pair("q", "harbor"), pair("query_by", "title")])
        .await;
    // Over 5,000 bytes of filter: the POST route must answer like the shortened GET.
    let padding: Vec<String> = (1000..2000).map(|n| n.to_string()).collect();
    let long_filter = format!("n:[0,1,2,3,4,{}]", padding.join(","));
    assert!(long_filter.len() > 5000);
    let long = client
        .search(name, &[pair("q", "*"), pair("filter_by", &long_filter)])
        .await;
    let short = client
        .search(name, &[pair("q", "*"), pair("filter_by", "n:[0,1,2,3,4]")])
        .await;
    let count = client.count_documents(name, Some(&long_filter)).await;
    let search = |q: &str| {
        json!({"collection":name,"q":q,"query_by":"title"})
            .as_object()
            .unwrap()
            .clone()
    };
    let mut bad = search("harbor");
    bad.insert("query_by".into(), "nope".into());
    let federated = client
        .multi_search(&MultiSearch {
            union: false,
            searches: vec![search("harbor"), bad],
            page: None,
            per_page: None,
        })
        .await;
    let union = client
        .multi_search(&MultiSearch {
            union: true,
            searches: vec![search("harbor"), search("harbor")],
            page: Some(1),
            per_page: Some(3),
        })
        .await;
    let mixed = client
        .multi_search(&MultiSearch {
            union: true,
            searches: vec![search("*"), search("harbor")],
            page: None,
            per_page: None,
        })
        .await;
    let stored = client
        .upsert_preset(preset, &json!({"q":"harbor","query_by":"title"}))
        .await;
    let got = client.preset(preset).await;
    let listed = client.presets().await;
    let removed = admin(reqwest::Method::DELETE, &format!("presets/{preset}"), None).await;
    let _ = client.delete_collection(name).await;

    let plain: Value = serde_json::from_str(&plain.unwrap()).unwrap();
    assert_eq!(plain["found"], 20);
    let (mut long, mut short): (Value, Value) = (
        serde_json::from_str(&long.unwrap()).unwrap(),
        serde_json::from_str(&short.unwrap()).unwrap(),
    );
    for v in [&mut long, &mut short] {
        for key in ["search_time_ms", "request_params"] {
            v.as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(long, short);
    assert_eq!(count.unwrap(), 5);
    let federated: Value = serde_json::from_str(&federated.unwrap()).unwrap();
    assert_eq!(federated["results"][0]["found"], 20);
    assert!(federated["results"][1]["code"].is_number());
    let union: Value = serde_json::from_str(&union.unwrap()).unwrap();
    assert_eq!(union["hits"].as_array().unwrap().len(), 3);
    assert_eq!(union["page"], 1);
    assert!(
        matches!(mixed, Err(Error::Http { status: 400, .. })),
        "{mixed:?}"
    );
    stored.unwrap();
    assert_eq!(got.unwrap()["value"]["q"], "harbor");
    assert!(listed.unwrap().iter().any(|p| p.name == preset));
    assert!(removed.is_object());
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_import_answers_per_line() {
    use serde_json::json;
    let client = client();
    wait_until_ready(&client).await;
    let name = "tessera_it_import";
    let alias = "tessera_it_import_alias";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name":name,"fields":[
            {"name":"t","type":"string"},{"name":"n","type":"int32","optional":true}
        ]}))
        .await
        .unwrap();
    let params = |action, dirty_values| ImportParams {
        action,
        batch_size: 100,
        dirty_values,
    };
    let body = [
        r#"{"id":"1","t":"a"}"#,
        r#"{"id":"2","t":"b","n":"x"}"#,
        "not json",
        r#"{"id":"1","t":"dup"}"#,
    ]
    .join("\n");
    let lines = client
        .import_batch(
            name,
            &params(ImportAction::Create, DirtyValues::default()),
            &body,
        )
        .await;
    let coerced = client
        .import_batch(
            name,
            &params(ImportAction::Create, DirtyValues::CoerceOrReject),
            r#"{"id":"3","t":"c","n":"12"}"#,
        )
        .await;
    let rejected = client
        .import_batch(
            name,
            &params(ImportAction::Create, DirtyValues::Reject),
            r#"{"id":"4","t":"d","n":"12"}"#,
        )
        .await;
    let emplace = client
        .import_batch(
            name,
            &params(ImportAction::Emplace, DirtyValues::default()),
            r#"{"id":"1","n":5}"#,
        )
        .await;
    admin(
        reqwest::Method::PUT,
        &format!("aliases/{alias}"),
        Some(&json!({"collection_name":name}).to_string()),
    )
    .await;
    let through_alias = client
        .import_batch(
            alias,
            &params(ImportAction::Create, DirtyValues::default()),
            r#"{"id":"5","t":"e"}"#,
        )
        .await;
    let count = client.count_documents(name, None).await;
    let _ = admin_request(reqwest::Method::DELETE, &format!("aliases/{alias}"))
        .send()
        .await;
    let _ = client.delete_collection(name).await;
    let lines = lines.unwrap();
    let shape: Vec<_> = lines.iter().map(|l| (l.success, l.code)).collect();
    assert_eq!(
        shape,
        [
            (true, None),
            (false, Some(400)),
            (false, Some(400)),
            (false, Some(409))
        ],
        "{lines:?}"
    );
    assert!(coerced.unwrap()[0].success);
    assert!(!rejected.unwrap()[0].success);
    assert!(emplace.unwrap()[0].success);
    assert!(through_alias.unwrap()[0].success);
    // 1, 3 and 5 are stored.
    assert_eq!(count.unwrap(), 3);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_export_stream() {
    let client = client();
    let (name, set) = ("tessera_it_export", "tessera_it_export_set");
    let _ = client.delete_collection(name).await;
    admin(
        reqwest::Method::PUT,
        &format!("/curation_sets/{set}"),
        Some(r#"{"items":[{"id":"r","rule":{"query":"*","match":"exact"},"filter_by":"n:<3"}]}"#),
    )
    .await;
    client
        .create_collection(&serde_json::json!({
            "name": name,
            "fields": [{"name": "n", "type": "int32"}],
            "curation_sets": [set],
        }))
        .await
        .unwrap();
    let docs: Vec<String> = (1..=12)
        .map(|i| serde_json::json!({"id": i.to_string(), "n": i}).to_string())
        .collect();
    let imported = admin_request(
        reqwest::Method::POST,
        &format!("/collections/{name}/documents/import?action=create"),
    )
    .body(docs.join("\n"))
    .send()
    .await
    .expect("import");
    assert!(imported.status().is_success());
    let read = |query: ExportQuery| {
        let client = client.clone();
        async move {
            let mut stream = client.export_stream(name, &query).await?;
            let mut lines = Vec::new();
            while let Some(line) = stream.next_line().await? {
                lines.push(line);
            }
            Ok::<_, Error>(lines)
        }
    };
    let filtered = read(ExportQuery {
        filter_by: Some("n:>5".into()),
        ..ExportQuery::default()
    })
    .await;
    let counted = client.count_documents(name, Some("n:>5")).await;
    let all = read(ExportQuery::default()).await;
    // Why the dialog offers only names from the schema: an unknown name is no error.
    let nope = read(ExportQuery {
        include_fields: Some("nope".into()),
        ..ExportQuery::default()
    })
    .await;
    let bad = read(ExportQuery {
        filter_by: Some("nope:=1".into()),
        ..ExportQuery::default()
    })
    .await;
    let _ = client.delete_collection(name).await;
    admin(
        reqwest::Method::DELETE,
        &format!("/curation_sets/{set}"),
        None,
    )
    .await;
    assert_eq!(filtered.unwrap().len() as u64, counted.unwrap());
    assert_eq!(all.unwrap().len(), 12);
    let nope = nope.unwrap();
    assert_eq!(nope.len(), 12);
    assert!(nope.iter().all(|l| l == "{}"), "{nope:?}");
    assert!(matches!(bad, Err(Error::Http { status: 400, .. })));
}
