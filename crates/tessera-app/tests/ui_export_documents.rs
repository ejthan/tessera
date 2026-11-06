// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the export dialog, through fake document and transfer backends.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use tessera_app::connection::ConnectionState;
use tessera_app::jobs::{Jobs, JobsGlobal};
use tessera_client::Collection;
use tessera_core::transfer::export::{ExportFormat, ExportScope, FieldPick};

use common::*;

const EXPORT: &str = key("cmd-shift-x", "ctrl-shift-x");

struct Fixture {
    handle: WindowHandle<Root>,
    docs: Arc<FakeDocs>,
    fake: Arc<FakeTransfer>,
    jobs: Entity<Jobs>,
    conn: Entity<ConnectionState>,
    _rt: tokio::runtime::Runtime,
}

fn products(names: &[&str]) -> Collection {
    let fields: Vec<_> = names
        .iter()
        .map(|n| json!({"name": n, "type": "string"}))
        .collect();
    serde_json::from_value(json!({"name": "products", "fields": fields})).unwrap()
}

async fn open_with(cx: &mut TestAppContext, schema: Collection, prod: bool) -> Fixture {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 7;
    fake_backend(cx, Some((vec![schema], Vec::new())));
    fake_connector(cx, |p| Ok(server_for(p)));
    let fake = fake_transfer(cx);
    let (staging, prod_profile, secrets) = two_profiles();
    let profile = if prod { prod_profile } else { staging };
    let (handle, state, rt) = open_app(cx, vec![profile.clone()], secrets);
    let id = profile.id;
    cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
    cx.condition(&state, |s, cx| {
        s.connection(id, cx)
            .is_some_and(|c| c.read(cx).connected.is_some())
    })
    .await;
    cx.update(|cx| {
        state.update(cx, |s, cx| s.open_collection(id, "products".into(), cx));
    });
    wait_for_element(cx, handle, format!("tab.{id}.collection.products"), true).await;
    let conn = cx.read(|cx| state.read(cx).connection(id, cx).unwrap());
    let jobs = cx.read(|cx| cx.global::<JobsGlobal>().0.clone());
    let f = Fixture {
        handle,
        docs,
        fake,
        jobs,
        conn,
        _rt: rt,
    };
    f.settle(cx);
    f
}

async fn open(cx: &mut TestAppContext) -> Fixture {
    open_with(cx, products(&["title", "price"]), false).await
}

impl Fixture {
    fn act<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
    ) -> R {
        let out = cx
            .update_window(self.handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let out = f(window, cx);
                window.render_frame(cx);
                out
            })
            .unwrap();
        cx.run_until_parked();
        out
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        self.act(cx, |_, _| {});
    }

    /// Lets `ms` of the dialog's wait pass.
    fn wait(&self, cx: &mut TestAppContext, ms: u64) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
        self.act(cx, |_, _| {});
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.act(cx, |window, cx| window.click(id, cx));
    }

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        let keys = keys.to_string();
        self.act(cx, |window, cx| window.press(&keys, cx));
        self.settle(cx);
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.try_find(id).is_some())
    }

    fn text(&self, cx: &mut TestAppContext, id: &str) -> String {
        let id = id.to_string();
        self.act(cx, |window, _| {
            let found = window.try_find(id);
            found
                .and_then(|e| e.label().map(str::to_string))
                .unwrap_or_default()
        })
    }

    fn disabled(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.find(id).disabled() == Some(true))
    }

    fn counts(&self) -> Vec<Option<String>> {
        self.docs.count_calls.lock().unwrap().clone()
    }

    fn job_count(&self, cx: &TestAppContext) -> usize {
        cx.read(|cx| self.jobs.read(cx).jobs().len())
    }

    /// Opens the dialog with the shortcut and lets its first count run.
    fn open_dialog(&self, cx: &mut TestAppContext) {
        self.press(cx, EXPORT);
        self.wait(cx, 300);
    }

    /// Types into the filter bar of the table and applies it.
    fn apply_table_filter(&self, cx: &mut TestAppContext, text: &str) {
        let panel = collection_panel(cx, self.handle, "products");
        let filter = cx.update(|cx| panel.read(cx).documents().read(cx).filter().clone());
        self.act(cx, |window, cx| {
            filter.update(cx, |f, cx| f.focus(window, cx));
            window.input(text, cx);
            window.press("enter", cx);
        });
        self.settle(cx);
    }

    fn tick_row(&self, cx: &mut TestAppContext, id: &str) {
        self.click(cx, &format!("documents.check.{id}"));
        self.settle(cx);
    }

    /// Answers the save dialog with `path`.
    fn save_to(&self, cx: &mut TestAppContext, path: Option<std::path::PathBuf>) {
        cx.simulate_new_path_selection(move |_| path);
        cx.run_until_parked();
        self.settle(cx);
    }
}

#[gpui_kit::test]
async fn opens_on_all_otherwise(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    assert!(!f.has(cx, "transfer.export.filter"));
    assert_eq!(f.text(cx, "transfer.export.count"), "Exports 7 documents");
}

#[gpui_kit::test]
async fn opens_on_filter_when_the_table_has_one(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.apply_table_filter(cx, "price:>1");
    f.open_dialog(cx);
    assert!(f.has(cx, "transfer.export.filter"));
    assert_eq!(f.counts().last().unwrap().as_deref(), Some("price:>1"));
}

#[gpui_kit::test]
async fn opens_on_selection_when_there_is_one(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.tick_row(cx, "1");
    f.tick_row(cx, "2");
    f.open_dialog(cx);
    assert_eq!(f.text(cx, "transfer.export.count"), "2 selected");
    assert!(f.counts().is_empty(), "a selection needs no request");
}

#[gpui_kit::test]
async fn selection_scope_is_disabled_without_one(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    assert!(f.disabled(cx, "transfer.export.scope.selection"));
    assert!(!f.disabled(cx, "transfer.export.scope.filter"));
}

#[gpui_kit::test]
async fn count_runs_after_a_pause(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.press(cx, EXPORT);
    assert!(f.text(cx, "transfer.export.count").contains("Counting"));
    f.wait(cx, 100);
    assert!(f.counts().is_empty());
    f.wait(cx, 100);
    assert_eq!(f.counts(), [None]);
    assert_eq!(f.text(cx, "transfer.export.count"), "Exports 7 documents");
    f.click(cx, "transfer.export.scope.filter");
    f.act(cx, |window, cx| window.input("price:>1", cx));
    f.wait(cx, 300);
    assert_eq!(f.counts().last().unwrap().as_deref(), Some("price:>1"));
}

#[gpui_kit::test]
async fn stale_count_is_dropped(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.scope.filter");
    f.act(cx, |window, cx| window.input("price:>1", cx));
    let hold = f.docs.hold_next();
    f.wait(cx, 300);
    *f.docs.found.lock().unwrap() = 3;
    f.act(cx, |window, cx| window.input("0", cx));
    f.wait(cx, 300);
    assert_eq!(f.text(cx, "transfer.export.count"), "Exports 3 documents");
    let _ = hold.send(());
    f.settle(cx);
    assert_eq!(f.text(cx, "transfer.export.count"), "Exports 3 documents");
}

#[gpui_kit::test]
async fn bad_filter_shows_inline_and_disables_start(cx: &mut TestAppContext) {
    let f = open(cx).await;
    *f.docs.fail.lock().unwrap() = Some((400, "Could not parse the filter query.".into()));
    f.open_dialog(cx);
    assert!(
        f.text(cx, "transfer.export.error")
            .contains("Could not parse")
    );
    assert!(f.disabled(cx, "transfer.export.start"));
}

#[gpui_kit::test]
async fn zero_count_allows_start(cx: &mut TestAppContext) {
    let f = open(cx).await;
    *f.docs.found.lock().unwrap() = 0;
    f.open_dialog(cx);
    assert_eq!(f.text(cx, "transfer.export.count"), "Exports 0 documents");
    assert!(!f.disabled(cx, "transfer.export.start"));
}

#[gpui_kit::test]
async fn fields_list_the_top_level_names(cx: &mut TestAppContext) {
    let f = open_with(cx, products(&["title", "meta.a", "meta.b", ".*"]), false).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.fields.include");
    for name in ["id", "title", "meta"] {
        assert!(
            f.has(cx, &format!("transfer.export.field.{name}")),
            "{name}"
        );
    }
    assert!(!f.has(cx, "transfer.export.field.meta.a"));
}

#[gpui_kit::test]
async fn unsafe_field_is_disabled_with_the_reason(cx: &mut TestAppContext) {
    let f = open_with(cx, products(&["title", "a,b"]), false).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.fields.include");
    assert!(f.disabled(cx, "transfer.export.field.a,b"));
    assert!(!f.disabled(cx, "transfer.export.field.title"));
    assert!(f.text(cx, "transfer.export.field_unsafe").contains("comma"));
}

#[gpui_kit::test]
async fn include_sends_include_fields(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.fields.include");
    f.click(cx, "transfer.export.field.price");
    f.click(cx, "transfer.export.field.id");
    f.click(cx, "transfer.export.start");
    let dir = tempfile::tempdir().unwrap();
    f.save_to(cx, Some(dir.path().join("out.jsonl")));
    let jobs = f.fake.exports.lock().unwrap();
    assert_eq!(
        jobs[0].fields,
        FieldPick::Include(vec!["id".into(), "price".into()])
    );
}

#[gpui_kit::test]
async fn exclude_sends_exclude_fields(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.fields.exclude");
    f.click(cx, "transfer.export.field.title");
    f.click(cx, "transfer.export.start");
    let dir = tempfile::tempdir().unwrap();
    f.save_to(cx, Some(dir.path().join("out.jsonl")));
    let jobs = f.fake.exports.lock().unwrap();
    assert_eq!(jobs[0].fields, FieldPick::Exclude(vec!["title".into()]));
}

#[gpui_kit::test]
async fn empty_pick_disables_start(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.fields.include");
    assert!(f.disabled(cx, "transfer.export.start"));
    assert_eq!(
        f.text(cx, "transfer.export.empty_pick"),
        "Pick at least one field."
    );
    f.click(cx, "transfer.export.field.title");
    assert!(!f.disabled(cx, "transfer.export.start"));
    assert!(!f.has(cx, "transfer.export.empty_pick"));
}

#[gpui_kit::test]
async fn include_without_id_notes_it(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.fields.include");
    f.click(cx, "transfer.export.field.title");
    assert!(f.has(cx, "transfer.export.no_id_note"));
    f.click(cx, "transfer.export.field.id");
    assert!(!f.has(cx, "transfer.export.no_id_note"));
}

#[gpui_kit::test]
async fn csv_shows_its_notes(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    assert!(!f.has(cx, "transfer.export.csv_note"));
    f.click(cx, "transfer.export.format.csv");
    assert!(
        f.text(cx, "transfer.export.csv_note")
            .contains("first 1,000")
    );
    assert!(
        f.text(cx, "transfer.export.csv_formula_note")
            .contains("formula")
    );
}

#[gpui_kit::test]
async fn start_asks_for_a_path_then_runs(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.format.csv");
    f.click(cx, "transfer.export.start");
    assert_eq!(f.job_count(cx), 0, "nothing runs before the path");
    let path = tempfile::tempdir().unwrap().path().join("out.csv");
    f.save_to(cx, Some(path.clone()));
    assert_eq!(f.job_count(cx), 1);
    {
        let jobs = f.fake.exports.lock().unwrap();
        assert_eq!(jobs[0].target, path);
        assert_eq!(jobs[0].format, ExportFormat::Csv);
        assert_eq!(jobs[0].scope, ExportScope::All);
        assert_eq!(jobs[0].collection, "products");
    }
    wait_for_element(cx, f.handle, "transfer.export.start".into(), false).await;
    assert!(f.has(cx, "jobs.panel"));
}

#[gpui_kit::test]
async fn cancelled_path_changes_nothing(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    f.click(cx, "transfer.export.start");
    f.save_to(cx, None);
    assert_eq!(f.job_count(cx), 0);
    assert!(f.has(cx, "transfer.export.start"));
    assert!(!f.has(cx, "transfer.export.error"));
}

#[gpui_kit::test]
async fn job_has_the_expected_count(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.tick_row(cx, "1");
    f.open_dialog(cx);
    f.click(cx, "transfer.export.start");
    let dir = tempfile::tempdir().unwrap();
    f.save_to(cx, Some(dir.path().join("a.jsonl")));
    f.open_dialog(cx);
    f.click(cx, "transfer.export.scope.all");
    f.wait(cx, 300);
    f.click(cx, "transfer.export.start");
    f.save_to(cx, Some(dir.path().join("b.jsonl")));
    let jobs = f.fake.exports.lock().unwrap();
    assert_eq!(jobs[0].scope, ExportScope::Selection(vec!["1".into()]));
    assert_eq!(jobs[0].expected, Some(1));
    assert_eq!(jobs[1].expected, Some(7));
}

#[gpui_kit::test]
async fn works_on_a_locked_profile(cx: &mut TestAppContext) {
    let f = open_with(cx, products(&["title"]), true).await;
    assert!(cx.read(|cx| f.conn.read(cx).writes_locked()));
    f.open_dialog(cx);
    f.click(cx, "transfer.export.start");
    let dir = tempfile::tempdir().unwrap();
    f.save_to(cx, Some(dir.path().join("out.jsonl")));
    assert_eq!(f.job_count(cx), 1, "no unlock was asked for");
}

#[gpui_kit::test]
async fn shows_connection_and_environment(cx: &mut TestAppContext) {
    let f = open_with(cx, products(&["title"]), true).await;
    f.open_dialog(cx);
    assert_eq!(f.text(cx, "transfer.export.connection"), "prod-ch");
}

#[gpui_kit::test]
async fn every_control_shows_its_shortcut(cx: &mut TestAppContext) {
    let f = open(cx).await;
    f.open_dialog(cx);
    for (id, chord) in [
        (
            "documents.export",
            key("kbd:cmd-shift-x", "kbd:ctrl-shift-x"),
        ),
        (
            "transfer.export.start",
            key("kbd:cmd-enter", "kbd:ctrl-enter"),
        ),
        ("transfer.export.cancel", "kbd:escape"),
    ] {
        let button = f.act(cx, |window, _| window.find(id).bounds());
        let kbd = gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
            .debug_bounds(chord)
            .unwrap();
        assert!(button.contains(&kbd.center()), "{id}");
    }
}
