// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the keys tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::Map;
use tessera_app::state::{AppState, ResourceKind};
use tessera_client::ApiKey;
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
