// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the AI tab: lists and forms.

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
use tessera_app::views::resources::ai::{AiView, ModelsView};
use tessera_app::views::resources::ai_form::ModelForm;
use tessera_app::views::root::RootView;
use tessera_client::Collection;
use tessera_core::profile::{Environment, Profile};

use common::*;

const KEY: &str = "sk-typed-secret";

fn map(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn gpt() -> Map<String, Value> {
    map(json!({
        "id": "gpt", "model_name": "openai/gpt-4o", "api_key": "sk-st*********",
        "openai_url": "http://stub", "access_token": "at-st*****", "history_collection": "chats",
        "max_bytes": 16384, "ttl": 86400, "extra_key": 1
    }))
}

fn other() -> Map<String, Value> {
    map(json!({
        "id": "other", "model_name": "google/gemini-2.5-flash", "api_key": "AI***",
        "history_collection": "chats", "max_bytes": 1000
    }))
}

fn collection(name: &str) -> Collection {
    serde_json::from_value(json!({"name": name, "fields": []})).unwrap()
}

fn key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

struct Fixture {
    handle: WindowHandle<Root>,
    admin: Arc<FakeAdmin>,
    _state: Entity<AppState>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn with(
        cx: &mut TestAppContext,
        profile: Profile,
        models: Vec<Map<String, Value>>,
    ) -> Self {
        let collections = vec![collection("chats"), collection("books")];
        fake_backend(cx, Some((collections, vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let admin = fake_admin(cx);
        *admin.conversation_models.lock().unwrap() = models;
        let id = profile.id;
        fake_connector(cx, |_| Ok(server("30.2", true)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        tessera_core::secrets::SecretStore::set(&*secrets, id, &"k".to_string().into()).unwrap();
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
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
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Ai, cx));
        });
        wait_for_element(cx, handle, "ai.conversation.reload".into(), true).await;
        let f = Self {
            handle,
            admin,
            _state: state,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, models: Vec<Map<String, Value>>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), models).await
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

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        cx.update_window(self.handle.into(), |_, window, cx| window.press(keys, cx))
            .unwrap();
        self.settle(cx);
    }

    fn tab(&self, cx: &mut TestAppContext) -> Entity<AiView> {
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
            let ResourceView::Ai(view) = panel.read(cx).view() else {
                panic!("not the AI view");
            };
            view.clone()
        })
    }

    fn view(&self, cx: &mut TestAppContext) -> Entity<ModelsView> {
        let tab = self.tab(cx);
        cx.update(|cx| tab.read(cx).current().clone())
    }

    fn form(&self, cx: &mut TestAppContext) -> Entity<ModelForm> {
        let view = self.view(cx);
        cx.update(|cx| view.read(cx).form().clone())
    }

    fn edit_form(
        &self,
        cx: &mut TestAppContext,
        change: impl FnOnce(&mut ModelForm, &mut gpui_kit::Window, &mut gpui_kit::Context<ModelForm>),
    ) {
        let form = self.form(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            form.update(cx, |f, cx| change(f, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn key_text(&self, cx: &mut TestAppContext) -> String {
        let form = self.form(cx);
        cx.update(|cx| form.read(cx).key_text(cx))
    }

    fn calls(&self) -> Vec<String> {
        self.admin.calls.lock().unwrap().clone()
    }

    fn sent(&self) -> Vec<(Option<String>, Value)> {
        self.admin.ai_saved.lock().unwrap().clone()
    }

    /// A valid form of a new model, without a key.
    fn new_model(&self, cx: &mut TestAppContext) {
        self.click(cx, "ai.conversation.new");
        self.fill(cx, "ai.form.id", "fresh");
        self.edit_form(cx, |f, window, cx| {
            f.set_model("openai/gpt-4o", window, cx);
            f.pick_history("chats", window, cx);
        });
        self.fill(cx, "ai.form.max_bytes", "1000");
    }
}

#[gpui_kit::test]
async fn tabs_switch_with_their_shortcuts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    assert!(f.has(cx, "ai.conversation.row.gpt"));
    f.press(cx, key("cmd-2", "ctrl-2"));
    assert!(f.has(cx, "ai.nl.reload"));
    assert!(!f.has(cx, "ai.conversation.reload"));
    assert!(f.has(cx, "ai.nl.empty"));
    f.press(cx, key("cmd-1", "ctrl-1"));
    assert!(f.has(cx, "ai.conversation.reload"));
    assert!(f.calls().contains(&"ai_models NlSearch".to_string()));
}

#[gpui_kit::test]
async fn empty_list_says_a_key_is_needed(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    let text = f.label(cx, "ai.conversation.empty");
    assert!(text.contains("provider"), "{text}");
    assert!(f.has(cx, "ai.conversation.docs"));
}

#[gpui_kit::test]
async fn key_input_is_masked(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.key", KEY);
    assert_eq!(f.key_text(cx), KEY);
    // A masked field reports no value.
    cx.update_window(f.handle.into(), |_, window, _| {
        assert_eq!(window.find("ai.form.key").value(), None);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn edit_starts_with_an_empty_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    assert_eq!(f.key_text(cx), "");
}

#[gpui_kit::test]
async fn stored_mask_is_never_shown(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    let form = f.form(cx);
    let draft = cx.update(|cx| form.read(cx).more_text(cx));
    assert!(!draft.contains("***"), "{draft}");
    assert_eq!(f.key_text(cx), "");
    let list = f.label(cx, "ai.conversation.row.gpt");
    assert!(!list.contains("***"), "{list}");
}

#[gpui_kit::test]
async fn save_without_a_key_sends_no_api_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.ttl", "60");
    f.click(cx, "ai.save");
    let sent = f.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], (Some("gpt".to_string()), json!({"ttl": 60})));
}

#[gpui_kit::test]
async fn save_with_a_key_sends_it_and_clears_the_field(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.key", KEY);
    assert_eq!(f.key_text(cx), KEY);
    f.click(cx, "ai.save");
    assert_eq!(f.sent()[0].1, json!({"api_key": KEY}));
    assert_eq!(f.key_text(cx), "");
    assert!(!f.has(cx, "ai.error"));
    // The same save again finds nothing to send.
    f.click(cx, "ai.save");
    assert_eq!(f.sent().len(), 1);
}

#[gpui_kit::test]
async fn create_answers_with_the_masked_model(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.new_model(cx);
    f.fill(cx, "ai.form.key", KEY);
    f.click(cx, "ai.save");
    let (editing, body) = f.sent()[0].clone();
    assert_eq!(editing, None);
    assert_eq!(body["api_key"], KEY);
    assert_eq!(body["id"], "fresh");
    assert_eq!(f.key_text(cx), "");
    assert!(f.has(cx, "ai.conversation.row.fresh"));
    let view = f.view(cx);
    assert!(cx.update(|cx| view.read(cx).gates(cx).iter().all(|(off, _)| !off)));
}

#[gpui_kit::test]
async fn selection_change_clears_the_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt(), other()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.key", KEY);
    f.click(cx, "ai.conversation.row.other");
    assert_eq!(f.key_text(cx), "");
}

#[gpui_kit::test]
async fn nothing_changed_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.save");
    assert!(f.sent().is_empty());
    assert!(f.has(cx, "ai.note"));
}

#[gpui_kit::test]
async fn provider_error_is_scrubbed(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    *f.admin.fail_ai.lock().unwrap() = Some((
        400,
        format!("OpenAI API error: Incorrect API key provided: {KEY}."),
    ));
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.key", KEY);
    f.click(cx, "ai.save");
    let text = f.label(cx, "ai.error");
    assert!(text.contains("Incorrect API key provided"), "{text}");
    assert!(!text.contains(KEY), "{text}");
}

#[gpui_kit::test]
async fn typed_mask_is_refused_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.key", "sk-st*********");
    f.click(cx, "ai.save");
    assert!(f.has(cx, "ai.problem"));
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn more_settings_hides_secret_keys(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    let form = f.form(cx);
    let more = cx.update(|cx| form.read(cx).more_text(cx));
    assert!(more.contains("extra_key"), "{more}");
    for hidden in ["api_key", "access_token", "openai_url", "ttl"] {
        assert!(!more.contains(hidden), "{more}");
    }
}

#[gpui_kit::test]
async fn bad_more_settings_stop_the_save(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.edit_form(cx, |form, window, cx| form.set_more("[1", window, cx));
    f.click(cx, "ai.save");
    assert!(f.has(cx, "ai.error"));
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn history_select_lists_collections(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    let form = f.form(cx);
    let names = cx.update(|cx| form.read(cx).history_names(cx));
    assert_eq!(names, ["chats", "books"]);
    assert!(f.has(cx, "ai.form.history"));
    f.press(cx, key("cmd-2", "ctrl-2"));
    f.click(cx, "ai.nl.new");
    assert!(!f.has(cx, "ai.form.history"));
    assert!(f.has(cx, "ai.form.temperature"));
}

#[gpui_kit::test]
async fn endpoint_field_follows_the_namespace(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.click(cx, "ai.conversation.new");
    assert!(!f.has(cx, "ai.form.endpoint"));
    f.edit_form(cx, |form, window, cx| {
        form.set_model("openai/x", window, cx)
    });
    assert!(f.has(cx, "ai.form.endpoint"));
    f.edit_form(cx, |form, window, cx| {
        form.set_model("google/x", window, cx)
    });
    assert!(!f.has(cx, "ai.form.endpoint"));
    assert!(!f.has(cx, "ai.form.account"));
    f.edit_form(cx, |form, window, cx| {
        form.set_model("cloudflare/x", window, cx);
    });
    assert!(f.has(cx, "ai.form.account"));
}

#[gpui_kit::test]
async fn delete_says_history_stays(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.delete");
    assert!(f.label(cx, "confirm.body").contains("history"));
    f.click(cx, "confirm.cancel");
    assert!(!f.calls().iter().any(|c| c.starts_with("delete_ai_model")));
    f.click(cx, "ai.delete");
    f.click(cx, "confirm.ok");
    assert!(
        f.calls()
            .contains(&"delete_ai_model Conversation gpt".to_string())
    );
    assert!(!f.has(cx, "ai.conversation.row.gpt"));
}

#[gpui_kit::test]
async fn prod_delete_is_typed(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.delete");
    assert!(f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn read_only_save_asks_to_unlock(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.form.ttl", "60");
    f.click(cx, "ai.save");
    assert!(f.has(cx, "danger.input"));
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn create_with_no_key_is_refused_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.new_model(cx);
    f.click(cx, "ai.save");
    assert!(f.has(cx, "ai.problem"));
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn list_error_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.admin.fail_ai.lock().unwrap() = Some((401, "Forbidden".into()));
    f.click(cx, "ai.conversation.reload");
    assert!(f.has(cx, "ai.list_error"));
    assert!(f.label(cx, "ai.list_error").contains("Forbidden"));
}
