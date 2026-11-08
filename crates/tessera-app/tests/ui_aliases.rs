// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the aliases view.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::state::{AppState, ResourceKind};
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::aliases::AliasDialog;
use tessera_app::views::root::RootView;
use tessera_client::{Alias, Collection};
use tessera_core::profile::{Environment, Profile};

use common::*;

const OK: &str = "books";

fn collection(name: &str, documents: u64) -> Collection {
    Collection {
        name: name.into(),
        num_documents: documents,
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
    relevance: Arc<FakeRelevance>,
    data: Arc<FakeData>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, profile: Profile, aliases: Vec<Alias>) -> Self {
        let collections = vec![collection(OK, 10), collection("films", 25)];
        let data = fake_backend(cx, Some((collections, aliases)));
        fake_documents(cx);
        install_search_fake(cx);
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
        let f = Self {
            handle,
            state,
            relevance,
            data,
            _rt: rt,
        };
        cx.update(|cx| {
            f.state
                .update(cx, |s, cx| s.open_resource(id, ResourceKind::Aliases, cx));
        });
        wait_for_element(cx, handle, "aliases.new".into(), true).await;
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, aliases: Vec<Alias>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), aliases).await
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
            window
                .find(id.to_string())
                .label()
                .unwrap_or_default()
                .to_string()
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

    fn calls(&self) -> Vec<String> {
        self.relevance.calls.lock().unwrap().clone()
    }

    /// The open dialog.
    fn dialog(&self, cx: &mut TestAppContext) -> Entity<AliasDialog> {
        let panels = self.handle.update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let area = view.read(cx).dock().read(cx);
            area.layout(DockPlacement::Center)
                .unwrap()
                .panels()
                .filter_map(|id| area.panel(id)?.view().downcast::<ResourcePanel>().ok())
                .next()
                .unwrap()
        });
        cx.update(|cx| {
            let ResourceView::Aliases(view) = panels.unwrap().read(cx).view() else {
                panic!("not the aliases view");
            };
            view.read(cx).dialog().unwrap()
        })
    }

    fn pick(&self, cx: &mut TestAppContext, target: &str) {
        let dialog = self.dialog(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            dialog.update(cx, |d, cx| d.pick_target(target, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    /// Opens New for `name` on `target` and saves.
    fn create(&self, cx: &mut TestAppContext, name: &str, target: &str) {
        self.press(cx, NEW_DOCUMENT);
        self.fill(cx, "aliases.dialog.name", name);
        self.pick(cx, target);
        self.click(cx, "aliases.dialog.save");
    }

    /// Selects the alias and opens Repoint on `target`, then saves.
    fn repoint(&self, cx: &mut TestAppContext, name: &str, target: &str) {
        self.click(cx, &format!("aliases.row.{name}"));
        self.press(cx, EDIT);
        self.pick(cx, target);
        self.click(cx, "aliases.dialog.save");
    }

    fn unlock(&self, cx: &mut TestAppContext, name: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.click("danger.input".to_string(), cx);
            window.input(name, cx);
            window.render_frame(cx);
            window.click("danger.ok".to_string(), cx);
        })
        .unwrap();
        self.settle(cx);
    }
}

#[gpui_kit::test]
async fn states_show_per_row(cx: &mut TestAppContext) {
    let f = Fixture::new(
        cx,
        vec![
            alias("good", OK),
            alias("gone", "nothing"),
            alias("chain", "good"),
            alias("empty", ""),
            alias(OK, "films"),
        ],
    )
    .await;
    assert_eq!(f.text(cx, "aliases.state.good"), "");
    assert!(f.text(cx, "aliases.state.gone").contains("does not exist"));
    assert!(
        f.text(cx, "aliases.state.chain")
            .contains("does not resolve")
    );
    assert!(f.text(cx, "aliases.state.empty").contains("No target"));
    assert!(
        f.text(cx, &format!("aliases.state.{OK}"))
            .contains("Hidden")
    );
}

#[gpui_kit::test]
async fn new_creates_and_reloads(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    assert!(f.has(cx, "aliases.empty"));
    let lists = *f.data.lists.lock().unwrap();
    f.create(cx, "shop", OK);
    assert_eq!(f.calls(), ["save_alias shop books"]);
    assert!(!f.has(cx, "aliases.dialog.name"));
    assert!(*f.data.lists.lock().unwrap() > lists + 1);
}

#[gpui_kit::test]
async fn new_refuses_an_existing_alias(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    f.create(cx, "shop", "films");
    assert!(f.text(cx, "aliases.dialog.error").contains(OK));
    assert!(f.has(cx, "aliases.dialog.repoint"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn new_refuses_a_collection_name(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.create(cx, "films", OK);
    assert!(f.text(cx, "aliases.dialog.error").contains("collection"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn new_refuses_a_bad_name(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.create(cx, "a b", OK);
    assert!(f.has(cx, "aliases.dialog.error"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn new_on_prod_confirms(cx: &mut TestAppContext) {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, vec![]).await;
    f.create(cx, "shop", OK);
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["save_alias shop books"]);
}

#[gpui_kit::test]
async fn locked_profile_sends_nothing_until_unlocked(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![alias("shop", OK)]).await;
    assert!(f.has(cx, "aliases.new.readonly"));
    f.press(cx, NEW_DOCUMENT);
    assert!(f.has(cx, "danger.input"));
    assert!(!f.has(cx, "aliases.dialog.name"));
    assert!(f.calls().is_empty());
    f.unlock(cx, "test");
    assert!(f.has(cx, "aliases.dialog.name"));
}

#[gpui_kit::test]
async fn repoint_confirms_and_saves(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    f.repoint(cx, "shop", "films");
    let body = f.text(cx, "confirm.body");
    assert!(body.contains("books (10 documents)"), "{body}");
    assert!(body.contains("films (25 documents)"), "{body}");
    assert!(body.contains("change at once"), "{body}");
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["save_alias shop films"]);
}

#[gpui_kit::test]
async fn repoint_on_prod_needs_the_name(cx: &mut TestAppContext) {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, vec![alias("shop", OK)]).await;
    f.repoint(cx, "shop", "films");
    assert!(f.has(cx, "danger.prod"));
    f.fill(cx, "danger.input", "wrong");
    f.click(cx, "danger.ok");
    assert!(f.calls().is_empty());
    f.fill(cx, "danger.input", "shop");
    f.click(cx, "danger.ok");
    assert_eq!(f.calls(), ["save_alias shop films"]);
}

#[gpui_kit::test]
async fn cancelling_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    f.repoint(cx, "shop", "films");
    f.click(cx, "confirm.cancel");
    f.press(cx, DELETE);
    f.click(cx, "confirm.cancel");
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn mismatch_in_the_answer_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    *f.relevance.stored_target.lock().unwrap() = Some("elsewhere".into());
    f.repoint(cx, "shop", "films");
    f.click(cx, "confirm.ok");
    assert!(f.text(cx, "aliases.error").contains("elsewhere"));
}

#[gpui_kit::test]
async fn a_failed_new_stays_in_the_dialog(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.relevance.fail.lock().unwrap() = Some((500, "no way".into()));
    f.create(cx, "shop", OK);
    assert!(f.text(cx, "aliases.dialog.error").contains("no way"));
}

#[gpui_kit::test]
async fn delete_confirms_and_says_what_it_breaks(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    f.click(cx, "aliases.row.shop");
    f.press(cx, DELETE);
    assert!(f.text(cx, "confirm.body").contains("404"));
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["delete_alias shop"]);
}

#[gpui_kit::test]
async fn delete_of_a_gone_alias_reloads(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    *f.relevance.fail.lock().unwrap() = Some((404, "Collection not found".into()));
    f.click(cx, "aliases.row.shop");
    f.press(cx, DELETE);
    let lists = *f.data.lists.lock().unwrap();
    f.click(cx, "confirm.ok");
    assert!(f.text(cx, "aliases.error").contains("gone"));
    assert!(*f.data.lists.lock().unwrap() > lists);
}

#[gpui_kit::test]
async fn focus_starts_on_cancel_in_confirm(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    f.click(cx, "aliases.row.shop");
    f.press(cx, DELETE);
    let focused = cx
        .update_window(f.handle.into(), |_, window, _| {
            window.find("confirm.cancel").focused()
        })
        .unwrap();
    assert_eq!(focused, Some(true));
}

#[gpui_kit::test]
async fn the_shortcut_saves_inside_the_name_input(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "aliases.dialog.name", "shop");
    f.pick(cx, OK);
    f.press(cx, SAVE);
    assert_eq!(f.calls(), ["save_alias shop books"]);
}

#[gpui_kit::test]
async fn nothing_selected_disables_the_row_actions(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![alias("shop", OK)]).await;
    f.press(cx, EDIT);
    f.press(cx, DELETE);
    assert!(!f.has(cx, "aliases.dialog.name"));
    assert!(!f.has(cx, "confirm.body"));
}
