// SPDX-License-Identifier: GPL-3.0-or-later

//! Aliases, stopwords and presets: the backend seam the resource views run through.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui_kit::Global;
use serde_json::Value;
use tempfile::TempDir;
use tessera_client::{Alias, Client, Error, ItemSet, SetKind, StopwordSet};
use tessera_core::relevance::attachment::{
    NoKnownSets, attached, attachment_patch, with_set, without_set,
};
use tessera_core::relevance::synonym_file::{SET_FILE_LIMIT, SetFileError};
use tessera_core::transfer::report::write_text;
use tokio::runtime::Handle;

use crate::collections::{BoxFuture, TokioBackend};

/// What an attach or detach left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachOutcome {
    /// The collection's list as read back.
    pub list: Vec<String>,
    /// Names left out because the server no longer has them.
    pub dropped: Vec<String>,
    /// The list read back is not the list sent.
    pub mismatch: bool,
}

/// Why an attach or detach sent nothing, or failed.
#[derive(Debug)]
pub enum AttachError {
    /// The server refused or did not answer.
    Server(Error),
    /// See [`NoKnownSets`].
    NoKnownSets(NoKnownSets),
}

impl From<Error> for AttachError {
    fn from(error: Error) -> Self {
        Self::Server(error)
    }
}

impl From<NoKnownSets> for AttachError {
    fn from(error: NoKnownSets) -> Self {
        Self::NoKnownSets(error)
    }
}

/// Everything the resource views ask of the server. Every write here is one the caller runs
/// detached, so closing a tab cannot abort it.
pub trait RelevanceBackend: Send + Sync {
    /// The sets of `kind`, with their items.
    fn sets(&self, client: Client, kind: SetKind) -> BoxFuture<Result<Vec<ItemSet>, Error>>;

    /// Set `name` as the server holds it now.
    fn set(&self, client: Client, kind: SetKind, name: String)
    -> BoxFuture<Result<ItemSet, Error>>;

    /// Replaces set `name`, or creates it, then reads it back: the answer echoes the input.
    fn save_set(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
        items: Vec<Value>,
    ) -> BoxFuture<Result<ItemSet, Error>>;

    /// Deletes set `name`.
    fn delete_set(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
    ) -> BoxFuture<Result<(), Error>>;

    /// Stores one item, then reads it back, because the answer echoes the input.
    fn save_item(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
        id: String,
        item: Value,
    ) -> BoxFuture<Result<Value, Error>>;

    /// Deletes one item.
    fn delete_item(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
        id: String,
    ) -> BoxFuture<Result<(), Error>>;

    /// Adds `set` to the list of `collection`, or takes it out. Reads the collection and the set
    /// names right before the `PATCH`, and the collection again after it.
    fn attach(
        &self,
        client: Client,
        kind: SetKind,
        collection: String,
        set: String,
        attach: bool,
    ) -> BoxFuture<Result<AttachOutcome, AttachError>>;

    /// The text of a set file; the size is checked before reading.
    fn read_set_file(&self, path: PathBuf) -> BoxFuture<Result<String, SetFileError>>;

    /// Writes a set file, whole or not at all.
    fn write_set_file(&self, path: PathBuf, text: String) -> BoxFuture<Result<(), String>>;

    /// The stopword sets.
    fn stopwords(&self, client: Client) -> BoxFuture<Result<Vec<StopwordSet>, Error>>;

    /// Stores set `id`, then reads it back, because the answer to the `PUT` echoes the input.
    fn save_stopwords(
        &self,
        client: Client,
        id: String,
        words: Vec<String>,
        locale: Option<String>,
    ) -> BoxFuture<Result<StopwordSet, Error>>;

    /// Deletes stopword set `id`.
    fn delete_stopwords(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>>;

    /// Points alias `name` at `collection`, creating it if needed.
    fn save_alias(
        &self,
        client: Client,
        name: String,
        collection: String,
    ) -> BoxFuture<Result<Alias, Error>>;

    /// Alias `name`.
    fn alias(&self, client: Client, name: String) -> BoxFuture<Result<Alias, Error>>;

    /// Deletes alias `name`.
    fn delete_alias(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;

    /// Deletes preset `name`.
    fn delete_preset(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;

    /// A private temp directory for the copy of a reindex.
    fn copy_dir(&self) -> BoxFuture<Result<Arc<TempDir>, String>>;
}

/// The backend the views use.
pub struct RelevanceBackendGlobal(pub Arc<dyn RelevanceBackend>);

impl Global for RelevanceBackendGlobal {}

impl RelevanceBackend for TokioBackend {
    fn sets(&self, client: Client, kind: SetKind) -> BoxFuture<Result<Vec<ItemSet>, Error>> {
        self.run(async move { client.sets(kind).await })
    }

    fn set(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
    ) -> BoxFuture<Result<ItemSet, Error>> {
        self.run(async move { client.set(kind, &name).await })
    }

    fn save_set(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
        items: Vec<Value>,
    ) -> BoxFuture<Result<ItemSet, Error>> {
        self.run(async move {
            client.upsert_set(kind, &name, &items).await?;
            client.set(kind, &name).await
        })
    }

    fn delete_set(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
    ) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_set(kind, &name).await })
    }

    fn attach(
        &self,
        client: Client,
        kind: SetKind,
        collection: String,
        set: String,
        attach: bool,
    ) -> BoxFuture<Result<AttachOutcome, AttachError>> {
        self.run(async move {
            let current = client.collection(&collection).await?;
            let sets = client.sets(kind).await?;
            let known: Vec<String> = sets.into_iter().map(|s| s.name).collect();
            let list = attached(&current, kind);
            let new = if attach {
                with_set(list, &known, &set)
            } else {
                without_set(list, &known, &set)
            }?;
            let patch = attachment_patch(kind, &new.list);
            client.update_collection(&collection, &patch).await?;
            let read = client.collection(&collection).await?;
            let list = attached(&read, kind).to_vec();
            Ok(AttachOutcome {
                mismatch: list != new.list,
                list,
                dropped: new.dropped,
            })
        })
    }

    fn read_set_file(&self, path: PathBuf) -> BoxFuture<Result<String, SetFileError>> {
        self.run(async move {
            let read = |e: std::io::Error| SetFileError::Read(e.to_string());
            let size = tokio::fs::metadata(&path).await.map_err(read)?.len();
            if size > SET_FILE_LIMIT as u64 {
                return Err(SetFileError::TooLarge(size));
            }
            tokio::fs::read_to_string(&path).await.map_err(read)
        })
    }

    fn write_set_file(&self, path: PathBuf, text: String) -> BoxFuture<Result<(), String>> {
        self.run(async move { write_text(&path, &text).await.map_err(|e| e.to_string()) })
    }

    fn save_item(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
        id: String,
        item: Value,
    ) -> BoxFuture<Result<Value, Error>> {
        self.run(async move {
            client.upsert_set_item(kind, &name, &id, &item).await?;
            client.set_item(kind, &name, &id).await
        })
    }

    fn delete_item(
        &self,
        client: Client,
        kind: SetKind,
        name: String,
        id: String,
    ) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_set_item(kind, &name, &id).await })
    }

    fn stopwords(&self, client: Client) -> BoxFuture<Result<Vec<StopwordSet>, Error>> {
        self.run(async move { client.stopwords().await })
    }

    fn save_stopwords(
        &self,
        client: Client,
        id: String,
        words: Vec<String>,
        locale: Option<String>,
    ) -> BoxFuture<Result<StopwordSet, Error>> {
        self.run(async move {
            client
                .upsert_stopwords(&id, &words, locale.as_deref())
                .await?;
            client.stopword_set(&id).await
        })
    }

    fn delete_stopwords(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_stopwords(&id).await })
    }

    fn save_alias(
        &self,
        client: Client,
        name: String,
        collection: String,
    ) -> BoxFuture<Result<Alias, Error>> {
        self.run(async move { client.upsert_alias(&name, &collection).await })
    }

    fn alias(&self, client: Client, name: String) -> BoxFuture<Result<Alias, Error>> {
        self.run(async move { client.alias(&name).await })
    }

    fn delete_alias(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_alias(&name).await })
    }

    fn delete_preset(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_preset(&name).await })
    }

    fn copy_dir(&self) -> BoxFuture<Result<Arc<TempDir>, String>> {
        self.run(async {
            let made = tokio::task::spawn_blocking(|| {
                let mut builder = tempfile::Builder::new();
                builder.prefix(COPY_PREFIX);
                // A temp directory takes the default mode, and this one holds every document.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    builder.permissions(std::fs::Permissions::from_mode(0o700));
                }
                builder.tempdir()
            });
            match made.await {
                Ok(Ok(dir)) => Ok(Arc::new(dir)),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn RelevanceBackend> {
    Arc::new(TokioBackend(handle))
}

/// What a reindex copy's temp directory is named after.
const COPY_PREFIX: &str = "tessera-reindex-";

/// How old a copy must be to go.
///
/// shortcut: the directory's own time, which an export running in a second instance for over a
/// day does not touch; that copy would fail and stop. Read the file's time if that ever matters.
const STALE_COPY: Duration = Duration::from_secs(24 * 60 * 60);

/// Removes the copies a crash left in the temp directory; each holds every document of its
/// collection. Blocks, so it does not run on the GPUI thread.
pub fn sweep_copies() {
    let root = std::env::temp_dir();
    let swept = mine(&root).and_then(|mine| remove_stale_copies(&root, SystemTime::now(), mine));
    if let Err(e) = swept {
        tracing::warn!("the sweep of old reindex copies failed: {e}");
    }
}

/// Whether this user owns an entry of `root`. std cannot name the user, but what this process
/// makes is owned by it.
#[cfg(unix)]
fn mine(root: &Path) -> std::io::Result<impl Fn(&Metadata) -> bool + use<>> {
    use std::os::unix::fs::MetadataExt as _;
    let uid = tempfile::tempfile_in(root)?.metadata()?.uid();
    Ok(move |entry: &Metadata| entry.uid() == uid)
}

/// The temp directory itself is per user here.
#[cfg(not(unix))]
fn mine(_: &Path) -> std::io::Result<impl Fn(&Metadata) -> bool + use<>> {
    Ok(|_: &Metadata| true)
}

fn remove_stale_copies(
    root: &Path,
    now: SystemTime,
    mine: impl Fn(&Metadata) -> bool,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(COPY_PREFIX) {
            continue;
        }
        // Not followed: a link named like a copy is not a directory here.
        let Ok(found) = entry.metadata() else {
            continue;
        };
        let age = found.modified().ok();
        let age = age.and_then(|at| now.duration_since(at).ok());
        if found.is_dir() && mine(&found) && age.is_some_and(|age| age > STALE_COPY) {
            let path = entry.path();
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!("{} could not be removed: {e}", path.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn answer(body: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(body)
    }

    async fn attach(
        server: &MockServer,
        set: &str,
        attach: bool,
    ) -> Result<AttachOutcome, AttachError> {
        let client =
            Client::new(&[&server.uri()], "k".to_string().into(), Default::default()).unwrap();
        tokio_backend(Handle::current())
            .attach(client, SetKind::Synonym, "c".into(), set.into(), attach)
            .await
    }

    #[tokio::test]
    async fn toggle_attaches_with_fresh_lists() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/c"))
            .respond_with(answer(json!({"name": "c", "synonym_sets": ["a", "gone"]})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/collections/c"))
            .respond_with(answer(json!({"name": "c", "synonym_sets": ["a", "b"]})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/synonym_sets"))
            .respond_with(answer(json!([{"name": "a"}, {"name": "b"}])))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/collections/c"))
            .respond_with(answer(json!({})))
            .mount(&server)
            .await;
        let outcome = attach(&server, "b", true).await.unwrap();
        assert_eq!(outcome.dropped, ["gone"]);
        assert!(!outcome.mismatch);
        let requests = server.received_requests().await.unwrap();
        let calls: Vec<String> = requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.url.path()))
            .collect();
        assert_eq!(
            calls,
            [
                "GET /collections/c",
                "GET /synonym_sets",
                "PATCH /collections/c",
                "GET /collections/c"
            ]
        );
        let body: Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(body, json!({"synonym_sets": ["a", "b"]}));
    }

    #[tokio::test]
    async fn a_failed_list_patches_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/c"))
            .respond_with(answer(json!({"name": "c", "synonym_sets": ["a"]})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/synonym_sets"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        assert!(attach(&server, "b", true).await.is_err());
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|r| r.method.as_str() == "GET"));
    }

    #[tokio::test]
    async fn no_known_sets_patches_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/c"))
            .respond_with(answer(json!({"name": "c", "synonym_sets": ["a", "b"]})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/synonym_sets"))
            .respond_with(answer(json!([])))
            .mount(&server)
            .await;
        assert!(matches!(
            attach(&server, "a", false).await,
            Err(AttachError::NoKnownSets(_))
        ));
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|r| r.method.as_str() == "GET"));
    }

    #[tokio::test]
    async fn copy_dir_is_private_and_goes_with_its_last_handle() {
        let dir = tokio_backend(Handle::current()).copy_dir().await.unwrap();
        let path = dir.path().to_path_buf();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("tessera-reindex-"), "{name}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn only_old_copies_of_this_user_are_swept() {
        let root = tempfile::tempdir().unwrap();
        let make = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("documents.jsonl"), "{}").unwrap();
            path
        };
        let (copy, other) = (make("tessera-reindex-old"), make("something-else"));
        let file = root.path().join("tessera-reindex-file");
        std::fs::write(&file, "kept").unwrap();
        #[cfg(unix)]
        let link = {
            let link = root.path().join("tessera-reindex-link");
            std::os::unix::fs::symlink(&other, &link).unwrap();
            link
        };

        let later = SystemTime::now() + 2 * STALE_COPY;
        let mine = || mine(root.path()).unwrap();
        remove_stale_copies(root.path(), SystemTime::now(), mine()).unwrap();
        assert!(copy.exists(), "younger than a day");
        remove_stale_copies(root.path(), later, |_| false).unwrap();
        assert!(copy.exists(), "someone else's");

        remove_stale_copies(root.path(), later, mine()).unwrap();
        assert!(!copy.exists());
        assert!(other.join("documents.jsonl").exists());
        assert!(file.exists());
        #[cfg(unix)]
        assert!(link.symlink_metadata().is_ok());
    }

    #[tokio::test]
    async fn a_set_file_over_the_limit_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(SET_FILE_LIMIT as u64 + 1).unwrap();
        let backend = tokio_backend(Handle::current());
        let answer = backend.read_set_file(path.clone()).await;
        assert_eq!(
            answer,
            Err(SetFileError::TooLarge(SET_FILE_LIMIT as u64 + 1))
        );
        file.set_len(2).unwrap();
        std::fs::write(&path, "[]").unwrap();
        assert_eq!(backend.read_set_file(path).await.unwrap(), "[]");
    }
}
