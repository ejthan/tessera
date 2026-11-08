// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the reindex assistant.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WeakEntity, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::connection::ConnectionState;
use tessera_app::jobs::{JobId, Jobs, JobsGlobal};
use tessera_app::state::{AppState, ResourceKind};
use tessera_app::views::dock::ReindexPanel;
use tessera_app::views::resources::reindex::{ReindexState, Step};
use tessera_app::views::root::RootView;
use tessera_client::{Alias, Collection, DirtyValues, Field, ImportAction, ImportParams};
use tessera_core::profile::{Environment, Profile};
use tessera_core::relevance::reindex::Compared;
use tessera_core::transfer::JobEnd;
use tessera_core::transfer::TransferError;
use tessera_core::transfer::export::{ExportFormat, ExportJob, ExportScope, FieldPick};
use tessera_core::transfer::import::{ImportJob, ImportProgress};
use tessera_core::transfer::preview::Preview;
use tessera_core::transfer::source::{Format, ReadOptions, Source};

use common::*;

const EXPORT: JobId = JobId(0);
const IMPORT: JobId = JobId(1);
const ALIAS: &str = "shop";

fn field(name: &str, attributes: Value) -> Field {
    Field {
        name: name.into(),
        r#type: "string".into(),
        attributes: attributes.as_object().unwrap().clone(),
    }
}

fn products() -> Collection {
    Collection {
        name: "products".into(),
        num_documents: 10,
        fields: vec![field("title", json!({}))],
        ..Collection::default()
    }
}

fn alias(name: &str, target: &str) -> Alias {
    Alias {
        name: name.into(),
        collection_name: target.into(),
    }
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    conn: WeakEntity<ConnectionState>,
    id: uuid::Uuid,
    relevance: Arc<FakeRelevance>,
    transfer: Arc<FakeTransfer>,
    data: Arc<FakeData>,
    jobs: Entity<Jobs>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(
        cx: &mut TestAppContext,
        profile: Profile,
        collections: Vec<Collection>,
        aliases: Vec<Alias>,
    ) -> Self {
        let data = fake_backend(cx, Some((collections, aliases)));
        fake_documents(cx);
        install_search_fake(cx);
        let relevance = fake_relevance(cx);
        let transfer = fake_transfer(cx);
        *relevance.alias_target.lock().unwrap() = Some("products".into());
        let id = profile.id;
        fake_connector(cx, |_| Ok(server("30.2", true)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        tessera_core::secrets::SecretStore::set(&*secrets, id, &"k".to_string().into()).unwrap();
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        let conn = cx.update(|cx| state.read(cx).connection(id, cx)).unwrap();
        let jobs = cx.update(|cx| cx.global::<JobsGlobal>().0.clone());
        let f = Self {
            handle,
            state,
            conn: conn.downgrade(),
            id,
            relevance,
            transfer,
            data,
            jobs,
            _rt: rt,
        };
        f.open(cx);
        wait_for_element(cx, handle, "reindex.start".into(), true).await;
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext) -> Self {
        let profile = Profile::new("test", vec![node()]);
        Self::with(
            cx,
            profile,
            vec![products()],
            vec![alias(ALIAS, "products")],
        )
        .await
    }

    fn open(&self, cx: &mut TestAppContext) {
        cx.update(|cx| {
            self.state
                .update(cx, |s, cx| s.open_reindex(self.id, ALIAS.into(), cx));
        });
    }

    fn settle(&self, cx: &mut TestAppContext) {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            window.try_find(id.to_string()).is_some()
        })
        .unwrap()
    }

    fn text(&self, cx: &mut TestAppContext, id: &str) -> String {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            let found = window.find(id.to_string());
            found.label().unwrap_or_default().to_string()
        })
        .unwrap()
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.click(id.to_string(), cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            fill(window, id, text, cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn reindex(&self, cx: &mut TestAppContext) -> Option<Entity<ReindexState>> {
        let conn = self.conn.upgrade()?;
        cx.read(|cx| conn.read(cx).reindex.get(ALIAS).cloned())
    }

    fn step(&self, cx: &mut TestAppContext) -> Step {
        let reindex = self.reindex(cx).unwrap();
        cx.read(|cx| reindex.read(cx).step())
    }

    fn target(&self, cx: &mut TestAppContext) -> String {
        let reindex = self.reindex(cx).unwrap();
        cx.read(|cx| reindex.read(cx).target().to_string())
    }

    fn calls(&self) -> Vec<String> {
        self.relevance.calls.lock().unwrap().clone()
    }

    fn moved(&self) -> bool {
        self.calls().iter().any(|c| c.starts_with("save_alias"))
    }

    fn created(&self) -> usize {
        self.data.creates.lock().unwrap().len()
    }

    fn set_count(&self, name: &str, count: u64) {
        let mut data = self.data.data.lock().unwrap();
        let all = &mut data.as_mut().unwrap().0;
        all.iter_mut()
            .find(|c| c.name == name)
            .unwrap()
            .num_documents = count;
    }

    fn start(&self, cx: &mut TestAppContext) {
        self.click(cx, "reindex.start");
        self.settle(cx);
    }

    fn export_done(&self, cx: &mut TestAppContext, docs: u64) {
        self.transfer
            .finish_export(EXPORT, export_outcome(JobEnd::Finished, docs, Some(10), 0));
        self.settle(cx);
    }

    fn import_done(&self, cx: &mut TestAppContext, id: JobId, imported: u64, failed: u64) {
        let progress = ImportProgress {
            imported,
            failed,
            ..Default::default()
        };
        self.transfer
            .finish(id, import_outcome(JobEnd::Finished, progress));
        self.settle(cx);
    }

    /// Runs the copy; the new collection ends with `count` documents.
    fn copied(&self, cx: &mut TestAppContext, count: u64) {
        self.start(cx);
        self.export_done(cx, 10);
        let target = self.target(cx);
        self.set_count(&target, count);
        self.import_done(cx, IMPORT, count, 0);
    }

    fn switch(&self, cx: &mut TestAppContext) {
        self.click(cx, "reindex.switch");
        self.click(cx, "confirm.ok");
    }

    fn dir(&self) -> std::path::PathBuf {
        self.relevance.copy_dirs.lock().unwrap()[0].clone()
    }
}

#[gpui_kit::test]
async fn nothing_is_sent_before_start(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    assert_eq!(f.created(), 0);
    assert!(f.calls().is_empty(), "{:?}", f.calls());
    let exports = f.transfer.exports.lock().unwrap().len();
    assert_eq!(exports, 0);
    assert_eq!(*f.data.gets.lock().unwrap(), Vec::<String>::new());
    assert!(f.target(cx).starts_with("products_2"));
}

#[gpui_kit::test]
async fn cancelling_nothing_created(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![products()], aliases).await;
    f.start(cx);
    assert!(f.has(cx, "confirm.body"));
    f.click(cx, "confirm.cancel");
    assert_eq!(f.created(), 0);
    assert_eq!(f.step(cx), Step::Schema);
}

fn with_unstored(optional: bool) -> Collection {
    let mut source = products();
    let attributes = json!({"store": false, "optional": optional});
    source.fields.push(field("secret", attributes));
    source
}

#[gpui_kit::test]
async fn required_unstored_blocks_collection_source(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![with_unstored(false)], aliases).await;
    assert!(f.text(cx, "reindex.blocked").contains("secret"));
    f.start(cx);
    assert_eq!(f.created(), 0);
}

#[gpui_kit::test]
async fn an_optional_unstored_field_warns(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![with_unstored(true)], aliases).await;
    assert!(f.text(cx, "reindex.note.unstored").contains("secret"));
    assert!(!f.has(cx, "reindex.blocked"));
}

fn write_file(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("docs.jsonl");
    std::fs::write(&path, "{\"id\":\"1\",\"title\":\"a\"}\n").unwrap();
    path
}

async fn pick_file(f: &Fixture, cx: &mut TestAppContext, path: std::path::PathBuf) {
    *f.transfer.preview.lock().unwrap() = Some(Ok(Preview {
        rows: Vec::new(),
        complete: true,
        estimated_docs: 1,
        total_bytes: Some(1),
    }));
    let reindex = f.reindex(cx).unwrap();
    cx.update(|cx| reindex.update(cx, |r, cx| r.set_file(path, cx)));
    wait_for_element(cx, f.handle, "reindex.file.ok".into(), true).await;
    f.settle(cx);
}

#[gpui_kit::test]
async fn file_source_is_allowed_then(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![with_unstored(false)], aliases).await;
    let dir = tempfile::tempdir().unwrap();
    pick_file(&f, cx, write_file(dir.path())).await;
    assert!(!f.has(cx, "reindex.blocked"));
    f.start(cx);
    assert_eq!(f.created(), 1);
    assert!(f.transfer.exports.lock().unwrap().is_empty());
    assert_eq!(f.transfer.imports.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
async fn blank_secret_blocks(cx: &mut TestAppContext) {
    let mut source = products();
    let embed = json!({"embed": {"from": ["title"], "model_config": {
        "model_name": "openai/x", "api_key": "sk-…abcd"}}});
    source.fields.push(field("v", embed));
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![source], aliases).await;
    assert!(f.has(cx, "reindex.blocked"));
    assert!(f.has(cx, "reindex.note.embedded"));
    f.start(cx);
    assert_eq!(f.created(), 0);
}

#[gpui_kit::test]
async fn missing_set_is_dropped_from_the_draft_and_named(cx: &mut TestAppContext) {
    let mut source = products();
    source.synonym_sets = vec!["gone".into()];
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![source], aliases).await;
    assert!(f.text(cx, "reindex.note.missing").contains("gone"));
    f.start(cx);
    let body = f.data.creates.lock().unwrap()[0].clone();
    assert!(!body.to_string().contains("gone"), "{body}");
}

#[gpui_kit::test]
async fn a_taken_name_is_refused_on_a_fresh_list(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.data
        .data
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .0
        .push(Collection {
            name: f.target(cx),
            ..Collection::default()
        });
    f.start(cx);
    assert_eq!(f.created(), 0);
    assert!(f.has(cx, "reindex.error"));
}

#[gpui_kit::test]
async fn start_creates_then_exports_then_imports(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    let target = f.target(cx);
    assert_eq!(f.created(), 1);
    assert_eq!(f.data.creates.lock().unwrap()[0]["name"], json!(target));
    assert!(f.calls().contains(&"copy_dir".to_string()));
    assert_eq!(f.step(cx), Step::Exporting(EXPORT));
    let path = f.dir().join("documents.jsonl");
    assert!(f.dir().is_dir());
    assert_eq!(
        f.transfer.exports.lock().unwrap()[0],
        ExportJob {
            collection: "products".into(),
            scope: ExportScope::All,
            fields: FieldPick::All,
            format: ExportFormat::Jsonl,
            target: path.clone(),
            expected: Some(10),
        }
    );
    assert!(f.transfer.imports.lock().unwrap().is_empty());
    f.export_done(cx, 10);
    assert_eq!(f.step(cx), Step::Importing(IMPORT));
    assert_eq!(
        f.transfer.imports.lock().unwrap()[0],
        ImportJob {
            source: Source::File(path),
            options: ReadOptions::new(Format::Jsonl),
            collection: target,
            params: ImportParams {
                action: ImportAction::Create,
                batch_size: 1000,
                dirty_values: DirtyValues::CoerceOrReject,
            },
        }
    );
}

#[gpui_kit::test]
async fn a_failed_create_stays_on_the_first_step(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    *f.data.create_error.lock().unwrap() = Some((409, "taken".into()));
    f.start(cx);
    assert!(f.text(cx, "reindex.error").contains("taken"));
    assert_eq!(f.step(cx), Step::Schema);
    assert!(f.transfer.exports.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn a_short_export_stops_before_the_import(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.export_done(cx, 9);
    assert!(f.transfer.imports.lock().unwrap().is_empty());
    assert_eq!(f.step(cx), Step::Exporting(EXPORT));
    assert!(f.text(cx, "reindex.stopped_detail").contains("9 of 10"));
    assert!(!f.dir().exists());
    assert!(f.has(cx, "reindex.delete_new"));
    assert!(f.has(cx, "reindex.start_over"));
    assert!(!f.has(cx, "reindex.warning"));
}

#[gpui_kit::test]
async fn an_export_that_found_more_warns_and_goes_on(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.export_done(cx, 11);
    assert!(f.has(cx, "reindex.warning"));
    assert_eq!(f.step(cx), Step::Importing(IMPORT));
}

#[gpui_kit::test]
async fn closing_the_tab_keeps_state_and_jobs(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    let panel = f
        .handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let dock = view.read(cx).dock().clone();
            let area = dock.read(cx);
            let center = area.layout(gpui_kit::component::dock::DockPlacement::Center);
            let ids: Vec<_> = center.unwrap().panels().collect();
            ids.into_iter()
                .find_map(|id| {
                    area.panel(id)?
                        .view()
                        .clone()
                        .downcast::<ReindexPanel>()
                        .ok()
                })
                .unwrap()
        })
        .unwrap();
    f.handle
        .update(cx, |root, window, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let dock = view.read(cx).dock().clone();
            dock.update(cx, |area, cx| area.remove_panel(panel, window, cx));
        })
        .unwrap();
    f.settle(cx);
    assert!(!f.has(cx, "reindex.start"));
    f.export_done(cx, 10);
    assert_eq!(f.step(cx), Step::Importing(IMPORT));
    assert!(f.dir().is_dir());
    f.open(cx);
    assert!(f.has(cx, "reindex.job"));
}

#[gpui_kit::test]
async fn stop_during_export_leaves_alias_and_removes_dir(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    assert!(f.has(cx, "reindex.cancel"));
    f.click(cx, "reindex.cancel");
    let tokens = cx.read(|cx| f.jobs.read(cx).jobs()[0].control.clone());
    assert!(tokens.stop.is_cancelled());
    f.transfer
        .finish_export(EXPORT, export_outcome(JobEnd::Stopped, 3, Some(10), 0));
    f.settle(cx);
    assert!(f.text(cx, "reindex.stopped").contains(ALIAS));
    assert!(!f.dir().exists());
    assert!(!f.moved());
    assert!(f.transfer.imports.lock().unwrap().is_empty());
    assert!(f.has(cx, "reindex.delete_new"));
    assert!(f.has(cx, "reindex.start_over"));
}

#[gpui_kit::test]
async fn failed_import_stays_on_copy_with_delete_offer(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.export_done(cx, 10);
    let err = JobEnd::Failed(TransferError::Io("disk full".into()));
    f.transfer
        .finish(IMPORT, import_outcome(err, ImportProgress::default()));
    f.settle(cx);
    assert_eq!(f.step(cx), Step::Importing(IMPORT));
    assert!(f.text(cx, "reindex.stopped_detail").contains("disk full"));
    assert!(!f.dir().exists());
    assert!(!f.moved());
    assert!(!f.has(cx, "reindex.switch"));

    let target = f.target(cx);
    f.click(cx, "reindex.delete_new");
    f.fill(cx, "danger.input", &target);
    f.click(cx, "danger.ok");
    assert_eq!(*f.data.deletes.lock().unwrap(), [target]);
    assert_eq!(f.step(cx), Step::Schema);
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn start_over_takes_a_new_name_and_drops_the_dir(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.transfer.finish_export(
        EXPORT,
        export_outcome(JobEnd::Failed(TransferError::Io("x".into())), 0, None, 0),
    );
    f.settle(cx);
    f.click(cx, "reindex.start_over");
    assert_eq!(f.step(cx), Step::Schema);
    assert!(!f.dir().exists());
    assert!(!f.has(cx, "reindex.stopped"));
}

#[gpui_kit::test]
async fn compare_equal_goes_on(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    assert_eq!(
        f.step(cx),
        Step::Compare(Compared {
            expected: 10,
            target: 10,
            failed: 0,
            unconfirmed: 0,
        })
    );
    assert!(!f.has(cx, "reindex.unequal"));
    assert!(!f.dir().exists());
    assert!(!f.moved());
    f.switch(cx);
    assert!(matches!(f.step(cx), Step::Switched(_)));
    let target = f.target(cx);
    let save = format!("save_alias {ALIAS} {target}");
    let calls = f.calls();
    let at = calls.iter().position(|c| *c == save).unwrap();
    assert_eq!(calls[at + 1], format!("alias {ALIAS}"));
}

/// Unequal counts are not a danger to confirm but a stop: neither alias nor collection moves.
#[gpui_kit::test]
async fn compare_unequal_offers_neither_switch_nor_old_delete(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 9);
    assert!(f.has(cx, "reindex.unequal"));
    f.click(cx, "reindex.switch");
    assert!(!f.has(cx, "confirm.body"));
    assert!(!f.has(cx, "danger.input"));
    assert!(!f.moved());
    assert!(!f.has(cx, "reindex.delete_old"));

    let target = f.target(cx);
    f.set_count(&target, 10);
    f.click(cx, "reindex.compare_again");
    assert!(!f.has(cx, "reindex.unequal"));
}

#[gpui_kit::test]
async fn failed_lines_block_the_switch_though_counts_agree(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.export_done(cx, 10);
    let target = f.target(cx);
    f.set_count(&target, 10);
    f.import_done(cx, IMPORT, 10, 1);
    assert!(f.has(cx, "reindex.unequal"));
    f.click(cx, "reindex.switch");
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn a_count_that_changed_since_the_compare_stops_the_switch(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.set_count("products", 11);
    f.click(cx, "reindex.switch");
    assert!(!f.has(cx, "confirm.body"));
    assert!(f.has(cx, "reindex.error"));
    assert!(f.has(cx, "reindex.unequal"));
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn a_count_that_changes_under_the_open_dialog_stops_the_switch(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.click(cx, "reindex.switch");
    assert!(f.has(cx, "confirm.body"));
    f.set_count("products", 11);
    f.click(cx, "confirm.ok");
    assert!(f.has(cx, "reindex.error"));
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn file_source_compares_with_imported(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let dir = tempfile::tempdir().unwrap();
    pick_file(&f, cx, write_file(dir.path())).await;
    f.start(cx);
    assert_eq!(f.step(cx), Step::Importing(EXPORT));
    let target = f.target(cx);
    f.set_count(&target, 4);
    f.import_done(cx, EXPORT, 4, 0);
    assert_eq!(
        f.step(cx),
        Step::Compare(Compared {
            expected: 4,
            target: 4,
            failed: 0,
            unconfirmed: 0,
        })
    );
    assert!(f.text(cx, "reindex.old_count").contains("10"));
    assert!(!f.has(cx, "reindex.unequal"));
}

#[gpui_kit::test]
async fn alias_moved_stops_the_switch(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    *f.relevance.alias_target.lock().unwrap() = Some("elsewhere".into());
    f.click(cx, "reindex.switch");
    assert!(f.text(cx, "reindex.error").contains("elsewhere"));
    assert!(!f.has(cx, "confirm.body"));
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn alias_moved_under_the_open_dialog_stops_the_switch(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.click(cx, "reindex.switch");
    *f.relevance.alias_target.lock().unwrap() = Some("elsewhere".into());
    f.click(cx, "confirm.ok");
    assert!(f.text(cx, "reindex.error").contains("elsewhere"));
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn a_deleted_alias_refuses_the_start(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.data.data.lock().unwrap().as_mut().unwrap().1.clear();
    f.start(cx);
    assert!(f.text(cx, "reindex.error").contains(ALIAS));
    assert_eq!(f.created(), 0);
    assert_eq!(f.step(cx), Step::Schema);
}

#[gpui_kit::test]
async fn an_alias_deleted_during_the_copy_leaves_a_way_out(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.export_done(cx, 10);
    *f.relevance.alias_deleted.lock().unwrap() = true;
    let target = f.target(cx);
    f.set_count(&target, 10);
    f.import_done(cx, IMPORT, 10, 0);
    assert_eq!(f.step(cx), Step::Importing(IMPORT));
    assert!(f.text(cx, "reindex.stopped").contains(&target));
    assert!(f.text(cx, "reindex.stopped_detail").contains(ALIAS));
    assert!(f.has(cx, "reindex.start_over"));
    assert!(!f.has(cx, "reindex.switch"));

    f.click(cx, "reindex.delete_new");
    f.fill(cx, "danger.input", &target);
    f.click(cx, "danger.ok");
    assert_eq!(*f.data.deletes.lock().unwrap(), [target]);
    assert_eq!(f.step(cx), Step::Schema);
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn a_failed_compare_leaves_a_way_out_and_can_be_repeated(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    f.export_done(cx, 10);
    *f.relevance.fail.lock().unwrap() = Some((503, "not ready".into()));
    let target = f.target(cx);
    f.set_count(&target, 10);
    f.import_done(cx, IMPORT, 10, 0);
    assert!(f.text(cx, "reindex.stopped_detail").contains("not ready"));
    assert!(f.has(cx, "reindex.delete_new"));
    assert!(f.has(cx, "reindex.start_over"));

    *f.relevance.fail.lock().unwrap() = None;
    f.click(cx, "reindex.compare_again");
    assert!(matches!(f.step(cx), Step::Compare(c) if c.equal()));
    assert!(!f.has(cx, "reindex.stopped"));
}

#[gpui_kit::test]
async fn a_new_collection_that_serves_by_now_is_not_deleted(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 9);
    let target = f.target(cx);
    f.click(cx, "reindex.delete_new");
    f.fill(cx, "danger.input", &target);
    *f.relevance.alias_target.lock().unwrap() = Some(target.clone());
    f.click(cx, "danger.ok");
    assert!(f.text(cx, "danger.error").contains(&target));
    assert!(f.data.deletes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn an_old_collection_that_serves_again_is_not_deleted(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    f.click(cx, "reindex.delete_old");
    f.fill(cx, "danger.input", "products");
    *f.relevance.alias_target.lock().unwrap() = Some("products".into());
    f.click(cx, "danger.ok");
    assert!(f.text(cx, "danger.error").contains("products"));
    assert!(f.data.deletes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn an_old_collection_is_kept_when_the_new_one_lost_documents_by_now(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    f.click(cx, "reindex.delete_old");
    f.fill(cx, "danger.input", "products");
    let target = f.target(cx);
    f.set_count(&target, 9);
    f.click(cx, "danger.ok");
    assert!(f.has(cx, "danger.error"));
    assert!(f.data.deletes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn mismatch_after_put_shows(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    *f.relevance.stored_target.lock().unwrap() = Some("other".into());
    f.switch(cx);
    assert!(f.text(cx, "reindex.error").contains("other"));
    assert!(matches!(f.step(cx), Step::Compare(_)));
    assert!(!f.has(cx, "reindex.delete_old"));
}

#[gpui_kit::test]
async fn switch_back_rechecks(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    *f.relevance.alias_target.lock().unwrap() = Some("elsewhere".into());
    f.click(cx, "reindex.switch_back");
    assert!(f.text(cx, "reindex.error").contains("elsewhere"));
    assert!(!f.has(cx, "confirm.body"));
    let back = format!("save_alias {ALIAS} products");
    assert!(!f.calls().contains(&back));

    *f.relevance.alias_target.lock().unwrap() = Some(f.target(cx));
    f.click(cx, "reindex.switch_back");
    assert!(f.text(cx, "confirm.body").contains("products"));
    f.click(cx, "confirm.ok");
    assert!(f.calls().contains(&back));
    assert!(matches!(f.step(cx), Step::Compare(_)));
}

#[gpui_kit::test]
async fn reload_after_the_switch_keeps_the_step(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    let target = f.target(cx);
    f.set_count(&target, 9);
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("cmd-r", "ctrl-r"), cx);
    })
    .unwrap();
    f.settle(cx);
    assert_eq!(
        f.step(cx),
        Step::Switched(Compared {
            expected: 10,
            target: 9,
            failed: 0,
            unconfirmed: 0,
        })
    );
    assert!(f.has(cx, "reindex.switch_back"));
}

#[gpui_kit::test]
async fn referenced_by_is_in_the_consequences(cx: &mut TestAppContext) {
    let orders = Collection {
        name: "orders".into(),
        fields: vec![field("product_id", json!({"reference": "shop.id"}))],
        ..Collection::default()
    };
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![products(), orders], aliases).await;
    assert!(
        f.text(cx, "reindex.note.joins")
            .contains("orders.product_id")
    );
    f.copied(cx, 10);
    f.click(cx, "reindex.switch");
    assert!(f.has(cx, "danger.input"));
    let line = f.text(cx, "danger.consequence.1");
    assert!(line.contains("orders.product_id"), "{line}");
    assert!(f.text(cx, "danger.consequence.0").contains("products"));
    f.fill(cx, "danger.input", ALIAS);
    f.click(cx, "danger.ok");
    assert!(f.moved());
}

#[gpui_kit::test]
async fn prod_switch_is_typed(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![products()], aliases).await;
    f.start(cx);
    f.click(cx, "confirm.ok");
    f.export_done(cx, 10);
    let target = f.target(cx);
    f.set_count(&target, 10);
    f.import_done(cx, IMPORT, 10, 0);
    f.click(cx, "reindex.switch");
    assert!(f.has(cx, "danger.prod"));
    f.click(cx, "danger.cancel");
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn locked_sends_nothing(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let aliases = vec![alias(ALIAS, "products")];
    let f = Fixture::with(cx, profile, vec![products()], aliases).await;
    f.start(cx);
    assert!(f.has(cx, "danger.input"), "the unlock dialog opens");
    assert_eq!(f.created(), 0);
}

#[gpui_kit::test]
async fn broken_alias_disables_reindex(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let aliases = vec![alias(ALIAS, "products"), alias("broken", "gone")];
    let f = Fixture::with(cx, profile, vec![products()], aliases).await;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_resource(f.id, ResourceKind::Aliases, cx));
    });
    f.settle(cx);
    wait_for_element(cx, f.handle, "aliases.new".into(), true).await;
    f.click(cx, "aliases.row.broken");
    assert!(f.has(cx, "aliases.reindex_blocked"));
    let conn = f.conn.upgrade().unwrap();
    let before = cx.read(|cx| conn.read(cx).reindex.len());
    f.click(cx, "aliases.reindex");
    assert_eq!(cx.read(|cx| conn.read(cx).reindex.len()), before);

    f.click(cx, "aliases.row.shop");
    assert!(!f.has(cx, "aliases.reindex_blocked"));
}

#[gpui_kit::test]
async fn delete_old_is_refused_while_the_alias_is_elsewhere(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    *f.relevance.alias_target.lock().unwrap() = Some("products".into());
    f.click(cx, "reindex.delete_old");
    assert!(!f.has(cx, "danger.input"));
    assert!(f.has(cx, "reindex.error"));
    assert!(f.data.deletes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn delete_old_is_refused_when_the_new_collection_lost_documents(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    let target = f.target(cx);
    f.set_count(&target, 9);
    f.click(cx, "reindex.delete_old");
    assert!(!f.has(cx, "danger.input"));
    assert!(f.data.deletes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn delete_old_is_typed_and_ends_the_assistant(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    let reindex = f.reindex(cx).unwrap();
    f.click(cx, "reindex.delete_old");
    assert!(f.text(cx, "danger.consequence.0").contains("products"));
    f.fill(cx, "danger.input", "products");
    f.click(cx, "danger.ok");
    assert_eq!(*f.data.deletes.lock().unwrap(), ["products"]);
    assert!(f.reindex(cx).is_none());
    assert_eq!(cx.read(|cx| reindex.read(cx).step()), Step::Done);
}

#[gpui_kit::test]
async fn done_removes_state(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.copied(cx, 10);
    f.switch(cx);
    f.click(cx, "reindex.keep");
    assert!(f.reindex(cx).is_none());
    assert!(f.data.deletes.lock().unwrap().is_empty());
    assert!(f.has(cx, "reindex.done"));
}

#[gpui_kit::test]
async fn a_disconnect_mid_copy_moves_nothing_and_removes_the_dir(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    let (id, dir) = (f.id, f.dir());
    cx.update(|cx| f.state.update(cx, |s, cx| s.disconnect(id, cx)));
    f.settle(cx);
    assert!(!dir.exists());
    f.transfer
        .finish_export(EXPORT, export_outcome(JobEnd::Stopped, 3, Some(10), 0));
    f.settle(cx);
    assert!(!f.moved());
    assert!(f.data.deletes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn quit_stops_the_job_and_removes_the_dir(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.start(cx);
    let tokens = cx.read(|cx| f.jobs.read(cx).jobs()[0].control.clone());
    let dir = f.dir();
    cx.quit();
    assert!(tokens.stop.is_cancelled());
    assert!(!dir.exists());
    assert!(!f.moved());
}

#[gpui_kit::test]
async fn a_failed_set_list_blocks_the_start(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    *f.relevance.fail_reads.lock().unwrap() = true;
    let reindex = f.reindex(cx).unwrap();
    cx.update(|cx| reindex.update(cx, |r, cx| r.start_over(cx)));
    assert!(f.text(cx, "reindex.error").contains("no list"));
    f.start(cx);
    assert_eq!(f.created(), 0);
}

#[gpui_kit::test]
async fn the_chord_of_the_control_starts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("cmd-enter", "ctrl-enter"), cx);
    })
    .unwrap();
    f.settle(cx);
    assert_eq!(f.created(), 1);
}

#[gpui_kit::test]
async fn the_chord_on_the_aliases_tab_opens_the_assistant(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_resource(f.id, ResourceKind::Aliases, cx));
    });
    wait_for_element(cx, f.handle, "aliases.new".into(), true).await;
    f.click(cx, "aliases.row.shop");
    assert!(!f.has(cx, "reindex.start"));
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("cmd-shift-r", "ctrl-shift-r"), cx);
    })
    .unwrap();
    assert!(f.has(cx, "reindex.start"));
}
