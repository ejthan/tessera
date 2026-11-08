// SPDX-License-Identifier: GPL-3.0-or-later

//! Import and export against a real Typesense server, on throwaway collections.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-core --test live_transfer -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use std::future::Future;
use std::path::Path;

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{Client, ClientOptions, DirtyValues, ImportAction, ImportParams};
use tessera_core::schema::SchemaDraft;
use tessera_core::transfer::export::{
    ExportFormat, ExportJob, ExportProgress, ExportScope, FieldPick, run_export,
};
use tessera_core::transfer::import::{ImportJob, ImportOutcome, ImportProgress, run_import};
use tessera_core::transfer::infer::infer_schema;
use tessera_core::transfer::source::{Format, ReadOptions, Source};
use tessera_core::transfer::{JobControl, JobEnd};
use tokio::sync::watch;
use uuid::Uuid;

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn client() -> Client {
    let url = env("TESSERA_IT_URL", "http://localhost:18108");
    let key = SecretString::from(env("TESSERA_IT_KEY", "xyz"));
    Client::new(&[url.as_str()], key, ClientOptions::default()).unwrap()
}

/// Runs `body` with `count` unused names and removes those collections, also on a panic.
async fn with_names<F, Fut>(count: usize, body: F)
where
    F: FnOnce(Client, Vec<String>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let client = client();
    let names: Vec<String> = (0..count)
        .map(|_| format!("tessera_it_{}", Uuid::new_v4().simple()))
        .collect();
    let run = tokio::spawn(body(client.clone(), names.clone())).await;
    for name in &names {
        // Most tests create only some of them.
        let _ = client.delete_collection(name).await;
    }
    run.unwrap();
}

async fn import(
    client: &Client,
    collection: &str,
    source: Source,
    options: ReadOptions,
) -> ImportOutcome {
    let (tx, _rx) = watch::channel(ImportProgress::default());
    let job = ImportJob {
        source,
        options,
        collection: collection.to_string(),
        params: ImportParams {
            action: ImportAction::Create,
            batch_size: 500,
            dirty_values: DirtyValues::CoerceOrReject,
        },
    };
    run_import(client.clone(), job, JobControl::default(), tx).await
}

fn rich_schema(name: &str) -> Value {
    json!({
        "name": name,
        "enable_nested_fields": true,
        "fields": [
            {"name": "title", "type": "string"},
            {"name": "n", "type": "int64"},
            {"name": "tags", "type": "string[]"},
            {"name": "loc", "type": "geopoint"},
            {"name": "o", "type": "object"},
        ],
    })
}

fn rich_lines(count: usize) -> String {
    (0..count)
        .map(|i| {
            let lat = (i % 90) as f64 / 2.0;
            json!({
                "id": i.to_string(),
                "title": format!("doc, \"{i}\""),
                "n": i,
                "tags": [format!("t{}", i % 7), "all"],
                "loc": [lat, 11.5],
                "o": {"city": format!("c{i}"), "zip": i % 100},
            })
            .to_string()
                + "\n"
        })
        .collect()
}

fn rich_fields() -> Vec<(String, String)> {
    [
        ("title", "string"),
        ("n", "int64"),
        ("tags", "string[]"),
        ("loc", "geopoint"),
    ]
    .map(|(name, ty)| (name.to_string(), ty.to_string()))
    .to_vec()
}

async fn export(client: &Client, collection: &str, format: ExportFormat, target: &Path) {
    let (tx, _rx) = watch::channel(ExportProgress::default());
    let job = ExportJob {
        collection: collection.to_string(),
        scope: ExportScope::All,
        fields: FieldPick::All,
        format,
        target: target.to_path_buf(),
        expected: None,
    };
    let outcome = run_export(client.clone(), job, JobControl::default(), tx).await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    assert_eq!(outcome.unlisted_values, 0);
}

fn sorted_documents(text: &str) -> Vec<Value> {
    let mut docs: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    docs.sort_by_key(|doc| doc["id"].as_str().unwrap().to_string());
    docs
}

async fn full_text(client: &Client, collection: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("all.jsonl");
    export(client, collection, ExportFormat::Jsonl, &path).await;
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_every_format_round_trips() {
    with_names(4, |client, names| async move {
        let dir = tempfile::tempdir().unwrap();
        client
            .create_collection(&rich_schema(&names[0]))
            .await
            .unwrap();
        let source = Source::Bytes(rich_lines(2000).into_bytes().into());
        let first = import(&client, &names[0], source, ReadOptions::new(Format::Jsonl)).await;
        assert!(matches!(first.end, JobEnd::Finished), "{:?}", first.end);
        assert_eq!(first.progress.imported, 2000);
        assert!(first.count.unwrap().ok);
        let original = full_text(&client, &names[0]).await;

        let rounds = [
            (ExportFormat::Jsonl, Format::Jsonl, "out.jsonl"),
            (ExportFormat::JsonArray, Format::JsonArray, "out.json"),
            (ExportFormat::Csv, Format::Csv, "out.csv"),
        ];
        for (i, (out, read, file)) in rounds.into_iter().enumerate() {
            let target = dir.path().join(file);
            export(&client, &names[0], out, &target).await;
            let name = &names[i + 1];
            client.create_collection(&rich_schema(name)).await.unwrap();
            let mut options = ReadOptions::new(read);
            options.fields = rich_fields();
            let outcome = import(&client, name, Source::File(target), options).await;
            assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
            assert_eq!(
                (outcome.progress.imported, outcome.progress.failed),
                (2000, 0),
                "{file}: {:?}",
                outcome.report.rows().first()
            );
            assert_eq!(client.collection(name).await.unwrap().num_documents, 2000);
            let copied = full_text(&client, name).await;
            assert_eq!(
                sorted_documents(&copied),
                sorted_documents(&original),
                "{file}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_csv_cells_follow_the_schema() {
    with_names(1, |client, names| async move {
        let schema = json!({
            "name": names[0],
            "fields": [
                {"name": "n", "type": "int64"},
                {"name": "price", "type": "float"},
                {"name": "on", "type": "bool"},
                {"name": "tags", "type": "string[]"},
                {"name": "zip", "type": "string"},
                {"name": "note", "type": "string", "optional": true},
            ],
        });
        client.create_collection(&schema).await.unwrap();
        let text = "id,n,price,on,tags,zip,note\n1,5,2.5,TRUE,\"[\"\"a\"\"]\",007,\n";
        let mut options = ReadOptions::new(Format::Csv);
        options.fields = [
            ("n", "int64"),
            ("price", "float"),
            ("on", "bool"),
            ("tags", "string[]"),
            ("zip", "string"),
            ("note", "string"),
        ]
        .map(|(name, ty)| (name.to_string(), ty.to_string()))
        .to_vec();
        let source = Source::Bytes(text.as_bytes().into());
        let outcome = import(&client, &names[0], source, options).await;
        assert_eq!(outcome.progress.imported, 1, "{:?}", outcome.report.rows());
        let doc = client.document(&names[0], "1").await.unwrap();
        assert_eq!(
            doc,
            json!({"id":"1","n":5,"price":2.5,"on":true,"tags":["a"],"zip":"007"})
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_failed_lines_import_after_the_schema_is_fixed() {
    with_names(1, |client, names| async move {
        let name = &names[0];
        let schema = json!({"name": name, "fields": [{"name": "t", "type": "string"}]});
        client.create_collection(&schema).await.unwrap();
        // The bad line follows a stored document, whose id the server echoes for it.
        let text = "{\"id\":\"1\",\"t\":\"a\"}\nnot json\n{\"id\":\"2\"}\n\n{\"id\":\"3\"}\n";
        let source = Source::Bytes(text.as_bytes().into());
        let outcome = import(&client, name, source, ReadOptions::new(Format::Jsonl)).await;
        assert_eq!(outcome.progress.imported, 1);
        assert_eq!(outcome.progress.failed, 3);
        assert_eq!(outcome.progress.unconfirmed, 0);
        let rows = outcome.report.rows();
        let found: Vec<(u64, Option<&str>)> =
            rows.iter().map(|r| (r.line, r.id.as_deref())).collect();
        assert_eq!(found, [(2, None), (3, Some("2")), (5, Some("3"))]);

        let dir = tempfile::tempdir().unwrap();
        let failed = dir.path().join("failed.jsonl");
        assert_eq!(outcome.report.save_failed(&failed).await.unwrap(), 3);

        let fix = json!({"fields": [
            {"name": "t", "drop": true},
            {"name": "t", "type": "string", "optional": true},
        ]});
        client.update_collection(name, &fix).await.unwrap();
        let again = import(
            &client,
            name,
            Source::File(failed),
            ReadOptions::new(Format::Jsonl),
        )
        .await;
        assert_eq!(again.progress.imported, 2);
        assert_eq!(again.progress.failed, 1, "the line that is no JSON stays");
        assert_eq!(client.collection(name).await.unwrap().num_documents, 3);
    })
    .await;
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_inferred_schema_is_accepted() {
    with_names(1, |client, names| async move {
        let source = Source::Bytes(rich_lines(50).into_bytes().into());
        let options = ReadOptions::new(Format::Jsonl);
        let inferred = infer_schema(&source, &options).unwrap();
        let draft = inferred.to_draft(&names[0]);
        client.create_collection(&draft.to_json()).await.unwrap();
        let outcome = import(&client, &names[0], source, options).await;
        assert_eq!(outcome.progress.imported, 50, "{:?}", outcome.report.rows());
        assert!(outcome.count.unwrap().ok);
    })
    .await;
}

fn copy_schema(name: &str) -> Value {
    json!({
        "name": name,
        "enable_nested_fields": true,
        "fields": [
            {"name": "title", "type": "string"},
            {"name": "loc", "type": "geopoint"},
            {"name": "o", "type": "object"},
            {"name": "o.city", "type": "string", "facet": true},
            {"name": "v", "type": "float[]", "num_dim": 3},
        ],
    })
}

fn copy_lines(count: usize) -> String {
    (0..count)
        .map(|i| {
            json!({
                "id": i.to_string(),
                "title": format!("doc {i}"),
                "loc": [(i % 90) as f64 / 2.0, 11.5],
                "o": {"city": format!("c{}", i % 10), "zip": i % 100},
                "v": [0.25, 0.5, (i % 4) as f64 / 4.0],
                "outside": {"note": format!("n{i}")},
            })
            .to_string()
                + "\n"
        })
        .collect()
}

/// The copy the reindex assistant runs: export to a file, a new collection from the schema, an
/// import of that file.
async fn copy_through_a_file(client: &Client, source: &str, target: &str) -> ImportOutcome {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("documents.jsonl");
    export(client, source, ExportFormat::Jsonl, &path).await;
    let collection = client.collection(source).await.unwrap();
    let mut draft = SchemaDraft::from_collection(&collection);
    draft.name = target.to_string();
    client.create_collection(&draft.to_json()).await.unwrap();
    import(
        client,
        target,
        Source::File(path),
        ReadOptions::new(Format::Jsonl),
    )
    .await
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_copy_through_a_file() {
    with_names(2, |client, names| async move {
        let (source, target) = (&names[0], &names[1]);
        client
            .create_collection(&copy_schema(source))
            .await
            .unwrap();
        let outcome = import(
            &client,
            source,
            Source::Bytes(copy_lines(2_000).into_bytes().into()),
            ReadOptions::new(Format::Jsonl),
        )
        .await;
        assert_eq!(outcome.progress.imported, 2_000, "{:?}", outcome.end);

        let outcome = copy_through_a_file(&client, source, target).await;
        assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
        assert_eq!(outcome.progress.imported, 2_000);
        assert_eq!(
            client.collection(target).await.unwrap().num_documents,
            2_000
        );

        let sorted = |text: String| {
            let mut lines: Vec<(String, String)> = text
                .lines()
                .map(|line| {
                    let doc: Value = serde_json::from_str(line).unwrap();
                    (doc["id"].as_str().unwrap().to_string(), line.to_string())
                })
                .collect();
            lines.sort();
            lines
        };
        assert_eq!(
            sorted(full_text(&client, source).await),
            sorted(full_text(&client, target).await)
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_a_copy_loses_unstored_fields() {
    with_names(4, |client, names| async move {
        for (source, target, optional) in
            [(&names[0], &names[1], true), (&names[2], &names[3], false)]
        {
            let schema = json!({
                "name": source,
                "fields": [
                    {"name": "title", "type": "string"},
                    {"name": "secret", "type": "string", "store": false, "optional": optional},
                ],
            });
            client.create_collection(&schema).await.unwrap();
            let lines: String = (0..20)
                .map(|i| {
                    json!({"id": i.to_string(), "title": "t", "secret": "s"}).to_string() + "\n"
                })
                .collect();
            let outcome = import(
                &client,
                source,
                Source::Bytes(lines.into_bytes().into()),
                ReadOptions::new(Format::Jsonl),
            )
            .await;
            assert_eq!(outcome.progress.imported, 20, "{:?}", outcome.end);

            let outcome = copy_through_a_file(&client, source, target).await;
            if optional {
                assert_eq!(outcome.progress.imported, 20, "{:?}", outcome.end);
                assert!(!full_text(&client, target).await.contains("secret"));
            } else {
                assert_eq!(outcome.progress.imported, 0);
                assert_eq!(outcome.progress.failed, 20, "{:?}", outcome.end);
                assert_eq!(outcome.report.total(), 20);
            }
        }
    })
    .await;
}
