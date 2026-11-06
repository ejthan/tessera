// SPDX-License-Identifier: GPL-3.0-or-later

//! Writes a backup: schemas without provider keys, all documents, the attached sets, stopwords
//! and aliases. Reads only, so it needs no unlock.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};
use tempfile::TempDir;
use tessera_client::{Alias, Client, Collection, ItemSet, SetKind, StopwordSet};
use tokio::sync::watch;
use url::Url;

use super::archive::{ArchiveError, create_private, create_private_dir, digest, pack};
use super::format::{
    ALIASES, COLLECTIONS, CURATION_SETS, FORMAT, MANIFEST, Manifest, ManifestCollection,
    ManifestFile, STOPWORDS, SYNONYM_SETS,
};
use crate::redact;
use crate::schema::SchemaDraft;
use crate::stemming::dictionaries_used;
use crate::transfer::export::{
    ExportFormat, ExportJob, ExportProgress, ExportScope, FieldPick, run_export,
};
use crate::transfer::{JobControl, JobEnd, TransferError};

/// The temp directory of an archive being written. A crash leaves it for the sweep.
pub const TEMP_PREFIX: &str = "tessera-backup-";

/// Keys of `model_config` that hold no credential, by the OpenAPI file. Anything else there is
/// blanked, because a provider's next key will not be on any list.
const PLAIN_CONFIG: [&str; 4] = [
    "model_name",
    "project_id",
    "indexing_prefix",
    "query_prefix",
];

/// Where a backup goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupTarget {
    /// A folder, which appears when complete.
    Folder(PathBuf),
    /// A `.tar.zst` file, which appears when complete.
    Archive(PathBuf),
}

/// What to back up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupPlan {
    /// The collections, by name.
    pub collections: Vec<String>,
    /// Where to.
    pub target: BackupTarget,
}

/// What a backup is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackupStep {
    /// Reading the server's lists.
    #[default]
    Reading,
    /// Writing a schema and exporting its documents.
    Collections,
    /// Writing sets, stopwords and aliases.
    Resources,
    /// Computing checksums and writing the manifest.
    Hashing,
    /// Packing the archive.
    Packing,
}

/// How far a backup is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackupProgress {
    /// The step.
    pub step: BackupStep,
    /// Collections finished.
    pub done: u64,
    /// Collections selected.
    pub total: u64,
    /// The running export.
    pub export: ExportProgress,
    /// The collection being exported.
    pub collection: String,
}

/// One collection that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRow {
    /// Its name.
    pub name: String,
    /// Documents written.
    pub documents: u64,
}

/// What the user should know about a backup. The app words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupWarning {
    /// Fields with `store: false`, which an export cannot hold.
    Unstored {
        /// The collection.
        collection: String,
        /// The fields.
        fields: Vec<String>,
    },
    /// Provider credentials that the backup does not hold.
    BlankedSecrets {
        /// The collection.
        collection: String,
        /// How many.
        count: usize,
    },
    /// Stemming dictionaries in use, which a backup does not hold.
    Dictionaries {
        /// The collection.
        collection: String,
        /// The dictionaries.
        names: Vec<String>,
    },
    /// A reference to a collection that is not selected.
    OutsideReference {
        /// The collection.
        collection: String,
        /// What it references.
        target: String,
    },
    /// The export wrote another count than the server said: a live collection may differ.
    CountMoved {
        /// The collection.
        collection: String,
        /// What the server said.
        expected: u64,
        /// What was written.
        written: u64,
    },
    /// An attached set the server does not have.
    MissingSet {
        /// The collection.
        collection: String,
        /// Its kind.
        kind: SetKind,
        /// Its name.
        name: String,
    },
}

/// Why a backup failed.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// The server or the connection.
    #[error(transparent)]
    Client(#[from] tessera_client::Error),
    /// A selected collection is not on the server.
    #[error("the collection {0:?} does not exist")]
    UnknownCollection(String),
    /// The target or its part folder exists.
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    /// An export.
    #[error("{collection}: {source}")]
    Export {
        /// The collection.
        collection: String,
        /// What happened.
        source: TransferError,
    },
    /// Packing or hashing.
    #[error(transparent)]
    Archive(ArchiveError),
    /// The file system.
    #[error("{0}")]
    Io(String),
}

/// What a backup did.
#[derive(Debug)]
pub struct BackupOutcome {
    /// How it ended.
    pub end: JobEnd<BackupError>,
    /// The collections written.
    pub collections: Vec<BackupRow>,
    /// What the user should know.
    pub warnings: Vec<BackupWarning>,
}

enum Halt {
    Stopped,
    Failed(BackupError),
}

impl<E: Into<BackupError>> From<E> for Halt {
    fn from(e: E) -> Self {
        Self::Failed(e.into())
    }
}

impl From<ArchiveError> for BackupError {
    fn from(e: ArchiveError) -> Self {
        Self::Archive(e)
    }
}

impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

fn archive_halt(e: ArchiveError) -> Halt {
    match e {
        ArchiveError::Stopped => Halt::Stopped,
        e => Halt::Failed(e.into()),
    }
}

fn stop_check(control: &JobControl) -> Result<(), Halt> {
    if control.stop.is_cancelled() || control.abort.is_cancelled() {
        return Err(Halt::Stopped);
    }
    Ok(())
}

/// Runs blocking file work off the async workers.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, BackupError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| BackupError::Io(e.to_string()))
}

fn is_secret_key(key: &str) -> bool {
    // `redact` masks `value` for API keys it shows; in a schema that is any metadata.
    key != "value" && redact::is_secret_param(key)
}

/// Whether a provider URL holds nothing that authenticates.
fn plain_url(text: &str) -> bool {
    Url::parse(text).is_ok_and(|url| {
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none_or(|q| redact::query(q) == q)
    })
}

fn blank(value: &mut Value) -> usize {
    if value.as_str() == Some("") {
        return 0;
    }
    *value = String::new().into();
    1
}

fn scrub_config(config: &mut Map<String, Value>) -> usize {
    let plain = |key: &str, value: &Value| match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => !is_secret_key(key),
        Value::String(text) if key == "url" => plain_url(text),
        _ => PLAIN_CONFIG.contains(&key),
    };
    config
        .iter_mut()
        .map(
            |(key, value)| {
                if plain(key, value) { 0 } else { blank(value) }
            },
        )
        .sum()
}

/// Blanks every credential left in a schema's JSON, `extra` of the draft included: all of
/// `model_config` but [`PLAIN_CONFIG`], and a key named like a secret at any depth.
fn scrub(value: &mut Value) -> usize {
    match value {
        Value::Object(map) => map
            .iter_mut()
            .map(|(key, child)| match child {
                Value::Object(config) if key == "model_config" => scrub_config(config),
                Value::Null => 0,
                child if is_secret_key(key) => blank(child),
                child => scrub(child),
            })
            .sum(),
        Value::Array(items) => items.iter_mut().map(scrub).sum(),
        _ => 0,
    }
}

/// A schema as a backup holds it, and how many credentials were left out.
fn backup_schema(collection: &Collection) -> (Value, usize) {
    let mut draft = SchemaDraft::from_collection(collection);
    let blanked = draft.blank_secrets();
    let mut schema = draft.to_json();
    let more = scrub(&mut schema);
    (schema, blanked + more)
}

fn unstored_fields(collection: &Collection) -> Vec<String> {
    let fields = collection.fields.iter().filter(|f| !f.flag("store"));
    fields.map(|f| f.name.clone()).collect()
}

/// What a backup of `selected` will warn about, from the lists alone.
pub fn backup_warnings(
    collections: &[Collection],
    selected: &[String],
    aliases: &[Alias],
) -> Vec<BackupWarning> {
    let mut warnings = Vec::new();
    for collection in collections.iter().filter(|c| selected.contains(&c.name)) {
        let name = || collection.name.clone();
        let fields = unstored_fields(collection);
        if !fields.is_empty() {
            warnings.push(BackupWarning::Unstored {
                collection: name(),
                fields,
            });
        }
        let (_, count) = backup_schema(collection);
        if count > 0 {
            warnings.push(BackupWarning::BlankedSecrets {
                collection: name(),
                count,
            });
        }
        let names = dictionaries_used(collection);
        if !names.is_empty() {
            warnings.push(BackupWarning::Dictionaries {
                collection: name(),
                names,
            });
        }
        let references = collection
            .fields
            .iter()
            .filter_map(|f| f.attr("reference")?.as_str());
        let mut seen = HashSet::new();
        for reference in references {
            let target = reference.split('.').next().unwrap_or(reference);
            let through_alias = aliases.iter().find(|a| a.name == target);
            let resolved = through_alias.map_or(target, |a| a.collection_name.as_str());
            if !selected.iter().any(|s| s == resolved) && seen.insert(target) {
                warnings.push(BackupWarning::OutsideReference {
                    collection: name(),
                    target: target.to_string(),
                });
            }
        }
    }
    warnings
}

/// What the server holds, read before anything is written.
struct Lists {
    collections: Vec<Collection>,
    aliases: Vec<Alias>,
    synonym_sets: Vec<ItemSet>,
    curation_sets: Vec<ItemSet>,
    stopwords: Vec<StopwordSet>,
    version: String,
}

async fn read_lists(client: &Client) -> Result<Lists, tessera_client::Error> {
    Ok(Lists {
        collections: client.collections().await?,
        aliases: client.aliases().await?,
        synonym_sets: client.sets(SetKind::Synonym).await?,
        curation_sets: client.sets(SetKind::Curation).await?,
        stopwords: client.stopwords().await?,
        version: client.debug().await?.version,
    })
}

/// The folder files are written to until the backup is complete.
struct Workspace {
    dir: PathBuf,
    /// Holds an archive's temp directory.
    temp: Option<TempDir>,
}

fn part_path(target: &Path) -> PathBuf {
    let mut part = target.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

fn prepare(target: &BackupTarget) -> Result<Workspace, BackupError> {
    let path = match target {
        BackupTarget::Folder(path) | BackupTarget::Archive(path) => path,
    };
    // `symlink_metadata`, so a link that leads nowhere counts too.
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(BackupError::Exists(path.clone()));
    }
    let workspace = match target {
        BackupTarget::Folder(path) => {
            let dir = part_path(path);
            create_private_dir(&dir).map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => BackupError::Exists(dir.clone()),
                _ => e.into(),
            })?;
            Workspace { dir, temp: None }
        }
        BackupTarget::Archive(_) => {
            let temp = tempfile::Builder::new().prefix(TEMP_PREFIX).tempdir()?;
            Workspace {
                dir: temp.path().to_path_buf(),
                temp: Some(temp),
            }
        }
    };
    create_private_dir(&workspace.dir.join(COLLECTIONS))?;
    Ok(workspace)
}

/// Writes a new private file.
async fn write_file(path: PathBuf, bytes: Vec<u8>) -> Result<(), BackupError> {
    blocking(move || {
        let mut file = create_private(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })
    .await??;
    Ok(())
}

async fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> Result<(), BackupError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| BackupError::Io(e.to_string()))?;
    write_file(dir.join(name), bytes).await
}

/// The sets of `kind` that `selected` attach, and a warning for each one the server lacks.
fn attached_sets(
    kind: SetKind,
    sets: &[ItemSet],
    selected: &[&Collection],
    warnings: &mut Vec<BackupWarning>,
) -> Vec<ItemSet> {
    let mut names: Vec<&str> = Vec::new();
    for collection in selected {
        let attached = match kind {
            SetKind::Synonym => &collection.synonym_sets,
            SetKind::Curation => &collection.curation_sets,
        };
        for name in attached {
            if !sets.iter().any(|s| &s.name == name) {
                warnings.push(BackupWarning::MissingSet {
                    collection: collection.name.clone(),
                    kind,
                    name: name.clone(),
                });
            } else if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
    }
    let wanted = |s: &&ItemSet| names.contains(&s.name.as_str());
    sets.iter().filter(wanted).cloned().collect()
}

/// One backup run: its inputs, and what it reports.
struct Run<'a> {
    client: &'a Client,
    control: &'a JobControl,
    progress: &'a watch::Sender<BackupProgress>,
    rows: Vec<BackupRow>,
    warnings: Vec<BackupWarning>,
}

impl Run<'_> {
    async fn export(
        &mut self,
        collection: &Collection,
        target: PathBuf,
    ) -> Result<ExportProgress, Halt> {
        // Takes the permissions of what stands there, so the documents are private throughout.
        write_file(target.clone(), Vec::new()).await?;
        let job = ExportJob {
            collection: collection.name.clone(),
            scope: ExportScope::All,
            fields: FieldPick::All,
            format: ExportFormat::Jsonl,
            target,
            expected: Some(collection.num_documents),
        };
        let (tx, mut rx) = watch::channel(ExportProgress::default());
        let forward = tokio::spawn({
            let progress = self.progress.clone();
            async move {
                while rx.changed().await.is_ok() {
                    let export = rx.borrow_and_update().clone();
                    progress.send_modify(|p| p.export = export);
                }
            }
        });
        let outcome = run_export(self.client.clone(), job, self.control.clone(), tx).await;
        forward.abort();
        match outcome.end {
            JobEnd::Finished => Ok(outcome.progress),
            JobEnd::Stopped => Err(Halt::Stopped),
            JobEnd::Failed(source) => Err(Halt::Failed(BackupError::Export {
                collection: collection.name.clone(),
                source,
            })),
        }
    }

    async fn collection(
        &mut self,
        collection: &Collection,
        file: u32,
        dir: &Path,
    ) -> Result<ManifestCollection, Halt> {
        let (schema, blanked_secrets) = backup_schema(collection);
        let mut entry = ManifestCollection {
            file,
            name: collection.name.clone(),
            documents: 0,
            unstored: unstored_fields(collection),
            blanked_secrets,
            dictionaries: dictionaries_used(collection),
        };
        write_json(dir, &entry.schema_path(), &schema).await?;
        let written = self
            .export(collection, dir.join(entry.documents_path()))
            .await?;
        entry.documents = written.docs;
        if written.docs != collection.num_documents {
            self.warnings.push(BackupWarning::CountMoved {
                collection: collection.name.clone(),
                expected: collection.num_documents,
                written: written.docs,
            });
        }
        self.rows.push(BackupRow {
            name: collection.name.clone(),
            documents: written.docs,
        });
        Ok(entry)
    }

    /// Writes the whole backup into `dir`, the manifest last.
    async fn fill(
        &mut self,
        lists: &Lists,
        selected: &[&Collection],
        dir: &Path,
    ) -> Result<(), Halt> {
        let names: Vec<String> = selected.iter().map(|c| c.name.clone()).collect();
        let found = backup_warnings(&lists.collections, &names, &lists.aliases);
        self.warnings.extend(found);

        let mut entries = Vec::with_capacity(selected.len());
        for (i, collection) in selected.iter().enumerate() {
            stop_check(self.control)?;
            self.progress.send_modify(|p| {
                p.step = BackupStep::Collections;
                p.done = i as u64;
                p.collection = collection.name.clone();
                p.export = ExportProgress::default();
            });
            entries.push(self.collection(collection, i as u32 + 1, dir).await?);
            self.progress.send_modify(|p| p.done = i as u64 + 1);
        }

        stop_check(self.control)?;
        self.progress
            .send_modify(|p| p.step = BackupStep::Resources);
        let synonyms = attached_sets(
            SetKind::Synonym,
            &lists.synonym_sets,
            selected,
            &mut self.warnings,
        );
        let curations = attached_sets(
            SetKind::Curation,
            &lists.curation_sets,
            selected,
            &mut self.warnings,
        );
        let aliases: Vec<&Alias> = lists
            .aliases
            .iter()
            .filter(|a| names.contains(&a.collection_name))
            .collect();
        write_json(dir, ALIASES, &aliases).await?;
        write_json(dir, STOPWORDS, &lists.stopwords).await?;
        write_json(dir, SYNONYM_SETS, &synonyms).await?;
        write_json(dir, CURATION_SETS, &curations).await?;

        self.progress.send_modify(|p| p.step = BackupStep::Hashing);
        let mut paths: Vec<String> = [ALIASES, STOPWORDS, SYNONYM_SETS, CURATION_SETS]
            .map(str::to_string)
            .to_vec();
        for entry in &entries {
            paths.push(entry.schema_path());
            paths.push(entry.documents_path());
        }
        let control = self.control.clone();
        let root = dir.to_path_buf();
        let files = blocking(move || {
            paths
                .into_iter()
                .map(|path| {
                    let (bytes, sha256) = digest(&root.join(&path), &control)?;
                    Ok(ManifestFile {
                        path,
                        bytes,
                        sha256,
                    })
                })
                .collect::<Result<Vec<_>, ArchiveError>>()
        })
        .await?
        .map_err(archive_halt)?;

        let manifest = Manifest {
            format: FORMAT,
            created_at: jiff::Timestamp::now().as_second(),
            tessera_version: env!("CARGO_PKG_VERSION").to_string(),
            server_version: lists.version.clone(),
            collections: entries,
            files,
        };
        // Last in a folder: a folder without a manifest is incomplete.
        write_json(dir, MANIFEST, &manifest).await?;
        Ok(())
    }

    async fn run(&mut self, plan: &BackupPlan) -> Result<(), Halt> {
        self.progress
            .send_modify(|p| p.total = plan.collections.len() as u64);
        let lists = read_lists(self.client).await?;
        stop_check(self.control)?;
        let mut selected = Vec::with_capacity(plan.collections.len());
        for name in &plan.collections {
            match lists.collections.iter().find(|c| &c.name == name) {
                Some(collection) => selected.push(collection),
                None => return Err(BackupError::UnknownCollection(name.clone()).into()),
            }
        }

        let target = plan.target.clone();
        let mut workspace = blocking(move || prepare(&target)).await??;
        let dir = workspace.dir.clone();
        let mut done = self.fill(&lists, &selected, &dir).await;
        if done.is_ok() {
            done = self.complete(&plan.target, &dir).await;
        }
        // Dropping a `TempDir` deletes it, which is blocking too.
        let temp = workspace.temp.take();
        let leftover = (done.is_err() && temp.is_none()).then_some(dir);
        blocking(move || {
            drop(temp);
            if let Some(dir) = leftover {
                // Nothing to tell: the run already ended with what matters.
                let _ = std::fs::remove_dir_all(dir);
            }
        })
        .await?;
        done
    }

    async fn complete(&self, target: &BackupTarget, dir: &Path) -> Result<(), Halt> {
        match target {
            BackupTarget::Folder(path) => {
                tokio::fs::rename(dir, path)
                    .await
                    .map_err(BackupError::from)?;
                Ok(())
            }
            BackupTarget::Archive(path) => {
                self.progress.send_modify(|p| p.step = BackupStep::Packing);
                let (dir, path) = (dir.to_path_buf(), path.clone());
                let control = self.control.clone();
                blocking(move || pack(&dir, &path, &control))
                    .await?
                    .map_err(archive_halt)
            }
        }
    }
}

/// One backup from start to end. Nothing appears at the target unless all of it succeeded, and
/// a failed read ends the run: a backup silently without its aliases is worse than none.
pub async fn run_backup(
    client: Client,
    plan: BackupPlan,
    control: JobControl,
    progress: watch::Sender<BackupProgress>,
) -> BackupOutcome {
    let mut run = Run {
        client: &client,
        control: &control,
        progress: &progress,
        rows: Vec::new(),
        warnings: Vec::new(),
    };
    let end = match run.run(&plan).await {
        Ok(()) => JobEnd::Finished,
        Err(Halt::Stopped) => JobEnd::Stopped,
        Err(Halt::Failed(e)) => JobEnd::Failed(e),
    };
    BackupOutcome {
        end,
        collections: run.rows,
        warnings: run.warnings,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    #[test]
    fn scrub_keeps_what_holds_no_credential() {
        let mut schema = json!({
            "metadata": {"value": "kept", "owner": "me"},
            "fields": [{"name": "api_key", "type": "string", "embed": {
                "from": ["title"],
                "model_config": {
                    "model_name": "openai/text-embedding-3-small",
                    "url": "https://proxy.example/v1?api-version=2",
                    "project_id": "p",
                    "indexing_prefix": "a",
                    "query_prefix": "b",
                }
            }}]
        });
        let before = schema.clone();
        assert_eq!(scrub(&mut schema), 0);
        assert_eq!(schema, before);
    }

    #[test]
    fn scrub_blanks_what_could_be_one() {
        let mut schema = json!({
            "metadata": {"owner_api_key": "a", "nested": [{"client_secret": "b"}]},
            "fields": [{"name": "e", "type": "float[]", "embed": {"model_config": {
                "model_name": "m",
                "client_id": "c",
                "some_future_credential": "d",
                "headers": {"authorization": "e"},
                "url": "https://user:f@proxy.example/v1",
            }}}]
        });
        assert_eq!(scrub(&mut schema), 6);
        let text = schema.to_string();
        for secret in ["\"a\"", "\"b\"", "\"c\"", "\"d\"", "f@", "authorization"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }

    #[test]
    fn a_blank_is_not_counted_twice() {
        let mut schema = json!({"metadata": {"api_key": ""}});
        assert_eq!(scrub(&mut schema), 0);
    }

    #[test]
    fn a_url_with_a_key_in_its_query_is_no_plain_url() {
        assert!(plain_url("https://a.example/v1"));
        assert!(plain_url("http://localhost:11434/api?x=1"));
        assert!(!plain_url("https://a.example/v1?api_key=k"));
        assert!(!plain_url("https://k@a.example/v1"));
        assert!(!plain_url("not a url"));
    }
}
