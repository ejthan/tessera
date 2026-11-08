// SPDX-License-Identifier: GPL-3.0-or-later

//! The reindex assistant of an alias: a new collection, a copy through a temp file, a compare,
//! the switch and the clean-up. The alias moves only in the switch, after a fresh compare, so
//! whatever ends the assistant before that leaves the old collection serving.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Hsla, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window, div, prelude::*,
};
use rust_i18n::t;
use serde_json::Value;
use tempfile::TempDir;
use tessera_client::{
    Alias, Client, Collection, DirtyValues, Error, ImportAction, ImportParams, SetKind,
};
use tessera_core::relevance::name_ok;
use tessera_core::relevance::reindex::{
    Compared, Preflight, preflight, reindex_name, without_missing,
};
use tessera_core::schema::SchemaDraft;
use tessera_core::transfer::export::{ExportFormat, ExportJob, ExportScope, FieldPick};
use tessera_core::transfer::import::ImportJob;
use tessera_core::transfer::source::{Format, ReadError, ReadOptions, Source, sniff};

use super::super::jobs::import_dialog::{Inspect, inspect};
use super::super::overview::grouped;
use super::super::root::RootView;
use super::super::schema_dialog::SchemaDialog;
use super::super::{collection_danger, env_badge};
use super::{Danger, ErrorSlot, client_of, confirm_write, notice, report_to};
use crate::actions::{
    CancelJob, DeleteResource, EditResource, NextStep, PickImportFile, REINDEX, ReloadResource,
    RemoveBodyFile, SaveReport, StepBack, StopWaiting,
};
use crate::collections::{BoxFuture, CollectionsBackendGlobal};
use crate::connection::{ConnectionState, error_text, long_error_text};
use crate::jobs::{JobFinished, JobId, JobOutcome, JobStatus, Jobs, JobsGlobal, Live};
use crate::relevance::RelevanceBackendGlobal;
use crate::runtime::Tokio;
use crate::state::AppState;
use crate::transfer::{TransferBackendGlobal, read_error_text};

/// Where the copy comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopySource {
    /// The collection behind the alias.
    Collection,
    /// A file picked by the user.
    File(PathBuf),
}

/// Where the assistant stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Choosing name, schema and source. Nothing was sent.
    Schema,
    /// The copy's export runs, or ended short.
    Exporting(JobId),
    /// The copy's import runs, or ended.
    Importing(JobId),
    /// The copy ended; these are the counts.
    Compare(Compared),
    /// The alias points to the new collection.
    Switched(Compared),
    /// Finished.
    Done,
}

/// The fresh lists of the server, which every preflight reads.
struct Lists {
    collections: Vec<Collection>,
    aliases: Vec<Alias>,
    synonym_sets: Vec<String>,
    curation_sets: Vec<String>,
}

/// What step one worked out.
struct Plan {
    source: Collection,
    draft: SchemaDraft,
    preflight: Preflight,
    lists: Lists,
}

/// What a finished import did.
#[derive(Clone, Copy)]
struct Imported {
    job: JobId,
    imported: u64,
    failed: u64,
    unconfirmed: u64,
}

/// The server's state right before a step that depends on it.
struct Fresh {
    /// `None` once someone else deleted it.
    alias: Option<Alias>,
    source: u64,
    target: u64,
}

impl Fresh {
    fn points_to(&self, collection: &str) -> bool {
        let alias = self.alias.as_ref();
        alias.is_some_and(|a| a.collection_name == collection)
    }
}

/// One reindex of one alias. It lives in the connection, so closing the tab loses nothing.
pub struct ReindexState {
    connection: WeakEntity<ConnectionState>,
    alias: String,
    plan: Option<Plan>,
    target: String,
    copy_source: CopySource,
    /// How to read the file source.
    file_options: Option<ReadOptions>,
    file_error: Option<SharedString>,
    step: Step,
    /// Holds the export until the import has read it.
    dir: Option<Arc<TempDir>>,
    imported: Option<Imported>,
    /// The old collection's count at the last compare.
    old_count: u64,
    /// Numbers that differ but do not stop the copy.
    warning: Option<SharedString>,
    /// Why the copy ended early.
    stopped: Option<SharedString>,
    error: Option<SharedString>,
    busy: bool,
    _subscriptions: [Subscription; 2],
}

impl ReindexState {
    /// A reindex of `alias` on `connection`, which holds it. Loads step one.
    pub fn new(
        connection: &Entity<ConnectionState>,
        alias: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let jobs = cx.global::<JobsGlobal>().0.clone();
        cx.on_release(|this: &mut Self, cx| this.release_dir(cx).detach())
            .detach();
        let subscriptions = [
            // Here, not on a view: the copy goes on with the tab closed.
            cx.subscribe(&jobs, |this, jobs, event: &JobFinished, cx| {
                this.job_finished(event.0, &jobs, cx);
            }),
            // The job is told first: on Windows a file still open only leaves the directory
            // behind. Waiting for it could outlast the time a quit gets, so what a quit cuts
            // short stays for the sweep at a later start.
            cx.on_app_quit(|this: &mut Self, cx| {
                if let Step::Exporting(id) | Step::Importing(id) = this.step {
                    let jobs = cx.global::<JobsGlobal>().0.clone();
                    jobs.update(cx, |jobs, cx| jobs.stop(id, cx));
                }
                this.release_dir(cx)
            }),
        ];
        let mut this = Self {
            connection: connection.downgrade(),
            alias,
            plan: None,
            target: String::new(),
            copy_source: CopySource::Collection,
            file_options: None,
            file_error: None,
            step: Step::Schema,
            dir: None,
            imported: None,
            old_count: 0,
            warning: None,
            stopped: None,
            error: None,
            busy: false,
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    /// Where it stands.
    pub fn step(&self) -> Step {
        self.step
    }

    /// The name of the new collection.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The temp directory, while it exists.
    pub fn dir(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.path().to_path_buf())
    }

    /// The draft of the new collection.
    pub fn draft(&self) -> Option<&SchemaDraft> {
        self.plan.as_ref().map(|plan| &plan.draft)
    }

    fn client(&self, cx: &App) -> Option<Client> {
        client_of(&self.connection.upgrade()?, cx)
    }

    fn source_name(&self) -> &str {
        self.plan
            .as_ref()
            .map_or("", |plan| plan.source.name.as_str())
    }

    fn fail(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(text.into());
        self.busy = false;
        cx.notify();
    }

    /// Off the GPUI thread: this deletes a file of the size of the export.
    fn release_dir(&mut self, cx: &App) -> Task<()> {
        let dir = self.dir.take();
        cx.background_spawn(async move { drop(dir) })
    }

    fn not_connected(&mut self, cx: &mut Context<Self>) {
        self.fail(t!("status.not_connected").to_string(), cx);
    }

    /// Loads the lists and works out step one. Sends no write.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return self.not_connected(cx);
        };
        let supported = self.connection.upgrade().is_some_and(|c| {
            let connected = c.read(cx).connected.as_ref();
            connected.is_some_and(|c| c.info.capabilities.supported)
        });
        self.busy = true;
        self.error = None;
        let list = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .list(client.clone());
        let relevance = cx.global::<RelevanceBackendGlobal>().0.clone();
        let names = move |kind| -> BoxFuture<Result<Vec<String>, Error>> {
            if !supported {
                return Box::pin(async { Ok(Vec::new()) });
            }
            let sets = relevance.sets(client.clone(), kind);
            Box::pin(async move { Ok(sets.await?.into_iter().map(|s| s.name).collect()) })
        };
        let (synonyms, curations) = (names(SetKind::Synonym), names(SetKind::Curation));
        cx.spawn(async move |this, cx| {
            let read = async {
                let (collections, aliases) = list.await?;
                Ok::<_, Error>(Lists {
                    collections,
                    aliases: aliases?,
                    synonym_sets: synonyms.await?,
                    curation_sets: curations.await?,
                })
            };
            let read = read.await;
            this.update(cx, |this, cx| this.loaded(read, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn loaded(&mut self, read: Result<Lists, Error>, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        let lists = match read {
            Ok(lists) => lists,
            Err(e) => {
                let text = long_error_text(&e, &connection.read(cx).profile);
                return self.fail(text, cx);
            }
        };
        let alias = lists.aliases.iter().find(|a| a.name == self.alias);
        let source = alias
            .and_then(|a| {
                lists
                    .collections
                    .iter()
                    .find(|c| c.name == a.collection_name)
            })
            .cloned();
        let Some(source) = source else {
            return self.fail(t!("reindex.alias_gone", alias = self.alias).to_string(), cx);
        };
        let mut draft = SchemaDraft::from_collection(&source);
        draft.blank_secrets();
        self.target = reindex_name(&source.name, jiff::Timestamp::now());
        self.set_plan(source, draft, lists, Vec::new());
        self.busy = false;
        cx.notify();
    }

    /// `missing` are sets an earlier run took out of the draft, which stay named.
    fn set_plan(
        &mut self,
        source: Collection,
        mut draft: SchemaDraft,
        lists: Lists,
        missing: Vec<String>,
    ) {
        let mut found = preflight(
            &self.alias,
            &source,
            &draft,
            &lists.collections,
            &lists.aliases,
            &lists.synonym_sets,
            &lists.curation_sets,
        );
        without_missing(&mut draft, &found);
        for name in missing {
            if !found.missing_sets.contains(&name) {
                found.missing_sets.push(name);
            }
        }
        self.plan = Some(Plan {
            source,
            draft,
            preflight: found,
            lists,
        });
    }

    /// Takes the name typed into the input.
    pub fn set_target(&mut self, name: String, cx: &mut Context<Self>) {
        if self.target != name {
            self.target = name;
            cx.notify();
        }
    }

    /// Takes the schema dialog's result; it validated it, and nothing was sent.
    fn schema_picked(&mut self, json: &Value, cx: &mut Context<Self>) {
        let (Some(plan), Ok(draft)) = (self.plan.take(), SchemaDraft::from_json(&json.to_string()))
        else {
            return;
        };
        self.target = draft.name.clone();
        self.set_plan(plan.source, draft, plan.lists, plan.preflight.missing_sets);
        cx.notify();
    }

    /// Goes back to copying from the old collection.
    fn use_collection(&mut self, cx: &mut Context<Self>) {
        self.copy_source = CopySource::Collection;
        self.file_options = None;
        self.file_error = None;
        cx.notify();
    }

    /// Takes `path` as the source: its format off the GPUI thread, then its first rows.
    pub fn set_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let tokio = cx.global::<Tokio>().0.clone();
        let transfer = cx.global::<TransferBackendGlobal>().0.clone();
        let shown = path.clone();
        let read = tokio.spawn_blocking(move || inspect(&shown));
        self.copy_source = CopySource::File(path.clone());
        self.file_options = None;
        self.file_error = None;
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let head = match read.await {
                Ok(Ok((_, head))) => Ok(head),
                Ok(Err(Inspect::NotAFile)) => Err(t!("transfer.import.not_a_file").to_string()),
                Ok(Err(Inspect::Io(detail))) => Err(read_error_text(&ReadError::Io(detail))),
                Err(e) => Err(read_error_text(&ReadError::Io(e.to_string()))),
            };
            let checked = match head {
                Ok(head) => {
                    let (format, csv) = sniff(Some(&path), &head);
                    let options = ReadOptions {
                        csv,
                        ..ReadOptions::new(format)
                    };
                    let source = Source::File(path);
                    let rows = transfer.preview(source, options.clone(), None).await;
                    rows.map(|_| options).map_err(|e| read_error_text(&e))
                }
                Err(text) => Err(text),
            };
            this.update(cx, |this, cx| {
                this.busy = false;
                match checked {
                    Ok(options) => this.file_options = Some(options),
                    Err(text) => this.file_error = Some(text.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("transfer.import.choose").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = picked.await;
            this.update(cx, |this, cx| match picked {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.set_file(path, cx);
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => this.fail(e.to_string(), cx),
                Err(e) => this.fail(e.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Why Start is disabled now, if a reason can be named.
    pub fn start_block(&self) -> Option<String> {
        let plan = self.plan.as_ref()?;
        let found = &plan.preflight;
        let lists = &plan.lists;
        let taken = lists.collections.iter().any(|c| c.name == self.target)
            || lists.aliases.iter().any(|a| a.name == self.target);
        if self.copy_source == CopySource::Collection && !found.required_unstored.is_empty() {
            let fields = found.required_unstored.join(", ");
            Some(t!("reindex.blocked.required_unstored", fields = fields).to_string())
        } else if found.blank_secrets > 0 {
            let count = found.blank_secrets;
            Some(t!("reindex.blocked.blank_secrets", count = count).to_string())
        } else if !name_ok(&self.target) {
            Some(t!("resources.bad_name").to_string())
        } else if taken {
            Some(t!("reindex.blocked.taken", name = self.target).to_string())
        } else {
            None
        }
    }

    fn can_start(&self) -> bool {
        let file_ready = match self.copy_source {
            CopySource::Collection => true,
            CopySource::File(_) => self.file_options.is_some(),
        };
        self.step == Step::Schema
            && !self.busy
            && self.plan.is_some()
            && file_ready
            && self.start_block().is_none()
    }

    /// Unlocks if needed, checks the name and the alias on a fresh list, confirms on prod,
    /// creates the collection and starts the copy.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.upgrade().filter(|_| self.can_start()) else {
            return;
        };
        let this = cx.entity().downgrade();
        connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.check_start(window, cx))
                    .ok();
            });
        });
    }

    fn check_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return self.not_connected(cx);
        };
        self.busy = true;
        self.error = None;
        let list = cx.global::<CollectionsBackendGlobal>().0.list(client);
        cx.spawn_in(window, async move |this, cx| {
            let fresh = list.await;
            this.update_in(cx, |this, window, cx| this.decide_start(fresh, window, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn decide_start(
        &mut self,
        fresh: Result<crate::collections::CollectionList, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        let profile = connection.read(cx).profile.clone();
        let (collections, aliases) = match fresh {
            Ok((collections, Ok(aliases))) => (collections, aliases),
            Ok((_, Err(e))) | Err(e) => return self.fail(error_text(&e, &profile), cx),
        };
        let name = self.target.clone();
        if collections.iter().any(|c| c.name == name) || aliases.iter().any(|a| a.name == name) {
            return self.fail(t!("reindex.blocked.taken", name = name).to_string(), cx);
        }
        let source = self.source_name().to_string();
        let current = aliases.iter().find(|a| a.name == self.alias);
        if current.is_none_or(|a| a.collection_name != source) {
            let text = self.moved_text(current);
            return self.fail(text, cx);
        }
        let count = collections.iter().find(|c| c.name == source);
        let (Some(plan), Some(count)) = (&mut self.plan, count) else {
            return self.fail(t!("reindex.alias_gone", alias = self.alias).to_string(), cx);
        };
        plan.source.num_documents = count.num_documents;
        if !profile.environment.is_prod() {
            let create = self.create(window, cx);
            cx.spawn(async move |this, cx| {
                if let Err(text) = create.await {
                    this.update(cx, |this, cx| this.fail(text, cx)).ok();
                }
            })
            .detach();
            return;
        }
        self.busy = false;
        let danger = Danger {
            title: t!("reindex.start_title", alias = self.alias).to_string(),
            lines: vec![t!("reindex.start_line", name = name, source = source).to_string()],
            expected: name,
            action: t!("reindex.start").to_string(),
            typed: false,
        };
        let this = cx.entity().downgrade();
        let run = move |window: &mut Window, cx: &mut App| {
            let task = this.update(cx, |this, cx| this.create(window, cx));
            task.unwrap_or_else(|_| Task::ready(Ok(())))
        };
        let report = report_to(cx.entity().downgrade());
        confirm_write(danger, &connection, run, report, window, cx);
    }

    /// Creates the new collection, and starts the copy. Runs on with the tab closed.
    fn create(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), SharedString>> {
        let (Some(client), Some(plan)) = (self.client(cx), &self.plan) else {
            return Task::ready(Err(gone()));
        };
        let mut draft = plan.draft.clone();
        draft.name = self.target.clone();
        self.busy = true;
        let create = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .create(client, draft.to_json());
        cx.spawn_in(window, async move |this, cx| {
            let created = create.await;
            let result = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let connection = this.connection.upgrade();
                let Some(connection) = connection else {
                    return Ok(());
                };
                match created {
                    Ok(_) => {
                        connection.update(cx, |c, cx| c.invalidate_collections(cx));
                        this.begin_copy(window, cx);
                        Ok(())
                    }
                    Err(e) => Err(long_error_text(&e, &connection.read(cx).profile).into()),
                }
            });
            result.unwrap_or(Ok(()))
        })
    }

    /// Opens the dock on Jobs, without moving focus, and starts the first job.
    fn begin_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        RootView::reveal_jobs(window, cx);
        self.stopped = None;
        self.warning = None;
        self.imported = None;
        if let CopySource::File(path) = self.copy_source.clone() {
            let mut options = self
                .file_options
                .clone()
                .unwrap_or(ReadOptions::new(Format::Jsonl));
            if let Some(plan) = &self.plan {
                // They type the cells of a CSV.
                options.fields = plan
                    .draft
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.r#type.as_str().to_string()))
                    .collect();
            }
            return self.start_import(Source::File(path), options, cx);
        }
        self.busy = true;
        let dir = cx.global::<RelevanceBackendGlobal>().0.copy_dir();
        cx.spawn(async move |this, cx| {
            let dir = dir.await;
            this.update(cx, |this, cx| this.start_export(dir, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn start_export(&mut self, dir: Result<Arc<TempDir>, String>, cx: &mut Context<Self>) {
        self.busy = false;
        let (Some(connection), Some(plan)) = (self.connection.upgrade(), &self.plan) else {
            return;
        };
        let dir = match dir {
            Ok(dir) => dir,
            Err(detail) => {
                let text = t!("reindex.no_dir", detail = detail, name = self.target);
                return self.fail(text.to_string(), cx);
            }
        };
        let jobs = cx.global::<JobsGlobal>().0.clone();
        jobs.update(cx, |jobs, _| jobs.hold_temp(&dir));
        let job = ExportJob {
            collection: plan.source.name.clone(),
            scope: ExportScope::All,
            fields: FieldPick::All,
            format: ExportFormat::Jsonl,
            target: dir.path().join(DOCUMENTS),
            expected: Some(plan.source.num_documents),
        };
        let label = t!("reindex.job_export", alias = self.alias)
            .to_string()
            .into();
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let id = jobs.update(cx, |jobs, cx| {
            jobs.start_export(&connection, job, label, cx)
        });
        self.dir = Some(dir);
        self.step = Step::Exporting(id);
        cx.notify();
    }

    fn start_import(&mut self, source: Source, options: ReadOptions, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        let job = ImportJob {
            source,
            options,
            collection: self.target.clone(),
            params: ImportParams {
                action: ImportAction::Create,
                batch_size: crate::settings::current(cx).batch_size,
                dirty_values: DirtyValues::CoerceOrReject,
            },
        };
        let label = t!("reindex.job_import", alias = self.alias)
            .to_string()
            .into();
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let id = jobs.update(cx, |jobs, cx| {
            jobs.start_import(&connection, job, label, cx)
        });
        self.step = Step::Importing(id);
        cx.notify();
    }

    /// Why the copy ended, in full; the directory goes. Nothing moved.
    fn stop(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.release_dir(cx).detach();
        self.stopped = Some(text);
        cx.notify();
    }

    fn job_finished(&mut self, id: JobId, jobs: &Entity<Jobs>, cx: &mut Context<Self>) {
        let (Step::Exporting(running) | Step::Importing(running)) = self.step else {
            return;
        };
        let job = jobs.read(cx).jobs().iter().find(|job| job.id == id);
        let Some(job) = job.filter(|_| running == id) else {
            return;
        };
        let status = job.status.clone();
        let numbers = job.live.clone();
        let outcome = match &job.outcome {
            Some(JobOutcome::Import(outcome)) => Some(outcome.progress.clone()),
            _ => None,
        };
        let text = match status {
            JobStatus::Finished => None,
            JobStatus::Failed(text) => Some(text),
            _ => Some(t!("reindex.job_stopped").to_string().into()),
        };
        if let Some(text) = text {
            return self.stop(text, cx);
        }
        match (self.step, numbers) {
            (Step::Exporting(_), Live::Export(progress)) => {
                let expected = self.plan.as_ref().map_or(0, |p| p.source.num_documents);
                if progress.docs < expected {
                    let text = t!(
                        "reindex.export_short",
                        docs = grouped(progress.docs),
                        expected = grouped(expected)
                    );
                    return self.stop(text.to_string().into(), cx);
                }
                if progress.docs != expected {
                    let text = t!(
                        "jobs.export_mismatch",
                        docs = grouped(progress.docs),
                        expected = grouped(expected)
                    );
                    self.warning = Some(text.to_string().into());
                }
                let Some(dir) = &self.dir else {
                    return;
                };
                let path = dir.path().join(DOCUMENTS);
                self.start_import(Source::File(path), ReadOptions::new(Format::Jsonl), cx);
            }
            (Step::Importing(_), _) => {
                self.release_dir(cx).detach();
                let progress = outcome.unwrap_or_default();
                self.imported = Some(Imported {
                    job: id,
                    imported: progress.imported,
                    failed: progress.failed,
                    unconfirmed: progress.unconfirmed,
                });
                self.compare(cx);
            }
            _ => {}
        }
    }

    /// Someone else moved or deleted the alias; overwriting that would lose their change.
    fn moved_text(&self, alias: Option<&Alias>) -> String {
        match alias {
            Some(alias) => t!(
                "reindex.alias_moved",
                alias = alias.name,
                target = alias.collection_name
            ),
            None => t!("reindex.alias_gone", alias = self.alias),
        }
        .to_string()
    }

    /// Reads the alias and both collections. `Err` when the connection is gone.
    fn fresh(
        &self,
        cx: &App,
    ) -> Result<impl Future<Output = Result<Fresh, Error>> + use<>, SharedString> {
        self.read_server(true, cx)
    }

    /// [`Self::fresh`]. Without `source` the old collection stays unread and counts 0: a delete
    /// of the new collection must work whatever became of the old one.
    fn read_server(
        &self,
        source: bool,
        cx: &App,
    ) -> Result<impl Future<Output = Result<Fresh, Error>> + use<>, SharedString> {
        let Some(client) = self.client(cx) else {
            return Err(gone());
        };
        let backend = cx.global::<CollectionsBackendGlobal>().0.clone();
        let old = source.then(|| backend.get(client.clone(), self.source_name().to_string()));
        let target = backend.get(client.clone(), self.target.clone());
        let alias = cx
            .global::<RelevanceBackendGlobal>()
            .0
            .alias(client, self.alias.clone());
        Ok(async move {
            let alias = match alias.await {
                Ok(alias) => Some(alias),
                Err(Error::Http { status: 404, .. }) => None,
                Err(e) => return Err(e),
            };
            let source = match old {
                Some(old) => old.await?.num_documents,
                None => 0,
            };
            Ok(Fresh {
                alias,
                source,
                target: target.await?.num_documents,
            })
        })
    }

    fn compared(&self, fresh: &Fresh) -> Compared {
        let done = self.imported.as_ref();
        Compared {
            expected: match self.copy_source {
                CopySource::Collection => fresh.source,
                CopySource::File(_) => done.map_or(0, |d| d.imported),
            },
            target: fresh.target,
            failed: done.map_or(0, |d| d.failed),
            unconfirmed: done.map_or(0, |d| d.unconfirmed),
        }
    }

    fn profile_text(&self, e: &Error, cx: &App) -> SharedString {
        let connection = self.connection.upgrade();
        connection.map_or_else(gone, |c| long_error_text(e, &c.read(cx).profile).into())
    }

    /// A copy without a compare counts as stopped, so that it can be abandoned.
    fn compare_failed(&mut self, text: SharedString, cx: &mut Context<Self>) {
        if matches!(self.step, Step::Importing(_)) {
            self.stopped = Some(text);
        } else {
            self.error = Some(text);
        }
        cx.notify();
    }

    /// Reads both sides and shows the compare. After the switch only the counts change.
    pub fn compare(&mut self, cx: &mut Context<Self>) {
        if self.imported.is_none() || self.step == Step::Done {
            return;
        }
        let fresh = match self.fresh(cx) {
            Ok(fresh) => fresh,
            Err(text) => return self.compare_failed(text, cx),
        };
        self.busy = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let fresh = fresh.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match fresh {
                    Ok(fresh) if fresh.alias.is_none() => {
                        this.compare_failed(this.moved_text(None).into(), cx);
                    }
                    Ok(fresh) => {
                        this.old_count = fresh.source;
                        this.stopped = None;
                        let compared = this.compared(&fresh);
                        this.step = match this.step {
                            Step::Switched(_) => Step::Switched(compared),
                            _ => Step::Compare(compared),
                        };
                    }
                    Err(e) => this.compare_failed(this.profile_text(&e, cx), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Whether the switch may go on, on what the server says now.
    fn may_switch(&self, fresh: &Fresh) -> Result<Compared, SharedString> {
        if !fresh.points_to(self.source_name()) {
            return Err(self.moved_text(fresh.alias.as_ref()).into());
        }
        let compared = self.compared(fresh);
        if !compared.equal() {
            return Err(t!(
                "reindex.counts_changed",
                expected = grouped(compared.expected),
                target = grouped(compared.target)
            )
            .to_string()
            .into());
        }
        Ok(compared)
    }

    /// Unlocks if needed, then reads alias and counts fresh and asks. Only an equal compare
    /// gets here, and a compare that is no longer equal goes back to step three.
    pub fn switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let equal = matches!(self.step, Step::Compare(c) if c.equal());
        let Some(connection) = self.connection.upgrade().filter(|_| equal && !self.busy) else {
            return;
        };
        let this = cx.entity().downgrade();
        connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.check_switch(window, cx))
                    .ok();
            });
        });
    }

    fn check_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fresh = match self.fresh(cx) {
            Ok(fresh) => fresh,
            Err(text) => return self.fail(text, cx),
        };
        self.busy = true;
        self.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let fresh = fresh.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match fresh {
                    Ok(fresh) => this.confirm_switch(&fresh, window, cx),
                    Err(e) => {
                        let text = this.profile_text(&e, cx);
                        this.fail(text, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm_switch(&mut self, fresh: &Fresh, window: &mut Window, cx: &mut Context<Self>) {
        if fresh.points_to(self.source_name()) {
            self.step = Step::Compare(self.compared(fresh));
        }
        if let Err(text) = self.may_switch(fresh) {
            return self.fail(text, cx);
        }
        let (Some(connection), Some(plan)) = (self.connection.upgrade(), &self.plan) else {
            return;
        };
        let mut lines = vec![
            t!(
                "reindex.switch_line",
                alias = self.alias,
                old = plan.source.name,
                old_docs = grouped(fresh.source),
                new = self.target,
                new_docs = grouped(fresh.target)
            )
            .to_string(),
        ];
        let joins = &plan.preflight.referenced_by;
        for (collection, field) in joins {
            let text = t!(
                "reindex.switch_join",
                collection = collection,
                field = field
            );
            lines.push(text.to_string());
        }
        let danger = Danger {
            title: t!("reindex.switch_title", alias = self.alias).to_string(),
            lines,
            expected: self.alias.clone(),
            action: t!("reindex.switch").to_string(),
            typed: connection.read(cx).profile.environment.is_prod() || !joins.is_empty(),
        };
        let this = cx.entity().downgrade();
        let run = move |_: &mut Window, cx: &mut App| {
            let task = this.update(cx, |this, cx| this.send_switch(cx));
            task.unwrap_or_else(|_| Task::ready(Ok(())))
        };
        let report = report_to(cx.entity().downgrade());
        confirm_write(danger, &connection, run, report, window, cx);
    }

    /// Reads once more, since the dialog may have stood open, then moves the alias and reads
    /// it back. Runs on with the tab closed.
    fn send_switch(&mut self, cx: &mut Context<Self>) -> Task<Result<(), SharedString>> {
        let (Some(client), Ok(fresh)) = (self.client(cx), self.fresh(cx)) else {
            return Task::ready(Err(gone()));
        };
        let relevance = cx.global::<RelevanceBackendGlobal>().0.clone();
        let (alias, target) = (self.alias.clone(), self.target.clone());
        cx.spawn(async move |this, cx| {
            let fresh = fresh.await;
            let compared = this.update(cx, |this, cx| {
                let fresh = fresh.map_err(|e| this.profile_text(&e, cx))?;
                this.may_switch(&fresh)
            });
            let compared = compared.map_err(|_| gone())??;
            let move_alias = relevance.save_alias(client.clone(), alias.clone(), target.clone());
            let moved = move_alias.await;
            let stored = match moved {
                Ok(_) => relevance.alias(client, alias.clone()).await,
                Err(e) => Err(e),
            };
            this.update(cx, |this, cx| {
                this.invalidate(cx);
                let stored = stored.map_err(|e| this.profile_text(&e, cx))?;
                if stored.collection_name != target {
                    return Err(t!(
                        "aliases.mismatch",
                        name = alias,
                        sent = target,
                        stored = stored.collection_name
                    )
                    .to_string()
                    .into());
                }
                this.step = Step::Switched(compared);
                this.error = None;
                cx.notify();
                Ok(())
            })
            .map_err(|_| gone())?
        })
    }

    fn invalidate(&self, cx: &mut Context<Self>) {
        if let Some(connection) = self.connection.upgrade() {
            let name = self.source_name().to_string();
            connection.update(cx, |c, cx| c.collection_changed(name, cx));
        }
    }

    /// Unlocks if needed, checks that the alias still points to the new collection, confirms
    /// and points it back.
    pub fn switch_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let switched = matches!(self.step, Step::Switched(_));
        let Some(connection) = self.connection.upgrade().filter(|_| switched && !self.busy) else {
            return;
        };
        let this = cx.entity().downgrade();
        connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.check_switch_back(window, cx))
                    .ok();
            });
        });
    }

    fn check_switch_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fresh = match self.fresh(cx) {
            Ok(fresh) => fresh,
            Err(text) => return self.fail(text, cx),
        };
        self.busy = true;
        self.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let fresh = fresh.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match fresh {
                    Ok(fresh) if fresh.points_to(&this.target) => {
                        this.confirm_switch_back(&fresh, window, cx);
                    }
                    Ok(fresh) => this.fail(this.moved_text(fresh.alias.as_ref()), cx),
                    Err(e) => {
                        let text = this.profile_text(&e, cx);
                        this.fail(text, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm_switch_back(&mut self, fresh: &Fresh, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        let line = t!(
            "reindex.switch_back_line",
            alias = self.alias,
            old = self.source_name(),
            old_docs = grouped(fresh.source),
            new = self.target,
            new_docs = grouped(fresh.target)
        );
        let danger = Danger {
            title: t!("reindex.switch_back_title", alias = self.alias).to_string(),
            lines: vec![line.to_string()],
            expected: self.alias.clone(),
            action: t!("reindex.switch_back").to_string(),
            typed: false,
        };
        let this = cx.entity().downgrade();
        let run = move |_: &mut Window, cx: &mut App| {
            let task = this.update(cx, |this, cx| this.send_switch_back(cx));
            task.unwrap_or_else(|_| Task::ready(Ok(())))
        };
        let report = report_to(cx.entity().downgrade());
        confirm_write(danger, &connection, run, report, window, cx);
    }

    fn send_switch_back(&mut self, cx: &mut Context<Self>) -> Task<Result<(), SharedString>> {
        let (Some(client), Ok(fresh)) = (self.client(cx), self.fresh(cx)) else {
            return Task::ready(Err(gone()));
        };
        let relevance = cx.global::<RelevanceBackendGlobal>().0.clone();
        let (alias, source) = (self.alias.clone(), self.source_name().to_string());
        let target = self.target.clone();
        cx.spawn(async move |this, cx| {
            let fresh = fresh.await;
            let checked = this.update(cx, |this, cx| {
                let fresh = fresh.map_err(|e| this.profile_text(&e, cx))?;
                if !fresh.points_to(&target) {
                    return Err(SharedString::from(this.moved_text(fresh.alias.as_ref())));
                }
                Ok(fresh)
            });
            let fresh = checked.map_err(|_| gone())??;
            let moved = relevance.save_alias(client.clone(), alias.clone(), source.clone());
            let moved = moved.await;
            let stored = match moved {
                Ok(_) => relevance.alias(client, alias.clone()).await,
                Err(e) => Err(e),
            };
            this.update(cx, |this, cx| {
                this.invalidate(cx);
                let stored = stored.map_err(|e| this.profile_text(&e, cx))?;
                if stored.collection_name != source {
                    return Err(t!(
                        "aliases.mismatch",
                        name = alias,
                        sent = source,
                        stored = stored.collection_name
                    )
                    .to_string()
                    .into());
                }
                this.old_count = fresh.source;
                this.step = Step::Compare(this.compared(&fresh));
                this.error = None;
                cx.notify();
                Ok(())
            })
            .map_err(|_| gone())?
        })
    }

    /// Whether the new collection can be abandoned: the copy ended, and the alias never moved.
    fn can_abandon(&self) -> bool {
        match self.step {
            Step::Exporting(_) | Step::Importing(_) => self.stopped.is_some(),
            Step::Compare(_) => true,
            _ => false,
        }
    }

    /// Asks for the name, then deletes the new collection and starts over. Refuses while the
    /// alias points to it.
    pub fn delete_new(
        &mut self,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_abandon() || self.busy {
            return;
        }
        self.check_delete(state, false, window, cx);
    }

    /// Asks for the name, then deletes the old collection. Refuses unless the alias points to
    /// the new collection and that holds at least what the old one does.
    pub fn delete_old(
        &mut self,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let equal = matches!(self.step, Step::Switched(c) if c.equal());
        if equal && !self.busy {
            self.check_delete(state, true, window, cx);
        }
    }

    fn check_delete(
        &mut self,
        state: Entity<AppState>,
        old: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fresh = match self.read_server(old, cx) {
            Ok(fresh) => fresh,
            Err(text) => return self.fail(text, cx),
        };
        self.busy = true;
        self.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let fresh = fresh.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match fresh {
                    Ok(fresh) => this.open_delete(state, old, &fresh, window, cx),
                    Err(e) => {
                        let text = this.profile_text(&e, cx);
                        this.fail(text, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_delete(
        &mut self,
        state: Entity<AppState>,
        old: bool,
        fresh: &Fresh,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        let (name, documents) = match self.may_delete(old, fresh) {
            Ok(found) => found,
            Err(text) => return self.fail(text, cx),
        };
        let this = cx.entity().downgrade();
        let check = move |cx: &mut App| {
            let task = this.update(cx, |this, cx| this.recheck_delete(old, cx));
            task.unwrap_or_else(|_| Task::ready(Err(gone())))
        };
        let this = cx.entity().downgrade();
        let after = move |cx: &mut App| {
            this.update(cx, |this, cx| {
                if old {
                    this.finish(cx);
                } else {
                    this.start_over(cx);
                }
            })
            .ok();
        };
        collection_danger::delete_then(
            state, connection, name, documents, check, after, window, cx,
        );
    }

    /// What may be deleted and its count, on what the server says now.
    fn may_delete(&self, old: bool, fresh: &Fresh) -> Result<(String, u64), SharedString> {
        let serving = fresh.points_to(&self.target);
        if old {
            if !serving {
                return Err(self.moved_text(fresh.alias.as_ref()).into());
            }
            if fresh.target < fresh.source {
                let text = t!(
                    "reindex.counts_changed",
                    expected = grouped(fresh.source),
                    target = grouped(fresh.target)
                );
                return Err(text.to_string().into());
            }
            Ok((self.source_name().to_string(), fresh.source))
        } else if serving {
            Err(self.moved_text(fresh.alias.as_ref()).into())
        } else {
            Ok((self.target.clone(), fresh.target))
        }
    }

    /// Reads once more, since the dialog may have stood open. Runs on with the tab closed.
    fn recheck_delete(&self, old: bool, cx: &mut Context<Self>) -> Task<Result<(), SharedString>> {
        let fresh = match self.read_server(old, cx) {
            Ok(fresh) => fresh,
            Err(text) => return Task::ready(Err(text)),
        };
        cx.spawn(async move |this, cx| {
            let fresh = fresh.await;
            this.update(cx, |this, cx| {
                let fresh = fresh.map_err(|e| this.profile_text(&e, cx))?;
                this.may_delete(old, &fresh).map(drop)
            })
            .map_err(|_| gone())?
        })
    }

    /// Back to step one, with a new name. A new collection that was kept stays.
    pub fn start_over(&mut self, cx: &mut Context<Self>) {
        self.release_dir(cx).detach();
        self.step = Step::Schema;
        self.plan = None;
        self.imported = None;
        self.stopped = None;
        self.warning = None;
        self.load(cx);
    }

    /// Ends the assistant and forgets it.
    pub fn finish(&mut self, cx: &mut Context<Self>) {
        self.release_dir(cx).detach();
        self.step = Step::Done;
        if let Some(connection) = self.connection.upgrade() {
            let alias = self.alias.clone();
            connection.update(cx, |c, _| {
                c.reindex.remove(&alias);
            });
        }
        cx.notify();
    }
}

/// A step's text after the connection went while it ran.
fn gone() -> SharedString {
    t!("status.not_connected").to_string().into()
}

/// The file inside the temp directory.
const DOCUMENTS: &str = "documents.jsonl";

impl ErrorSlot for ReindexState {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

/// The content of a reindex tab. Everything lasting is in [`ReindexState`].
pub struct ReindexView {
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    reindex: Entity<ReindexState>,
    name: Entity<InputState>,
    jobs: Entity<Jobs>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 4],
}

impl ReindexView {
    /// The tab of `reindex`.
    pub fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        reindex: Entity<ReindexState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(reindex.read(cx).target.clone(), window, cx);
            input
        });
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let subscriptions = [
            cx.observe_in(&reindex, window, |this, _, window, cx| {
                this.sync_name(window, cx);
                cx.notify();
            }),
            cx.observe(&connection, |_, _, cx| cx.notify()),
            cx.observe(&jobs, |_, _, cx| cx.notify()),
            cx.subscribe(&name, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().trim().to_string();
                    this.reindex.update(cx, |r, cx| r.set_target(value, cx));
                }
            }),
        ];
        Self {
            state,
            connection,
            reindex,
            name,
            jobs,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The state behind this tab.
    pub fn reindex(&self) -> &Entity<ReindexState> {
        &self.reindex
    }

    fn sync_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.reindex.read(cx).target.clone();
        if self.name.read(cx).value().trim() != target {
            self.name
                .update(cx, |input, cx| input.set_value(target, window, cx));
        }
    }

    /// The job the copy waits for, while it runs.
    fn running_job(&self, cx: &App) -> Option<(JobId, JobStatus)> {
        let (Step::Exporting(id) | Step::Importing(id)) = self.reindex.read(cx).step else {
            return None;
        };
        let job = self.jobs.read(cx).jobs().iter().find(|job| job.id == id)?;
        job.is_running().then(|| (id, job.status.clone()))
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        let r = self.reindex.read(cx);
        let running = self.running_job(cx);
        let stopping = matches!(running, Some((_, JobStatus::Stopping)));
        vec![
            (!self.can_next(cx), &NextStep),
            (!self.can_back(cx), &StepBack),
            (!(r.step == Step::Schema && r.plan.is_some()), &EditResource),
            (!self.can_delete(cx), &DeleteResource),
            (
                r.imported.is_none() || r.step == Step::Done,
                &ReloadResource,
            ),
            (r.imported.is_none(), &SaveReport),
            (running.is_none(), &CancelJob),
            (!stopping, &StopWaiting),
            (r.step != Step::Schema, &PickImportFile),
            (r.copy_source == CopySource::Collection, &RemoveBodyFile),
        ]
    }

    fn can_next(&self, cx: &App) -> bool {
        let r = self.reindex.read(cx);
        match r.step {
            Step::Schema => r.can_start(),
            Step::Compare(compared) => compared.equal() && !r.busy,
            Step::Switched(_) => !r.busy,
            _ => false,
        }
    }

    fn can_back(&self, cx: &App) -> bool {
        let r = self.reindex.read(cx);
        (r.can_abandon() || matches!(r.step, Step::Switched(_))) && !r.busy
    }

    fn can_delete(&self, cx: &App) -> bool {
        let r = self.reindex.read(cx);
        !r.busy && (r.can_abandon() || matches!(r.step, Step::Switched(c) if c.equal()))
    }

    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let step = self.reindex.read(cx).step;
        self.reindex.update(cx, |r, cx| match step {
            Step::Schema => r.start(window, cx),
            Step::Compare(_) => r.switch(window, cx),
            Step::Switched(_) => r.finish(cx),
            _ => {}
        });
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_back(cx) {
            return;
        }
        self.reindex.update(cx, |r, cx| {
            if matches!(r.step, Step::Switched(_)) {
                r.switch_back(window, cx);
            } else {
                r.start_over(cx);
            }
        });
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        self.reindex.update(cx, |r, cx| {
            if matches!(r.step, Step::Switched(_)) {
                r.delete_old(state, window, cx);
            } else {
                r.delete_new(state, window, cx);
            }
        });
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reindex.update(cx, |r, cx| r.compare(cx));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.running_job(cx) {
            self.jobs.update(cx, |jobs, cx| jobs.stop(id, cx));
        }
    }

    fn stop_waiting(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.running_job(cx) {
            self.jobs.update(cx, |jobs, cx| jobs.abort(id, cx));
        }
    }

    fn show_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(imported) = self.reindex.read(cx).imported {
            RootView::reveal_job(imported.job, window, cx);
        }
    }

    fn edit_schema(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let r = self.reindex.read(cx);
        let Some(mut draft) = r.draft().cloned().filter(|_| r.step == Step::Schema) else {
            return;
        };
        draft.name = r.target.clone();
        let reindex = self.reindex.downgrade();
        SchemaDialog::open_pick(
            self.state.clone(),
            self.connection.clone(),
            draft,
            move |json, _, cx| {
                reindex.update(cx, |r, cx| r.schema_picked(&json, cx)).ok();
            },
            window,
            cx,
        );
    }

    fn button(
        &self,
        id: &'static str,
        label: String,
        action: &'static dyn Action,
        enabled: bool,
        window: &Window,
    ) -> Button {
        let focus = self.focus_handle.clone();
        Button::new(id)
            .small()
            .label(label)
            .children(Kbd::binding_for_action(action, Some(REINDEX), window))
            .disabled(!enabled)
            .on_click(move |_, window, cx| focus.dispatch_action(action, window, cx))
    }

    /// [`Self::button`] of a write, which shows the lock while writes are locked.
    fn write_button(
        &self,
        id: &'static str,
        label: String,
        action: &'static dyn Action,
        enabled: bool,
        window: &Window,
        cx: &App,
    ) -> Button {
        let locked = self.connection.read(cx).writes_locked();
        self.button(id, label, action, enabled, window)
            .when(locked, |b| b.child(env_badge::lock(id, false, cx)))
    }

    fn stepper(&self, step: Step, cx: &App) -> impl IntoElement {
        let current = match step {
            Step::Schema => 0,
            Step::Exporting(_) | Step::Importing(_) => 1,
            Step::Compare(_) => 2,
            Step::Switched(_) | Step::Done => 4,
        };
        let (active, muted) = (cx.theme().foreground, cx.theme().muted_foreground);
        h_flex().gap_3().text_sm().children(
            ["schema", "copy", "compare", "switch", "clean_up"]
                .into_iter()
                .enumerate()
                .map(move |(i, key)| {
                    let text = t!(format!("reindex.step.{key}"), n = i + 1).to_string();
                    let color = if i == current { active } else { muted };
                    div().text_color(color).child(text)
                }),
        )
    }

    fn schema_notes(&self, cx: &App) -> Vec<(&'static str, SharedString, Hsla)> {
        let r = self.reindex.read(cx);
        let (warning, muted) = (cx.theme().warning, cx.theme().muted_foreground);
        let Some(plan) = &r.plan else {
            return Vec::new();
        };
        let found = &plan.preflight;
        let mut notes: Vec<(&'static str, SharedString, Hsla)> = Vec::new();
        let mut add = |id, text: std::borrow::Cow<'_, str>, color| {
            notes.push((id, text.to_string().into(), color));
        };
        if !found.unstored.is_empty() {
            let fields = found.unstored.join(", ");
            add(
                "reindex.note.unstored",
                t!("reindex.notes.unstored", fields = fields),
                warning,
            );
        }
        if !found.referenced_by.is_empty() {
            let joins: Vec<String> = found
                .referenced_by
                .iter()
                .map(|(collection, field)| format!("{collection}.{field}"))
                .collect();
            let joins = joins.join(", ");
            add(
                "reindex.note.joins",
                t!("reindex.notes.joins", joins = joins),
                warning,
            );
        }
        if !found.missing_sets.is_empty() {
            let sets = found.missing_sets.join(", ");
            add(
                "reindex.note.missing",
                t!("reindex.notes.missing", sets = sets),
                warning,
            );
        }
        if !found.embedded.is_empty() {
            let fields = found.embedded.join(", ");
            add(
                "reindex.note.embedded",
                t!("reindex.notes.embedded", fields = fields),
                warning,
            );
        }
        add("reindex.note.writers", t!("reindex.notes.writers"), warning);
        if r.copy_source == CopySource::Collection {
            add("reindex.note.disk", t!("reindex.notes.disk"), muted);
        }
        if !found.other_aliases.is_empty() {
            let aliases = found.other_aliases.join(", ");
            let source = r.source_name();
            add(
                "reindex.note.aliases",
                t!("reindex.notes.aliases", source = source, aliases = aliases),
                muted,
            );
        }
        notes
    }

    fn schema_body(&self, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let r = self.reindex.read(cx);
        let danger = cx.theme().danger;
        let on_file = r.copy_source != CopySource::Collection;
        let shown_file = match &r.copy_source {
            CopySource::File(path) => path.display().to_string(),
            CopySource::Collection => String::new(),
        };
        let file_ready = on_file && r.file_options.is_some();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(t!("reindex.new_collection").to_string())
                    .child(
                        div().w_80().child(
                            Input::new(&self.name)
                                .id("reindex.name")
                                .aria_label(t!("reindex.new_collection").to_string()),
                        ),
                    )
                    .child(self.button(
                        "reindex.edit_schema",
                        t!("reindex.edit_schema").to_string(),
                        &EditResource,
                        r.plan.is_some(),
                        window,
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(t!("reindex.source").to_string())
                    .child(self.button(
                        "reindex.source.collection",
                        t!("reindex.source_collection", name = r.source_name()).to_string(),
                        &RemoveBodyFile,
                        on_file,
                        window,
                    ))
                    .child(self.button(
                        "reindex.source.file",
                        t!("reindex.source_file").to_string(),
                        &PickImportFile,
                        true,
                        window,
                    ))
                    .children(file_ready.then(|| {
                        notice(
                            "reindex.file.ok",
                            shown_file.into(),
                            cx.theme().muted_foreground,
                        )
                    })),
            )
            .children(
                r.file_error
                    .clone()
                    .map(|e| notice("reindex.file.error", e, danger)),
            )
            .children(
                self.schema_notes(cx)
                    .into_iter()
                    .map(|(id, text, color)| notice(id, text, color)),
            )
            .children(
                r.start_block()
                    .map(|text| notice("reindex.blocked", text.into(), danger)),
            )
            .child(
                h_flex().justify_end().child(
                    self.write_button(
                        "reindex.start",
                        t!("reindex.start").to_string(),
                        &NextStep,
                        r.can_start(),
                        window,
                        cx,
                    )
                    .primary(),
                ),
            )
    }

    fn job_line(&self, id: JobId, cx: &App) -> Option<String> {
        let job = self.jobs.read(cx).jobs().iter().find(|job| job.id == id)?;
        Some(
            match &job.live {
                Live::Export(p) => t!(
                    "reindex.export_live",
                    docs = grouped(p.docs),
                    total = grouped(p.expected.unwrap_or_default())
                ),
                Live::Import(p) => t!(
                    "reindex.import_live",
                    imported = grouped(p.imported),
                    failed = grouped(p.failed),
                    unconfirmed = grouped(p.unconfirmed)
                ),
                Live::Backup(_) | Live::Restore(_) => return None,
            }
            .to_string(),
        )
    }

    fn copy_body(&self, id: JobId, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let r = self.reindex.read(cx);
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let running = self.running_job(cx);
        let stopping = matches!(running, Some((_, JobStatus::Stopping)));
        v_flex()
            .gap_2()
            .children(
                self.job_line(id, cx)
                    .map(|text| notice("reindex.job", text.into(), muted)),
            )
            .children(r.stopped.clone().map(|text| {
                let moved = if r.imported.is_some() {
                    t!("reindex.compare_failed", target = r.target)
                } else {
                    t!(
                        "reindex.stopped",
                        alias = r.alias,
                        source = r.source_name(),
                        target = r.target
                    )
                };
                v_flex()
                    .child(notice("reindex.stopped", moved.to_string().into(), danger))
                    .child(notice("reindex.stopped_detail", text, danger))
            }))
            .child(
                h_flex()
                    .gap_2()
                    .children(running.is_some().then(|| {
                        self.button(
                            "reindex.cancel",
                            t!("jobs.cancel").to_string(),
                            &CancelJob,
                            true,
                            window,
                        )
                    }))
                    .children(stopping.then(|| {
                        self.button(
                            "reindex.stop_waiting",
                            t!("jobs.stop_waiting").to_string(),
                            &StopWaiting,
                            true,
                            window,
                        )
                    }))
                    .children(r.imported.is_some().then(|| {
                        self.button(
                            "reindex.compare_again",
                            t!("reindex.compare_again").to_string(),
                            &ReloadResource,
                            !r.busy,
                            window,
                        )
                    }))
                    .children(self.abandon_buttons(window, cx)),
            )
    }

    /// `Delete {new}` and `Start over`, once the copy ended.
    fn abandon_buttons(&self, window: &Window, cx: &App) -> Vec<Button> {
        let r = self.reindex.read(cx);
        if !r.can_abandon() {
            return Vec::new();
        }
        vec![
            self.button(
                "reindex.start_over",
                t!("reindex.start_over").to_string(),
                &StepBack,
                !r.busy,
                window,
            ),
            self.write_button(
                "reindex.delete_new",
                t!("reindex.delete", name = r.target).to_string(),
                &DeleteResource,
                !r.busy,
                window,
                cx,
            )
            .danger(),
        ]
    }

    fn compare_body(
        &self,
        compared: Compared,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let r = self.reindex.read(cx);
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let equal = compared.equal();
        let counts = t!(
            "reindex.counts",
            expected = grouped(compared.expected),
            target = grouped(compared.target),
            failed = grouped(compared.failed),
            unconfirmed = grouped(compared.unconfirmed)
        );
        let on_file = r.copy_source != CopySource::Collection;
        v_flex()
            .gap_2()
            .child(notice("reindex.counts", counts.to_string().into(), muted))
            .children(on_file.then(|| {
                let text = t!(
                    "reindex.old_count",
                    count = grouped(r.old_count),
                    name = r.source_name()
                );
                notice("reindex.old_count", text.to_string().into(), muted)
            }))
            .children((!equal).then(|| {
                notice(
                    "reindex.unequal",
                    t!("reindex.unequal", alias = r.alias, source = r.source_name())
                        .to_string()
                        .into(),
                    danger,
                )
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(self.button(
                        "reindex.save_report",
                        t!("jobs.save_report").to_string(),
                        &SaveReport,
                        true,
                        window,
                    ))
                    .child(self.button(
                        "reindex.compare_again",
                        t!("reindex.compare_again").to_string(),
                        &ReloadResource,
                        !r.busy,
                        window,
                    ))
                    .children(self.abandon_buttons(window, cx))
                    .child(
                        self.write_button(
                            "reindex.switch",
                            t!("reindex.switch").to_string(),
                            &NextStep,
                            equal && !r.busy,
                            window,
                            cx,
                        )
                        .primary(),
                    ),
            )
    }

    fn switched_body(
        &self,
        compared: Compared,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let r = self.reindex.read(cx);
        let muted = cx.theme().muted_foreground;
        let text = t!("reindex.switched", alias = r.alias, target = r.target);
        v_flex()
            .gap_2()
            .child(notice("reindex.switched", text.to_string().into(), muted))
            .child(notice(
                "reindex.clean_up_note",
                t!("reindex.clean_up_note").to_string().into(),
                muted,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(self.write_button(
                        "reindex.switch_back",
                        t!("reindex.switch_back").to_string(),
                        &StepBack,
                        !r.busy,
                        window,
                        cx,
                    ))
                    .children(compared.equal().then(|| {
                        self.write_button(
                            "reindex.delete_old",
                            t!("reindex.delete", name = r.source_name()).to_string(),
                            &DeleteResource,
                            !r.busy,
                            window,
                            cx,
                        )
                        .danger()
                    }))
                    .child(
                        self.button(
                            "reindex.keep",
                            t!("reindex.keep").to_string(),
                            &NextStep,
                            !r.busy,
                            window,
                        )
                        .primary(),
                    ),
            )
    }
}

impl Focusable for ReindexView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ReindexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let r = self.reindex.read(cx);
        let (danger, warning) = (cx.theme().danger, cx.theme().warning);
        let step = r.step;
        let heading = r.plan.as_ref().map(|plan| {
            t!(
                "reindex.heading",
                alias = r.alias,
                source = plan.source.name,
                count = grouped(plan.source.num_documents)
            )
            .to_string()
        });
        let (warned, failed) = (r.warning.clone(), r.error.clone());
        let alias = r.alias.clone();
        v_flex()
            .id("reindex")
            .track_focus(&self.focus_handle)
            .size_full()
            .gap_3()
            .p_3()
            .overflow_y_scroll()
            .on_action(cx.listener(|this, _: &NextStep, window, cx| this.next(window, cx)))
            .on_action(cx.listener(|this, _: &StepBack, window, cx| this.back(window, cx)))
            .on_action(cx.listener(|this, _: &EditResource, window, cx| {
                this.edit_schema(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &CancelJob, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &StopWaiting, _, cx| this.stop_waiting(cx)))
            .on_action(cx.listener(|this, _: &SaveReport, window, cx| {
                this.show_report(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PickImportFile, window, cx| {
                this.reindex.update(cx, |r, cx| r.pick_file(window, cx));
            }))
            .on_action(cx.listener(|this, _: &RemoveBodyFile, _, cx| {
                this.reindex.update(cx, |r, cx| r.use_collection(cx));
            }))
            .child(self.stepper(step, cx))
            .children(
                heading.map(|text| notice("reindex.heading", text.into(), cx.theme().foreground)),
            )
            .children(failed.map(|e| notice("reindex.error", e, danger)))
            .children(warned.map(|e| notice("reindex.warning", e, warning)))
            .child(match step {
                Step::Schema => self.schema_body(window, cx).into_any_element(),
                Step::Exporting(id) | Step::Importing(id) => {
                    self.copy_body(id, window, cx).into_any_element()
                }
                Step::Compare(compared) => {
                    self.compare_body(compared, window, cx).into_any_element()
                }
                Step::Switched(compared) => {
                    self.switched_body(compared, window, cx).into_any_element()
                }
                Step::Done => notice(
                    "reindex.done",
                    t!("reindex.done", alias = alias).to_string().into(),
                    cx.theme().muted_foreground,
                )
                .into_any_element(),
            })
    }
}
