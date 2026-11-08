// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the presets view and of preset mode in the workbench.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::state::{AppState, ResourceKind};
use tessera_app::views::dock::SearchPanel;
use tessera_app::views::root::RootView;
use tessera_app::views::search::SearchWorkbench;
use tessera_client::{Collection, Preset};
use tessera_core::profile::{Environment, Profile};
use tessera_core::search::params::SearchDraft;

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    relevance: Arc<FakeRelevance>,
    search: Arc<FakeSearch>,
    _state: Entity<AppState>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, profile: Profile, presets: &[(&str, Value)]) -> Self {
        let books = Collection {
            name: "books".into(),
            ..Collection::default()
        };
        fake_backend(cx, Some((vec![books], vec![])));
        fake_documents(cx);
        let search = install_search_fake(cx);
        *search.presets.lock().unwrap() = presets
            .iter()
            .map(|(name, value)| Preset {
                name: (*name).to_string(),
                value: value.clone(),
            })
            .collect();
        let relevance = fake_relevance(cx);
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
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Presets, cx));
        });
        wait_for_element(cx, handle, "presets.new".into(), true).await;
        let f = Self {
            handle,
            relevance,
            search,
            _state: state,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, presets: &[(&str, Value)]) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), presets).await
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

    fn workbench(&self, cx: &mut TestAppContext) -> Entity<SearchWorkbench> {
        let panel = self
            .handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<SearchPanel>().ok())
                    .next()
                    .unwrap()
            })
            .unwrap();
        cx.update(|cx| panel.read(cx).workbench().clone())
    }

    fn mode(&self, cx: &mut TestAppContext) -> Option<String> {
        let workbench = self.workbench(cx);
        cx.update(|cx| workbench.read(cx).preset().map(str::to_string))
    }

    fn saved(&self) -> Vec<(String, Value)> {
        self.search.saved_presets.lock().unwrap().clone()
    }

    fn calls(&self) -> Vec<String> {
        self.relevance.calls.lock().unwrap().clone()
    }
}

#[gpui_kit::test]
async fn kinds_show(cx: &mut TestAppContext) {
    let f = Fixture::new(
        cx,
        &[
            ("one", json!({"q": "dune", "query_by": "title"})),
            (
                "many",
                json!({"searches": [{"collection": "a"}, {"collection": "b"}]}),
            ),
            ("all", json!({"searches": [{}, {}], "union": true})),
        ],
    )
    .await;
    assert_eq!(f.label(cx, "presets.kind.one"), "Search");
    assert_eq!(f.label(cx, "presets.summary.one"), "dune, title");
    assert_eq!(f.label(cx, "presets.kind.many"), "Multi-search of 2");
    assert_eq!(f.label(cx, "presets.summary.many"), "a, b");
    assert_eq!(f.label(cx, "presets.kind.all"), "Union of 2");
}

#[gpui_kit::test]
async fn invalid_cannot_open_but_deletes(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("bad", json!("str"))]).await;
    assert!(f.label(cx, "presets.kind.bad").contains("object"));
    f.click(cx, "presets.row.bad");
    f.press(cx, EDIT);
    f.click(cx, "presets.open");
    assert!(!f.has(cx, "search.run"));
    f.press(cx, DELETE);
    assert!(f.label(cx, "confirm.body").contains("silently lose"));
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["delete_preset bad"]);
}

#[gpui_kit::test]
async fn delete_on_prod_needs_the_name(cx: &mut TestAppContext) {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, &[("one", json!({"q": "a"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, DELETE);
    assert!(f.has(cx, "danger.prod"));
    f.fill(cx, "danger.input", "one");
    f.click(cx, "danger.ok");
    assert_eq!(f.calls(), ["delete_preset one"]);
}

#[gpui_kit::test]
async fn open_loads_drafts_in_preset_mode(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("one", json!({"q": "dune"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, EDIT);
    assert_eq!(f.mode(cx).as_deref(), Some("one"));
    assert!(f.label(cx, "presets.band").contains("Preset one"));
    let workbench = f.workbench(cx);
    let draft = cx.update(|cx| workbench.read(cx).drafts()[0].clone());
    assert_eq!(draft.collection, "books");
    assert_eq!(draft.params["q"], "dune");
    assert!(f.saved().is_empty());
}

#[gpui_kit::test]
async fn save_writes_to_that_name(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("one", json!({"q": "dune"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, EDIT);
    f.press(cx, SAVE_QUERY);
    let saved = f.saved();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, "one");
    assert_eq!(saved[0].1["q"], "dune");
    assert!(!f.has(cx, "search.save.name"));
}

#[gpui_kit::test]
async fn save_on_prod_confirms(cx: &mut TestAppContext) {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, &[("one", json!({"q": "dune"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, EDIT);
    f.click(cx, "presets.save");
    assert!(f.saved().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.saved().len(), 1);
}

#[gpui_kit::test]
async fn save_strips_secrets(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("one", json!({"q": "a", "x_api_key": "hush"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, EDIT);
    f.click(cx, "presets.save");
    let saved = f.saved();
    assert!(saved[0].1.get("x_api_key").is_none(), "{saved:?}");
}

#[gpui_kit::test]
async fn new_sends_nothing_before_save(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("one", json!({"q": "a"}))]).await;
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "one");
    f.click(cx, "resources.name.ok");
    assert!(f.label(cx, "resources.name.error").contains("one"));
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    assert_eq!(f.mode(cx).as_deref(), Some("fresh"));
    assert!(f.saved().is_empty());
    let workbench = f.workbench(cx);
    let draft = cx.update(|cx| workbench.read(cx).drafts()[0].clone());
    assert_eq!(draft, SearchDraft::new("books"));
    f.press(cx, SAVE_QUERY);
    assert_eq!(f.saved()[0].0, "fresh");
}

#[gpui_kit::test]
async fn load_from_history_leaves_the_mode(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("one", json!({"q": "a"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, EDIT);
    let workbench = f.workbench(cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        workbench.update(cx, |w, cx| {
            w.load(vec![SearchDraft::new("books")], false, window, cx);
        });
    })
    .unwrap();
    assert_eq!(f.mode(cx), None);
    assert!(!f.has(cx, "presets.band"));
}

#[gpui_kit::test]
async fn leave_keeps_the_drafts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, &[("one", json!({"q": "dune"}))]).await;
    f.click(cx, "presets.row.one");
    f.press(cx, EDIT);
    f.click(cx, "presets.leave");
    assert_eq!(f.mode(cx), None);
    let workbench = f.workbench(cx);
    let draft = cx.update(|cx| workbench.read(cx).drafts()[0].clone());
    assert_eq!(draft.params["q"], "dune");
    f.press(cx, SAVE_QUERY);
    assert!(f.has(cx, "search.save.name"));
}
