// SPDX-License-Identifier: GPL-3.0-or-later

//! Compares a verified backup with a server, and restores it. A restore creates what is
//! missing. An existing collection is never written to, and a set, stopword set or alias that
//! differs changes only with Replace on. It never deletes anything.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use tessera_client::{
    Alias, Client, DirtyValues, ImportAction, ImportParams, ItemSet, SetKind, StopwordSet,
};
use tokio::sync::watch;

use super::archive::Verified;
use super::format::{
    ALIASES, CURATION_SETS, MANIFEST_LIMIT, Manifest, ManifestCollection, STOPWORDS, SYNONYM_SETS,
};
use crate::redact;
use crate::relevance::changes::changes;
use crate::relevance::reindex::{Preflight, draft_curation_sets, without_missing};
use crate::schema::SchemaDraft;
use crate::transfer::import::{ImportJob, ImportOutcome, ImportProgress, run_import};
use crate::transfer::source::{Format, ReadOptions, Source};
use crate::transfer::{JobControl, JobEnd};

/// Why the files of a backup could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// Over [`MANIFEST_LIMIT`].
    #[error("{0} is too large")]
    TooLarge(String),
    /// Not what the file should hold.
    #[error("{path}: {message}")]
    Bad {
        /// The file.
        path: String,
        /// What the parser says.
        message: String,
    },
    /// The file system.
    #[error("{0}")]
    Io(String),
}

/// The JSON files of a backup. `schemas` runs parallel to the manifest's collections.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loaded {
    /// One per manifest collection, named as the manifest names it.
    pub schemas: Vec<SchemaDraft>,
    /// Aliases.
    pub aliases: Vec<Alias>,
    /// Stopword sets.
    pub stopwords: Vec<StopwordSet>,
    /// Synonym sets.
    pub synonym_sets: Vec<ItemSet>,
    /// Curation sets.
    pub curation_sets: Vec<ItemSet>,
}

fn read_text(folder: &Path, manifest: &Manifest, path: &str) -> Result<Option<Vec<u8>>, LoadError> {
    let Some(file) = manifest.files.iter().find(|f| f.path == path) else {
        return Ok(None);
    };
    if file.bytes > MANIFEST_LIMIT {
        return Err(LoadError::TooLarge(path.to_string()));
    }
    fs::read(folder.join(path))
        .map(Some)
        .map_err(|e| LoadError::Io(e.to_string()))
}

fn bad(path: &str, message: impl ToString) -> LoadError {
    LoadError::Bad {
        path: path.to_string(),
        message: message.to_string(),
    }
}

/// A list file. One the manifest does not list is empty.
fn read_list<T: DeserializeOwned>(
    folder: &Path,
    manifest: &Manifest,
    path: &str,
) -> Result<Vec<T>, LoadError> {
    match read_text(folder, manifest, path)? {
        Some(text) => serde_json::from_slice(&text).map_err(|e| bad(path, e)),
        None => Ok(Vec::new()),
    }
}

/// Reads what `plan` needs from a folder that passed `verify`. Blocking.
pub fn load(backup: &Verified) -> Result<Loaded, LoadError> {
    let (folder, manifest) = (backup.folder(), backup.manifest());
    let mut schemas = Vec::with_capacity(manifest.collections.len());
    for entry in &manifest.collections {
        let path = entry.schema_path();
        let text = read_text(folder, manifest, &path)?;
        let text = String::from_utf8(text.unwrap_or_default()).map_err(|e| bad(&path, e))?;
        let mut draft = SchemaDraft::from_json(&text).map_err(|e| bad(&path, e))?;
        // The plan shows the manifest's name, so that is the one that may be created.
        draft.name = entry.name.clone();
        schemas.push(draft);
    }
    Ok(Loaded {
        schemas,
        aliases: read_list(folder, manifest, ALIASES)?,
        stopwords: read_list(folder, manifest, STOPWORDS)?,
        synonym_sets: read_list(folder, manifest, SYNONYM_SETS)?,
        curation_sets: read_list(folder, manifest, CURATION_SETS)?,
    })
}

/// The server a backup is compared with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RestoreView {
    /// Collection names.
    pub collections: Vec<String>,
    /// Aliases.
    pub aliases: Vec<Alias>,
    /// Synonym sets.
    pub synonym_sets: Vec<ItemSet>,
    /// Curation sets.
    pub curation_sets: Vec<ItemSet>,
    /// Stopword sets.
    pub stopwords: Vec<StopwordSet>,
    /// Stemming dictionary ids.
    pub dictionaries: Vec<String>,
}

impl RestoreView {
    /// Reads the server's lists.
    pub async fn read(client: &Client) -> Result<Self, tessera_client::Error> {
        let summaries = client.collections_summary().await?;
        Ok(Self {
            collections: summaries.into_iter().map(|c| c.name).collect(),
            aliases: client.aliases().await?,
            synonym_sets: client.sets(SetKind::Synonym).await?,
            curation_sets: client.sets(SetKind::Curation).await?,
            stopwords: client.stopwords().await?,
            dictionaries: client.stemming_dictionaries().await?,
        })
    }
}

/// What kind of resource a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKind {
    /// A stopword set.
    Stopwords,
    /// A synonym set.
    SynonymSet,
    /// A curation set.
    CurationSet,
    /// A collection with its documents.
    Collection,
    /// An alias.
    Alias,
}

/// Why a collection cannot be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreBlock {
    /// The name is taken by a collection, or by an alias that a new collection would hide.
    CollectionExists,
    /// This many provider credentials are not in the backup, so it would not embed.
    BlankSecrets(usize),
    /// Required fields with `store: false`: every document would fail.
    RequiredUnstored(Vec<String>),
    /// The schema holds the mask, which must not be sent.
    Mask,
    /// The manifest lists this name twice, and this row is the second.
    DuplicateName,
}

/// How a row compares with the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// The server does not have it.
    New,
    /// The server has it, the same.
    Same,
    /// The server has something else under that name.
    Differs,
    /// It cannot be restored.
    Blocked(RestoreBlock),
}

/// One resource of a backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    /// Its kind.
    pub kind: RowKind,
    /// Its name.
    pub name: String,
    /// How it compares.
    pub state: RowState,
}

/// What the user should know about a restore. The app words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreWarning {
    /// Fields whose values the backup could not hold.
    Unstored {
        /// The collection.
        collection: String,
        /// The fields.
        fields: Vec<String>,
    },
    /// A stemming dictionary the server does not have.
    MissingDictionary {
        /// The collection.
        collection: String,
        /// The dictionary.
        name: String,
    },
    /// A reference to a collection neither the backup nor the server has.
    MissingReference {
        /// The collection.
        collection: String,
        /// What it references.
        target: String,
    },
    /// Collections that reference each other: restored in manifest order, and lines that find
    /// no target show as failed.
    ReferenceCycle(Vec<String>),
    /// A set the schema attaches and neither the backup nor the server has: left out.
    MissingSet {
        /// The collection.
        collection: String,
        /// Its kind.
        kind: SetKind,
        /// Its name.
        name: String,
    },
    /// An alias left out, because its target would not exist.
    AliasNoTarget {
        /// The alias.
        alias: String,
        /// Its target.
        target: String,
    },
    /// An alias left out, because a collection of that name would hide it.
    AliasShadowed(String),
}

/// What a restore would do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestorePlan {
    /// One per resource: stopwords, sets, collections, aliases.
    pub rows: Vec<PlanRow>,
    /// What the user should know.
    pub warnings: Vec<RestoreWarning>,
    /// The collections that can be restored, referenced ones first.
    pub order: Vec<String>,
}

fn same_stopwords(a: &StopwordSet, b: &StopwordSet) -> bool {
    let sorted = |set: &StopwordSet| {
        let mut words = set.stopwords.clone();
        words.sort();
        words
    };
    let locale = |set: &StopwordSet| set.locale.clone().unwrap_or_default();
    locale(a) == locale(b) && sorted(a) == sorted(b)
}

fn set_state(kind: SetKind, set: &ItemSet, on_server: &[ItemSet]) -> RowState {
    match on_server.iter().find(|s| s.name == set.name) {
        None => RowState::New,
        Some(old) if changes(kind, &old.items, &set.items).is_empty() => RowState::Same,
        Some(_) => RowState::Differs,
    }
}

/// What on the server or in the backup stops a collection, if anything.
fn block(
    entry: &ManifestCollection,
    draft: &SchemaDraft,
    taken: &HashSet<&str>,
) -> Option<RestoreBlock> {
    if taken.contains(entry.name.as_str()) {
        return Some(RestoreBlock::CollectionExists);
    }
    if redact::text_has_mask(&draft.to_json().to_string()) {
        return Some(RestoreBlock::Mask);
    }
    // The manifest counts what `empty_secrets` does not know: unknown keys of `model_config`.
    let blank = draft.empty_secrets().max(entry.blanked_secrets);
    if blank > 0 {
        return Some(RestoreBlock::BlankSecrets(blank));
    }
    let required: Vec<String> = entry
        .unstored
        .iter()
        .filter(|name| draft.fields.iter().any(|f| &f.name == *name && !f.optional))
        .cloned()
        .collect();
    (!required.is_empty()).then_some(RestoreBlock::RequiredUnstored(required))
}

fn reference_target(reference: &str) -> &str {
    reference.split('.').next().unwrap_or(reference)
}

/// `unplaced` so that what waits for a cycle comes after it: the reverse of a depth-first
/// post-order along `dependents`, without recursion because a manifest can chain 100,000 names.
fn after_their_targets(unplaced: &[usize], dependents: &[Vec<usize>]) -> Vec<usize> {
    let mut seen = vec![false; dependents.len()];
    let mut order = Vec::with_capacity(unplaced.len());
    for &root in unplaced {
        if std::mem::replace(&mut seen[root], true) {
            continue;
        }
        let mut stack = vec![(root, 0)];
        while let Some((node, next)) = stack.last_mut() {
            match dependents[*node].get(*next) {
                Some(&dependent) => {
                    *next += 1;
                    if !std::mem::replace(&mut seen[dependent], true) {
                        stack.push((dependent, 0));
                    }
                }
                None => {
                    order.push(*node);
                    stack.pop();
                }
            }
        }
    }
    order.reverse();
    order
}

/// `unplaced` without what only waits for a cycle and is no part of one: nothing there waits
/// for it. Something between two cycles stays, which only adds a name to a warning.
fn in_cycles(
    unplaced: &[usize],
    dependents: &[Vec<usize>],
    targets: &[Vec<usize>],
) -> HashSet<usize> {
    let mut waited_for: Vec<usize> = dependents.iter().map(Vec::len).collect();
    let mut free: Vec<usize> = unplaced
        .iter()
        .copied()
        .filter(|&i| waited_for[i] == 0)
        .collect();
    let mut cycle: HashSet<usize> = unplaced.iter().copied().collect();
    while let Some(i) = free.pop() {
        cycle.remove(&i);
        for &target in &targets[i] {
            waited_for[target] -= 1;
            if waited_for[target] == 0 && cycle.contains(&target) {
                free.push(target);
            }
        }
    }
    cycle
}

/// The names of `drafts`, referenced ones first (Kahn), by name and through the backup's
/// aliases, and the members of a cycle, which come after what they need and before what needs
/// them.
fn reference_order(drafts: &[&SchemaDraft], aliases: &[Alias]) -> (Vec<String>, Vec<String>) {
    let at: HashMap<&str, usize> = drafts
        .iter()
        .enumerate()
        .map(|(i, d)| (d.name.as_str(), i))
        .collect();
    let resolve = |target: &str| {
        at.get(target).copied().or_else(|| {
            let alias = aliases.iter().find(|a| a.name == target)?;
            at.get(alias.collection_name.as_str()).copied()
        })
    };
    let mut waits_for = vec![0; drafts.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); drafts.len()];
    let mut all_targets: Vec<Vec<usize>> = vec![Vec::new(); drafts.len()];
    for (i, draft) in drafts.iter().enumerate() {
        let targets: HashSet<usize> = draft
            .fields
            .iter()
            .filter_map(|f| resolve(reference_target(&f.reference)))
            .filter(|&target| target != i)
            .collect();
        waits_for[i] = targets.len();
        for &target in &targets {
            dependents[target].push(i);
        }
        all_targets[i] = targets.into_iter().collect();
    }
    let mut queue: VecDeque<usize> = (0..drafts.len()).filter(|&i| waits_for[i] == 0).collect();
    let mut placed = vec![false; drafts.len()];
    let mut order = Vec::with_capacity(drafts.len());
    while let Some(i) = queue.pop_front() {
        placed[i] = true;
        order.push(drafts[i].name.clone());
        for &dependent in &dependents[i] {
            waits_for[dependent] -= 1;
            if waits_for[dependent] == 0 {
                queue.push_back(dependent);
            }
        }
    }
    let unplaced: Vec<usize> = (0..drafts.len()).filter(|&i| !placed[i]).collect();
    let cycle = in_cycles(&unplaced, &dependents, &all_targets);
    let rest = after_their_targets(&unplaced, &dependents);
    order.extend(rest.into_iter().map(|i| drafts[i].name.clone()));
    let members = unplaced.into_iter().filter(|i| cycle.contains(i));
    (order, members.map(|i| drafts[i].name.clone()).collect())
}

/// The sets `draft` attaches that are not `known`.
fn missing_sets(
    draft: &SchemaDraft,
    synonyms: &HashSet<&str>,
    curations: &HashSet<&str>,
) -> Vec<(SetKind, String)> {
    let synonym = draft
        .synonym_sets
        .iter()
        .map(String::as_str)
        .filter(|n| !synonyms.contains(n))
        .map(|n| (SetKind::Synonym, n.to_string()));
    let curation = draft_curation_sets(draft)
        .filter(|n| !curations.contains(n))
        .map(|n| (SetKind::Curation, n.to_string()));
    synonym.chain(curation).collect()
}

fn names_of(sets: &[&[ItemSet]]) -> HashSet<String> {
    sets.iter()
        .flat_map(|list| list.iter().map(|s| s.name.clone()))
        .collect()
}

fn alias_left_out(alias: &Alias, collections: &HashSet<&str>) -> Option<RestoreWarning> {
    if collections.contains(alias.name.as_str()) {
        return Some(RestoreWarning::AliasShadowed(alias.name.clone()));
    }
    (!collections.contains(alias.collection_name.as_str())).then(|| RestoreWarning::AliasNoTarget {
        alias: alias.name.clone(),
        target: alias.collection_name.clone(),
    })
}

fn collection_warnings(
    entry: &ManifestCollection,
    draft: &SchemaDraft,
    known: (&HashSet<&str>, &HashSet<&str>),
    backup: &HashSet<&str>,
    loaded: &Loaded,
    server: &RestoreView,
    warnings: &mut Vec<RestoreWarning>,
) {
    let collection = || entry.name.clone();
    if !entry.unstored.is_empty() {
        warnings.push(RestoreWarning::Unstored {
            collection: collection(),
            fields: entry.unstored.clone(),
        });
    }
    for name in entry
        .dictionaries
        .iter()
        .filter(|d| !server.dictionaries.contains(d))
    {
        warnings.push(RestoreWarning::MissingDictionary {
            collection: collection(),
            name: name.clone(),
        });
    }
    let reachable = |target: &str| {
        backup.contains(target)
            || server.collections.iter().any(|c| c == target)
            || server.aliases.iter().any(|a| a.name == target)
            || loaded.aliases.iter().any(|a| a.name == target)
    };
    let mut named = HashSet::new();
    for field in draft.fields.iter().filter(|f| !f.reference.is_empty()) {
        let target = reference_target(&field.reference);
        if !reachable(target) && named.insert(target) {
            warnings.push(RestoreWarning::MissingReference {
                collection: collection(),
                target: target.to_string(),
            });
        }
    }
    for (kind, name) in missing_sets(draft, known.0, known.1) {
        warnings.push(RestoreWarning::MissingSet {
            collection: collection(),
            kind,
            name,
        });
    }
}

/// Compares a backup with `server`. Pure: an order of items or a server-filled key is no
/// difference.
pub fn plan(manifest: &Manifest, loaded: &Loaded, server: &RestoreView) -> RestorePlan {
    let row = |kind, name: &str, state| PlanRow {
        kind,
        name: name.to_string(),
        state,
    };
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for set in &loaded.stopwords {
        let state = match server.stopwords.iter().find(|s| s.id == set.id) {
            None => RowState::New,
            Some(old) if same_stopwords(old, set) => RowState::Same,
            Some(_) => RowState::Differs,
        };
        rows.push(row(RowKind::Stopwords, &set.id, state));
    }
    for set in &loaded.synonym_sets {
        let state = set_state(SetKind::Synonym, set, &server.synonym_sets);
        rows.push(row(RowKind::SynonymSet, &set.name, state));
    }
    for set in &loaded.curation_sets {
        let state = set_state(SetKind::Curation, set, &server.curation_sets);
        rows.push(row(RowKind::CurationSet, &set.name, state));
    }

    let synonyms = names_of(&[&loaded.synonym_sets, &server.synonym_sets]);
    let curations = names_of(&[&loaded.curation_sets, &server.curation_sets]);
    let synonyms: HashSet<&str> = synonyms.iter().map(String::as_str).collect();
    let curations: HashSet<&str> = curations.iter().map(String::as_str).collect();
    let backup: HashSet<&str> = manifest
        .collections
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    // An alias counts: a collection of its name would take its searches away from its target.
    let mut taken: HashSet<&str> = server.collections.iter().map(String::as_str).collect();
    taken.extend(server.aliases.iter().map(|a| a.name.as_str()));

    let mut open: Vec<&SchemaDraft> = Vec::new();
    let mut listed = HashSet::new();
    for (entry, draft) in manifest.collections.iter().zip(&loaded.schemas) {
        // A hostile manifest can list a name twice, and the second would meet the first.
        let twice = !listed.insert(entry.name.as_str());
        let found = if twice {
            Some(RestoreBlock::DuplicateName)
        } else {
            block(entry, draft, &taken)
        };
        let state = match found {
            Some(block) => RowState::Blocked(block),
            None => {
                open.push(draft);
                collection_warnings(
                    entry,
                    draft,
                    (&synonyms, &curations),
                    &backup,
                    loaded,
                    server,
                    &mut warnings,
                );
                RowState::New
            }
        };
        rows.push(row(RowKind::Collection, &entry.name, state));
    }
    let (order, cycle) = reference_order(&open, &loaded.aliases);
    if !cycle.is_empty() {
        warnings.push(RestoreWarning::ReferenceCycle(cycle));
    }

    let mut standing: HashSet<&str> = server.collections.iter().map(String::as_str).collect();
    standing.extend(order.iter().map(String::as_str));
    for alias in &loaded.aliases {
        if let Some(warning) = alias_left_out(alias, &standing) {
            warnings.push(warning);
            continue;
        }
        let state = match server.aliases.iter().find(|a| a.name == alias.name) {
            None => RowState::New,
            Some(old) if old.collection_name == alias.collection_name => RowState::Same,
            Some(_) => RowState::Differs,
        };
        rows.push(row(RowKind::Alias, &alias.name, state));
    }
    RestorePlan {
        rows,
        warnings,
        order,
    }
}

/// What to restore.
#[derive(Debug, Clone)]
pub struct RestoreJob {
    /// The verified backup.
    pub backup: Verified,
    /// Its JSON files.
    pub loaded: Loaded,
    /// The rows the user left ticked.
    pub ticked: HashSet<(RowKind, String)>,
    /// Whether ticked `Differs` rows are written.
    pub replace: bool,
    /// Documents per import batch.
    pub batch_size: u32,
}

/// What a restore is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreStep {
    /// Reading the server again.
    #[default]
    Checking,
    /// Writing stopwords and sets.
    Resources,
    /// Creating a collection and importing its documents.
    Collections,
    /// Writing aliases.
    Aliases,
}

/// How far a restore is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RestoreProgress {
    /// The step.
    pub step: RestoreStep,
    /// Rows finished.
    pub done: u64,
    /// Rows in all.
    pub total: u64,
    /// The running import.
    pub import: ImportProgress,
    /// The collection being imported.
    pub collection: String,
}

/// Why a row failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowFailure {
    /// The server's or the transfer's text.
    Text(String),
    /// The collection holds another count than the backup.
    Count {
        /// What the manifest says.
        expected: u64,
        /// What the server says, if it answered.
        found: Option<u64>,
    },
    /// Documents that were not stored.
    Lines {
        /// Refused.
        failed: u64,
        /// Sent, and nobody knows.
        unconfirmed: u64,
        /// The server's reason for the first refused line.
        first_error: Option<String>,
    },
    /// Stopped on request, partway.
    Stopped,
}

/// What became of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowResult {
    /// Written.
    Done,
    /// It differs and stays, because Replace was off.
    Kept,
    /// Nothing to do, or not reached.
    Skipped,
    /// Not written, or not completely.
    Failed(RowFailure),
}

/// One resource after a restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreRow {
    /// Its kind.
    pub kind: RowKind,
    /// Its name.
    pub name: String,
    /// What became of it.
    pub result: RowResult,
}

/// Why a restore ended early.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    /// The server could not be read, and nothing was written.
    #[error(transparent)]
    Read(#[from] tessera_client::Error),
    /// These ticked collections appeared since the plan, and nothing was written.
    #[error("these collections exist now: {}", .0.join(", "))]
    NowExists(Vec<String>),
    /// A row failed, see its result. What was done before it stays.
    #[error("{kind:?} {name} failed")]
    Row {
        /// Its kind.
        kind: RowKind,
        /// Its name.
        name: String,
    },
}

/// What a restore did.
#[derive(Debug)]
pub struct RestoreOutcome {
    /// How it ended.
    pub end: JobEnd<RestoreError>,
    /// One per resource.
    pub rows: Vec<RestoreRow>,
    /// The collections this run created: what to delete before a retry.
    pub to_delete: Vec<String>,
    /// What the user should know.
    pub warnings: Vec<RestoreWarning>,
}

enum Halt {
    Stopped,
    Failed(RestoreError),
}

impl From<tessera_client::Error> for Halt {
    fn from(e: tessera_client::Error) -> Self {
        Self::Failed(e.into())
    }
}

fn is_resource(kind: RowKind) -> bool {
    matches!(
        kind,
        RowKind::Stopwords | RowKind::SynonymSet | RowKind::CurationSet
    )
}

/// Why an import that ended cannot stand, compared with the manifest.
fn import_failure(outcome: &ImportOutcome, documents: u64, stopped: bool) -> Option<RowFailure> {
    let progress = &outcome.progress;
    match &outcome.end {
        JobEnd::Stopped => Some(RowFailure::Stopped),
        JobEnd::Failed(e) => Some(RowFailure::Text(e.to_string())),
        JobEnd::Finished if progress.failed > 0 || progress.unconfirmed > 0 => {
            Some(RowFailure::Lines {
                failed: progress.failed,
                unconfirmed: progress.unconfirmed,
                first_error: progress.first_error.as_deref().map(str::to_string),
            })
        }
        // A stop after the last batch ends the count, not the import: nothing says it differs.
        JobEnd::Finished if stopped && outcome.count.is_none() => Some(RowFailure::Stopped),
        JobEnd::Finished => {
            let found = outcome.count.as_ref().map(|c| c.after);
            (found != Some(documents)).then_some(RowFailure::Count {
                expected: documents,
                found,
            })
        }
    }
}

struct Run<'a> {
    client: &'a Client,
    job: &'a RestoreJob,
    control: &'a JobControl,
    progress: &'a watch::Sender<RestoreProgress>,
    rows: Vec<RestoreRow>,
    to_delete: Vec<String>,
    warnings: Vec<RestoreWarning>,
}

impl Run<'_> {
    fn stop_check(&self) -> Result<(), Halt> {
        if self.control.stop.is_cancelled() || self.control.abort.is_cancelled() {
            return Err(Halt::Stopped);
        }
        Ok(())
    }

    fn ticked(&self, kind: RowKind, name: &str) -> bool {
        self.job.ticked.contains(&(kind, name.to_string()))
    }

    /// Whether the run means to write `row`.
    fn writes(&self, row: &PlanRow) -> bool {
        let ticked = self.ticked(row.kind, &row.name);
        match row.state {
            RowState::New => ticked,
            RowState::Differs => ticked && self.job.replace,
            RowState::Same | RowState::Blocked(_) => false,
        }
    }

    /// Whether to write row `at`, marking it when a difference stays.
    fn take(&mut self, at: usize, row: &PlanRow) -> bool {
        let writes = self.writes(row);
        if !writes && row.state == RowState::Differs {
            self.rows[at].result = RowResult::Kept;
        }
        writes
    }

    fn mark(&mut self, at: usize, result: RowResult) {
        self.rows[at].result = result;
        self.progress.send_modify(|p| p.done += 1);
    }

    /// Marks a failed row and turns it into the end of the run.
    fn fail(&mut self, at: usize, failure: RowFailure) -> Halt {
        let end = match failure {
            RowFailure::Stopped => Halt::Stopped,
            _ => Halt::Failed(RestoreError::Row {
                kind: self.rows[at].kind,
                name: self.rows[at].name.clone(),
            }),
        };
        self.mark(at, RowResult::Failed(failure));
        end
    }

    fn warn(&mut self, warning: RestoreWarning) {
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    /// The collections that stand now.
    fn standing_collections(&self, view: &RestoreView) -> HashSet<String> {
        let created = self.to_delete.iter();
        view.collections.iter().chain(created).cloned().collect()
    }

    async fn write_alias(&mut self, at: usize, alias: &Alias) -> Result<(), Halt> {
        self.stop_check()?;
        match self
            .client
            .upsert_alias(&alias.name, &alias.collection_name)
            .await
        {
            Ok(_) => {
                self.mark(at, RowResult::Done);
                Ok(())
            }
            Err(e) => Err(self.fail(at, RowFailure::Text(e.to_string()))),
        }
    }

    /// Writes the aliases `draft` references whose target stands: a reference through an alias
    /// finds nothing until the alias exists, so the import cannot wait for [`Self::aliases`].
    async fn aliases_of(
        &mut self,
        draft: &SchemaDraft,
        fresh: &RestorePlan,
        view: &RestoreView,
    ) -> Result<(), Halt> {
        let job = self.job;
        let linked = draft.fields.iter().filter(|f| !f.reference.is_empty());
        let targets: HashSet<&str> = linked.map(|f| reference_target(&f.reference)).collect();
        for (at, row) in fresh.rows.iter().enumerate() {
            if row.kind != RowKind::Alias
                || !targets.contains(row.name.as_str())
                || self.rows[at].result == RowResult::Done
                || !self.take(at, row)
            {
                continue;
            }
            let standing = self.standing_collections(view);
            let standing: HashSet<&str> = standing.iter().map(String::as_str).collect();
            let found = job.loaded.aliases.iter().find(|a| a.name == row.name);
            if let Some(alias) = found.filter(|a| alias_left_out(a, &standing).is_none()) {
                self.write_alias(at, alias).await?;
            }
        }
        Ok(())
    }

    async fn write_resource(&self, row: &PlanRow) -> Result<(), tessera_client::Error> {
        let loaded = &self.job.loaded;
        let set = |sets: &'_ [ItemSet]| {
            let found = sets.iter().find(|s| s.name == row.name);
            found.map_or_else(Vec::new, |s| s.items.clone())
        };
        match row.kind {
            RowKind::Stopwords => match loaded.stopwords.iter().find(|s| s.id == row.name) {
                Some(s) => {
                    let locale = s.locale.as_deref();
                    self.client
                        .upsert_stopwords(&s.id, &s.stopwords, locale)
                        .await
                }
                None => Ok(()),
            },
            RowKind::SynonymSet => {
                let items = set(&loaded.synonym_sets);
                let upsert = self.client.upsert_set(SetKind::Synonym, &row.name, &items);
                upsert.await.map(drop)
            }
            _ => {
                let items = set(&loaded.curation_sets);
                let upsert = self.client.upsert_set(SetKind::Curation, &row.name, &items);
                upsert.await.map(drop)
            }
        }
    }

    /// The sets that stand on the server now, of one kind.
    fn standing(&self, kind: RowKind, server: &[ItemSet]) -> HashSet<String> {
        let done = self
            .rows
            .iter()
            .filter(|r| r.kind == kind && r.result == RowResult::Done)
            .map(|r| r.name.clone());
        server.iter().map(|s| s.name.clone()).chain(done).collect()
    }

    /// `draft` without the sets that are not there, which `POST` would answer 404 to.
    fn without_missing_sets(&mut self, draft: &mut SchemaDraft, view: &RestoreView) {
        let synonyms = self.standing(RowKind::SynonymSet, &view.synonym_sets);
        let curations = self.standing(RowKind::CurationSet, &view.curation_sets);
        let missing = missing_sets(
            draft,
            &synonyms.iter().map(String::as_str).collect(),
            &curations.iter().map(String::as_str).collect(),
        );
        for (kind, name) in &missing {
            self.warn(RestoreWarning::MissingSet {
                collection: draft.name.clone(),
                kind: *kind,
                name: name.clone(),
            });
        }
        let preflight = Preflight {
            missing_sets: missing.into_iter().map(|(_, name)| name).collect(),
            ..Preflight::default()
        };
        without_missing(draft, &preflight);
    }

    async fn import(&self, entry: &ManifestCollection) -> ImportOutcome {
        let job = ImportJob {
            source: Source::File(self.job.backup.folder().join(entry.documents_path())),
            options: ReadOptions::new(Format::Jsonl),
            collection: entry.name.clone(),
            params: ImportParams {
                action: ImportAction::Create,
                batch_size: self.job.batch_size,
                dirty_values: DirtyValues::CoerceOrReject,
            },
        };
        let (tx, mut rx) = watch::channel(ImportProgress::default());
        let forward = tokio::spawn({
            let progress = self.progress.clone();
            async move {
                while rx.changed().await.is_ok() {
                    let import = rx.borrow_and_update().clone();
                    progress.send_modify(|p| p.import = import);
                }
            }
        });
        let outcome = run_import(self.client.clone(), job, self.control.clone(), tx).await;
        forward.abort();
        outcome
    }

    /// Creates the collection and fills it. Only a collection this run created is imported
    /// into: a name that appeared meanwhile answers 409 here and stops the run.
    async fn collection(
        &mut self,
        name: &str,
        fresh: &RestorePlan,
        view: &RestoreView,
    ) -> Result<(), Halt> {
        let job = self.job;
        let Some(at) = self
            .rows
            .iter()
            .position(|r| r.kind == RowKind::Collection && r.name == name)
        else {
            return Ok(());
        };
        let found = (job.backup.manifest().collections.iter()).zip(&job.loaded.schemas);
        let Some((entry, draft)) = found.into_iter().find(|(c, _)| c.name == name) else {
            return Ok(());
        };
        let mut draft = draft.clone();
        self.without_missing_sets(&mut draft, view);
        self.progress.send_modify(|p| {
            p.collection = name.to_string();
            p.import = ImportProgress::default();
        });
        if let Err(e) = self.client.create_collection(&draft.to_json()).await {
            return Err(self.fail(at, RowFailure::Text(e.to_string())));
        }
        self.to_delete.push(name.to_string());
        self.aliases_of(&draft, fresh, view).await?;
        let outcome = self.import(entry).await;
        let stopped = self.stop_check().is_err();
        if let Some(failure) = import_failure(&outcome, entry.documents, stopped) {
            return Err(self.fail(at, failure));
        }
        self.mark(at, RowResult::Done);
        Ok(())
    }

    /// Last, so a client moves only when the data is there. Those a reference needs are out
    /// already, see [`Self::aliases_of`].
    async fn aliases(&mut self, fresh: &RestorePlan, view: &RestoreView) -> Result<(), Halt> {
        let job = self.job;
        let standing = self.standing_collections(view);
        let standing: HashSet<&str> = standing.iter().map(String::as_str).collect();
        for (at, row) in fresh.rows.iter().enumerate() {
            if row.kind != RowKind::Alias
                || self.rows[at].result == RowResult::Done
                || !self.take(at, row)
            {
                continue;
            }
            let Some(alias) = job.loaded.aliases.iter().find(|a| a.name == row.name) else {
                continue;
            };
            if let Some(warning) = alias_left_out(alias, &standing) {
                self.warn(warning);
                self.progress.send_modify(|p| p.total -= 1);
                continue;
            }
            self.write_alias(at, alias).await?;
        }
        Ok(())
    }

    async fn run(&mut self) -> Result<(), Halt> {
        // Right before the first write: the plan on screen may be old. `POST /collections`
        // would answer 409 too, but by then the sets would have changed.
        let view = RestoreView::read(self.client).await?;
        let job = self.job;
        let fresh = plan(job.backup.manifest(), &job.loaded, &view);
        self.rows = fresh
            .rows
            .iter()
            .map(|r| RestoreRow {
                kind: r.kind,
                name: r.name.clone(),
                result: RowResult::Skipped,
            })
            .collect();
        self.warnings = fresh.warnings.clone();
        let total = fresh.rows.iter().filter(|r| self.writes(r)).count() as u64;
        self.progress.send_modify(|p| p.total = total);
        let taken: Vec<String> = fresh
            .rows
            .iter()
            .filter(|r| {
                r.kind == RowKind::Collection
                    && r.state == RowState::Blocked(RestoreBlock::CollectionExists)
                    && self.ticked(r.kind, &r.name)
            })
            .map(|r| r.name.clone())
            .collect();
        if !taken.is_empty() {
            return Err(Halt::Failed(RestoreError::NowExists(taken)));
        }

        self.progress
            .send_modify(|p| p.step = RestoreStep::Resources);
        for (at, row) in fresh.rows.iter().enumerate() {
            if !is_resource(row.kind) || !self.take(at, row) {
                continue;
            }
            self.stop_check()?;
            match self.write_resource(row).await {
                Ok(()) => self.mark(at, RowResult::Done),
                Err(e) => return Err(self.fail(at, RowFailure::Text(e.to_string()))),
            }
        }

        self.progress
            .send_modify(|p| p.step = RestoreStep::Collections);
        for name in &fresh.order {
            if self.ticked(RowKind::Collection, name) {
                self.stop_check()?;
                self.collection(name, &fresh, &view).await?;
            }
        }

        self.progress.send_modify(|p| p.step = RestoreStep::Aliases);
        self.aliases(&fresh, &view).await
    }
}

/// One restore from start to end. A failure ends the run: what was done stays and is listed,
/// and `to_delete` names what stands in the way of a retry.
pub async fn run_restore(
    client: Client,
    job: RestoreJob,
    control: JobControl,
    progress: watch::Sender<RestoreProgress>,
) -> RestoreOutcome {
    let mut run = Run {
        client: &client,
        job: &job,
        control: &control,
        progress: &progress,
        rows: Vec::new(),
        to_delete: Vec::new(),
        warnings: Vec::new(),
    };
    let end = match run.run().await {
        Ok(()) => JobEnd::Finished,
        Err(Halt::Stopped) => JobEnd::Stopped,
        Err(Halt::Failed(e)) => JobEnd::Failed(e),
    };
    RestoreOutcome {
        end,
        rows: run.rows,
        to_delete: run.to_delete,
        warnings: run.warnings,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::transfer::report::Report;

    #[tokio::test]
    async fn a_stop_after_the_last_batch_is_a_stop() {
        let outcome = ImportOutcome {
            end: JobEnd::Finished,
            progress: ImportProgress::default(),
            report: Arc::new(Report::new().await.unwrap()),
            stopped_at_line: None,
            count: None,
        };
        assert_eq!(import_failure(&outcome, 2, true), Some(RowFailure::Stopped));
        assert_eq!(
            import_failure(&outcome, 2, false),
            Some(RowFailure::Count {
                expected: 2,
                found: None,
            })
        );
    }
}
