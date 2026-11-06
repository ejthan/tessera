// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the resources tabs and their navigation.

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
use tessera_app::views::root::RootView;
use tessera_core::profile::{Environment, Profile};

use common::*;

const OPEN_ALIASES: &str = key("alt-cmd-1", "ctrl-alt-1");
const OPEN_STOPWORDS: &str = key("alt-cmd-4", "ctrl-alt-4");
const OPEN_PRESETS_TAB: &str = key("alt-cmd-5", "ctrl-alt-5");

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    id: uuid::Uuid,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn new(cx: &mut TestAppContext, profile: Profile, version: &'static str) -> Self {
        fake_backend(cx, Some((vec![], vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let id = profile.id;
        fake_connector(cx, move |_| Ok(server(version, true)));
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
            _rt: rt,
        }
    }

    async fn connected(cx: &mut TestAppContext) -> Self {
        Self::new(cx, Profile::new("test", vec![node()]), "30.2").await
    }

    fn has(&self, cx: &mut TestAppContext, id: String) -> bool {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    }

    fn row(&self, kind: &str) -> String {
        format!("sidebar.{}.{kind}", self.id)
    }

    fn panels(&self, cx: &mut TestAppContext) -> Vec<Entity<ResourcePanel>> {
        self.handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<ResourcePanel>().ok())
                    .collect()
            })
            .unwrap()
    }

    fn open(&self, cx: &mut TestAppContext, kind: ResourceKind) {
        let id = self.id;
        cx.update(|cx| self.state.update(cx, |s, cx| s.open_resource(id, kind, cx)));
        cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press(keys, cx);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn sidebar_shows_resource_rows_when_connected(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    for kind in ["aliases", "stopwords", "presets"] {
        let row = f.row(kind);
        assert!(f.has(cx, row), "{kind}");
    }
}

#[gpui_kit::test]
async fn synonyms_rows_hide_below_v30(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Profile::new("old", vec![node()]), "29.0").await;
    for kind in ["synonyms", "curations"] {
        let row = f.row(kind);
        assert!(!f.has(cx, row), "{kind}");
    }
    let row = f.row("aliases");
    assert!(f.has(cx, row));
}

#[gpui_kit::test]
async fn opening_twice_selects_the_same_tab(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.open(cx, ResourceKind::Aliases);
    f.open(cx, ResourceKind::Stopwords);
    f.open(cx, ResourceKind::Aliases);
    let kinds: Vec<_> = f
        .panels(cx)
        .iter()
        .map(|p| cx.read(|cx| p.read(cx).kind()))
        .collect();
    assert_eq!(kinds, [ResourceKind::Aliases, ResourceKind::Stopwords]);
}

#[gpui_kit::test]
async fn tab_title_shows_environment(cx: &mut TestAppContext) {
    let mut profile = Profile::new("prod", vec![node()]);
    profile.environment = Environment::Prod;
    profile.read_only = true;
    let f = Fixture::new(cx, profile, "30.2").await;
    f.open(cx, ResourceKind::Presets);
    let tab = format!("tab.{}.presets", f.id);
    assert!(f.has(cx, tab.clone()));
    let lock = format!("{tab}.readonly");
    assert!(f.has(cx, lock));
    cx.update_window(f.handle.into(), |_, window, _| {
        assert_eq!(window.find(tab).label(), Some("Presets"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn shortcut_opens_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.press(cx, OPEN_ALIASES);
    f.press(cx, OPEN_STOPWORDS);
    f.press(cx, OPEN_PRESETS_TAB);
    f.press(cx, OPEN_ALIASES);
    assert_eq!(f.panels(cx).len(), 3);
}
