// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the import dialog, through a fake transfer backend.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, ClipboardItem, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::connection::ConnectionState;
use tessera_app::jobs::{Jobs, JobsGlobal};
use tessera_app::state::AppState;
use tessera_app::views::jobs::import_dialog::{DocumentImportDialog, Target};
use tessera_client::{Collection, DirtyValues, ImportAction};
use tessera_core::documents::validate::{DocIssue, IssueSeverity};
use tessera_core::profile::Profile;
use tessera_core::schema::{FieldDraft, FieldType};
use tessera_core::transfer::CLIPBOARD_LIMIT;
use tessera_core::transfer::infer::{InferNote, InferredSchema};
use tessera_core::transfer::preview::{Preview, PreviewError, PreviewRow};
use tessera_core::transfer::source::{Format, ReadError, Source};

use common::*;

const CHOOSE: &str = key("cmd-o", "ctrl-o");
const PASTE: &str = key("cmd-shift-v", "ctrl-shift-v");
const IMPORT: &str = key("cmd-shift-i", "ctrl-shift-i");
const START: &str = key("cmd-enter", "ctrl-enter");

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    profile: Profile,
    fake: Arc<FakeTransfer>,
    backend: Arc<FakeData>,
    jobs: Entity<Jobs>,
    conn: Entity<ConnectionState>,
    _rt: tokio::runtime::Runtime,
}

fn books() -> Collection {
    serde_json::from_value(serde_json::json!({
        "name": "books",
        "num_documents": 3,
        "fields": [{"name": "title", "type": "string"}]
    }))
    .unwrap()
}

fn row(line: u64) -> PreviewRow {
    PreviewRow {
        line,
        text: format!("{{\"id\":\"{line}\"}}"),
        issues: Vec::new(),
        error: None,
    }
}

fn preview(estimated_docs: u64) -> Preview {
    Preview {
        rows: vec![row(1), row(2)],
        complete: estimated_docs == 2,
        estimated_docs,
        total_bytes: Some(20),
    }
}

async fn connected_as(cx: &mut TestAppContext, prod: bool) -> Fixture {
    let backend = fake_backend(cx, Some((vec![books()], Vec::new())));
    fake_connector(cx, |p| Ok(server_for(p)));
    let fake = fake_transfer(cx);
    *fake.preview.lock().unwrap() = Some(Ok(preview(2)));
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
    let conn = cx.read(|cx| state.read(cx).connection(id, cx).unwrap());
    let jobs = cx.read(|cx| cx.global::<JobsGlobal>().0.clone());
    Fixture {
        handle,
        state,
        profile,
        fake,
        backend,
        jobs,
        conn,
        _rt: rt,
    }
}

async fn connected(cx: &mut TestAppContext) -> Fixture {
    connected_as(cx, false).await
}

impl Fixture {
    fn open(
        &self,
        cx: &mut TestAppContext,
        target: Option<&str>,
        source: Option<Source>,
    ) -> Entity<DocumentImportDialog> {
        let (state, conn) = (self.state.clone(), self.conn.clone());
        let target = target.map(str::to_string);
        let dialog = cx
            .update_window(self.handle.into(), |_, window, cx| {
                window.render_frame(cx);
                DocumentImportDialog::open(state, conn, target, source, window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        // The first frame builds the dialog, which takes focus for the next one.
        self.act(cx, |_, _| ());
        dialog
    }

    fn open_bytes(&self, cx: &mut TestAppContext, text: &str) -> Entity<DocumentImportDialog> {
        let source = Source::Bytes(Arc::from(text.as_bytes()));
        let dialog = self.open(cx, Some("books"), Some(source));
        settle(cx);
        dialog
    }

    fn act<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
    ) -> R {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            f(window, cx)
        })
        .unwrap()
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.act(cx, |window, cx| window.click(id, cx));
        cx.run_until_parked();
    }

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        self.act(cx, |window, cx| window.press(keys, cx));
        cx.run_until_parked();
    }

    fn text(&self, cx: &mut TestAppContext, id: &str) -> Option<String> {
        let id = id.to_string();
        self.act(cx, |window, _| {
            window
                .try_find(id)
                .and_then(|e| e.label().map(str::to_string))
        })
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.try_find(id).is_some())
    }

    fn disabled(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.find(id).disabled() == Some(true))
    }

    fn start_disabled(&self, cx: &mut TestAppContext) -> bool {
        self.disabled(cx, "transfer.import.start")
    }

    fn previews(&self) -> usize {
        self.fake.previews.lock().unwrap().len()
    }

    fn job_count(&self, cx: &TestAppContext) -> usize {
        cx.read(|cx| self.jobs.read(cx).jobs().len())
    }

    /// Waits for real file reads on tokio's blocking pool, then for the preview.
    async fn wait_for_source(&self, cx: &mut TestAppContext) {
        cx.wait_for(
            self.handle.into(),
            Duration::from_millis(5000),
            |window, _| {
                let source = window.try_find("transfer.import.source");
                let shown = source.is_some_and(|e| e.label().is_some_and(|l| !l.is_empty()));
                shown || window.try_find("transfer.import.source_error").is_some()
            },
        )
        .await;
        settle(cx);
    }
}

/// Lets the preview's delay pass.
fn settle(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
}

fn temp_file(name: &str, text: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, text).unwrap();
    (dir, path)
}

#[gpui_kit::test]
async fn opens_with_the_collection_as_target(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    let previews = f.fake.previews.lock().unwrap();
    assert_eq!(previews[0].2.as_ref().unwrap().name, "books");
    assert_eq!(previews[0].1.fields, [("title".into(), "string".into())]);
}

#[gpui_kit::test]
async fn shortcut_opens_it_from_documents_and_collections(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.profile.id;
    cx.update(|cx| f.state.update(cx, |s, cx| s.open_collections(id, cx)));
    cx.run_until_parked();
    f.press(cx, IMPORT);
    assert!(f.has(cx, "transfer.import.start"));
    f.click(cx, "transfer.import.cancel");
    assert!(!f.has(cx, "transfer.import.start"));

    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "books".into(), cx))
    });
    cx.run_until_parked();
    f.press(cx, IMPORT);
    assert!(f.has(cx, "transfer.import.start"));
    f.press(cx, PASTE);
    let clipboard = ClipboardItem::new_string("{}\n".into());
    cx.write_to_clipboard(clipboard);
    f.press(cx, PASTE);
    settle(cx);
    let previews = f.fake.previews.lock().unwrap();
    assert_eq!(previews[0].2.as_ref().unwrap().name, "books", "preset");
}

#[gpui_kit::test]
async fn shows_connection_and_environment(cx: &mut TestAppContext) {
    let f = connected_as(cx, true).await;
    f.open(cx, None, None);
    assert_eq!(
        f.text(cx, "transfer.import.connection").as_deref(),
        Some("prod-ch")
    );
    assert_eq!(f.text(cx, "transfer.import.env").as_deref(), Some("PROD"));
}

#[gpui_kit::test]
async fn choosing_a_file_sniffs_and_previews(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, path) = temp_file("books.tsv", b"id\ttitle\n1\ta\n");
    f.open(cx, Some("books"), None);
    f.press(cx, CHOOSE);
    let picked = path.clone();
    cx.simulate_path_prompt_response(|_| Some(vec![picked]));
    f.wait_for_source(cx).await;
    let shown = f.text(cx, "transfer.import.source").unwrap();
    assert!(shown.contains("books.tsv"), "{shown}");
    let previews = f.fake.previews.lock().unwrap();
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].0, Source::File(path));
    assert_eq!(previews[0].1.format, Format::Csv);
    assert_eq!(previews[0].1.csv.delimiter, b'\t');
}

#[gpui_kit::test]
async fn csv_options_show_only_for_csv(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    assert!(!f.has(cx, "transfer.import.delimiter.comma"));
    assert!(!f.has(cx, "transfer.import.header"));
    f.click(cx, "transfer.import.format.csv");
    assert!(f.has(cx, "transfer.import.delimiter.comma"));
    assert!(f.has(cx, "transfer.import.header"));
}

#[gpui_kit::test]
async fn format_override_reruns_the_preview(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    assert_eq!(f.previews(), 1);
    f.click(cx, "transfer.import.format.json");
    settle(cx);
    assert_eq!(f.previews(), 2);
    assert_eq!(
        f.fake.previews.lock().unwrap()[1].1.format,
        Format::JsonArray
    );
}

#[gpui_kit::test]
async fn stale_preview_is_dropped(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    // The first answer never comes while the second is asked for.
    *f.fake.preview.lock().unwrap() = None;
    f.open_bytes(cx, "{}\n");
    assert_eq!(f.previews(), 1);
    *f.fake.preview.lock().unwrap() = Some(Ok(preview(77_000)));
    f.click(cx, "transfer.import.format.json");
    f.click(cx, "transfer.import.format.csv");
    settle(cx);
    assert_eq!(f.previews(), 2, "a change within the delay asks nothing");
    let title = f.text(cx, "transfer.import.preview.title").unwrap();
    assert!(title.contains("77,000"), "{title}");
}

#[gpui_kit::test]
async fn preview_lists_issues_and_rejections(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let mut answer = preview(2);
    answer.rows[0].issues.push(DocIssue {
        path: "n".into(),
        key: "document.wrong_type",
        severity: IssueSeverity::Error,
        detail: "int32".into(),
    });
    answer.rows[1].error = Some(PreviewError::InvalidUtf8);
    *f.fake.preview.lock().unwrap() = Some(Ok(answer));
    f.open_bytes(cx, "{}\n");
    assert!(f.has(cx, "transfer.import.preview.row.0"));
    let issue = f.text(cx, "transfer.import.preview.issue.0").unwrap();
    assert!(issue.contains("n: Expected type int32"), "{issue}");
    let rejected = f.text(cx, "transfer.import.preview.issue.1").unwrap();
    assert!(rejected.contains("Not valid UTF-8"), "{rejected}");
}

#[gpui_kit::test]
async fn read_error_is_inline_and_disables_start(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    *f.fake.preview.lock().unwrap() = Some(Err(ReadError::Csv {
        line: 3,
        message: "repeated header".into(),
    }));
    f.open_bytes(cx, "{}\n");
    let error = f.text(cx, "transfer.import.preview.error").unwrap();
    assert!(error.contains("line 3"), "{error}");
    assert!(error.contains("repeated header"), "{error}");
    assert!(f.start_disabled(cx));
}

#[gpui_kit::test]
async fn paste_uses_the_clipboard(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open(cx, Some("books"), None);
    cx.write_to_clipboard(ClipboardItem::new_string("[{\"a\":1}]".into()));
    f.press(cx, PASTE);
    settle(cx);
    let shown = f.text(cx, "transfer.import.source").unwrap();
    assert!(shown.starts_with("Clipboard"), "{shown}");
    {
        let previews = f.fake.previews.lock().unwrap();
        assert_eq!(previews[0].0, Source::Bytes(Arc::from(&b"[{\"a\":1}]"[..])));
        assert_eq!(previews[0].1.format, Format::JsonArray);
    }
    cx.write_to_clipboard(ClipboardItem::new_string("x".repeat(CLIPBOARD_LIMIT + 1)));
    f.press(cx, PASTE);
    let error = f.text(cx, "transfer.import.source_error").unwrap();
    assert!(error.contains("Use a file above 16 MiB"), "{error}");
    assert!(f.start_disabled(cx));
}

#[gpui_kit::test]
async fn empty_clipboard_says_so(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open(cx, Some("books"), None);
    f.click(cx, "transfer.import.paste");
    assert!(f.has(cx, "transfer.import.source_error"));
}

#[gpui_kit::test]
async fn dropped_file_opens_the_dialog(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, path) = temp_file("drop.jsonl", b"{}\n");
    let id = f.profile.id;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "books".into(), cx))
    });
    cx.run_until_parked();
    let bounds = f.act(cx, |window, _| window.find("documents.import").bounds());
    drop_file(cx, f.handle, bounds.center(), path.clone());
    f.wait_for_source(cx).await;
    assert_eq!(f.fake.previews.lock().unwrap()[0].0, Source::File(path));
}

/// Drops `path` as the platform would.
fn drop_file(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    position: gpui_kit::Point<gpui_kit::Pixels>,
    path: PathBuf,
) {
    use gpui_kit::{FileDropEvent, PlatformInput};
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Entered {
                position,
                paths: gpui_kit::ExternalPaths(vec![path].into()),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Submit { position }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn folder_is_refused(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let dir = tempfile::tempdir().unwrap();
    f.open(cx, Some("books"), Some(Source::File(dir.path().into())));
    f.wait_for_source(cx).await;
    let error = f.text(cx, "transfer.import.source_error").unwrap();
    assert!(error.contains("not a folder"), "{error}");
    assert_eq!(f.previews(), 0);
}

#[gpui_kit::test]
async fn batch_size_bounds(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    assert!(!f.start_disabled(cx));
    for (text, ok) in [
        ("0", false),
        ("100001", false),
        ("x", false),
        ("100000", true),
    ] {
        f.act(cx, |window, cx| {
            fill(window, "transfer.import.batch", text, cx)
        });
        cx.run_until_parked();
        assert_eq!(!f.has(cx, "transfer.import.batch_error"), ok, "{text}");
        assert_eq!(!f.start_disabled(cx), ok, "{text}");
    }
}

#[gpui_kit::test]
async fn start_runs_the_job_and_closes(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    f.click(cx, "transfer.import.start");
    wait_for_element(cx, f.handle, "transfer.import.start".into(), false).await;
    assert!(f.has(cx, "jobs.panel"));
    assert_eq!(f.job_count(cx), 1);
    assert_eq!(f.fake.imports.lock().unwrap()[0].collection, "books");
}

#[gpui_kit::test]
async fn job_gets_the_chosen_options(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    f.click(cx, "transfer.import.format.csv");
    f.click(cx, "transfer.import.delimiter.pipe");
    f.click(cx, "transfer.import.header");
    f.click(cx, "transfer.import.action.emplace");
    f.click(cx, "transfer.import.dirty.drop");
    f.act(cx, |window, cx| {
        fill(window, "transfer.import.batch", "250", cx)
    });
    settle(cx);
    f.click(cx, "transfer.import.start");
    // emplace on a small local import asks first.
    if f.has(cx, "confirm.ok") {
        f.click(cx, "confirm.ok");
    }
    let imports = f.fake.imports.lock().unwrap();
    let job = &imports[0];
    assert_eq!(job.params.action, ImportAction::Emplace);
    assert_eq!(job.params.dirty_values, DirtyValues::Drop);
    assert_eq!(job.params.batch_size, 250);
    assert_eq!(job.options.format, Format::Csv);
    assert_eq!(job.options.csv.delimiter, b'|');
    assert!(!job.options.csv.header);
}

#[gpui_kit::test]
async fn locked_profile_starts_nothing_until_unlocked(cx: &mut TestAppContext) {
    let f = connected_as(cx, true).await;
    f.open_bytes(cx, "{}\n");
    assert!(
        f.has(cx, "transfer.import.start.readonly"),
        "Start shows the lock"
    );
    assert!(f.has(cx, "transfer.import.locked"), "the dialog says so");
    f.press(cx, SAVE);
    assert_eq!(f.job_count(cx), 0);
    assert!(f.has(cx, "danger.input"), "asks to unlock");
    assert_eq!(f.job_count(cx), 0);
}

#[gpui_kit::test]
async fn cancel_changes_nothing(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    f.click(cx, "transfer.import.cancel");
    wait_for_element(cx, f.handle, "transfer.import.start".into(), false).await;
    assert_eq!(f.job_count(cx), 0);
    assert!(f.backend.creates.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn every_control_shows_its_shortcut(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.open_bytes(cx, "{}\n");
    for (id, chord) in [
        ("transfer.import.choose", key("kbd:cmd-o", "kbd:ctrl-o")),
        (
            "transfer.import.paste",
            key("kbd:cmd-shift-v", "kbd:ctrl-shift-v"),
        ),
        (
            "transfer.import.start",
            key("kbd:cmd-enter", "kbd:ctrl-enter"),
        ),
    ] {
        let button = f.act(cx, |window, _| window.find(id).bounds());
        let kbd = gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
            .debug_bounds(chord)
            .unwrap();
        assert!(button.contains(&kbd.center()), "{id}");
    }
}

/// A schema proposal with `n` and a field that may be a `geopoint`.
fn inferred() -> InferredSchema {
    InferredSchema {
        fields: vec![
            FieldDraft::new("title", FieldType::String),
            FieldDraft::new("place", FieldType::FloatArray),
        ],
        nested: false,
        notes: vec![
            InferNote::MaybeGeopoint("place".into()),
            InferNote::MixedTypes("title".into()),
        ],
        sampled: 42,
    }
}

impl Fixture {
    /// A dialog on `path`'s name with `count` estimated documents.
    fn start_dialog(&self, cx: &mut TestAppContext, count: u64) -> Entity<DocumentImportDialog> {
        *self.fake.preview.lock().unwrap() = Some(Ok(preview(count)));
        self.open_bytes(cx, "{}\n")
    }

    /// The dialog with "New collection…" chosen on a file named `catalog (1).jsonl`.
    async fn new_target(
        &self,
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<DocumentImportDialog>) {
        *self.fake.infer.lock().unwrap() = Some(Ok(inferred()));
        let (dir, path) = temp_file("catalog (1).jsonl", b"{}\n");
        let dialog = self.open(cx, None, Some(Source::File(path)));
        self.wait_for_source(cx).await;
        self.update(cx, &dialog, |d, window, cx| d.pick_new(window, cx));
        settle(cx);
        (dir, dialog)
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        dialog: &Entity<DocumentImportDialog>,
        f: impl FnOnce(
            &mut DocumentImportDialog,
            &mut gpui_kit::Window,
            &mut gpui_kit::Context<DocumentImportDialog>,
        ) -> R,
    ) -> R {
        let dialog = dialog.clone();
        let result = self.act(cx, |window, cx| dialog.update(cx, |d, cx| f(d, window, cx)));
        cx.run_until_parked();
        result
    }

    fn set_action(&self, cx: &mut TestAppContext, action: &str) {
        self.click(cx, &format!("transfer.import.action.{action}"));
    }

    fn started(&self) -> usize {
        self.fake.imports.lock().unwrap().len()
    }

    fn start(&self, cx: &mut TestAppContext) {
        self.click(cx, "transfer.import.start");
        settle(cx);
    }

    fn unlock(&self, cx: &mut TestAppContext) {
        cx.update(|cx| self.conn.update(cx, |c, cx| c.unlock(cx)));
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
async fn create_on_local_starts_without_a_dialog(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start_dialog(cx, 5000);
    f.start(cx);
    assert_eq!(f.started(), 1);
}

#[gpui_kit::test]
async fn create_on_prod_confirms_with_focus_on_cancel(cx: &mut TestAppContext) {
    let f = connected_as(cx, true).await;
    f.unlock(cx);
    f.start_dialog(cx, 5);
    f.start(cx);
    assert_eq!(f.started(), 0);
    assert_eq!(
        f.act(cx, |window, _| window.find("confirm.cancel").focused()),
        Some(true)
    );
    f.click(cx, "confirm.ok");
    settle(cx);
    assert_eq!(f.started(), 1);
    wait_for_element(cx, f.handle, "transfer.import.start".into(), false).await;
}

#[gpui_kit::test]
async fn upsert_under_1000_confirms(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start_dialog(cx, 1000);
    f.set_action(cx, "upsert");
    f.start(cx);
    assert!(f.has(cx, "confirm.ok"));
    assert!(!f.has(cx, "danger.input"));
    assert_eq!(f.started(), 0);
    f.click(cx, "confirm.ok");
    settle(cx);
    assert_eq!(f.started(), 1);
}

#[gpui_kit::test]
async fn upsert_over_1000_types_the_name(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start_dialog(cx, 1001);
    f.set_action(cx, "upsert");
    f.start(cx);
    assert!(f.has(cx, "danger.input"));
    assert_eq!(f.started(), 0);
    f.act(cx, |window, cx| {
        fill(window, "danger.input", "books", cx);
        window.click("danger.ok".to_string(), cx);
    });
    settle(cx);
    assert_eq!(f.started(), 1);
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    wait_for_element(cx, f.handle, "transfer.import.start".into(), false).await;
}

#[gpui_kit::test]
async fn update_on_prod_types_the_name(cx: &mut TestAppContext) {
    let f = connected_as(cx, true).await;
    f.unlock(cx);
    f.start_dialog(cx, 5);
    f.set_action(cx, "update");
    f.start(cx);
    assert!(f.has(cx, "danger.input"));
    assert_eq!(f.started(), 0);
}

#[gpui_kit::test]
async fn new_collection_on_local_starts_without_a_dialog(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.start(cx);
    assert!(!f.has(cx, "confirm.ok"));
    assert_eq!(f.started(), 1);
}

#[gpui_kit::test]
async fn new_collection_on_prod_confirms(cx: &mut TestAppContext) {
    let f = connected_as(cx, true).await;
    f.unlock(cx);
    let _new = f.new_target(cx).await;
    f.start(cx);
    assert!(f.has(cx, "confirm.ok"));
    let body = f.text(cx, "confirm.body").unwrap();
    assert!(
        body.contains("Creates the collection catalog__1_"),
        "{body}"
    );
    assert!(f.backend.creates.lock().unwrap().is_empty());
    f.click(cx, "confirm.ok");
    settle(cx);
    assert_eq!(f.backend.creates.lock().unwrap().len(), 1);
    assert_eq!(f.started(), 1);
}

#[gpui_kit::test]
async fn cancelling_the_confirmation_sends_nothing(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start_dialog(cx, 10);
    f.set_action(cx, "upsert");
    f.start(cx);
    f.click(cx, "confirm.cancel");
    settle(cx);
    assert_eq!(f.started(), 0);
    assert!(f.has(cx, "transfer.import.start"), "the dialog stays");
}

#[gpui_kit::test]
async fn consequences_name_count_and_collection(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start_dialog(cx, 10);
    f.set_action(cx, "upsert");
    f.start(cx);
    let body = f.text(cx, "confirm.body").unwrap();
    assert!(body.contains("Imports into books on staging-eu."), "{body}");
    assert!(
        body.contains("From the clipboard, about 10 documents."),
        "{body}"
    );
    assert!(body.contains("same id are replaced"), "{body}");
    assert!(body.contains("already imported stay"), "{body}");
}

#[gpui_kit::test]
async fn new_shows_the_inferred_fields_and_notes(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    let fields = f.text(cx, "transfer.infer.fields").unwrap();
    assert!(fields.contains("title"), "{fields}");
    assert!(fields.contains("place"), "{fields}");
    let note = f.text(cx, "transfer.infer.note.1").unwrap();
    assert!(note.contains("title"), "{note}");
    assert!(f.text(cx, "transfer.infer.sampled").unwrap().contains("42"));
}

#[gpui_kit::test]
async fn new_proposes_a_safe_name(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, dialog) = f.new_target(cx).await;
    assert_eq!(cx.read(|cx| dialog.read(cx).new_name(cx)), "catalog__1_");
    f.act(cx, |window, cx| {
        fill(window, "transfer.import.new_name", "", cx)
    });
    cx.run_until_parked();
    assert!(f.has(cx, "transfer.import.name_error"));
    assert!(f.start_disabled(cx));
}

#[gpui_kit::test]
async fn update_is_disabled_for_new(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let dialog = f.start_dialog(cx, 10);
    f.set_action(cx, "update");
    *f.fake.infer.lock().unwrap() = Some(Ok(inferred()));
    f.update(cx, &dialog, |d, window, cx| d.pick_new(window, cx));
    settle(cx);
    assert!(f.disabled(cx, "transfer.import.action.update"));
    assert!(f.disabled(cx, "transfer.import.action.emplace"));
    assert!(f.has(cx, "transfer.import.no_update_new"));
    f.start(cx);
    assert_eq!(
        f.fake.imports.lock().unwrap()[0].params.action,
        ImportAction::Create
    );
}

#[gpui_kit::test]
async fn nothing_is_created_before_start(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.click(cx, "transfer.import.edit_schema");
    f.click(cx, "schema.submit");
    settle(cx);
    f.click(cx, "transfer.import.cancel");
    settle(cx);
    assert!(f.backend.creates.lock().unwrap().is_empty());
    assert_eq!(f.started(), 0);
}

#[gpui_kit::test]
async fn start_creates_then_imports(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.start(cx);
    let creates = f.backend.creates.lock().unwrap();
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0]["name"], "catalog__1_");
    assert_eq!(creates[0]["fields"][0]["name"], "title");
    assert_eq!(f.fake.imports.lock().unwrap()[0].collection, "catalog__1_");
}

#[gpui_kit::test]
async fn create_failure_stays_in_the_dialog(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    *f.backend.create_error.lock().unwrap() = Some((409, "already exists".into()));
    f.start(cx);
    let error = f.text(cx, "transfer.import.create_error").unwrap();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(f.started(), 0);
    assert!(f.has(cx, "transfer.import.start"));
}

#[gpui_kit::test]
async fn use_geopoint_changes_the_draft(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    let before = f.previews();
    f.click(cx, "transfer.infer.use_geopoint.place");
    settle(cx);
    assert!(!f.has(cx, "transfer.infer.use_geopoint.place"));
    assert_eq!(f.previews(), before + 1);
    let previews = f.fake.previews.lock().unwrap();
    let schema = previews.last().unwrap().2.as_ref().unwrap();
    let place = schema.fields.iter().find(|x| x.name == "place").unwrap();
    assert_eq!(place.r#type, "geopoint");
}

#[gpui_kit::test]
async fn edit_schema_opens_pick_mode(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.click(cx, "transfer.import.edit_schema");
    assert_eq!(
        f.act(cx, |window, _| window
            .find("schema.submit")
            .label()
            .map(str::to_string))
            .as_deref(),
        Some("Use schema")
    );
}

#[gpui_kit::test]
async fn pick_mode_sends_nothing(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, dialog) = f.new_target(cx).await;
    f.click(cx, "transfer.import.edit_schema");
    f.act(cx, |window, cx| fill(window, "schema.name", "renamed", cx));
    cx.run_until_parked();
    f.click(cx, "schema.submit");
    settle(cx);
    assert!(!f.has(cx, "schema.submit"), "closed");
    assert!(f.backend.creates.lock().unwrap().is_empty());
    assert_eq!(cx.read(|cx| dialog.read(cx).new_name(cx)), "renamed");
}

#[gpui_kit::test]
async fn escape_closes_only_the_schema_dialog(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.click(cx, "transfer.import.edit_schema");
    f.press(cx, "escape");
    settle(cx);
    assert!(!f.has(cx, "schema.submit"));
    assert!(f.has(cx, "transfer.import.start"));
}

#[gpui_kit::test]
async fn invalid_draft_cannot_be_picked(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.click(cx, "transfer.import.edit_schema");
    f.act(cx, |window, cx| fill(window, "schema.name", "", cx));
    cx.run_until_parked();
    assert!(f.disabled(cx, "schema.submit"));
    f.click(cx, "schema.submit");
    assert!(f.has(cx, "schema.submit"), "stays open");
}

#[gpui_kit::test]
async fn read_only_profile_can_edit_schema_without_unlocking(cx: &mut TestAppContext) {
    let f = connected_as(cx, true).await;
    let _new = f.new_target(cx).await;
    f.click(cx, "transfer.import.edit_schema");
    f.click(cx, "schema.submit");
    settle(cx);
    assert!(!f.has(cx, "schema.submit"));
    assert!(!f.has(cx, "danger.input"));
    assert!(f.has(cx, "transfer.import.start"));
}

impl Fixture {
    /// Loads the list again, now answering with `data`.
    fn reload(
        &self,
        cx: &mut TestAppContext,
        data: Option<(Vec<Collection>, Vec<tessera_client::Alias>)>,
    ) {
        *self.backend.data.lock().unwrap() = data;
        cx.update(|cx| self.conn.update(cx, |c, cx| c.invalidate_collections(cx)));
        cx.run_until_parked();
    }

    /// Chooses "New collection…" and tells what the select then holds.
    fn select_new(
        &self,
        cx: &mut TestAppContext,
        dialog: &Entity<DocumentImportDialog>,
    ) -> Option<Target> {
        self.update(cx, dialog, |d, window, cx| d.pick_new(window, cx));
        cx.read(|cx| dialog.read(cx).chosen(cx))
    }
}

#[gpui_kit::test]
async fn empty_server_offers_a_new_collection(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.reload(cx, Some((Vec::new(), Vec::new())));
    let dialog = f.open(cx, None, None);
    assert_eq!(f.select_new(cx, &dialog), Some(Target::New));
}

#[gpui_kit::test]
async fn failed_list_says_so_and_offers_a_new_collection(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.reload(cx, None);
    let dialog = f.open(cx, None, None);
    cx.run_until_parked();
    let text = f.text(cx, "transfer.import.list_error").unwrap();
    assert!(text.contains("could not be loaded"), "{text}");
    assert_eq!(f.select_new(cx, &dialog), Some(Target::New));
}

#[gpui_kit::test]
async fn preset_target_survives_a_list_still_loading(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    cx.update(|cx| f.conn.update(cx, |c, cx| c.invalidate_collections(cx)));
    let dialog = f.open(
        cx,
        Some("books"),
        Some(Source::Bytes(Arc::from(&b"{}\n"[..]))),
    );
    settle(cx);
    assert!(!f.start_disabled(cx));
    cx.update(|cx| f.conn.update(cx, |c, cx| c.invalidate_collections(cx)));
    settle(cx);
    assert_eq!(
        cx.read(|cx| dialog.read(cx).chosen(cx)),
        Some(Target::Collection("books".into()))
    );
}

#[gpui_kit::test]
async fn second_start_during_a_create_is_ignored(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    let release = f.backend.hold_create();
    f.press(cx, START);
    f.press(cx, START);
    assert_eq!(f.backend.creates.lock().unwrap().len(), 1);
    release.send(()).unwrap();
    settle(cx);
    assert_eq!(f.backend.creates.lock().unwrap().len(), 1);
    assert_eq!(f.started(), 1);
}

#[gpui_kit::test]
async fn created_collection_reaches_the_list(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let _new = f.new_target(cx).await;
    f.start(cx);
    let names: Vec<String> = cx.read(|cx| {
        let cache = &f.conn.read(cx).collection_cache;
        cache
            .collections
            .iter()
            .flatten()
            .map(|c| c.name.clone())
            .collect()
    });
    assert!(names.contains(&"catalog__1_".to_string()), "{names:?}");
}

#[gpui_kit::test]
async fn changed_file_reruns_the_estimate_and_the_danger(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, path) = temp_file("books.jsonl", b"{}\n");
    f.open(cx, Some("books"), Some(Source::File(path.clone())));
    f.wait_for_source(cx).await;
    f.set_action(cx, "upsert");
    std::fs::write(&path, "{}\n".repeat(5000)).unwrap();
    *f.fake.preview.lock().unwrap() = Some(Ok(preview(5000)));
    let previews = f.previews();
    f.click(cx, "transfer.import.start");
    cx.wait_for(f.handle.into(), Duration::from_millis(5000), |window, _| {
        window.try_find("transfer.import.file_changed").is_some()
    })
    .await;
    settle(cx);
    assert_eq!(f.started(), 0);
    assert!(!f.has(cx, "confirm.ok"));
    assert_eq!(f.previews(), previews + 1);
    let shown = f.text(cx, "transfer.import.source").unwrap();
    assert!(shown.contains("14.6 KB"), "the new size: {shown}");
    f.click(cx, "transfer.import.start");
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    assert!(!f.has(cx, "transfer.import.file_changed"));
}

#[gpui_kit::test]
async fn file_gone_at_start_says_so(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, path) = temp_file("books.jsonl", b"{}\n");
    f.open(cx, Some("books"), Some(Source::File(path.clone())));
    f.wait_for_source(cx).await;
    std::fs::remove_file(&path).unwrap();
    f.start(cx);
    assert!(f.has(cx, "transfer.import.source_error"));
    assert_eq!(f.started(), 0);
}

#[gpui_kit::test]
async fn paste_cancels_a_file_still_being_read(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let (_dir, path) = temp_file("books.jsonl", b"{}\n");
    let dialog = f.open(cx, Some("books"), None);
    cx.write_to_clipboard(ClipboardItem::new_string("{}\n".into()));
    f.act(cx, |window, cx| {
        dialog.update(cx, |d, cx| d.read_file(path, cx));
        window.press(PASTE, cx);
    });
    std::thread::sleep(Duration::from_millis(100));
    settle(cx);
    let shown = f.text(cx, "transfer.import.source").unwrap();
    assert!(shown.starts_with("Clipboard"), "{shown}");
}
