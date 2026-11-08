// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the scoped key panel.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Map, Value, json};
use tessera_app::state::ResourceKind;
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::scoped_keys::ScopedKeysView;
use tessera_app::views::root::RootView;
use tessera_client::{ApiKey, Collection};
use tessera_core::keys::NEVER;
use tessera_core::keys::scoped::generate;
use tessera_core::profile::Profile;

use common::*;

const PARENT: &str = "parentkeyvalue01";
const FILTER: &str = r#"{"filter_by":"o:=1","expires_at":4102444800}"#;

struct Fixture {
    handle: WindowHandle<Root>,
    admin: Arc<FakeAdmin>,
    _rt: tokio::runtime::Runtime,
}

fn listed(id: u64, prefix: &str, actions: &[&str]) -> ApiKey {
    ApiKey {
        id,
        description: "k".into(),
        actions: actions.iter().map(ToString::to_string).collect(),
        collections: vec!["*".into()],
        expires_at: Some(NEVER),
        value_prefix: Some(prefix.into()),
        extra: Map::new(),
    }
}

fn scoped() -> String {
    let params: Value = serde_json::from_str(FILTER).unwrap();
    generate(PARENT, &params).unwrap()
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, profile: Profile, keys: Vec<ApiKey>) -> Self {
        let collection = Collection {
            name: "books".into(),
            ..Collection::default()
        };
        fake_backend(cx, Some((vec![collection], vec![])));
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
        cx.update(|cx| {
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Keys, cx));
        });
        wait_for_element(cx, handle, "keys.reload".into(), true).await;
        let f = Self {
            handle,
            admin,
            _rt: rt,
        };
        f.click(cx, "keys.view.scoped");
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

    fn panel(&self, cx: &mut TestAppContext) -> Entity<ScopedKeysView> {
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
            view.read(cx).scoped().clone()
        })
    }

    fn set_params(&self, cx: &mut TestAppContext, text: &str) {
        let panel = self.panel(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            let editor = panel.read(cx).params().clone();
            editor.update(cx, |e, cx| e.set_text(text, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn generated(&self, cx: &mut TestAppContext) -> String {
        let panel = self.panel(cx);
        cx.update(|cx| panel.read(cx).generated(cx))
    }

    fn pick(&self, cx: &mut TestAppContext) {
        let panel = self.panel(cx);
        cx.update(|cx| panel.update(cx, |p, cx| p.pick("books".into(), cx)));
        self.settle(cx);
    }

    fn calls(&self) -> Vec<String> {
        self.admin.calls.lock().unwrap().clone()
    }
}

#[gpui_kit::test]
async fn typing_generates_a_key_locally(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.set_params(cx, FILTER);
    assert_eq!(f.generated(cx), scoped());
    assert_eq!(f.calls(), ["keys"]);
}

#[gpui_kit::test]
async fn bad_json_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.set_params(cx, "{nope");
    assert!(f.label(cx, "scoped.error").contains("not JSON"));
    assert_eq!(f.generated(cx), "");
}

#[gpui_kit::test]
async fn number_refused_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.set_params(cx, r#"{"a":1e21}"#);
    assert!(f.label(cx, "scoped.error").contains("JavaScript"));
}

#[gpui_kit::test]
async fn parent_with_more_actions_warns(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![listed(5, "pare", &["*"])]).await;
    f.fill(cx, "scoped.parent", PARENT);
    assert!(f.label(cx, "scoped.note").contains('5'));
}

#[gpui_kit::test]
async fn a_fitting_parent_says_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![listed(5, "pare", &["documents:search"])]).await;
    f.fill(cx, "scoped.parent", PARENT);
    assert!(!f.has(cx, "scoped.note"));
}

#[gpui_kit::test]
async fn bootstrap_warns_as_unknown(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    assert!(f.label(cx, "scoped.note").contains("bootstrap"));
}

#[gpui_kit::test]
async fn decode_shows_params_and_expiry(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.decode", &scoped());
    assert!(f.label(cx, "scoped.decoded.prefix").contains("pare"));
    assert!(f.label(cx, "scoped.decoded.expires").contains("2100"));
}

#[gpui_kit::test]
async fn a_key_without_expiry_says_so(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.decode", &generate(PARENT, &json!({})).unwrap());
    assert_eq!(f.label(cx, "scoped.decoded.expires"), "No expiry");
}

#[gpui_kit::test]
async fn bad_text_says_why(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.decode", "$$$");
    assert!(f.label(cx, "scoped.decode_error").contains("base64"));
}

#[gpui_kit::test]
async fn signature_valid_and_invalid(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.decode", &scoped());
    f.fill(cx, "scoped.parent", PARENT);
    assert_eq!(f.label(cx, "scoped.signature"), "Signature valid");
    f.fill(cx, "scoped.parent", "parentkeyvalue02");
    assert_eq!(f.label(cx, "scoped.signature"), "Signature does not match");
}

#[gpui_kit::test]
async fn not_verified_without_parent(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.decode", &scoped());
    assert_eq!(f.label(cx, "scoped.signature"), "Not verified");
}

#[gpui_kit::test]
async fn test_runs_with_the_scoped_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.set_params(cx, FILTER);
    f.pick(cx);
    f.click(cx, "scoped.run");
    let seen = f.admin.searches.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, scoped());
    assert_eq!(seen[0].1, "books");
    assert!(
        seen[0]
            .2
            .contains(&("enable_analytics".into(), "false".into()))
    );
    assert!(f.label(cx, "scoped.visible").contains("visible"));
}

#[gpui_kit::test]
async fn the_chord_of_run_runs(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.pick(cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.press(key("cmd-enter", "ctrl-enter"), cx);
    })
    .unwrap();
    f.settle(cx);
    assert_eq!(f.admin.searches.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
async fn the_decoded_key_wins_in_a_test(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.fill(cx, "scoped.decode", "pasted");
    f.pick(cx);
    f.click(cx, "scoped.run");
    assert_eq!(f.admin.searches.lock().unwrap()[0].0, "pasted");
}

#[gpui_kit::test]
async fn forbidden_shows_the_hint(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.admin.fail_search.lock().unwrap() = Some((401, "Forbidden - nope".into()));
    f.fill(cx, "scoped.parent", PARENT);
    f.pick(cx);
    f.click(cx, "scoped.run");
    assert!(
        f.label(cx, "scoped.test_error")
            .contains("Forbidden - nope")
    );
    assert!(
        f.label(cx, "scoped.forbidden_hint")
            .contains("documents:search")
    );
}

#[gpui_kit::test]
async fn test_runs_on_a_read_only_profile(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    f.pick(cx);
    f.click(cx, "scoped.run");
    assert_eq!(f.admin.searches.lock().unwrap().len(), 1);
    assert!(!f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn views_switch_with_their_shortcuts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.fill(cx, "scoped.parent", PARENT);
    let press = |f: &Fixture, cx: &mut TestAppContext, keys: &str| {
        cx.update_window(f.handle.into(), |_, window, cx| window.press(keys, cx))
            .unwrap();
        f.settle(cx);
    };
    press(&f, cx, key("cmd-1", "ctrl-1"));
    assert!(!f.has(cx, "scoped.parent"));
    press(&f, cx, key("cmd-2", "ctrl-2"));
    assert!(f.has(cx, "scoped.parent"));
    let panel = f.panel(cx);
    assert!(
        !cx.update(|cx| panel.read(cx).generated(cx)).is_empty(),
        "text stays"
    );
}
