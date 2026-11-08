// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the command palette and quick switch.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{Action, AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use tessera_app::actions::{
    DeleteDocuments, OpenPalette, PALETTE_HIDDEN, QuickSwitch, action_label,
};
use tessera_app::state::AppState;
use tessera_app::views::dock::{CollectionPanel, SearchPanel};
use tessera_app::views::root::RootView;
use tessera_app::views::search::SearchWorkbench;
use tessera_client::Collection;
use tessera_core::profile::{Environment, Profile};
use tessera_core::search::params::SearchDraft;
use tessera_core::search::saved::SavedQuery;

use common::*;

fn books() -> Collection {
    serde_json::from_value(json!({"name": "books", "fields": [
        {"name": "title", "type": "string"},
        {"name": "price", "type": "float", "sort": true},
    ]}))
    .unwrap()
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    id: uuid::Uuid,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    /// `profile` connected, with `books` of 3 documents on the server.
    async fn connected(cx: &mut TestAppContext, profiles: Vec<Profile>) -> Self {
        fake_backend(cx, Some((vec![books()], vec![])));
        *fake_documents(cx).found.lock().unwrap() = 3;
        install_search_fake(cx);
        fake_connector(cx, |_| Ok(server("30.2", true)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        for profile in &profiles {
            tessera_core::secrets::SecretStore::set(&*secrets, profile.id, &"k".to_string().into())
                .unwrap();
        }
        let id = profiles[0].id;
        let (handle, state, rt) = open_app(cx, profiles, secrets);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        Self {
            handle,
            state,
            id,
            _rt: rt,
        }
    }

    async fn on_documents(cx: &mut TestAppContext, profile: Profile) -> Self {
        let f = Self::connected(cx, vec![profile]).await;
        cx.update(|cx| {
            f.state
                .update(cx, |s, cx| s.open_collection(f.id, "books".into(), cx))
        });
        wait_for_element(cx, f.handle, format!("tab.{}.collection.books", f.id), true).await;
        f.settle(cx);
        f
    }

    async fn on_search(cx: &mut TestAppContext) -> Self {
        let f = Self::connected(cx, vec![Profile::new("test", vec![node()])]).await;
        f.open_search(cx);
        f
    }

    fn open_search(&self, cx: &mut TestAppContext) {
        cx.update(|cx| {
            self.state
                .update(cx, |s, cx| s.open_search(self.id, None, None, cx))
        });
        self.settle(cx);
    }

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

    fn dispatch(&self, cx: &mut TestAppContext, action: Box<dyn Action>) {
        self.act(cx, |window, cx| window.dispatch_action(action, cx));
        self.settle(cx);
    }

    fn palette(&self, cx: &mut TestAppContext) {
        self.dispatch(cx, Box::new(OpenPalette));
        assert!(self.has(cx, "palette"));
    }

    fn query(&self, cx: &mut TestAppContext, text: &str) {
        self.act(cx, |window, cx| window.input(text, cx));
        self.settle(cx);
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        self.act(cx, |window, cx| window.press(key, cx));
        self.settle(cx);
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.try_find(id).is_some())
    }

    fn has_action(&self, cx: &mut TestAppContext, name: &str) -> bool {
        self.has(cx, &format!("palette.action.tessera::{name}"))
    }

    fn text_of(&self, cx: &mut TestAppContext, id: &str) -> String {
        let id = id.to_string();
        self.act(cx, |window, _| {
            window.find(id).label().unwrap_or_default().to_string()
        })
    }

    fn workbench(&self, cx: &mut TestAppContext) -> Entity<SearchWorkbench> {
        self.handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                let panel = area
                    .layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .find_map(|id| area.panel(id)?.view().downcast::<SearchPanel>().ok())
                    .unwrap();
                panel.read(cx).workbench().clone()
            })
            .unwrap()
    }

    fn panel(&self, cx: &mut TestAppContext) -> Entity<CollectionPanel> {
        collection_panel(cx, self.handle, "books")
    }

    /// Selects the first row, which gives the keyboard to the table.
    fn select_row(&self, cx: &mut TestAppContext) {
        let panel = self.panel(cx);
        let table = cx.update(|cx| panel.read(cx).documents().read(cx).table().clone());
        cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, 1, cx)));
        self.act(cx, |window, cx| {
            gpui_kit::Focusable::focus_handle(table.read(cx), cx).focus(window, cx);
        });
        self.settle(cx);
    }

    fn focused_in_table(&self, cx: &mut TestAppContext) -> bool {
        let panel = self.panel(cx);
        let table = cx.update(|cx| panel.read(cx).documents().read(cx).table().clone());
        self.act(cx, |window, cx| {
            gpui_kit::Focusable::focus_handle(table.read(cx), cx).is_focused(window)
        })
    }
}

#[gpui_kit::test]
async fn every_action_has_a_label_or_is_hidden(cx: &mut TestAppContext) {
    open(cx);
    let names = cx.update(|cx| cx.all_action_names().to_vec());
    let ours: Vec<_> = names
        .iter()
        .filter(|n| n.starts_with("tessera::"))
        .collect();
    assert!(ours.len() > 50, "{ours:?}");
    for name in ours {
        let hidden = PALETTE_HIDDEN.contains(name);
        assert_ne!(action_label(name).is_some(), hidden, "{name}");
    }
}

#[gpui_kit::test]
async fn lists_actions_of_the_focused_view_only(cx: &mut TestAppContext) {
    let f = Fixture::on_documents(cx, Profile::new("test", vec![node()])).await;
    f.select_row(cx);
    f.palette(cx);
    f.query(cx, "new");
    assert!(f.has_action(cx, "NewDocument"));
    assert!(!f.has_action(cx, "NewCollection"));
}

#[gpui_kit::test]
async fn resource_tabs_are_listed_for_the_selected_connection(cx: &mut TestAppContext) {
    let f = Fixture::on_search(cx).await;
    f.palette(cx);
    f.query(cx, "curations");
    assert!(f.has_action(cx, "OpenCurations"));
    f.press(cx, SELECT_ALL);
    f.press(cx, "backspace");
    f.query(cx, "aliases");
    assert!(f.has_action(cx, "OpenAliases"));
}

#[gpui_kit::test]
async fn dispatch_runs_in_the_previous_focus(cx: &mut TestAppContext) {
    let f = Fixture::on_documents(cx, Profile::new("test", vec![node()])).await;
    f.select_row(cx);
    f.palette(cx);
    f.query(cx, "delete documents");
    f.press(cx, "enter");
    wait_for_element(cx, f.handle, "palette".into(), false).await;
    assert!(f.has(cx, "confirm.ok"), "the documents' confirmation");
    assert!(!f.has(cx, "danger.input"), "never the collection's");
}

#[gpui_kit::test]
async fn a_write_keeps_the_unlock_of_its_control(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::on_documents(cx, profile).await;
    f.select_row(cx);
    f.palette(cx);
    f.query(cx, "delete documents");
    f.press(cx, "enter");
    assert!(!f.has(cx, "confirm.body"), "the unlock comes first");
    assert!(f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn a_disabled_control_is_not_listed(cx: &mut TestAppContext) {
    let f = Fixture::on_documents(cx, Profile::new("test", vec![node()])).await;
    let panel = f.panel(cx);
    let table = cx.update(|cx| panel.read(cx).documents().read(cx).table().clone());
    f.act(cx, |window, cx| {
        gpui_kit::Focusable::focus_handle(table.read(cx), cx).focus(window, cx);
    });
    f.palette(cx);
    f.query(cx, "delete");
    assert!(f.has_action(cx, "DeleteByQuery"));
    assert!(!f.has_action(cx, "DeleteDocuments"), "no row selected");
}

#[gpui_kit::test]
async fn a_blocked_run_is_not_listed(cx: &mut TestAppContext) {
    let f = Fixture::on_search(cx).await;
    f.palette(cx);
    f.query(cx, "run");
    assert!(f.has_action(cx, "RunSearch"));
    f.press(cx, "escape");
    f.press(cx, "escape");
    f.act(cx, |window, cx| window.press(SHOW_JSON, cx));
    f.settle(cx);
    let workbench = f.workbench(cx);
    let editor = cx.update(|cx| workbench.read(cx).json().clone());
    f.act(cx, |window, cx| {
        editor.update(cx, |e, cx| e.set_text("{ nope", window, cx));
    });
    f.settle(cx);
    f.palette(cx);
    f.query(cx, "run");
    assert!(!f.has_action(cx, "RunSearch"));
}

#[gpui_kit::test]
async fn lock_again_is_listed_only_when_unlocked(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::on_documents(cx, profile).await;
    f.select_row(cx);
    f.palette(cx);
    f.query(cx, "lock");
    assert!(!f.has_action(cx, "LockAgain"));
    f.press(cx, "escape");
    f.press(cx, "escape");
    let conn = cx
        .update(|cx| f.state.read(cx).connection(f.id, cx))
        .unwrap();
    cx.update(|cx| conn.update(cx, |c, cx| c.unlock(cx)));
    f.settle(cx);
    f.palette(cx);
    f.query(cx, "lock");
    assert!(f.has_action(cx, "LockAgain"));
}

#[gpui_kit::test]
async fn collections_of_every_connection_are_listed_with_their_badge(cx: &mut TestAppContext) {
    let first = Profile::new("first", vec![node()]);
    let mut second = Profile::new("second", vec![node()]);
    second.environment = Environment::Prod;
    let second_id = second.id;
    let f = Fixture::connected(cx, vec![first, second]).await;
    cx.update(|cx| f.state.update(cx, |s, cx| s.connect(second_id, None, cx)));
    f.settle(cx);
    f.open_search(cx);
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_search(second_id, None, None, cx))
    });
    f.settle(cx);
    f.palette(cx);
    f.query(cx, "books");
    let row = |id: uuid::Uuid| format!("palette.collection.{id}.books");
    assert!(f.has(cx, &row(f.id)));
    assert!(f.has(cx, &row(second_id)));
    assert_eq!(f.text_of(cx, &format!("{}.env", row(second_id))), "PROD");
}

#[gpui_kit::test]
async fn quick_switch_lists_only_collections(cx: &mut TestAppContext) {
    let f = Fixture::on_search(cx).await;
    f.dispatch(cx, Box::new(QuickSwitch));
    assert!(f.has(cx, "palette"));
    assert!(f.has(cx, &format!("palette.collection.{}.books", f.id)));
    assert!(!f.has(cx, &format!("palette.connection.{}", f.id)));
    assert!(!f.has_action(cx, "RunSearch"));
    f.press(cx, "enter");
    wait_for_element(cx, f.handle, "palette".into(), false).await;
    wait_for_element(cx, f.handle, format!("tab.{}.collection.books", f.id), true).await;
}

#[gpui_kit::test]
async fn saved_query_opens_search_loaded(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx, vec![Profile::new("test", vec![node()])]).await;
    let mut one = SearchDraft::new("books");
    one.params.insert("q".into(), json!("first"));
    let query = SavedQuery {
        id: uuid::Uuid::new_v4(),
        profile: f.id,
        name: "cheap books".into(),
        tags: vec![],
        union: true,
        searches: vec![one, SearchDraft::new("books")],
    };
    cx.update(|cx| {
        f.state.update(cx, |s, cx| s.save_query(query, cx)).detach();
    });
    f.settle(cx);
    f.palette(cx);
    f.query(cx, "cheap");
    f.press(cx, "enter");
    wait_for_element(cx, f.handle, format!("tab.{}.search", f.id), true).await;
    f.settle(cx);
    assert!(f.has(cx, "search.tab.1"), "both searches");
    let workbench = f.workbench(cx);
    assert!(cx.update(|cx| workbench.read(cx).union()), "the union too");
}

#[gpui_kit::test]
async fn esc_closes_and_keeps_focus(cx: &mut TestAppContext) {
    let f = Fixture::on_documents(cx, Profile::new("test", vec![node()])).await;
    f.select_row(cx);
    assert!(f.focused_in_table(cx));
    f.palette(cx);
    f.query(cx, "x");
    f.press(cx, "escape");
    assert!(f.has(cx, "palette"), "the first Esc clears the query");
    f.press(cx, "escape");
    wait_for_element(cx, f.handle, "palette".into(), false).await;
    assert!(f.focused_in_table(cx));
}

#[gpui_kit::test]
async fn no_palette_over_a_dialog(cx: &mut TestAppContext) {
    let f = Fixture::on_documents(cx, Profile::new("test", vec![node()])).await;
    f.select_row(cx);
    f.dispatch(cx, DeleteDocuments.boxed_clone());
    assert!(f.has(cx, "confirm.ok"));
    f.dispatch(cx, Box::new(OpenPalette));
    assert!(!f.has(cx, "palette"));
    f.dispatch(cx, Box::new(QuickSwitch));
    assert!(!f.has(cx, "palette"));
}

#[gpui_kit::test]
async fn shortcut_is_shown_in_the_row(cx: &mut TestAppContext) {
    let f = Fixture::on_search(cx).await;
    f.palette(cx);
    f.query(cx, "run search");
    assert!(f.has(cx, "palette.action.tessera::RunSearch.kbd"));
}

#[gpui_kit::test]
async fn fuzzy_query_finds_scattered_letters(cx: &mut TestAppContext) {
    let f = Fixture::on_search(cx).await;
    f.palette(cx);
    f.query(cx, "rnsrch");
    assert!(f.has_action(cx, "RunSearch"));
    assert!(!f.has_action(cx, "AddSearch"));
}

#[gpui_kit::test]
async fn the_chord_opens_it_from_an_input(cx: &mut TestAppContext) {
    let f = Fixture::on_search(cx).await;
    let workbench = f.workbench(cx);
    let editor = cx.update(|cx| workbench.read(cx).json().clone());
    f.act(cx, |window, cx| window.press(SHOW_JSON, cx));
    f.settle(cx);
    f.act(cx, |window, cx| {
        editor.update(cx, |e, cx| e.focus(window, cx))
    });
    f.press(cx, key("cmd-k", "ctrl-k"));
    assert!(f.has(cx, "palette"));
}
