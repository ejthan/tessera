// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the analytics tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Map, Value, json};
use tessera_app::state::{AppState, ResourceKind};
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::analytics::AnalyticsView;
use tessera_app::views::root::RootView;
use tessera_client::Collection;
use tessera_core::profile::{Environment, Profile};

use common::*;

fn rule(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn popular() -> Map<String, Value> {
    rule(json!({
        "name": "popular", "type": "popular_queries", "collection": "products",
        "event_type": "search", "rule_tag": "web",
        "params": {"destination_collection": "top", "limit": 100}
    }))
}

fn counter() -> Map<String, Value> {
    rule(json!({
        "name": "clicks", "type": "counter", "collection": "products", "event_type": "click",
        "params": {"destination_collection": "products", "counter_field": "n", "weight": 1}
    }))
}

fn collection(name: &str) -> Collection {
    serde_json::from_value(json!({"name": name, "fields": []})).unwrap()
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    admin: Arc<FakeAdmin>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(
        cx: &mut TestAppContext,
        profile: Profile,
        rules: Vec<Map<String, Value>>,
    ) -> Self {
        let collections = vec![collection("products"), collection("top")];
        fake_backend(cx, Some((collections, vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let admin = fake_admin(cx);
        *admin.rules.lock().unwrap() = rules;
        let id = profile.id;
        fake_connector(cx, |_| Ok(server("30.2", true)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        tessera_core::secrets::SecretStore::set(&*secrets, id, &"k".to_string().into()).unwrap();
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
        // Tall enough to show the form and both sections below it at once.
        let tall = gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1800.));
        cx.simulate_window_resize(handle.into(), tall);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        cx.update(|cx| {
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Analytics, cx));
        });
        wait_for_element(cx, handle, "analytics.reload".into(), true).await;
        let f = Self {
            handle,
            state,
            admin,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, rules: Vec<Map<String, Value>>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), rules).await
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

    fn view(&self, cx: &mut TestAppContext) -> Entity<AnalyticsView> {
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
            let ResourceView::Analytics(view) = panel.read(cx).view() else {
                panic!("not the analytics view");
            };
            view.clone()
        })
    }

    fn form(
        &self,
        cx: &mut TestAppContext,
        pick: impl FnOnce(
            &mut AnalyticsView,
            &mut gpui_kit::Window,
            &mut gpui_kit::Context<AnalyticsView>,
        ),
    ) {
        let view = self.view(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            view.update(cx, |v, cx| pick(v, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn calls(&self) -> Vec<String> {
        self.admin.calls.lock().unwrap().clone()
    }

    /// A valid form of a new query rule named `name`.
    fn new_rule(&self, cx: &mut TestAppContext, name: &str) {
        self.click(cx, "analytics.new");
        self.fill(cx, "analytics.form.name", name);
        self.form(cx, |v, window, cx| {
            v.pick_source("products", window, cx);
            v.pick_destination("top", window, cx);
        });
        self.fill(cx, "analytics.form.limit", "100");
    }
}

#[gpui_kit::test]
async fn notice_always_shows(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    assert!(f.has(cx, "analytics.empty"));
    assert!(f.label(cx, "analytics.flags").contains("--analytics-dir"));
    let f = Fixture::new(cx, vec![popular()]).await;
    assert!(f.has(cx, "analytics.flags"));
    assert!(f.has(cx, "analytics.flags_docs"));
}

#[gpui_kit::test]
async fn form_follows_the_type(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.click(cx, "analytics.new");
    assert!(f.has(cx, "analytics.form.limit"));
    assert!(f.has(cx, "analytics.form.meta_fields"));
    assert!(!f.has(cx, "analytics.form.weight"));
    f.form(cx, |v, window, cx| v.pick_type("counter", window, cx));
    assert!(!f.has(cx, "analytics.form.limit"));
    assert!(f.has(cx, "analytics.form.counter_field"));
    assert!(f.has(cx, "analytics.form.weight"));
    assert!(f.has(cx, "analytics.form.destination"));
    f.form(cx, |v, window, cx| v.pick_type("log", window, cx));
    assert!(!f.has(cx, "analytics.form.destination"));
    assert!(!f.has(cx, "analytics.form.weight"));
}

#[gpui_kit::test]
async fn new_posts_and_selects(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.new_rule(cx, "fresh");
    f.click(cx, "analytics.save");
    let saved = f.admin.saved.lock().unwrap().clone();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, None);
    assert_eq!(saved[0].1["name"], "fresh");
    assert_eq!(saved[0].1["type"], "popular_queries");
    assert_eq!(saved[0].1["collection"], "products");
    assert_eq!(saved[0].1["event_type"], "search");
    assert_eq!(saved[0].1["params"]["destination_collection"], "top");
    assert_eq!(saved[0].1["params"]["limit"], 100);
    assert!(f.has(cx, "analytics.row.fresh"));
    assert!(f.has(cx, "analytics.test.send"));
}

#[gpui_kit::test]
async fn bad_name_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.new_rule(cx, "a b");
    f.click(cx, "analytics.save");
    assert!(f.has(cx, "analytics.problem.0"));
    assert!(f.admin.saved.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn edit_cannot_change_name_or_type(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![popular()]).await;
    f.click(cx, "analytics.row.popular");
    f.fill(cx, "analytics.form.name", "other");
    f.form(cx, |v, window, cx| v.pick_type("counter", window, cx));
    assert!(f.has(cx, "analytics.form.limit"));
    f.fill(cx, "analytics.form.tag", "");
    f.click(cx, "analytics.save");
    let saved = f.admin.saved.lock().unwrap().clone();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0.as_deref(), Some("popular"));
    assert!(saved[0].1.get("name").is_none() && saved[0].1.get("type").is_none());
    assert_eq!(saved[0].1["rule_tag"], "");
}

#[gpui_kit::test]
async fn edit_of_a_vanished_rule_shows_the_message(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![popular()]).await;
    f.click(cx, "analytics.row.popular");
    f.admin.rules.lock().unwrap().clear();
    f.click(cx, "analytics.save");
    assert!(f.label(cx, "analytics.error").contains("Rule not found"));
}

#[gpui_kit::test]
async fn unknown_type_is_json_only_and_deletable(cx: &mut TestAppContext) {
    let strange = rule(json!({"name": "odd", "type": "future", "collection": "c"}));
    let f = Fixture::new(cx, vec![strange]).await;
    f.click(cx, "analytics.row.odd");
    assert!(f.has(cx, "analytics.unknown"));
    assert!(!f.has(cx, "analytics.save"));
    f.click(cx, "analytics.delete");
    f.click(cx, "confirm.ok");
    assert!(f.calls().contains(&"delete_rule odd".to_string()));
}

#[gpui_kit::test]
async fn destination_shows_counts_sorted(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![popular(), counter()]).await;
    *f.admin.top.lock().unwrap() = json!({
        "found": 1, "out_of": 1, "page": 1, "search_time_ms": 0,
        "hits": [{"document": {"id": "1", "q": "shoes", "count": 7}}]
    })
    .to_string();
    f.click(cx, "analytics.row.popular");
    assert_eq!(f.label(cx, "analytics.top.count.shoes"), "7");
    assert!(
        f.calls()
            .contains(&"top_documents top count:desc".to_string())
    );
    f.click(cx, "analytics.row.clicks");
    assert!(
        f.calls()
            .contains(&"top_documents products n:desc".to_string())
    );
}

#[gpui_kit::test]
async fn open_in_search_opens_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![popular()]).await;
    *f.admin.top.lock().unwrap() = json!({
        "found": 1, "out_of": 1, "page": 1, "search_time_ms": 0,
        "hits": [{"document": {"id": "1", "q": "shoes", "count": 7}}]
    })
    .to_string();
    f.click(cx, "analytics.row.popular");
    f.click(cx, "analytics.top.row.shoes");
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = events.clone();
    let state = f.state.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&state, move |_, e: &tessera_app::state::OpenSearch, _| {
            let drafts = e.1.clone().map(|(d, _)| d).unwrap_or_default();
            let first = drafts.first().cloned();
            seen.lock().unwrap().push(first);
        })
    });
    f.click(cx, "analytics.open_search");
    let events = events.lock().unwrap();
    let draft = events[0].as_ref().unwrap();
    assert_eq!(draft.collection, "products");
    assert_eq!(draft.params["q"], "shoes");
}

#[gpui_kit::test]
async fn delete_cancel_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![popular()]).await;
    f.click(cx, "analytics.row.popular");
    f.click(cx, "analytics.delete");
    assert!(f.label(cx, "confirm.body").contains("destination"));
    f.click(cx, "confirm.cancel");
    assert!(!f.calls().iter().any(|c| c.starts_with("delete_rule")));
}

fn read_only() -> Profile {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    profile
}

#[gpui_kit::test]
async fn read_only_save_asks_to_unlock(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, read_only(), vec![popular()]).await;
    f.click(cx, "analytics.row.popular");
    f.click(cx, "analytics.save");
    assert!(f.has(cx, "danger.input"));
    assert!(f.admin.saved.lock().unwrap().is_empty());
}

fn send_test(f: &Fixture, cx: &mut TestAppContext) {
    f.fill(cx, "analytics.test.user", "u");
    f.fill(cx, "analytics.test.target", "42");
    f.click(cx, "analytics.test.send");
}

#[gpui_kit::test]
async fn test_event_sends_the_rules_event_type(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![counter()]).await;
    f.click(cx, "analytics.row.clicks");
    send_test(&f, cx);
    assert_eq!(
        *f.admin.events.lock().unwrap(),
        [json!({
            "name": "clicks", "event_type": "click", "data": {"user_id": "u", "doc_id": "42"}
        })]
    );
    assert!(f.label(cx, "analytics.test.note").contains("accepted"));
}

#[gpui_kit::test]
async fn test_event_on_prod_asks_first(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    profile.read_only = false;
    let f = Fixture::with(cx, profile, vec![counter()]).await;
    f.click(cx, "analytics.row.clicks");
    send_test(&f, cx);
    assert!(f.admin.events.lock().unwrap().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.admin.events.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
async fn rate_limit_shows_in_full_with_the_hint(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![counter()]).await;
    f.click(cx, "analytics.row.clicks");
    *f.admin.fail.lock().unwrap() = Some((500, "event rate limit reached.".into()));
    send_test(&f, cx);
    let note = f.label(cx, "analytics.test.note");
    assert!(note.contains("event rate limit reached."), "{note}");
    assert!(note.contains("--analytics-minute-rate-limit"), "{note}");
}

#[gpui_kit::test]
async fn flush_asks_to_unlock_when_locked(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, read_only(), vec![popular()]).await;
    f.click(cx, "analytics.row.popular");
    f.click(cx, "analytics.flush");
    assert!(f.has(cx, "danger.input"));
    assert!(!f.calls().contains(&"flush".to_string()));
}

#[gpui_kit::test]
async fn flush_reads_the_destination_again(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![popular()]).await;
    f.click(cx, "analytics.row.popular");
    let reads = |f: &Fixture| {
        let calls = f.calls();
        calls
            .iter()
            .filter(|c| c.starts_with("top_documents"))
            .count()
    };
    let before = reads(&f);
    f.click(cx, "analytics.flush");
    assert!(f.calls().contains(&"flush".to_string()));
    cx.executor().advance_clock(Duration::from_secs(3));
    f.settle(cx);
    assert_eq!(reads(&f), before + 1);
}
