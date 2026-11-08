// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the stemming tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use tessera_app::admin::DictionaryFileError;
use tessera_app::state::ResourceKind;
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::stemming::StemmingView;
use tessera_app::views::resources::stemming_dialog::StemmingDialog;
use tessera_app::views::root::RootView;
use tessera_client::{Collection, StemmingDictionary, StemmingWord};
use tessera_core::profile::{Environment, Profile};
use tessera_core::stemming::{BadLine, ImportCheck, LineProblem};

use common::*;

fn word(word: &str, root: &str) -> StemmingWord {
    StemmingWord {
        word: word.into(),
        root: root.into(),
    }
}

fn english() -> StemmingDictionary {
    StemmingDictionary {
        id: "english".into(),
        words: vec![word("mice", "mouse"), word("people", "person")],
    }
}

fn using_english(name: &str) -> Collection {
    serde_json::from_value(json!({
        "name": name,
        "fields": [{"name": "t", "type": "string", "stem_dictionary": "english"}]
    }))
    .unwrap()
}

struct Fixture {
    handle: WindowHandle<Root>,
    admin: Arc<FakeAdmin>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(
        cx: &mut TestAppContext,
        profile: Profile,
        collections: Vec<Collection>,
        dictionaries: Vec<StemmingDictionary>,
    ) -> Self {
        fake_backend(cx, Some((collections, vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let admin = fake_admin(cx);
        *admin.dictionaries.lock().unwrap() = dictionaries;
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
        cx.update(|cx| {
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Stemming, cx));
        });
        wait_for_element(cx, handle, "stemming.reload".into(), true).await;
        let f = Self {
            handle,
            admin,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, dictionaries: Vec<StemmingDictionary>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), vec![], dictionaries).await
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            window.try_find(id.to_string()).is_some()
        })
        .unwrap()
    }

    fn label(&self, cx: &mut TestAppContext, id: &str) -> String {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            let found = window.try_find(id.to_string());
            found.and_then(|e| e.label().map(ToString::to_string))
        })
        .unwrap()
        .unwrap_or_default()
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

    fn view(&self, cx: &mut TestAppContext) -> Entity<StemmingView> {
        let panel = self
            .handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<ResourcePanel>().ok())
                    .next()
                    .unwrap()
            })
            .unwrap();
        cx.update(|cx| {
            let ResourceView::Stemming(view) = panel.read(cx).view() else {
                panic!("not the stemming view");
            };
            view.clone()
        })
    }

    fn dialog(&self, cx: &mut TestAppContext) -> Entity<StemmingDialog> {
        let view = self.view(cx);
        cx.update(|cx| view.read(cx).dialog().unwrap().clone())
    }

    /// The dialog holds a file that reads as `file`.
    fn pick(&self, cx: &mut TestAppContext, file: Result<Vec<StemmingWord>, DictionaryFileError>) {
        *self.admin.file.lock().unwrap() = Some(file);
        let dialog = self.dialog(cx);
        cx.update(|cx| dialog.update(cx, |d, cx| d.read_file(PathBuf::from("words.jsonl"), cx)));
        self.settle(cx);
    }

    fn calls(&self) -> Vec<String> {
        self.admin.calls.lock().unwrap().clone()
    }

    /// Opens the import, with `id` and a good file.
    fn prepare(&self, cx: &mut TestAppContext, id: &str) {
        self.click(cx, "stemming.new");
        self.fill(cx, "stemming.dialog.id", id);
        self.pick(cx, Ok(vec![word("a", "b"), word("c", "d")]));
    }

    fn imported(&self) -> Vec<String> {
        let calls = self.calls();
        calls
            .into_iter()
            .filter(|c| c.starts_with("import"))
            .collect()
    }
}

#[gpui_kit::test]
async fn list_and_words_show(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![english()]).await;
    f.click(cx, "stemming.row.english");
    assert_eq!(f.label(cx, "stemming.words.root.mice"), "mouse");
    assert_eq!(f.label(cx, "stemming.words.root.people"), "person");
    f.fill(cx, "stemming.words.filter", "mou");
    assert!(f.has(cx, "stemming.words.row.mice"));
    assert!(!f.has(cx, "stemming.words.row.people"));
}

#[gpui_kit::test]
async fn file_with_a_bad_line_sends_nothing_and_names_the_line(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.click(cx, "stemming.new");
    f.fill(cx, "stemming.dialog.id", "fresh");
    let bad = BadLine {
        line: 7,
        problem: LineProblem::MissingRoot,
    };
    f.pick(cx, Err(DictionaryFileError::Lines(vec![bad])));
    let text = f.label(cx, "stemming.dialog.lines");
    assert!(text.contains('7') && text.contains("root"), "{text}");
    f.click(cx, "stemming.dialog.import");
    assert!(f.imported().is_empty());
}

#[gpui_kit::test]
async fn too_large_file_says_so(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.click(cx, "stemming.new");
    f.pick(cx, Err(DictionaryFileError::TooLarge(99_000_000)));
    assert!(f.label(cx, "stemming.dialog.file_error").contains("large"));
}

#[gpui_kit::test]
async fn empty_file_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.click(cx, "stemming.new");
    f.fill(cx, "stemming.dialog.id", "fresh");
    f.pick(cx, Ok(vec![]));
    assert!(f.has(cx, "stemming.dialog.file_error"));
    f.click(cx, "stemming.dialog.import");
    assert!(f.imported().is_empty());
}

#[gpui_kit::test]
async fn import_of_a_new_id_asks_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.prepare(cx, "fresh");
    assert!(f.label(cx, "stemming.dialog.words").contains('2'));
    f.click(cx, "stemming.dialog.import");
    assert_eq!(f.imported(), ["import fresh 2"]);
    assert!(!f.has(cx, "stemming.dialog.id"));
    assert!(f.has(cx, "stemming.result"));
    assert!(f.has(cx, "stemming.row.fresh"));
}

#[gpui_kit::test]
async fn import_into_an_existing_id_says_it_merges(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![english()]).await;
    f.click(cx, "stemming.row.english");
    f.click(cx, "stemming.new");
    f.pick(cx, Ok(vec![word("a", "b")]));
    f.click(cx, "stemming.dialog.import");
    let body = f.label(cx, "confirm.body");
    assert!(body.contains("keeps its root"), "{body}");
    assert!(!body.contains("new root"), "{body}");
    assert!(f.imported().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.imported(), ["import english 1"]);
    assert!(!f.has(cx, "stemming.dialog.id"));
    assert!(f.has(cx, "stemming.result"));
}

#[gpui_kit::test]
async fn an_id_that_appeared_after_opening_still_asks(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.prepare(cx, "english");
    f.admin.dictionaries.lock().unwrap().push(english());
    f.click(cx, "stemming.dialog.import");
    assert!(f.label(cx, "confirm.body").contains("keeps its root"));
    assert!(f.imported().is_empty());
}

#[gpui_kit::test]
async fn an_unreadable_list_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![english()]).await;
    f.prepare(cx, "english");
    *f.admin.fail_dictionaries.lock().unwrap() = Some((503, "Not Ready".into()));
    f.click(cx, "stemming.dialog.import");
    assert!(f.label(cx, "stemming.dialog.error").contains("Not Ready"));
    assert!(!f.has(cx, "confirm.ok"));
    assert!(f.imported().is_empty());
    let dialog = f.dialog(cx);
    assert!(cx.read(|cx| dialog.read(cx).can_import(cx)));
}

#[gpui_kit::test]
async fn merge_on_prod_is_typed_and_closes_the_form(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    profile.read_only = false;
    let f = Fixture::with(cx, profile, vec![], vec![english()]).await;
    f.prepare(cx, "english");
    f.click(cx, "stemming.dialog.import");
    f.fill(cx, "danger.input", "english");
    f.click(cx, "danger.ok");
    assert_eq!(f.imported(), ["import english 2"]);
    assert!(!f.has(cx, "danger.input"));
    assert!(!f.has(cx, "stemming.dialog.id"));
}

#[gpui_kit::test]
async fn failed_merge_shows_on_the_form_and_keeps_the_file(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![english()]).await;
    f.prepare(cx, "english");
    *f.admin.fail_import.lock().unwrap() = Some(tessera_client::Error::Timeout);
    f.click(cx, "stemming.dialog.import");
    f.click(cx, "confirm.ok");
    assert!(
        f.label(cx, "stemming.dialog.error")
            .contains("may still be applying")
    );
    let dialog = f.dialog(cx);
    assert!(cx.read(|cx| dialog.read(cx).can_import(cx)));
}

#[gpui_kit::test]
async fn a_timeout_says_the_server_may_still_be_working(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.prepare(cx, "fresh");
    *f.admin.fail_import.lock().unwrap() = Some(tessera_client::Error::Timeout);
    f.click(cx, "stemming.dialog.import");
    assert!(
        f.label(cx, "stemming.dialog.error")
            .contains("may still be applying")
    );
}

#[gpui_kit::test]
async fn enter_twice_imports_once(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.prepare(cx, "fresh");
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(SAVE, cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    f.settle(cx);
    assert_eq!(f.imported(), ["import fresh 2"]);
}

#[gpui_kit::test]
async fn cancel_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![english()]).await;
    f.prepare(cx, "english");
    f.click(cx, "stemming.dialog.import");
    f.click(cx, "confirm.cancel");
    assert!(f.imported().is_empty());
    assert!(f.has(cx, "stemming.dialog.id"));
    let dialog = f.dialog(cx);
    assert!(cx.read(|cx| dialog.read(cx).can_import(cx)));
}

#[gpui_kit::test]
async fn result_states_kept_roots(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.admin.check.lock().unwrap() = Some(ImportCheck {
        sent: 2,
        missing: vec![],
        kept_root: 1,
    });
    f.prepare(cx, "fresh");
    f.click(cx, "stemming.dialog.import");
    let text = f.label(cx, "stemming.result");
    assert!(text.contains("2 words sent"), "{text}");
    assert!(text.contains("1 existing words kept their root"), "{text}");
    assert!(text.contains("delete"), "{text}");
    assert!(!f.has(cx, "stemming.missing"));
}

#[gpui_kit::test]
async fn missing_word_warns(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    let missing: Vec<String> = (0..12).map(|i| format!("w{i}")).collect();
    *f.admin.check.lock().unwrap() = Some(ImportCheck {
        sent: 12,
        missing,
        kept_root: 0,
    });
    f.prepare(cx, "fresh");
    f.click(cx, "stemming.dialog.import");
    let text = f.label(cx, "stemming.missing");
    assert!(text.contains("12") && text.contains("w9"), "{text}");
    assert!(!text.contains("w10"), "{text}");
}

#[gpui_kit::test]
async fn delete_names_collections(cx: &mut TestAppContext) {
    let collections = vec![using_english("a"), using_english("b")];
    let profile = Profile::new("test", vec![node()]);
    let f = Fixture::with(cx, profile, collections, vec![english()]).await;
    f.click(cx, "stemming.row.english");
    f.click(cx, "stemming.delete");
    let line = f.label(cx, "danger.consequence.1");
    assert!(line.contains("a, b"), "{line}");
}

#[gpui_kit::test]
async fn delete_is_typed_when_used(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let f = Fixture::with(cx, profile, vec![using_english("a")], vec![english()]).await;
    f.click(cx, "stemming.row.english");
    f.click(cx, "stemming.delete");
    assert!(f.has(cx, "danger.input"));
    f.fill(cx, "danger.input", "english");
    f.click(cx, "danger.ok");
    assert!(f.calls().contains(&"delete_dictionary english".to_string()));
}

#[gpui_kit::test]
async fn delete_is_plain_when_unused_and_typed_on_prod(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![english()]).await;
    f.click(cx, "stemming.row.english");
    f.click(cx, "stemming.delete");
    assert!(!f.has(cx, "danger.input"));
    f.click(cx, "confirm.cancel");
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    profile.read_only = false;
    let f = Fixture::with(cx, profile, vec![], vec![english()]).await;
    f.click(cx, "stemming.row.english");
    f.click(cx, "stemming.delete");
    assert!(f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn read_only_import_asks_to_unlock(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![], vec![]).await;
    f.prepare(cx, "fresh");
    f.click(cx, "stemming.dialog.import");
    assert!(f.has(cx, "danger.input"));
    assert!(f.imported().is_empty());
}

#[gpui_kit::test]
async fn bad_name_keeps_import_disabled(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.prepare(cx, "a b");
    let dialog = f.dialog(cx);
    assert!(!cx.read(|cx| dialog.read(cx).can_import(cx)));
    f.fill(cx, "stemming.dialog.id", "fine");
    assert!(cx.read(|cx| dialog.read(cx).can_import(cx)));
}

#[gpui_kit::test]
async fn escape_waits_for_an_import_so_its_failure_is_seen(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    let (release, hold) = tokio::sync::oneshot::channel();
    *f.admin.import_hold.lock().unwrap() = Some(hold);
    *f.admin.fail_import.lock().unwrap() = Some(tessera_client::Error::Timeout);
    f.prepare(cx, "fresh");
    f.click(cx, "stemming.dialog.import");
    cx.update_window(f.handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    assert!(
        f.has(cx, "stemming.dialog.id"),
        "the form stays while it runs"
    );
    release.send(()).unwrap();
    f.settle(cx);
    assert!(f.has(cx, "stemming.dialog.error"));
}
