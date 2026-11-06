// SPDX-License-Identifier: GPL-3.0-or-later

//! The backup and restore engines against a wiremock server, and the plan.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{Alias, Client, ClientOptions, Collection, ItemSet, SetKind, StopwordSet};
use tessera_core::backup::archive::{read_manifest, unpack, verify};
use tessera_core::backup::format::{FORMAT, Manifest, ManifestCollection};
use tessera_core::backup::restore::{
    Loaded, RestoreBlock, RestoreError, RestoreJob, RestoreOutcome, RestorePlan, RestoreRow,
    RestoreView, RestoreWarning, RowFailure, RowKind, RowResult, RowState, load, plan, run_restore,
};
use tessera_core::backup::run::{
    BackupError, BackupOutcome, BackupPlan, BackupStep, BackupTarget, BackupWarning,
    backup_warnings, run_backup,
};
use tessera_core::schema::SchemaDraft;
use tessera_core::transfer::{JobControl, JobEnd};
use tokio::sync::watch;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// A small Typesense: lists from its fields, and collections that fill up.
#[derive(Clone, Default)]
struct Fake {
    collections: Vec<Value>,
    aliases: Vec<Value>,
    synonym_sets: Vec<Value>,
    curation_sets: Vec<Value>,
    stopwords: Vec<Value>,
    dictionaries: Vec<String>,
    /// Export bodies by collection.
    documents: HashMap<String, String>,
    /// Paths that answer 500.
    broken: Vec<String>,
    /// A collection whose import refuses every line.
    refuse_lines: Option<String>,
    /// A collection whose count stays at 0 whatever was imported.
    count_stays: Option<String>,
    /// Created collection -> stored documents.
    stored: Arc<Mutex<HashMap<String, u64>>>,
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

impl Respond for Fake {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let get = request.method.as_str() == "GET";
        if self.broken.iter().any(|b| b == path) {
            return ResponseTemplate::new(500).set_body_json(json!({"message": "broken"}));
        }
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        match segments.as_slice() {
            ["debug"] => ok(json!({"version": "30.2", "state": 1})),
            ["collections"] if get => ok(Value::Array(self.collections.clone())),
            ["collections"] => {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let name = body["name"].as_str().unwrap().to_string();
                self.stored.lock().unwrap().insert(name.clone(), 0);
                ResponseTemplate::new(201).set_body_json(json!({"name": name, "fields": []}))
            }
            ["collections", name] => {
                let count = if self.count_stays.as_deref() == Some(name) {
                    0
                } else {
                    *self.stored.lock().unwrap().get(*name).unwrap_or(&0)
                };
                ok(json!({"name": name, "fields": [], "num_documents": count}))
            }
            ["collections", name, "documents", "export"] => ResponseTemplate::new(200)
                .set_body_string(self.documents.get(*name).cloned().unwrap_or_default()),
            ["collections", name, "documents", "import"] => {
                let lines = request.body.iter().filter(|b| **b == b'\n').count();
                let refuse = self.refuse_lines.as_deref() == Some(name);
                if !refuse {
                    *self
                        .stored
                        .lock()
                        .unwrap()
                        .entry(name.to_string())
                        .or_default() += lines as u64;
                }
                let line = if refuse {
                    "{\"success\":false,\"code\":400,\"error\":\"refused\"}\n"
                } else {
                    "{\"success\":true}\n"
                };
                ResponseTemplate::new(200).set_body_string(line.repeat(lines))
            }
            ["aliases"] => ok(json!({"aliases": self.aliases})),
            ["aliases", name] => {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                ok(json!({"name": name, "collection_name": body["collection_name"]}))
            }
            ["synonym_sets"] => ok(Value::Array(self.synonym_sets.clone())),
            ["curation_sets"] => ok(Value::Array(self.curation_sets.clone())),
            ["synonym_sets" | "curation_sets", name] => ok(json!({"name": name, "items": []})),
            ["stopwords"] => ok(json!({"stopwords": self.stopwords})),
            ["stopwords", _] => ok(json!({})),
            ["stemming", "dictionaries"] => ok(json!({"dictionaries": self.dictionaries})),
            _ => ResponseTemplate::new(404).set_body_json(json!({"message": path})),
        }
    }
}

async fn serve(fake: Fake) -> (MockServer, Client) {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(fake).mount(&server).await;
    let client = Client::new(
        &[server.uri().as_str()],
        SecretString::from("k"),
        ClientOptions::default(),
    )
    .unwrap();
    (server, client)
}

/// Every request that is not a read, as `METHOD path`.
async fn writes(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap();
    requests
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

async fn backup(client: Client, names: &[&str], target: BackupTarget) -> BackupOutcome {
    backup_with(client, names, target, JobControl::default()).await
}

async fn backup_with(
    client: Client,
    names: &[&str],
    target: BackupTarget,
    control: JobControl,
) -> BackupOutcome {
    let plan = BackupPlan {
        collections: names.iter().map(ToString::to_string).collect(),
        target,
    };
    let (tx, _rx) = watch::channel(Default::default());
    run_backup(client, plan, control, tx).await
}

fn field(name: &str) -> Value {
    json!({"name": name, "type": "string"})
}

fn books() -> Value {
    json!({
        "name": "books",
        "fields": [field("title")],
        "num_documents": 2,
        "synonym_sets": ["syn"],
        "curation_sets": ["cur"],
    })
}

fn shop_fake() -> Fake {
    Fake {
        collections: vec![
            books(),
            json!({"name": "other", "fields": [field("a")], "num_documents": 0,
                   "synonym_sets": ["other_syn"]}),
        ],
        aliases: vec![
            json!({"name": "shop", "collection_name": "books"}),
            json!({"name": "else", "collection_name": "other"}),
        ],
        synonym_sets: vec![
            json!({"name": "syn", "items": [{"id": "1", "synonyms": ["a", "b"]}]}),
            json!({"name": "other_syn", "items": []}),
        ],
        curation_sets: vec![json!({"name": "cur", "items": []})],
        stopwords: vec![json!({"id": "s", "stopwords": ["the"], "locale": "en"})],
        documents: HashMap::from([(
            "books".to_string(),
            "{\"id\":\"1\"}\n{\"id\":\"2\"}\n".to_string(),
        )]),
        ..Fake::default()
    }
}

fn json_file(dir: &Path, name: &str) -> Value {
    serde_json::from_slice(&fs::read(dir.join(name)).unwrap()).unwrap()
}

fn assert_finished<E: std::fmt::Debug>(end: &JobEnd<E>) {
    assert!(matches!(end, JobEnd::Finished), "{end:?}");
}

#[tokio::test]
async fn a_folder_backup_holds_everything_for_its_selection() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, client) = serve(shop_fake()).await;

    let outcome = backup(client, &["books"], BackupTarget::Folder(target.clone())).await;

    assert_finished(&outcome.end);
    assert_eq!(outcome.collections[0].documents, 2);
    assert_eq!(outcome.warnings, []);
    assert!(!root.path().join("b.part").exists());
    let manifest = read_manifest(&target).unwrap();
    verify(&target, &manifest, &JobControl::default()).unwrap();
    assert_eq!(manifest.format, FORMAT);
    assert_eq!(manifest.server_version, "30.2");
    assert_eq!(manifest.collections[0].name, "books");
    assert_eq!(manifest.collections[0].documents, 2);
    assert_eq!(manifest.files.len(), 6);

    assert_eq!(
        json_file(&target, "aliases.json"),
        json!([{"name": "shop", "collection_name": "books"}])
    );
    assert_eq!(
        json_file(&target, "stopwords.json"),
        json!([{"id": "s", "stopwords": ["the"], "locale": "en"}])
    );
    assert_eq!(json_file(&target, "synonym_sets.json")[0]["name"], "syn");
    assert_eq!(
        json_file(&target, "synonym_sets.json")
            .as_array()
            .unwrap()
            .len(),
        1,
        "only attached sets"
    );
    assert_eq!(json_file(&target, "curation_sets.json")[0]["name"], "cur");
    assert_eq!(
        json_file(&target, "collections/0001.schema.json")["name"],
        "books"
    );
    assert_eq!(
        fs::read_to_string(target.join("collections/0001.jsonl")).unwrap(),
        "{\"id\":\"1\"}\n{\"id\":\"2\"}\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_folder_backup_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, client) = serve(shop_fake()).await;

    backup(client, &["books"], BackupTarget::Folder(target.clone())).await;

    assert_eq!(mode(target.clone()), 0o700);
    assert_eq!(mode(target.join("collections")), 0o700);
    for file in [
        "manifest.json",
        "aliases.json",
        "collections/0001.schema.json",
        "collections/0001.jsonl",
    ] {
        assert_eq!(mode(target.join(file)), 0o600, "{file}");
    }
}

#[tokio::test]
async fn an_archive_backup_unpacks_and_verifies() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b.tar.zst");
    let (_server, client) = serve(shop_fake()).await;

    let outcome = backup(client, &["books"], BackupTarget::Archive(target.clone())).await;

    assert_finished(&outcome.end);
    let into = root.path().join("into");
    fs::create_dir(&into).unwrap();
    let control = JobControl::default();
    let manifest = unpack(&target, &into, &control).unwrap();
    verify(&into, &manifest, &control).unwrap();
}

#[tokio::test]
async fn an_empty_collection_has_an_empty_file() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, client) = serve(shop_fake()).await;

    let outcome = backup(client, &["other"], BackupTarget::Folder(target.clone())).await;

    assert_finished(&outcome.end);
    assert_eq!(
        fs::read(target.join("collections/0001.jsonl")).unwrap(),
        b""
    );
}

#[tokio::test]
async fn a_failed_read_stops_the_run_and_leaves_nothing() {
    for path in [
        "/aliases",
        "/synonym_sets",
        "/curation_sets",
        "/stopwords",
        "/debug",
    ] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("b");
        let fake = Fake {
            broken: vec![path.to_string()],
            ..shop_fake()
        };
        let (_server, client) = serve(fake).await;

        let outcome = backup(client, &["books"], BackupTarget::Folder(target.clone())).await;

        assert!(
            matches!(outcome.end, JobEnd::Failed(BackupError::Client(_))),
            "{path}: {:?}",
            outcome.end
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0, "{path}");
    }
}

#[tokio::test]
async fn a_failed_export_removes_the_part_folder() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let fake = Fake {
        broken: vec!["/collections/books/documents/export".to_string()],
        ..shop_fake()
    };
    let (_server, client) = serve(fake).await;

    let outcome = backup(client, &["books"], BackupTarget::Folder(target)).await;

    assert!(
        matches!(&outcome.end, JobEnd::Failed(BackupError::Export { collection, .. }) if collection == "books"),
        "{:?}",
        outcome.end
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_stop_removes_the_partial_backup() {
    for archive in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("b");
        let target = if archive {
            BackupTarget::Archive(path)
        } else {
            BackupTarget::Folder(path)
        };
        let (server, client) = serve(shop_fake()).await;
        let control = JobControl::default();
        control.stop.cancel();

        let outcome = backup_with(client, &["books"], target, control).await;

        assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        assert_eq!(writes(&server).await, Vec::<String>::new());
    }
}

#[tokio::test]
async fn a_target_that_exists_is_refused() {
    for archive in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("b");
        fs::write(&path, b"keep").unwrap();
        let target = if archive {
            BackupTarget::Archive(path.clone())
        } else {
            BackupTarget::Folder(path.clone())
        };
        let (_server, client) = serve(shop_fake()).await;

        let outcome = backup(client, &["books"], target).await;

        assert!(
            matches!(&outcome.end, JobEnd::Failed(BackupError::Exists(p)) if *p == path),
            "{:?}",
            outcome.end
        );
        assert_eq!(fs::read(&path).unwrap(), b"keep");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_as_target_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("b");
    std::os::unix::fs::symlink(root.path().join("nowhere"), &path).unwrap();
    let (_server, client) = serve(shop_fake()).await;

    let outcome = backup(client, &["books"], BackupTarget::Folder(path)).await;

    assert!(matches!(
        outcome.end,
        JobEnd::Failed(BackupError::Exists(_))
    ));
    assert!(!root.path().join("nowhere").exists());
}

#[tokio::test]
async fn a_part_folder_left_by_a_quit_is_refused_and_named() {
    let root = tempfile::tempdir().unwrap();
    let part = root.path().join("b.part");
    fs::create_dir(&part).unwrap();
    fs::write(part.join("mine"), b"keep").unwrap();
    let (_server, client) = serve(shop_fake()).await;

    let outcome = backup(
        client,
        &["books"],
        BackupTarget::Folder(root.path().join("b")),
    )
    .await;

    assert!(
        matches!(&outcome.end, JobEnd::Failed(BackupError::Exists(p)) if *p == part),
        "{:?}",
        outcome.end
    );
    assert_eq!(fs::read(part.join("mine")).unwrap(), b"keep");
}

#[tokio::test]
async fn an_unknown_collection_stops_the_run() {
    let root = tempfile::tempdir().unwrap();
    let (_server, client) = serve(shop_fake()).await;

    let outcome = backup(
        client,
        &["gone"],
        BackupTarget::Folder(root.path().join("b")),
    )
    .await;

    assert!(
        matches!(&outcome.end, JobEnd::Failed(BackupError::UnknownCollection(n)) if n == "gone")
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_count_that_moved_and_a_missing_set_are_warnings() {
    let root = tempfile::tempdir().unwrap();
    let mut fake = shop_fake();
    fake.collections[0]["num_documents"] = 5.into();
    fake.synonym_sets.clear();
    let (_server, client) = serve(fake).await;

    let outcome = backup(
        client,
        &["books"],
        BackupTarget::Folder(root.path().join("b")),
    )
    .await;

    assert_finished(&outcome.end);
    assert_eq!(
        outcome.warnings,
        [
            BackupWarning::CountMoved {
                collection: "books".into(),
                expected: 5,
                written: 2,
            },
            BackupWarning::MissingSet {
                collection: "books".into(),
                kind: SetKind::Synonym,
                name: "syn".into(),
            },
        ]
    );
}

fn collection(value: Value) -> Collection {
    serde_json::from_value(value).unwrap()
}

#[test]
fn warnings_come_from_the_lists() {
    let books = collection(json!({
        "name": "books",
        "fields": [
            {"name": "hidden", "type": "string", "store": false},
            {"name": "body", "type": "string", "stem_dictionary": "irregular"},
            {"name": "plain", "type": "string", "stem_dictionary": ""},
            {"name": "author", "type": "string", "reference": "authors.id"},
            {"name": "shop", "type": "string", "reference": "shop_alias.id"},
            {"name": "inside", "type": "string", "reference": "others_alias.id"},
            {"name": "v", "type": "float[]", "embed": {"from": ["body"], "model_config": {
                "model_name": "openai/x", "api_key": "sk"
            }}},
        ],
    }));
    let others = collection(json!({"name": "others", "fields": []}));
    let aliases = [
        Alias {
            name: "shop_alias".into(),
            collection_name: "shop".into(),
        },
        Alias {
            name: "others_alias".into(),
            collection_name: "others".into(),
        },
    ];
    let got = backup_warnings(
        &[books, others],
        &["books".to_string(), "others".to_string()],
        &aliases,
    );
    let book = || "books".to_string();
    assert_eq!(
        got,
        [
            BackupWarning::Unstored {
                collection: book(),
                fields: vec!["hidden".into()],
            },
            BackupWarning::BlankedSecrets {
                collection: book(),
                count: 1,
            },
            BackupWarning::Dictionaries {
                collection: book(),
                names: vec!["irregular".into()],
            },
            BackupWarning::OutsideReference {
                collection: book(),
                target: "authors".into(),
            },
            BackupWarning::OutsideReference {
                collection: book(),
                target: "shop_alias".into(),
            },
        ]
    );
}

const KEY: &str = "sk-live-do-not-keep";
const OTHER_KEY: &str = "other-do-not-keep";
const EXTRA_KEY: &str = "extra-do-not-keep";
const META_KEY: &str = "metadata-do-not-keep";

fn all_bytes(dir: &Path, out: &mut Vec<u8>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all_bytes(&path, out);
        } else {
            out.extend(fs::read(path).unwrap());
        }
    }
}

fn holds(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

fn secret_fake() -> Fake {
    Fake {
        collections: vec![json!({
            "name": "embedded",
            "num_documents": 1,
            "metadata": {"api_key": META_KEY},
            "voice_query_model": {"model_config": {"token": EXTRA_KEY}},
            "fields": [
                field("title"),
                {"name": "v", "type": "float[]", "embed": {
                    "from": ["title"],
                    "model_config": {
                        "model_name": "openai/text-embedding-3-small",
                        "api_key": KEY,
                        "unknown_to_tessera": OTHER_KEY,
                    },
                }},
            ],
        })],
        documents: HashMap::from([("embedded".to_string(), "{\"id\":\"1\"}\n".to_string())]),
        ..Fake::default()
    }
}

#[tokio::test]
async fn a_backup_holds_no_provider_key() {
    for archive in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("b");
        let target = if archive {
            BackupTarget::Archive(path.clone())
        } else {
            BackupTarget::Folder(path.clone())
        };
        let (_server, client) = serve(secret_fake()).await;

        let outcome = backup(client, &["embedded"], target).await;

        assert_finished(&outcome.end);
        let folder = if archive {
            let into = root.path().join("into");
            fs::create_dir(&into).unwrap();
            unpack(&path, &into, &JobControl::default()).unwrap();
            into
        } else {
            path
        };
        let mut bytes = Vec::new();
        all_bytes(&folder, &mut bytes);
        for secret in [KEY, OTHER_KEY, EXTRA_KEY, META_KEY] {
            assert!(!holds(&bytes, secret), "{secret}");
        }
        assert!(holds(&bytes, "openai/text-embedding-3-small"));
        let manifest = read_manifest(&folder).unwrap();
        assert_eq!(manifest.collections[0].blanked_secrets, 4);
        assert!(outcome.warnings.contains(&BackupWarning::BlankedSecrets {
            collection: "embedded".into(),
            count: 4,
        }));
    }
}

#[tokio::test]
async fn a_restore_of_blanked_keys_says_so_and_blocks_the_collection() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, client) = serve(secret_fake()).await;
    backup(client, &["embedded"], BackupTarget::Folder(target.clone())).await;

    let manifest = read_manifest(&target).unwrap();
    let loaded = load(&target, &manifest).unwrap();
    let got = plan(&manifest, &loaded, &RestoreView::default());

    assert_eq!(
        got.rows[0].state,
        RowState::Blocked(RestoreBlock::BlankSecrets(4))
    );
    assert_eq!(got.order, Vec::<String>::new());
}

/// A backup of the given parts on disk, without digests: `run_restore` starts after `verify`.
struct Backup {
    _dir: tempfile::TempDir,
    folder: PathBuf,
    manifest: Manifest,
    loaded: Loaded,
}

fn draft(value: Value) -> SchemaDraft {
    SchemaDraft::from_value(&value).unwrap()
}

fn backup_of(collections: Vec<Value>, resources: Loaded) -> Backup {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_path_buf();
    fs::create_dir(folder.join("collections")).unwrap();
    let mut entries = Vec::new();
    let mut schemas = Vec::new();
    for (i, schema) in collections.iter().enumerate() {
        let entry = ManifestCollection {
            file: i as u32 + 1,
            name: schema["name"].as_str().unwrap().to_string(),
            documents: 2,
            unstored: Vec::new(),
            blanked_secrets: 0,
            dictionaries: Vec::new(),
        };
        fs::write(
            folder.join(entry.documents_path()),
            "{\"id\":\"1\"}\n{\"id\":\"2\"}\n",
        )
        .unwrap();
        entries.push(entry);
        schemas.push(draft(schema.clone()));
    }
    let manifest = Manifest {
        format: FORMAT,
        created_at: 0,
        tessera_version: "0.1.0".into(),
        server_version: "30.2".into(),
        collections: entries,
        files: Vec::new(),
    };
    Backup {
        _dir: dir,
        folder,
        manifest,
        loaded: Loaded {
            schemas,
            ..resources
        },
    }
}

impl Backup {
    fn plan(&self, view: &RestoreView) -> RestorePlan {
        plan(&self.manifest, &self.loaded, view)
    }

    /// A job with every row of the backup ticked.
    fn job(&self, replace: bool) -> RestoreJob {
        let rows = self.plan(&RestoreView::default()).rows;
        let aliases = self.loaded.aliases.iter();
        let aliases = aliases.map(|a| (RowKind::Alias, a.name.clone()));
        RestoreJob {
            folder: self.folder.clone(),
            manifest: self.manifest.clone(),
            loaded: self.loaded.clone(),
            ticked: rows
                .into_iter()
                .map(|r| (r.kind, r.name))
                .chain(aliases)
                .collect(),
            replace,
            batch_size: 100,
        }
    }
}

async fn restore(client: Client, job: RestoreJob) -> RestoreOutcome {
    let (tx, _rx) = watch::channel(Default::default());
    run_restore(client, job, JobControl::default(), tx).await
}

fn stopwords(id: &str, words: &[&str]) -> StopwordSet {
    StopwordSet {
        id: id.into(),
        stopwords: words.iter().map(ToString::to_string).collect(),
        locale: Some("en".into()),
    }
}

fn item_set(name: &str, items: Vec<Value>) -> ItemSet {
    ItemSet {
        name: name.into(),
        items,
    }
}

fn alias(name: &str, target: &str) -> Alias {
    Alias {
        name: name.into(),
        collection_name: target.into(),
    }
}

fn schema(name: &str) -> Value {
    json!({"name": name, "fields": [field("title")]})
}

/// A collection as the server lists it.
fn existing(name: &str) -> Value {
    json!({"name": name, "fields": [field("title")], "num_documents": 0})
}

fn referencing(name: &str, target: &str) -> Value {
    json!({"name": name, "fields": [
        field("title"),
        {"name": "other", "type": "string", "reference": format!("{target}.id")},
    ]})
}

fn resources() -> Loaded {
    Loaded {
        aliases: vec![alias("shop", "books")],
        stopwords: vec![stopwords("s", &["the", "a"])],
        synonym_sets: vec![item_set(
            "syn",
            vec![json!({"id": "1", "synonyms": ["a", "b"]})],
        )],
        curation_sets: vec![item_set("cur", Vec::new())],
        ..Loaded::default()
    }
}

fn result(rows: &[RestoreRow], kind: RowKind, name: &str) -> RowResult {
    let row = rows.iter().find(|r| r.kind == kind && r.name == name);
    row.unwrap().result.clone()
}

#[tokio::test]
async fn writes_come_in_their_order() {
    let backup = backup_of(
        vec![referencing("books", "authors"), schema("authors")],
        resources(),
    );
    let (server, client) = serve(Fake::default()).await;

    let outcome = restore(client, backup.job(false)).await;

    assert_finished(&outcome.end);
    assert_eq!(
        writes(&server).await,
        [
            "PUT /stopwords/s",
            "PUT /synonym_sets/syn",
            "PUT /curation_sets/cur",
            "POST /collections",
            "POST /collections/authors/documents/import",
            "POST /collections",
            "POST /collections/books/documents/import",
            "PUT /aliases/shop",
        ]
    );
    assert_eq!(outcome.to_delete, ["authors", "books"]);
    assert!(outcome.rows.iter().all(|r| r.result == RowResult::Done));
}

#[tokio::test]
async fn documents_are_created_never_upserted() {
    let backup = backup_of(vec![schema("books")], Loaded::default());
    let (server, client) = serve(Fake::default()).await;

    restore(client, backup.job(false)).await;

    let requests = server.received_requests().await.unwrap();
    let import = requests
        .iter()
        .find(|r| r.url.path().ends_with("/import"))
        .unwrap();
    let query = import.url.query().unwrap();
    assert!(query.contains("action=create"), "{query}");
    assert!(query.contains("dirty_values=coerce_or_reject"), "{query}");
}

#[tokio::test]
async fn a_refresh_that_finds_a_new_collection_writes_nothing() {
    let backup = backup_of(vec![schema("books")], resources());
    let job = backup.job(true);
    let fake = Fake {
        collections: vec![existing("books")],
        ..Fake::default()
    };
    let (server, client) = serve(fake).await;

    let outcome = restore(client, job).await;

    assert!(
        matches!(&outcome.end, JobEnd::Failed(RestoreError::NowExists(n)) if n == &["books"]),
        "{:?}",
        outcome.end
    );
    assert_eq!(writes(&server).await, Vec::<String>::new());
    assert_eq!(outcome.to_delete, Vec::<String>::new());
}

#[tokio::test]
async fn an_alias_taken_since_the_plan_stops_a_collection_of_that_name_too() {
    let backup = backup_of(vec![schema("books")], Loaded::default());
    let fake = Fake {
        aliases: vec![json!({"name": "books", "collection_name": "live"})],
        collections: vec![existing("live")],
        ..Fake::default()
    };
    let job = backup.job(false);
    let (server, client) = serve(fake).await;

    let outcome = restore(client, job).await;

    assert!(matches!(
        outcome.end,
        JobEnd::Failed(RestoreError::NowExists(_))
    ));
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_import_stops_before_the_next_collection_and_the_aliases() {
    let backup = backup_of(
        vec![schema("books"), schema("zzz")],
        Loaded {
            aliases: vec![alias("shop", "books")],
            ..Loaded::default()
        },
    );
    let fake = Fake {
        refuse_lines: Some("books".into()),
        ..Fake::default()
    };
    let (server, client) = serve(fake).await;

    let outcome = restore(client, backup.job(false)).await;

    assert!(
        matches!(&outcome.end, JobEnd::Failed(RestoreError::Row { name, .. }) if name == "books"),
        "{:?}",
        outcome.end
    );
    assert_eq!(
        writes(&server).await,
        [
            "POST /collections",
            "POST /collections/books/documents/import"
        ]
    );
    assert_eq!(
        result(&outcome.rows, RowKind::Collection, "books"),
        RowResult::Failed(RowFailure::Lines {
            failed: 2,
            unconfirmed: 0,
        })
    );
    assert_eq!(
        result(&outcome.rows, RowKind::Collection, "zzz"),
        RowResult::Skipped
    );
    assert_eq!(
        result(&outcome.rows, RowKind::Alias, "shop"),
        RowResult::Skipped
    );
    assert_eq!(outcome.to_delete, ["books"]);
}

#[tokio::test]
async fn a_count_that_differs_from_the_manifest_stops_the_run() {
    let backup = backup_of(vec![schema("books"), schema("zzz")], Loaded::default());
    let fake = Fake {
        count_stays: Some("books".into()),
        ..Fake::default()
    };
    let (server, client) = serve(fake).await;

    let outcome = restore(client, backup.job(false)).await;

    assert!(matches!(outcome.end, JobEnd::Failed(_)));
    assert_eq!(
        result(&outcome.rows, RowKind::Collection, "books"),
        RowResult::Failed(RowFailure::Count {
            expected: 2,
            found: Some(0),
        })
    );
    assert_eq!(writes(&server).await.len(), 2);
}

#[tokio::test]
async fn an_import_the_server_refuses_marks_the_row() {
    let backup = backup_of(vec![schema("books")], Loaded::default());
    let fake = Fake {
        broken: vec!["/collections/books/documents/import".into()],
        ..Fake::default()
    };
    let (_server, client) = serve(fake).await;

    let outcome = restore(client, backup.job(false)).await;

    assert!(matches!(outcome.end, JobEnd::Failed(_)));
    assert_eq!(outcome.to_delete, ["books"]);
    assert!(matches!(
        result(&outcome.rows, RowKind::Collection, "books"),
        RowResult::Failed(_)
    ));
}

#[tokio::test]
async fn a_conflict_on_create_imports_nothing() {
    let backup = backup_of(vec![schema("books")], Loaded::default());
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"message": "already exists"})),
        )
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(Fake::default())
        .mount(&server)
        .await;
    let client = Client::new(
        &[server.uri().as_str()],
        SecretString::from("k"),
        ClientOptions::default(),
    )
    .unwrap();

    let outcome = restore(client, backup.job(false)).await;

    assert_eq!(writes(&server).await, ["POST /collections"]);
    assert_eq!(outcome.to_delete, Vec::<String>::new());
    assert!(matches!(
        result(&outcome.rows, RowKind::Collection, "books"),
        RowResult::Failed(RowFailure::Text(_))
    ));
}

/// A server where stopwords, a set and an alias of [`resources`] differ.
fn differing() -> Fake {
    Fake {
        collections: vec![existing("books")],
        stopwords: vec![json!({"id": "s", "stopwords": ["other"], "locale": "en"})],
        synonym_sets: vec![json!({"name": "syn", "items": []})],
        curation_sets: vec![json!({"name": "cur", "items": []})],
        aliases: vec![json!({"name": "shop", "collection_name": "old"})],
        ..Fake::default()
    }
}

#[tokio::test]
async fn replace_off_writes_nothing_to_a_row_that_differs() {
    let backup = backup_of(Vec::new(), resources());
    let (server, client) = serve(differing()).await;

    let outcome = restore(client, backup.job(false)).await;

    assert_finished(&outcome.end);
    assert_eq!(writes(&server).await, Vec::<String>::new());
    for (kind, name) in [
        (RowKind::Stopwords, "s"),
        (RowKind::SynonymSet, "syn"),
        (RowKind::Alias, "shop"),
    ] {
        assert_eq!(result(&outcome.rows, kind, name), RowResult::Kept, "{name}");
    }
    assert_eq!(
        result(&outcome.rows, RowKind::CurationSet, "cur"),
        RowResult::Skipped,
        "the same stays without a word"
    );
}

#[tokio::test]
async fn replace_on_writes_what_differs_and_only_that() {
    let backup = backup_of(Vec::new(), resources());
    let (server, client) = serve(differing()).await;

    let outcome = restore(client, backup.job(true)).await;

    assert_finished(&outcome.end);
    assert_eq!(
        writes(&server).await,
        [
            "PUT /stopwords/s",
            "PUT /synonym_sets/syn",
            "PUT /aliases/shop"
        ]
    );
}

#[tokio::test]
async fn replace_leaves_a_row_that_is_not_ticked() {
    let backup = backup_of(Vec::new(), resources());
    let mut job = backup.job(true);
    job.ticked.remove(&(RowKind::SynonymSet, "syn".to_string()));
    let (server, client) = serve(differing()).await;

    let outcome = restore(client, job).await;

    assert!(
        !writes(&server)
            .await
            .contains(&"PUT /synonym_sets/syn".to_string())
    );
    assert_eq!(
        result(&outcome.rows, RowKind::SynonymSet, "syn"),
        RowResult::Kept
    );
}

#[tokio::test]
async fn a_row_that_is_not_ticked_is_not_written() {
    let backup = backup_of(vec![schema("books")], resources());
    let job = RestoreJob {
        ticked: HashSet::from([(RowKind::Stopwords, "s".to_string())]),
        ..backup.job(false)
    };
    let (server, client) = serve(Fake::default()).await;

    let outcome = restore(client, job).await;

    assert_finished(&outcome.end);
    assert_eq!(writes(&server).await, ["PUT /stopwords/s"]);
}

#[tokio::test]
async fn an_alias_whose_target_was_not_restored_is_left_out_and_named() {
    let backup = backup_of(vec![schema("books")], resources());
    let mut job = backup.job(false);
    job.ticked
        .remove(&(RowKind::Collection, "books".to_string()));
    let (server, client) = serve(Fake::default()).await;

    let outcome = restore(client, job).await;

    assert_finished(&outcome.end);
    assert!(
        !writes(&server)
            .await
            .contains(&"PUT /aliases/shop".to_string())
    );
    assert!(outcome.warnings.contains(&RestoreWarning::AliasNoTarget {
        alias: "shop".into(),
        target: "books".into(),
    }));
}

#[tokio::test]
async fn a_stop_writes_nothing_more() {
    let backup = backup_of(vec![schema("books")], resources());
    let (server, client) = serve(Fake::default()).await;
    let control = JobControl::default();
    control.stop.cancel();
    let (tx, _rx) = watch::channel(Default::default());

    let outcome = run_restore(client, backup.job(false), control, tx).await;

    assert!(matches!(outcome.end, JobEnd::Stopped));
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_set_neither_in_the_backup_nor_on_the_server_is_dropped_from_the_schema() {
    let attached = json!({
        "name": "books",
        "fields": [field("title")],
        "synonym_sets": ["syn", "gone"],
        "curation_sets": ["cur", "gone_too"],
    });
    let backup = backup_of(vec![attached], resources());
    let (server, client) = serve(Fake::default()).await;

    let outcome = restore(client, backup.job(false)).await;

    assert_finished(&outcome.end);
    let requests = server.received_requests().await.unwrap();
    let create = requests
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == "/collections")
        .unwrap();
    let body: Value = serde_json::from_slice(&create.body).unwrap();
    assert_eq!(body["synonym_sets"], json!(["syn"]));
    assert_eq!(body["curation_sets"], json!(["cur"]));
    for (kind, name) in [(SetKind::Synonym, "gone"), (SetKind::Curation, "gone_too")] {
        assert!(outcome.warnings.contains(&RestoreWarning::MissingSet {
            collection: "books".into(),
            kind,
            name: name.into(),
        }));
    }
}

fn state(plan: &RestorePlan, kind: RowKind, name: &str) -> RowState {
    let row = plan.rows.iter().find(|r| r.kind == kind && r.name == name);
    row.unwrap().state.clone()
}

#[test]
fn plan_shows_new_for_what_the_server_lacks() {
    let backup = backup_of(vec![schema("books")], resources());
    let got = backup.plan(&RestoreView::default());
    assert_eq!(got.rows.len(), 5);
    assert!(got.rows.iter().all(|r| r.state == RowState::New));
    assert_eq!(got.order, ["books"]);
}

#[test]
fn plan_ignores_order_and_server_filled_keys() {
    let backup = backup_of(vec![schema("books")], resources());
    let view = RestoreView {
        stopwords: vec![stopwords("s", &["a", "the"])],
        synonym_sets: vec![item_set(
            "syn",
            vec![json!({"id": "1", "root": "", "synonyms": ["a", "b"]})],
        )],
        curation_sets: vec![item_set("cur", Vec::new())],
        aliases: vec![alias("shop", "books")],
        ..RestoreView::default()
    };
    let got = backup.plan(&view);
    assert_eq!(state(&got, RowKind::Stopwords, "s"), RowState::Same);
    assert_eq!(state(&got, RowKind::SynonymSet, "syn"), RowState::Same);
    assert_eq!(state(&got, RowKind::CurationSet, "cur"), RowState::Same);
    assert_eq!(state(&got, RowKind::Alias, "shop"), RowState::Same);
}

#[test]
fn plan_shows_what_differs() {
    let backup = backup_of(Vec::new(), resources());
    let view = RestoreView {
        collections: vec!["books".into()],
        stopwords: vec![StopwordSet {
            locale: Some("de".into()),
            ..stopwords("s", &["the", "a"])
        }],
        synonym_sets: vec![item_set(
            "syn",
            vec![json!({"id": "2", "synonyms": ["c", "d"]})],
        )],
        curation_sets: vec![item_set(
            "cur",
            vec![json!({"id": "gone", "rule": {"tags": ["a"]}})],
        )],
        aliases: vec![alias("shop", "elsewhere")],
        ..RestoreView::default()
    };
    let got = backup.plan(&view);
    for (kind, name) in [
        (RowKind::Stopwords, "s"),
        (RowKind::SynonymSet, "syn"),
        (RowKind::CurationSet, "cur"),
        (RowKind::Alias, "shop"),
    ] {
        assert_eq!(state(&got, kind, name), RowState::Differs, "{name}");
    }
}

#[test]
fn plan_blocks_an_existing_collection() {
    let backup = backup_of(vec![schema("books")], Loaded::default());
    let view = RestoreView {
        collections: vec!["books".into()],
        ..RestoreView::default()
    };
    let got = backup.plan(&view);
    assert_eq!(
        state(&got, RowKind::Collection, "books"),
        RowState::Blocked(RestoreBlock::CollectionExists)
    );
    assert_eq!(got.order, Vec::<String>::new());
}

#[test]
fn plan_blocks_a_name_an_alias_holds() {
    let backup = backup_of(vec![schema("books")], Loaded::default());
    let view = RestoreView {
        collections: vec!["live".into()],
        aliases: vec![alias("books", "live")],
        ..RestoreView::default()
    };
    assert_eq!(
        state(&backup.plan(&view), RowKind::Collection, "books"),
        RowState::Blocked(RestoreBlock::CollectionExists)
    );
}

#[test]
fn plan_blocks_a_name_listed_twice() {
    let mut backup = backup_of(vec![schema("books"), schema("books")], Loaded::default());
    backup.manifest.collections[1].name = "books".into();
    let got = backup.plan(&RestoreView::default());
    assert_eq!(got.rows[0].state, RowState::New);
    assert_eq!(
        got.rows[1].state,
        RowState::Blocked(RestoreBlock::CollectionExists)
    );
    assert_eq!(got.order, ["books"]);
}

#[test]
fn plan_blocks_blank_secrets() {
    let embedded = json!({"name": "e", "fields": [
        {"name": "v", "type": "float[]", "embed": {"from": ["t"], "model_config": {
            "model_name": "openai/x", "api_key": ""
        }}},
    ]});
    let backup = backup_of(vec![embedded], Loaded::default());
    assert_eq!(
        state(
            &backup.plan(&RestoreView::default()),
            RowKind::Collection,
            "e"
        ),
        RowState::Blocked(RestoreBlock::BlankSecrets(1))
    );
}

#[test]
fn plan_blocks_a_required_unstored_field_and_warns_of_an_optional_one() {
    let unstored = json!({"name": "u", "fields": [
        field("req"),
        {"name": "opt", "type": "string", "optional": true},
    ]});
    let mut backup = backup_of(vec![unstored], Loaded::default());
    backup.manifest.collections[0].unstored = vec!["opt".into()];
    let got = backup.plan(&RestoreView::default());
    assert_eq!(state(&got, RowKind::Collection, "u"), RowState::New);
    assert_eq!(
        got.warnings,
        [RestoreWarning::Unstored {
            collection: "u".into(),
            fields: vec!["opt".into()],
        }]
    );

    backup.manifest.collections[0].unstored = vec!["opt".into(), "req".into()];
    assert_eq!(
        state(
            &backup.plan(&RestoreView::default()),
            RowKind::Collection,
            "u"
        ),
        RowState::Blocked(RestoreBlock::RequiredUnstored(vec!["req".into()]))
    );
}

#[test]
fn plan_blocks_the_mask() {
    let masked = json!({"name": "m", "fields": [], "metadata": {"key": "••••"}});
    let backup = backup_of(vec![masked], Loaded::default());
    assert_eq!(
        state(
            &backup.plan(&RestoreView::default()),
            RowKind::Collection,
            "m"
        ),
        RowState::Blocked(RestoreBlock::Mask)
    );
}

#[test]
fn plan_warns_of_a_missing_dictionary_and_reference() {
    let mut backup = backup_of(vec![referencing("books", "gone")], Loaded::default());
    backup.manifest.collections[0].dictionaries = vec!["irregular".into()];
    let got = backup.plan(&RestoreView::default());
    assert_eq!(
        got.warnings,
        [
            RestoreWarning::MissingDictionary {
                collection: "books".into(),
                name: "irregular".into(),
            },
            RestoreWarning::MissingReference {
                collection: "books".into(),
                target: "gone".into(),
            },
        ]
    );
}

#[test]
fn referenced_collections_come_first() {
    let backup = backup_of(
        vec![
            referencing("c", "b"),
            referencing("b", "a"),
            schema("a"),
            schema("free"),
        ],
        Loaded::default(),
    );
    let got = backup.plan(&RestoreView::default());
    assert_eq!(got.order, ["a", "free", "b", "c"]);
    assert_eq!(got.warnings, []);
}

#[test]
fn a_reference_through_an_alias_of_the_backup_counts() {
    let backup = backup_of(
        vec![referencing("books", "authors_alias"), schema("authors")],
        Loaded {
            aliases: vec![alias("authors_alias", "authors")],
            ..Loaded::default()
        },
    );
    let got = backup.plan(&RestoreView::default());
    assert_eq!(got.order, ["authors", "books"]);
    assert_eq!(got.warnings, []);
}

#[test]
fn a_cycle_keeps_manifest_order_and_warns() {
    let backup = backup_of(
        vec![
            referencing("x", "y"),
            referencing("y", "x"),
            referencing("self", "self"),
        ],
        Loaded::default(),
    );
    let got = backup.plan(&RestoreView::default());
    assert_eq!(got.order, ["self", "x", "y"]);
    assert_eq!(
        got.warnings,
        [RestoreWarning::ReferenceCycle(vec!["x".into(), "y".into()])]
    );
}

#[test]
fn an_alias_with_no_target_is_left_out_and_named() {
    let backup = backup_of(
        Vec::new(),
        Loaded {
            aliases: vec![alias("shop", "gone")],
            ..Loaded::default()
        },
    );
    let got = backup.plan(&RestoreView::default());
    assert_eq!(got.rows, []);
    assert_eq!(
        got.warnings,
        [RestoreWarning::AliasNoTarget {
            alias: "shop".into(),
            target: "gone".into(),
        }]
    );
}

#[test]
fn an_alias_a_collection_would_hide_is_left_out_and_named() {
    let backup = backup_of(
        vec![schema("books")],
        Loaded {
            aliases: vec![alias("books", "books"), alias("elsewhere", "live")],
            ..Loaded::default()
        },
    );
    let view = RestoreView {
        collections: vec!["live".into(), "elsewhere".into()],
        ..RestoreView::default()
    };
    let got = backup.plan(&view);
    assert_eq!(
        got.warnings,
        [
            RestoreWarning::AliasShadowed("books".into()),
            RestoreWarning::AliasShadowed("elsewhere".into()),
        ]
    );
}

#[tokio::test]
async fn load_reads_a_backup_and_names_from_the_manifest_win() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, client) = serve(shop_fake()).await;
    backup(client, &["books"], BackupTarget::Folder(target.clone())).await;
    let mut manifest = read_manifest(&target).unwrap();

    manifest.collections[0].name = "renamed".into();
    let loaded = load(&target, &manifest).unwrap();

    assert_eq!(loaded.schemas[0].name, "renamed");
    assert_eq!(loaded.aliases, [alias("shop", "books")]);
    assert_eq!(loaded.synonym_sets[0].name, "syn");
    assert_eq!(loaded.curation_sets[0].name, "cur");
    assert_eq!(loaded.stopwords[0].id, "s");
}

#[tokio::test]
async fn load_refuses_a_schema_that_is_no_schema() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, client) = serve(shop_fake()).await;
    backup(client, &["books"], BackupTarget::Folder(target.clone())).await;
    let manifest = read_manifest(&target).unwrap();
    fs::write(target.join("collections/0001.schema.json"), b"[1]").unwrap();

    assert!(matches!(
        load(&target, &manifest),
        Err(tessera_core::backup::restore::LoadError::Bad { .. })
    ));
}

#[tokio::test]
async fn a_backup_restores_into_an_empty_server() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b");
    let (_server, source) = serve(shop_fake()).await;
    backup(source, &["books"], BackupTarget::Folder(target.clone())).await;
    let manifest = read_manifest(&target).unwrap();
    verify(&target, &manifest, &JobControl::default()).unwrap();
    let loaded = load(&target, &manifest).unwrap();
    let rows = plan(&manifest, &loaded, &RestoreView::default()).rows;
    let job = RestoreJob {
        folder: target,
        manifest,
        loaded,
        ticked: rows.into_iter().map(|r| (r.kind, r.name)).collect(),
        replace: false,
        batch_size: 100,
    };
    let (server, client) = serve(Fake::default()).await;

    let outcome = restore(client, job).await;

    assert_finished(&outcome.end);
    assert_eq!(writes(&server).await.len(), 6);
}

#[tokio::test]
async fn progress_ends_on_the_last_step() {
    let root = tempfile::tempdir().unwrap();
    let (_server, client) = serve(shop_fake()).await;
    let (tx, rx) = watch::channel(Default::default());
    let plan = BackupPlan {
        collections: vec!["books".into()],
        target: BackupTarget::Folder(root.path().join("b")),
    };

    run_backup(client, plan, JobControl::default(), tx).await;

    let last = rx.borrow().clone();
    assert_eq!(last.step, BackupStep::Hashing);
    assert_eq!((last.done, last.total), (1, 1));
    assert_eq!(last.collection, "books");
}
