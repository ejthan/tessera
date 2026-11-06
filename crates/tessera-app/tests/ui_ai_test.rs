// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the AI test panels.

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
use tessera_app::views::resources::ai_test::AiTestPanel;
use tessera_app::views::root::RootView;
use tessera_client::Collection;
use tessera_core::profile::Profile;

use common::*;

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

fn nl() -> Map<String, Value> {
    map(json!({
        "id": "nl", "model_name": "openai/gpt-4.1", "api_key": "sk-st*********", "max_bytes": 1000
    }))
}

fn hit(title: &str) -> Value {
    json!({"document": {"id": "1", "t": title}, "highlights": [], "text_match": 1})
}

fn chat(id: &str, answer: &str, turns: Value) -> Value {
    json!({"found": 1, "hits": [hit("source")], "search_time_ms": 1, "conversation": {
        "answer": answer, "conversation_id": id, "query": "hello",
        "conversation_history": {"conversation": turns, "last_updated": 1}
    }})
}

fn parsed() -> Value {
    json!({"found": 1, "hits": [hit("source")], "search_time_ms": 1, "parsed_nl_query": {
        "generated_params": {"q": "*", "filter_by": "o:=1", "sort_by": "o:desc"},
        "augmented_params": {"q": "books", "filter_by": "o:=1 && p:=2", "sort_by": "o:desc"},
        "parse_time_ms": 7,
        "llm_response": {"content": "raw llm text", "model": "gpt-4.1"}
    }})
}

fn collection(name: &str) -> Collection {
    serde_json::from_value(json!({"name": name, "fields": [
        {"name": "t", "type": "string"},
        {"name": "o", "type": "int32"},
        {"name": "v", "type": "float[]", "num_dim": 4},
        {"name": "e", "type": "float[]", "embed": {"from": ["t"]}},
    ]}))
    .unwrap()
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
        *admin.nl_models.lock().unwrap() = vec![nl()];
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

    fn close_tab(&self, cx: &mut TestAppContext) {
        let panel = self.resource_panel(cx);
        self.handle
            .update(cx, |root, window, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let dock = view.read(cx).dock().clone();
                dock.update(cx, |area, cx| area.remove_panel(panel, window, cx));
            })
            .unwrap();
        self.settle(cx);
    }

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        cx.update_window(self.handle.into(), |_, window, cx| window.press(keys, cx))
            .unwrap();
        self.settle(cx);
    }

    fn to_nl(&self, cx: &mut TestAppContext) {
        self.press(cx, key("cmd-2", "ctrl-2"));
    }

    fn resource_panel(&self, cx: &mut TestAppContext) -> Entity<ResourcePanel> {
        self.handle
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
            .unwrap()
    }

    fn tab(&self, cx: &mut TestAppContext) -> Entity<AiView> {
        let panel = self.resource_panel(cx);
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

    fn panel(&self, cx: &mut TestAppContext) -> Entity<AiTestPanel> {
        let view = self.view(cx);
        cx.update(|cx| view.read(cx).test_panel().clone())
    }

    fn pick(&self, cx: &mut TestAppContext, name: &str) {
        let panel = self.panel(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| p.pick(name.to_string(), window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn ask(&self, cx: &mut TestAppContext, question: &str) {
        let panel = self.panel(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| p.set_question(question, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    /// Selects a model, picks `books` and types a question.
    fn ready(&self, cx: &mut TestAppContext, row: &str) {
        self.click(cx, row);
        self.pick(cx, "books");
        self.ask(cx, "hello");
    }

    fn searches(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.admin.ai_searches.lock().unwrap().clone()
    }

    fn answers(&self, text: Value) {
        self.admin
            .ai_answers
            .lock()
            .unwrap()
            .push_back(text.to_string());
    }
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

#[gpui_kit::test]
async fn query_by_defaults_to_text_and_embedded_fields(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.pick(cx, "books");
    let panel = f.panel(cx);
    assert_eq!(cx.update(|cx| panel.read(cx).query_by_text(cx)), "t,e");
}

#[gpui_kit::test]
async fn chat_needs_unlock_on_a_read_only_profile(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![gpt()]).await;
    f.ready(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.test.run");
    assert!(f.has(cx, "danger.input"));
    assert!(f.searches().is_empty());
}

#[gpui_kit::test]
async fn chat_shows_answer_turns_and_sources(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.answers(chat(
        "c1",
        "stub answer",
        json!([{"user": "hello"}, {"assistant": "stub answer"}]),
    ));
    f.ready(cx, "ai.conversation.row.gpt");
    assert!(f.label(cx, "ai.test.chat_note").contains("history"));
    f.click(cx, "ai.test.run");
    assert!(f.label(cx, "ai.test.turn.0").contains("hello"));
    assert!(f.label(cx, "ai.test.turn.1").contains("stub answer"));
    assert!(f.label(cx, "ai.test.found").contains('1'));
    let searches = f.searches();
    assert_eq!(searches[0].0, "books");
    assert_eq!(param(&searches[0].1, "conversation"), Some("true"));
    assert_eq!(param(&searches[0].1, "conversation_model_id"), Some("gpt"));
    assert_eq!(param(&searches[0].1, "conversation_id"), None);
}

#[gpui_kit::test]
async fn follow_up_sends_the_conversation_id(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.answers(chat(
        "c1",
        "one",
        json!([{"user": "hello"}, {"assistant": "one"}]),
    ));
    f.ready(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.test.run");
    f.ask(cx, "and then");
    f.click(cx, "ai.test.run");
    assert_eq!(param(&f.searches()[1].1, "conversation_id"), Some("c1"));
}

#[gpui_kit::test]
async fn new_conversation_clears_it(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.answers(chat(
        "c1",
        "one",
        json!([{"user": "hello"}, {"assistant": "one"}]),
    ));
    f.ready(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.test.run");
    assert!(f.has(cx, "ai.test.turn.0"));
    f.click(cx, "ai.test.new_conversation");
    assert!(!f.has(cx, "ai.test.turn.0"));
    f.click(cx, "ai.test.run");
    assert_eq!(param(&f.searches()[1].1, "conversation_id"), None);
}

#[gpui_kit::test]
async fn plain_vector_field_is_refused_and_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.ready(cx, "ai.conversation.row.gpt");
    f.fill(cx, "ai.test.query_by", "t,v");
    f.click(cx, "ai.test.run");
    assert!(f.label(cx, "ai.test.error").contains('v'));
    assert!(f.searches().is_empty());
}

#[gpui_kit::test]
async fn long_question_is_refused_and_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.ready(cx, "ai.conversation.row.gpt");
    f.ask(cx, &"x".repeat(5000));
    f.click(cx, "ai.test.run");
    assert!(f.has(cx, "ai.test.error"));
    assert!(f.searches().is_empty());
}

#[gpui_kit::test]
async fn empty_question_cannot_run(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.click(cx, "ai.conversation.row.gpt");
    f.pick(cx, "books");
    f.click(cx, "ai.test.run");
    assert!(f.searches().is_empty());
    assert!(!f.has(cx, "ai.test.error"));
}

#[gpui_kit::test]
async fn nl_runs_on_a_read_only_profile(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![]).await;
    f.answers(parsed());
    f.to_nl(cx);
    f.ready(cx, "ai.nl.row.nl");
    f.click(cx, "ai.test.run");
    assert!(!f.has(cx, "danger.input"));
    assert_eq!(f.searches().len(), 1);
}

#[gpui_kit::test]
async fn nl_shows_generated_and_augmented(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.answers(parsed());
    f.to_nl(cx);
    f.ready(cx, "ai.nl.row.nl");
    f.click(cx, "ai.test.run");
    assert_eq!(f.label(cx, "ai.test.generated.filter_by"), "o:=1");
    assert_eq!(f.label(cx, "ai.test.augmented.q"), "books");
    assert_eq!(f.label(cx, "ai.test.generated.sort_by"), "o:desc");
    assert!(f.label(cx, "ai.test.parse_time").contains('7'));
    assert!(f.label(cx, "ai.test.found").contains('1'));
}

#[gpui_kit::test]
async fn nl_fall_back_warns(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.answers(json!({"found": 1, "hits": [hit("source")], "search_time_ms": 1}));
    f.to_nl(cx);
    f.ready(cx, "ai.nl.row.nl");
    f.click(cx, "ai.test.run");
    assert!(f.has(cx, "ai.test.fell_back"));
    assert!(f.has(cx, "ai.test.found"));
}

#[gpui_kit::test]
async fn nl_error_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.answers(
        json!({"found": 0, "hits": [], "search_time_ms": 1, "parsed_nl_query": {
            "error": "Failed to get response from OpenAI: 500",
            "generated_params": {}, "augmented_params": {"q": "x"}
        }}),
    );
    f.to_nl(cx);
    f.ready(cx, "ai.nl.row.nl");
    f.click(cx, "ai.test.run");
    assert!(f.label(cx, "ai.test.error").contains("OpenAI: 500"));
    assert!(!f.has(cx, "ai.test.fell_back"));
}

#[gpui_kit::test]
async fn timeout_says_the_server_may_still_be_calling(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    *f.admin.fail_ai_search.lock().unwrap() = Some((TIMEOUT, String::new()));
    f.to_nl(cx);
    f.ready(cx, "ai.nl.row.nl");
    f.click(cx, "ai.test.run");
    assert!(f.label(cx, "ai.test.error").contains("provider"));
}

#[gpui_kit::test]
async fn params_exclude_vector_fields(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    f.ready(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.test.run");
    assert_eq!(param(&f.searches()[0].1, "exclude_fields"), Some("v,e"));
}

#[gpui_kit::test]
async fn params_disable_analytics(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    f.answers(parsed());
    f.to_nl(cx);
    f.ready(cx, "ai.nl.row.nl");
    f.click(cx, "ai.test.run");
    assert_eq!(param(&f.searches()[0].1, "enable_analytics"), Some("false"));
    assert_eq!(param(&f.searches()[0].1, "nl_query_debug"), Some("true"));
}

#[gpui_kit::test]
async fn second_run_waits_for_the_first(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    let (release, held) = tokio::sync::oneshot::channel();
    *f.admin.ai_hold.lock().unwrap() = Some(held);
    f.ready(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.test.run");
    f.click(cx, "ai.test.run");
    assert_eq!(f.searches().len(), 1);
    release.send(()).unwrap();
}

#[gpui_kit::test]
async fn closing_the_tab_does_not_panic_on_a_late_answer(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![gpt()]).await;
    let (release, held) = tokio::sync::oneshot::channel();
    *f.admin.ai_hold.lock().unwrap() = Some(held);
    f.ready(cx, "ai.conversation.row.gpt");
    f.click(cx, "ai.test.run");
    assert_eq!(f.searches().len(), 1);
    f.close_tab(cx);
    release.send(()).unwrap();
    f.settle(cx);
    assert!(!f.has(cx, "ai.test.run"));
}
