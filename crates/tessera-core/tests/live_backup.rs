// SPDX-License-Identifier: GPL-3.0-or-later

//! A backup of the seeded data on one server, restored into a second, empty one, as a folder
//! and as an archive.
//!
//! The second server is emptied afterwards, so the dev port and 8108 are refused there, and it
//! must start empty. Throwaway servers:
//! `docker run --rm -d --name t302 -p 18208:8108 --tmpfs /data typesense/typesense:30.2
//! --data-dir /data --api-key=xyz --enable-cors=false`, the same on 18209, then
//! `cargo xtask seed --url http://localhost:18208 --key xyz` and
//! `TESSERA_IT_URL=http://localhost:18208 TESSERA_IT_URL2=http://localhost:18209 cargo test -p
//! tessera-core --test live_backup -- --ignored`. `TESSERA_IT_KEY` overrides the key `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{
    Alias, Client, ClientOptions, DirtyValues, ImportAction, ImportParams, ItemSet, SetKind,
    StopwordSet,
};
use tessera_core::backup::archive::{read_manifest, unpack, verify};
use tessera_core::backup::restore::{RestoreJob, RestoreView, RowState, load, plan, run_restore};
use tessera_core::backup::run::{BackupPlan, BackupTarget, run_backup};
use tessera_core::relevance::changes::changes;
use tessera_core::transfer::export::{ExportFormat, ExportJob, ExportScope, FieldPick, run_export};
use tessera_core::transfer::{JobControl, JobEnd};
use tokio::sync::watch;
use url::Url;

const COLLECTIONS: [&str; 4] = ["books", "authors", "books_ref", "products"];

fn client(url: &str) -> Client {
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".to_string());
    Client::new(&[url], SecretString::from(key), ClientOptions::default()).unwrap()
}

fn source() -> Client {
    client(&std::env::var("TESSERA_IT_URL").expect("TESSERA_IT_URL names the seeded server"))
}

fn target() -> Client {
    let url = std::env::var("TESSERA_IT_URL2").expect("TESSERA_IT_URL2 names an empty server");
    let port = Url::parse(&url).unwrap().port();
    assert!(
        !matches!(port, Some(18108 | 8108)),
        "the restore target is emptied, so it must be a throwaway server"
    );
    client(&url)
}

async fn sorted_documents(client: &Client, name: &str, dir: &Path) -> Vec<Value> {
    let path = dir.join(format!("{}-{name}.jsonl", uuid::Uuid::new_v4().simple()));
    let job = ExportJob {
        collection: name.to_string(),
        scope: ExportScope::All,
        fields: FieldPick::All,
        format: ExportFormat::Jsonl,
        target: path.clone(),
        expected: None,
    };
    let (tx, _rx) = watch::channel(Default::default());
    let outcome = run_export(client.clone(), job, JobControl::default(), tx).await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    let text = std::fs::read_to_string(path).unwrap();
    let mut docs: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    docs.sort_by_key(|d| d["id"].as_str().unwrap().to_string());
    docs
}

async fn empty(client: &Client) {
    for alias in client.aliases().await.unwrap() {
        client.delete_alias(&alias.name).await.unwrap();
    }
    for collection in client.collections().await.unwrap() {
        client.delete_collection(&collection.name).await.unwrap();
    }
    for kind in [SetKind::Synonym, SetKind::Curation] {
        for set in client.sets(kind).await.unwrap() {
            client.delete_set(kind, &set.name).await.unwrap();
        }
    }
    for set in client.stopwords().await.unwrap() {
        client.delete_stopwords(&set.id).await.unwrap();
    }
}

fn set_named<'a>(sets: &'a [ItemSet], name: &str) -> &'a ItemSet {
    sets.iter().find(|s| s.name == name).unwrap()
}

fn sorted_stopwords(mut sets: Vec<StopwordSet>) -> Vec<StopwordSet> {
    sets.sort_by_key(|s| s.id.clone());
    sets.iter_mut().for_each(|s| s.stopwords.sort());
    sets
}

fn sorted_aliases(mut aliases: Vec<Alias>) -> Vec<Alias> {
    aliases.retain(|a| COLLECTIONS.contains(&a.collection_name.as_str()));
    aliases.sort_by_key(|a| a.name.clone());
    aliases
}

/// Backs up, restores into the empty `to`, and compares, then empties it again.
async fn round_trip(from: &Client, to: &Client, backup: BackupTarget, folder: &Path, dir: &Path) {
    let plan_b = BackupPlan {
        collections: COLLECTIONS.map(str::to_string).to_vec(),
        target: backup.clone(),
    };
    let (tx, _rx) = watch::channel(Default::default());
    let outcome = run_backup(from.clone(), plan_b, JobControl::default(), tx).await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);

    let control = JobControl::default();
    let manifest = match &backup {
        BackupTarget::Folder(_) => read_manifest(folder).unwrap(),
        BackupTarget::Archive(path) => unpack(path, folder, &control).unwrap(),
    };
    let verified = verify(folder, &manifest, &control).unwrap();
    let loaded = load(&verified).unwrap();
    let view = RestoreView::read(to).await.unwrap();
    assert!(
        view.collections.is_empty(),
        "the second server must be empty"
    );
    let shown = plan(&manifest, &loaded, &view);
    assert!(
        shown.rows.iter().all(|r| r.state == RowState::New),
        "{:?}",
        shown.rows
    );

    let job = RestoreJob {
        backup: verified,
        loaded,
        ticked: shown.rows.into_iter().map(|r| (r.kind, r.name)).collect(),
        replace: false,
        batch_size: 1000,
    };
    let (tx, _rx) = watch::channel(Default::default());
    let restored = run_restore(to.clone(), job, control, tx).await;
    assert!(
        matches!(restored.end, JobEnd::Finished),
        "{:?} {:?}",
        restored.end,
        restored.rows
    );

    for name in COLLECTIONS {
        let (a, b) = (
            from.collection(name).await.unwrap(),
            to.collection(name).await.unwrap(),
        );
        assert_eq!(a.num_documents, b.num_documents, "{name}");
        assert_eq!(
            sorted_documents(from, name, dir).await,
            sorted_documents(to, name, dir).await,
            "{name}"
        );
        assert_eq!(a.fields.len(), b.fields.len(), "{name}");
        for (kind, names) in [
            (SetKind::Synonym, &a.synonym_sets),
            (SetKind::Curation, &a.curation_sets),
        ] {
            let (ours, theirs) = (from.sets(kind).await.unwrap(), to.sets(kind).await.unwrap());
            for set in names {
                let (old, new) = (set_named(&ours, set), set_named(&theirs, set));
                assert!(changes(kind, &old.items, &new.items).is_empty(), "{set}");
            }
        }
    }
    assert_eq!(
        sorted_stopwords(from.stopwords().await.unwrap()),
        sorted_stopwords(to.stopwords().await.unwrap())
    );
    assert_eq!(
        sorted_aliases(from.aliases().await.unwrap()),
        sorted_aliases(to.aliases().await.unwrap())
    );
    empty(to).await;
}

#[tokio::test]
#[ignore = "needs two Typesense servers, the first seeded"]
async fn a_backup_round_trips_into_a_second_server() {
    let (from, to) = (source(), target());
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("exports");
    std::fs::create_dir(&scratch).unwrap();

    let folder = dir.path().join("folder");
    round_trip(
        &from,
        &to,
        BackupTarget::Folder(folder.clone()),
        &folder,
        &scratch,
    )
    .await;

    let unpacked = dir.path().join("unpacked");
    std::fs::create_dir(&unpacked).unwrap();
    round_trip(
        &from,
        &to,
        BackupTarget::Archive(dir.path().join("b.tar.zst")),
        &unpacked,
        &scratch,
    )
    .await;
}

/// The server answers 400 "Referenced collection `authors` not found." to an import while the
/// alias is missing, so the alias cannot wait for the end of a restore.
#[tokio::test]
#[ignore = "needs two Typesense servers"]
async fn a_reference_through_an_alias_is_restored() {
    let (from, to) = (source(), target());
    let names = ["live_alias_authors", "live_alias_books"];
    let alias = "live_alias_people";
    for name in names {
        from.delete_collection(name).await.ok();
    }
    from.delete_alias(alias).await.ok();
    from.create_collection(&json!({
        "name": names[0],
        "fields": [{"name": "name", "type": "string"}],
    }))
    .await
    .unwrap();
    from.upsert_alias(alias, names[0]).await.unwrap();
    from.create_collection(&json!({
        "name": names[1],
        "fields": [
            {"name": "title", "type": "string"},
            {"name": "author_id", "type": "string", "reference": format!("{alias}.id")},
        ],
    }))
    .await
    .unwrap();
    let import = |name: &'static str, body: &'static str| {
        let client = from.clone();
        async move {
            let params = ImportParams {
                action: ImportAction::Create,
                batch_size: 10,
                dirty_values: DirtyValues::CoerceOrReject,
            };
            client.import_batch(name, &params, body).await.unwrap()
        }
    };
    import(names[0], "{\"id\":\"1\",\"name\":\"a\"}\n").await;
    import(
        names[1],
        "{\"id\":\"1\",\"title\":\"t\",\"author_id\":\"1\"}\n",
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("b");
    let plan_b = BackupPlan {
        // The dependent first, so only the order of the plan can save it.
        collections: vec![names[1].to_string(), names[0].to_string()],
        target: BackupTarget::Folder(folder.clone()),
    };
    let (tx, _rx) = watch::channel(Default::default());
    let outcome = run_backup(from.clone(), plan_b, JobControl::default(), tx).await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);

    let control = JobControl::default();
    let manifest = read_manifest(&folder).unwrap();
    let backup = verify(&folder, &manifest, &control).unwrap();
    let loaded = load(&backup).unwrap();
    let view = RestoreView::read(&to).await.unwrap();
    let rows = plan(&manifest, &loaded, &view).rows;
    let job = RestoreJob {
        backup,
        loaded,
        ticked: rows.into_iter().map(|r| (r.kind, r.name)).collect(),
        replace: false,
        batch_size: 1000,
    };
    let (tx, _rx) = watch::channel(Default::default());
    let restored = run_restore(to.clone(), job, control, tx).await;
    assert!(
        matches!(restored.end, JobEnd::Finished),
        "{:?} {:?}",
        restored.end,
        restored.rows
    );
    assert_eq!(to.collection(names[1]).await.unwrap().num_documents, 1);
    assert_eq!(to.aliases().await.unwrap()[0].collection_name, names[0]);

    for name in names {
        from.delete_collection(name).await.ok();
    }
    from.delete_alias(alias).await.ok();
    empty(&to).await;
}
