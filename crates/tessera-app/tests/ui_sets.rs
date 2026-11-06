// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the synonym sets view.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::state::ResourceKind;
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::sets::SetsView;
use tessera_app::views::root::RootView;
use tessera_client::{Collection, ItemSet};
use tessera_core::profile::{Environment, Profile};

use common::*;

fn set(name: &str, items: Vec<Value>) -> ItemSet {
    ItemSet {
        name: name.into(),
        items,
    }
}

fn item(id: &str, words: &[&str]) -> Value {
    json!({"id": id, "root": "", "synonyms": words})
}

fn two() -> Vec<ItemSet> {
    vec![set(
        "s",
        vec![item("a", &["x", "y"]), item("b", &["p", "q"])],
    )]
}

struct Fixture {
    handle: WindowHandle<Root>,
    relevance: Arc<FakeRelevance>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, profile: Profile, sets: Vec<ItemSet>) -> Self {
        let mut books = Collection {
            name: "books".into(),
            ..Collection::default()
        };
        books.synonym_sets = vec!["s".into()];
        fake_backend(cx, Some((vec![books], vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let relevance = fake_relevance(cx);
        *relevance.sets.lock().unwrap() = sets;
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
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Synonyms, cx));
        });
        wait_for_element(cx, handle, "synonyms.new".into(), true).await;
        let f = Self {
            handle,
            relevance,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, sets: Vec<ItemSet>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), sets).await
    }

    async fn prod(cx: &mut TestAppContext, sets: Vec<ItemSet>) -> Self {
        let mut profile = Profile::new("live", vec![node()]);
        profile.environment = Environment::Prod;
        Self::with(cx, profile, sets).await
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

    fn label(&self, cx: &mut TestAppContext, id: &str) -> String {
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

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| window.press(keys, cx))
            .unwrap();
        self.settle(cx);
    }

    fn view(&self, cx: &mut TestAppContext) -> Entity<SetsView> {
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
            let ResourceView::Sets(view) = panel.read(cx).view() else {
                panic!("not the sets view");
            };
            view.clone()
        })
    }

    fn set_json(&self, cx: &mut TestAppContext, text: &str) {
        let view = self.view(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            let editor = view.read(cx).json().clone();
            editor.update(cx, |e, cx| e.set_text(text, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn set_words(&self, cx: &mut TestAppContext, text: &str) {
        let view = self.view(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            let dialog = view.read(cx).dialog().unwrap().clone();
            let words = dialog.read(cx).words().clone();
            words.update(cx, |e, cx| e.set_text(text, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn calls(&self) -> Vec<String> {
        self.relevance.calls.lock().unwrap().clone()
    }

    fn open_json(&self, cx: &mut TestAppContext) {
        self.click(cx, "synonyms.row.s");
        self.click(cx, "sets.tab.json");
    }
}

#[gpui_kit::test]
async fn sets_list_with_counts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    assert_eq!(f.label(cx, "synonyms.items.s"), "2");
    assert_eq!(f.label(cx, "synonyms.collections.s"), "books");
    f.click(cx, "synonyms.row.s");
    assert_eq!(f.label(cx, "sets.current"), "s");
    assert_eq!(f.label(cx, "synonyms.items.synonyms.a"), "x, y");
}

#[gpui_kit::test]
async fn an_empty_list_explains_itself(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    assert!(f.has(cx, "synonyms.empty"));
    assert!(f.has(cx, "synonyms.docs"));
}

#[gpui_kit::test]
async fn new_refuses_existing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "s");
    f.click(cx, "resources.name.ok");
    assert!(f.label(cx, "resources.name.error").contains("2 items"));
    assert!(f.calls().is_empty());
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    assert_eq!(f.calls(), ["save_set fresh 0"]);
    assert_eq!(f.label(cx, "sets.current"), "fresh");
}

#[gpui_kit::test]
async fn new_on_prod_confirms(cx: &mut TestAppContext) {
    let f = Fixture::prod(cx, vec![]).await;
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["save_set fresh 0"]);
}

#[gpui_kit::test]
async fn item_save_uses_the_item_endpoint(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.click(cx, "sets.items.add");
    f.fill(cx, "sets.item.id", "c");
    f.set_words(cx, "one\ntwo");
    f.click(cx, "sets.item.save");
    assert_eq!(f.calls(), ["save_item s c"]);
    assert!(!f.has(cx, "sets.item.id"));
    assert_eq!(f.label(cx, "synonyms.items.s"), "3");
}

#[gpui_kit::test]
async fn adding_an_id_that_exists_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.click(cx, "sets.items.add");
    f.fill(cx, "sets.item.id", "a");
    f.set_words(cx, "one\ntwo");
    f.click(cx, "sets.item.save");
    assert!(f.label(cx, "sets.item.error").contains("already exists"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn validation_blocks_save(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.click(cx, "sets.items.add");
    assert!(f.has(cx, "sets.item.issue.0"));
    f.fill(cx, "sets.item.id", "c");
    f.set_words(cx, "one");
    assert!(f.label(cx, "sets.item.issue.0").contains("two words"));
    f.click(cx, "sets.item.save");
    f.press(cx, SAVE);
    assert!(f.calls().is_empty());
    // One word is enough one way.
    f.click(cx, "sets.item.one_way");
    f.fill(cx, "sets.item.root", "r");
    assert!(!f.has(cx, "sets.item.issue.0"));
    f.press(cx, SAVE);
    assert_eq!(f.calls(), ["save_item s c"]);
}

#[gpui_kit::test]
async fn changing_an_id_saves_then_deletes(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.click(cx, "synonyms.items.row.a");
    f.click(cx, "sets.items.edit");
    f.fill(cx, "sets.item.id", "z");
    f.click(cx, "sets.item.save");
    assert_eq!(f.calls(), ["save_item s z", "delete_item s a"]);
}

#[gpui_kit::test]
async fn a_failed_delete_after_a_new_id_says_both_exist(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    *f.relevance.fail_call.lock().unwrap() = Some(("delete_item".into(), "no way".into()));
    f.click(cx, "synonyms.row.s");
    f.click(cx, "synonyms.items.row.a");
    f.click(cx, "sets.items.edit");
    f.fill(cx, "sets.item.id", "z");
    f.click(cx, "sets.item.save");
    let error = f.label(cx, "sets.error");
    assert!(
        error.contains("both exist") && error.contains("no way"),
        "{error}"
    );
}

#[gpui_kit::test]
async fn a_failed_save_stays_in_the_dialog(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    *f.relevance.fail.lock().unwrap() = Some((400, "no way".into()));
    f.click(cx, "synonyms.row.s");
    f.click(cx, "sets.items.add");
    f.fill(cx, "sets.item.id", "c");
    f.set_words(cx, "one\ntwo");
    f.click(cx, "sets.item.save");
    assert!(f.label(cx, "sets.item.error").contains("no way"));
}

#[gpui_kit::test]
async fn dropped_keys_show(cx: &mut TestAppContext) {
    let stored = json!({"id": "a", "root": "", "synonyms": ["x", "y"], "kept": 1, "lost": 2});
    let f = Fixture::new(cx, vec![set("s", vec![stored])]).await;
    *f.relevance.stored_item.lock().unwrap() =
        Some(json!({"id": "a", "root": "", "synonyms": ["x", "y"], "kept": 1}));
    f.click(cx, "synonyms.row.s");
    f.click(cx, "synonyms.items.row.a");
    f.click(cx, "sets.items.edit");
    f.click(cx, "sets.item.save");
    assert!(f.label(cx, "sets.note").contains("lost"));
}

#[gpui_kit::test]
async fn item_delete_confirms(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.click(cx, "synonyms.items.row.a");
    f.click(cx, "sets.items.delete");
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["delete_item s a"]);
    assert_eq!(f.label(cx, "synonyms.items.s"), "1");
}

#[gpui_kit::test]
async fn json_without_removals_saves_at_once(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.open_json(cx);
    f.set_json(
        cx,
        r#"{"items":[{"id":"a","synonyms":["x","y"]},{"id":"b","synonyms":["p","q"]},
            {"id":"c","synonyms":["r","s"]}]}"#,
    );
    f.press(cx, SAVE);
    assert_eq!(f.calls(), ["save_set s 3"]);
    assert!(!f.has(cx, "confirm.body"));
}

#[gpui_kit::test]
async fn json_review_counts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.open_json(cx);
    f.set_json(
        cx,
        r#"{"items":[{"id":"a","synonyms":["x","z"]},{"id":"c","synonyms":["r","s"]}]}"#,
    );
    f.click(cx, "sets.json.save");
    let body = f.label(cx, "confirm.body");
    assert!(body.contains("Adds 1, changes 1 and removes 1"), "{body}");
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["save_set s 2"]);
}

#[gpui_kit::test]
async fn json_count_differences_show(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    *f.relevance.keep_items.lock().unwrap() = Some(1);
    f.open_json(cx);
    f.click(cx, "sets.json.save");
    assert!(f.label(cx, "sets.error").contains("Sent 2"));
}

#[gpui_kit::test]
async fn json_duplicate_ids_are_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.open_json(cx);
    f.set_json(
        cx,
        r#"{"items":[{"id":"a","synonyms":["x","y"]},{"id":"a","synonyms":["p","q"]}]}"#,
    );
    f.click(cx, "sets.json.save");
    let error = f.label(cx, "sets.error");
    assert!(error.contains("Rows 1 and 2"), "{error}");
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn json_removal_on_prod_needs_the_name(cx: &mut TestAppContext) {
    let f = Fixture::prod(cx, two()).await;
    f.open_json(cx);
    f.set_json(cx, r#"{"items":[{"id":"a","synonyms":["x","y"]}]}"#);
    f.click(cx, "sets.json.save");
    assert!(f.label(cx, "danger.consequence.0").contains("removes 1"));
    f.fill(cx, "danger.input", "s");
    f.click(cx, "danger.ok");
    assert_eq!(f.calls(), ["save_set s 1"]);
}

#[gpui_kit::test]
async fn json_on_prod_confirms_without_a_name(cx: &mut TestAppContext) {
    let f = Fixture::prod(cx, two()).await;
    f.open_json(cx);
    f.click(cx, "sets.json.save");
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["save_set s 2"]);
}

#[gpui_kit::test]
async fn unsaved_json_asks_before_another_set_replaces_it(cx: &mut TestAppContext) {
    let mut sets = two();
    sets.push(set("t", vec![item("c", &["m", "n"])]));
    let f = Fixture::new(cx, sets).await;
    f.open_json(cx);
    f.set_json(cx, "{}");
    f.click(cx, "synonyms.row.t");
    f.click(cx, "confirm.cancel");
    assert_eq!(f.label(cx, "sets.current"), "s");
    f.click(cx, "synonyms.row.t");
    f.click(cx, "confirm.ok");
    assert_eq!(f.label(cx, "sets.current"), "t");
}

#[gpui_kit::test]
async fn shortcuts_reach_the_items(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.press(cx, SHOW_FORM);
    f.press(cx, NEW_DOCUMENT);
    assert!(f.has(cx, "sets.item.id"));
    f.click(cx, "sets.item.cancel");
    f.click(cx, "synonyms.items.row.a");
    f.press(cx, SHOW_FORM);
    f.press(cx, DELETE);
    assert!(f.label(cx, "confirm.body").contains("set s"));
}

#[gpui_kit::test]
async fn locked_sends_nothing(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, two()).await;
    f.click(cx, "synonyms.row.s");
    assert!(f.has(cx, "sets.items.add.readonly"));
    f.click(cx, "sets.items.add");
    assert!(f.has(cx, "danger.input"));
    f.press(cx, "escape");
    assert!(f.calls().is_empty());
    f.click(cx, "sets.tab.json");
    f.click(cx, "sets.json.save");
    assert!(f.has(cx, "danger.input"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn cancelling_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::prod(cx, two()).await;
    f.click(cx, "synonyms.row.s");
    f.click(cx, "synonyms.items.row.a");
    f.click(cx, "sets.items.delete");
    f.click(cx, "confirm.cancel");
    f.click(cx, "sets.items.edit");
    f.click(cx, "sets.item.save");
    f.click(cx, "confirm.cancel");
    f.click(cx, "sets.item.cancel");
    f.click(cx, "synonyms.new");
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    f.click(cx, "confirm.cancel");
    assert!(f.calls().is_empty());
}
