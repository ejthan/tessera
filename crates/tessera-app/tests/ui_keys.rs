// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the keys tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveDate, TimeZone as _};
use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::Map;
use tessera_app::state::{AppState, ResourceKind};
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::key_dialog::KeyDialog;
use tessera_app::views::resources::keys::KeysView;
use tessera_app::views::root::RootView;
use tessera_client::{ApiKey, NewKey};
use tessera_core::profile::{Environment, Profile};

use common::*;

/// Sat, 01 Jan 2100.
const LATER: i64 = 4102444800;
const NEVER: i64 = 64723363199;

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    id: uuid::Uuid,
    admin: Arc<FakeAdmin>,
    _rt: tokio::runtime::Runtime,
}

/// The key of the fake connection.
const CLIENT_KEY: &str = "k";

fn api_key(id: u64, description: &str, expires_at: i64) -> ApiKey {
    ApiKey {
        id,
        description: description.into(),
        actions: vec!["documents:search".into()],
        collections: vec!["*".into()],
        expires_at: Some(expires_at),
        value_prefix: Some("abcd".into()),
        extra: Map::new(),
    }
}

impl Fixture {
    /// Connected, with the tab not open.
    async fn connected(cx: &mut TestAppContext, profile: Profile, keys: Vec<ApiKey>) -> Self {
        fake_backend(cx, Some((vec![], vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let admin = fake_admin(cx);
        *admin.keys.lock().unwrap() = keys;
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
        Self {
            handle,
            state,
            id,
            admin,
            _rt: rt,
        }
    }

    async fn with(cx: &mut TestAppContext, profile: Profile, keys: Vec<ApiKey>) -> Self {
        let f = Self::connected(cx, profile, keys).await;
        cx.update(|cx| {
            f.state
                .update(cx, |s, cx| s.open_resource(f.id, ResourceKind::Keys, cx));
        });
        wait_for_element(cx, f.handle, "keys.reload".into(), true).await;
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, keys: Vec<ApiKey>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), keys).await
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

    fn view(&self, cx: &mut TestAppContext) -> Entity<KeysView> {
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
            let ResourceView::Keys(view) = panel.read(cx).view() else {
                panic!("not the keys view");
            };
            view.clone()
        })
    }

    fn dialog(&self, cx: &mut TestAppContext) -> Entity<KeyDialog> {
        let view = self.view(cx);
        cx.update(|cx| view.read(cx).dialog().unwrap().clone())
    }

    fn calls(&self) -> Vec<String> {
        self.admin.calls.lock().unwrap().clone()
    }
}

#[gpui_kit::test]
async fn keys_row_opens_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx, Profile::new("test", vec![node()]), vec![]).await;
    f.click(cx, &format!("sidebar.{}.keys", f.id));
    assert!(f.has(cx, "keys.reload"));
    assert!(f.has(cx, "keys.empty"));
}

#[gpui_kit::test]
async fn the_chord_opens_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx, Profile::new("test", vec![node()]), vec![]).await;
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("alt-cmd-6", "ctrl-alt-6"), cx);
    })
    .unwrap();
    assert!(f.has(cx, "keys.reload"));
}

#[gpui_kit::test]
async fn list_shows_prefix_not_value(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![api_key(1, "search", LATER)]).await;
    assert_eq!(f.label(cx, "keys.prefix.1"), "abcd••••");
    assert_eq!(f.label(cx, "keys.description.1"), "search");
    assert_eq!(f.label(cx, "keys.actions.1"), "documents:search");
}

#[gpui_kit::test]
async fn the_row_shows_what_the_key_may_do(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![api_key(1, "search", LATER)]).await;
    f.click(cx, "keys.row.1");
    assert!(
        f.label(cx, "keys.detail.actions")
            .contains("documents:search")
    );
    assert!(f.label(cx, "keys.detail.collections").contains('*'));
    assert!(f.label(cx, "keys.detail.expires").contains("2100"));
}

#[gpui_kit::test]
async fn expired_key_is_marked(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![api_key(1, "old", 1000)]).await;
    f.click(cx, "keys.row.1");
    assert!(f.label(cx, "keys.detail.expires").contains("Expired"));
}

#[gpui_kit::test]
async fn key_without_expiry_reads_never(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![api_key(1, "forever", NEVER)]).await;
    assert_eq!(f.label(cx, "keys.expires.1"), "Never");
}

#[gpui_kit::test]
async fn delete_asks_and_cancel_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![api_key(1, "search", LATER)]).await;
    f.click(cx, "keys.row.1");
    f.click(cx, "keys.delete");
    assert!(f.label(cx, "confirm.body").contains("scoped key"));
    f.click(cx, "confirm.cancel");
    assert_eq!(f.calls(), ["keys"]);
    f.click(cx, "keys.delete");
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["keys", "delete_key 1", "keys"]);
}

#[gpui_kit::test]
async fn delete_of_the_own_key_says_the_connection_ends(cx: &mut TestAppContext) {
    let mut own = api_key(2, "own", LATER);
    own.value_prefix = Some(CLIENT_KEY.into());
    let f = Fixture::new(cx, vec![api_key(1, "other", LATER), own]).await;
    f.click(cx, "keys.row.1");
    f.click(cx, "keys.delete");
    assert!(!f.label(cx, "confirm.body").contains("401"));
    f.click(cx, "confirm.cancel");
    f.click(cx, "keys.row.2");
    f.click(cx, "keys.delete");
    assert!(f.label(cx, "confirm.body").contains("401"));
}

#[gpui_kit::test]
async fn delete_on_prod_is_typed(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, vec![api_key(1, "search", LATER)]).await;
    f.click(cx, "keys.row.1");
    f.click(cx, "keys.delete");
    assert!(f.has(cx, "danger.input"));
    f.fill(cx, "danger.input", "search");
    f.click(cx, "danger.ok");
    assert!(f.calls().contains(&"delete_key 1".to_string()));
}

#[gpui_kit::test]
async fn restricted_key_shows_the_server_message(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx, Profile::new("test", vec![node()]), vec![]).await;
    *f.admin.fail_list.lock().unwrap() = Some((401, "Forbidden - nope".into()));
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_resource(f.id, ResourceKind::Keys, cx));
    });
    wait_for_element(cx, f.handle, "keys.reload".into(), true).await;
    assert!(f.label(cx, "keys.list_error").contains("Forbidden - nope"));
}

#[gpui_kit::test]
async fn read_only_delete_asks_to_unlock(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![api_key(1, "search", LATER)]).await;
    f.click(cx, "keys.row.1");
    f.click(cx, "keys.delete");
    assert!(f.has(cx, "danger.input"), "the unlock dialog opens");
    assert_eq!(f.calls(), ["keys"]);
}

fn open_form(f: &Fixture, cx: &mut TestAppContext) {
    f.click(cx, "keys.new");
    assert!(f.has(cx, "keys.form.description"));
}

fn sent(f: &Fixture) -> Vec<NewKey> {
    f.admin.created.lock().unwrap().clone()
}

/// Opens the form, names the key and saves it as a search-only key.
fn create(f: &Fixture, cx: &mut TestAppContext) {
    open_form(f, cx);
    f.fill(cx, "keys.form.description", "new key");
    f.click(cx, "keys.template.search_only");
    f.click(cx, "keys.form.save");
}

#[gpui_kit::test]
async fn template_search_only_fills_the_matrix(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    create(&f, cx);
    assert_eq!(
        sent(&f),
        [NewKey {
            description: "new key".into(),
            actions: vec!["documents:search".into()],
            collections: vec!["*".into()],
            expires_at: None,
        }]
    );
}

#[gpui_kit::test]
async fn wildcard_warns(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    open_form(&f, cx);
    assert!(!f.has(cx, "keys.wildcard_warning"));
    f.click(cx, "keys.wildcard");
    assert!(f.has(cx, "keys.wildcard_warning"));
    f.click(cx, "keys.template.search_only");
    assert!(!f.has(cx, "keys.wildcard_warning"));
    f.click(cx, "keys.template.admin");
    assert!(f.has(cx, "keys.wildcard_warning"));
}

#[gpui_kit::test]
async fn empty_description_stays_in_the_form_and_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    open_form(&f, cx);
    f.click(cx, "keys.template.search_only");
    f.click(cx, "keys.form.save");
    assert!(f.label(cx, "keys.problem.0").contains("description"));
    assert!(f.has(cx, "keys.form.description"));
    assert!(sent(&f).is_empty());
}

fn pick_day(f: &Fixture, cx: &mut TestAppContext, day: NaiveDate) {
    let dialog = f.dialog(cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        let state = dialog.read(cx).date().clone();
        let at = day.and_hms_opt(0, 0, 0).unwrap();
        state.update(cx, |s, cx| s.set_date_time(at, window, cx));
    })
    .unwrap();
    f.settle(cx);
}

#[gpui_kit::test]
async fn past_date_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    open_form(&f, cx);
    f.fill(cx, "keys.form.description", "new key");
    f.click(cx, "keys.template.search_only");
    pick_day(&f, cx, NaiveDate::from_ymd_opt(2001, 1, 1).unwrap());
    f.click(cx, "keys.form.save");
    assert!(f.label(cx, "keys.problem.0").contains("past"));
    assert!(sent(&f).is_empty());
}

#[gpui_kit::test]
async fn date_means_the_end_of_that_day(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    open_form(&f, cx);
    f.fill(cx, "keys.form.description", "new key");
    f.click(cx, "keys.template.search_only");
    let day = NaiveDate::from_ymd_opt(2099, 6, 15).unwrap();
    pick_day(&f, cx, day);
    f.click(cx, "keys.form.save");
    let end = day.and_hms_opt(23, 59, 59).unwrap();
    let want = Local.from_local_datetime(&end).earliest().unwrap();
    assert_eq!(sent(&f)[0].expires_at, Some(want.timestamp()));
}

#[gpui_kit::test]
async fn create_sends_the_chosen_actions(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    open_form(&f, cx);
    f.fill(cx, "keys.form.description", "writer");
    f.click(cx, "keys.action.documents:upsert");
    f.click(cx, "keys.action.aliases:list");
    f.click(cx, "keys.action.aliases:list");
    f.click(cx, "keys.all_collections");
    f.fill(cx, "keys.form.collections", "org_.*\nbooks\n");
    f.click(cx, "keys.form.save");
    let key = &sent(&f)[0];
    assert_eq!(key.actions, ["documents:upsert"]);
    assert_eq!(key.collections, ["org_.*", "books"]);
}

#[gpui_kit::test]
async fn a_running_create_keeps_the_form_open(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.admin.hold_create.lock().unwrap() = true;
    create(&f, cx);
    cx.update_window(f.handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    assert!(f.has(cx, "keys.form.description"));
}

#[gpui_kit::test]
async fn a_failed_create_stays_in_the_form(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.admin.fail.lock().unwrap() = Some((400, "no such thing".into()));
    create(&f, cx);
    assert!(f.label(cx, "keys.form.error").contains("no such thing"));
    assert!(f.has(cx, "keys.form.description"));
    assert!(!f.has(cx, "keys.reveal_copy"));
}

#[gpui_kit::test]
async fn reveal_shows_masked_and_copy_gives_the_value(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    create(&f, cx);
    assert!(!f.has(cx, "keys.form.description"));
    assert!(f.has(cx, "keys.reveal_warning"));
    assert!(!f.label(cx, "keys.reveal_value").contains(VALUE));
    f.click(cx, "keys.reveal_copy");
    let copied = cx.update(|cx| cx.read_from_clipboard()).unwrap();
    assert_eq!(copied.text().as_deref(), Some(VALUE));
}

#[gpui_kit::test]
async fn reveal_works_from_the_keyboard(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    create(&f, cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("cmd-shift-c", "ctrl-shift-c"), cx);
    })
    .unwrap();
    f.settle(cx);
    let copied = cx.update(|cx| cx.read_from_clipboard()).unwrap();
    assert_eq!(copied.text().as_deref(), Some(VALUE));
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("cmd-enter", "ctrl-enter"), cx);
    })
    .unwrap();
    f.settle(cx);
    assert!(!f.has(cx, "keys.reveal_copy"));
}

#[gpui_kit::test]
async fn reveal_ignores_escape(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    create(&f, cx);
    assert!(f.has(cx, "keys.reveal_copy"));
    cx.update_window(f.handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    assert!(f.has(cx, "keys.reveal_copy"));
}

#[gpui_kit::test]
async fn done_closes_the_reveal_and_selects_the_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    create(&f, cx);
    let view = f.view(cx);
    let reveal = cx.update(|cx| view.read(cx).reveal().unwrap().clone());
    assert!(reveal.upgrade().is_some());
    f.click(cx, "keys.reveal_done");
    assert!(!f.has(cx, "keys.reveal_copy"));
    assert!(
        reveal.upgrade().is_none(),
        "the value is gone with the dialog"
    );
    assert!(f.has(cx, "keys.row.77"));
    assert!(
        f.label(cx, "keys.detail.actions")
            .contains("documents:search")
    );
}

#[gpui_kit::test]
async fn read_only_save_asks_to_unlock(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![]).await;
    create(&f, cx);
    assert!(f.has(cx, "danger.input"));
    assert!(sent(&f).is_empty());
}

#[gpui_kit::test]
async fn save_to_a_profile_keeps_the_reveal_open(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    create(&f, cx);
    f.click(cx, "keys.reveal_profile");
    assert!(f.has(cx, "connect.key"));
    assert!(f.has(cx, "keys.reveal_copy"));
}
