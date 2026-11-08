// SPDX-License-Identifier: GPL-3.0-or-later

//! The test panel of an AI model. A chat turn calls the provider and stores the turn, so it is a
//! write. An NL search only reads. Neither stops when the tab closes, because the server goes
//! on either way: the answer is dropped.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::{Collection, Error, ModelKind, SearchRoute, search_route};
use tessera_core::ai::{
    ChatRole, NlOutcome, ParsedNl, chat_answer, chat_params, nl_outcome, nl_params, query_fields,
};
use tessera_core::documents::columns::exclude_fields;
use tessera_core::search::results::parse_result;
use tessera_widgets::json_editor::JsonEditor;

use super::super::search::hits::HitsView;
use super::{client_of, notice};
use crate::actions::{NewConversation, RunTest, TEST_PANEL};
use crate::admin::AdminBackendGlobal;
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, error_text};

/// Height of the sources.
const HITS_HEIGHT: f32 = 260.;
/// Height of the raw LLM text.
const LLM_HEIGHT: f32 = 120.;

/// One of `q`, `filter_by`, `sort_by` as a parsed query holds it.
fn param(params: &Map<String, Value>, key: &str) -> String {
    match params.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Runs one model's tests, under its form.
pub struct AiTestPanel {
    kind: ModelKind,
    connection: Entity<ConnectionState>,
    model: Option<String>,
    collection: Entity<SelectState<Vec<String>>>,
    list_loaded: Option<Instant>,
    picked: Option<String>,
    /// What the server answered for it.
    schema: Option<Collection>,
    read_task: Option<Task<()>>,
    query_by: Entity<InputState>,
    question: Entity<TextareaState>,
    /// Kept for follow-ups until New conversation.
    conversation_id: Option<String>,
    turns: Vec<(ChatRole, String)>,
    hits: Entity<HitsView>,
    found: Option<u64>,
    parsed: Option<ParsedNl>,
    llm: Entity<JsonEditor>,
    fell_back: bool,
    running: bool,
    /// Changes when what a running answer belongs to is gone.
    generation: u64,
    /// Why nothing was sent, or what went wrong.
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 3],
}

impl AiTestPanel {
    /// A panel with nothing picked.
    pub(super) fn new(
        kind: ModelKind,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let collection = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let question = cx.new(|cx| TextareaState::new(window, cx).rows(2));
        let subscriptions = [
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.take_collections(window, cx);
            }),
            cx.subscribe_in(
                &collection,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.pick(name.clone(), window, cx);
                    }
                },
            ),
            cx.subscribe(&question, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            kind,
            connection,
            model: None,
            collection,
            list_loaded: None,
            picked: None,
            schema: None,
            read_task: None,
            query_by: cx.new(|cx| InputState::new(window, cx)),
            question,
            conversation_id: None,
            turns: Vec::new(),
            hits: cx.new(HitsView::new),
            found: None,
            parsed: None,
            llm: cx.new(|cx| JsonEditor::plain("", window, cx)),
            fell_back: false,
            running: false,
            generation: 0,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.take_collections(window, cx);
        this
    }

    fn take_collections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cache = &self.connection.read(cx).collection_cache;
        // The connection also changes with every poll.
        if cache.loaded_at.is_none() || cache.loaded_at == self.list_loaded {
            return;
        }
        self.list_loaded = cache.loaded_at;
        let collections = cache.collections.iter().flatten();
        let names: Vec<String> = collections.map(|c| c.name.clone()).collect();
        self.collection
            .update(cx, |s, cx| s.set_items(names, window, cx));
        if let Some(name) = self.picked.clone() {
            self.collection
                .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
        }
        cx.notify();
    }

    /// The model of the form above. Another one starts over.
    pub(super) fn set_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if model != self.model {
            self.model = model;
            self.start_over(cx);
        }
    }

    /// Forgets the conversation and every answer. One still running is dropped when it comes.
    fn start_over(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.conversation_id = None;
        self.turns.clear();
        self.drop_result(cx);
    }

    fn drop_result(&mut self, cx: &mut Context<Self>) {
        self.found = None;
        self.parsed = None;
        self.fell_back = false;
        self.error = None;
        cx.notify();
    }

    /// Takes `name` as the collection, with a fresh read of it.
    pub fn pick(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.picked = Some(name.clone());
        self.schema = None;
        self.error = None;
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let get = cx.global::<CollectionsBackendGlobal>().0.get(client, name);
        self.read_task = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = get.await;
            this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(schema) => {
                        let fields = query_fields(&schema).join(",");
                        this.query_by
                            .update(cx, |i, cx| i.set_value(fields, window, cx));
                        this.schema = Some(schema);
                    }
                    Err(e) => {
                        let profile = &this.connection.read(cx).profile;
                        this.error = Some(error_text(&e, profile).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Types the question.
    pub fn set_question(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.question
            .update(cx, |q, cx| q.set_value(text, window, cx));
    }

    /// What `query_by` holds.
    pub fn query_by_text(&self, cx: &App) -> String {
        self.query_by.read(cx).value().to_string()
    }

    fn question_text(&self, cx: &App) -> String {
        self.question.read(cx).value().trim().to_string()
    }

    fn run_blocked(&self, cx: &App) -> bool {
        self.running
            || self.model.is_none()
            || self.schema.is_none()
            || self.question_text(cx).is_empty()
    }

    /// The parameters of a run, or what stops it. Nothing is sent before this passes.
    fn params(&self, cx: &App) -> Result<(String, Vec<(String, String)>), SharedString> {
        let (Some(model), Some(schema)) = (&self.model, &self.schema) else {
            return Err(t!("ai.bad_query_by", name = "").to_string().into());
        };
        let query_by = self.query_by.read(cx).value().to_string();
        let names: Vec<&str> = query_by.split(',').map(str::trim).collect();
        // A plain vector field gets no answer from 30.0 under `conversation=true`.
        let allowed = query_fields(schema);
        if let Some(bad) = names.iter().find(|n| !allowed.iter().any(|a| a == *n)) {
            return Err(t!("ai.bad_query_by", name = bad).to_string().into());
        }
        let query_by = names.join(",");
        let question = self.question_text(cx);
        // Also what reaches the LLM's prompt, which must not hold an embedding.
        let exclude = exclude_fields(schema);
        let params = match self.kind {
            ModelKind::Conversation => chat_params(
                model,
                &question,
                &query_by,
                self.conversation_id.as_deref(),
                exclude.as_deref(),
            ),
            ModelKind::NlSearch => nl_params(model, &question, &query_by, exclude.as_deref()),
        };
        match search_route(&schema.name, &params) {
            Ok(SearchRoute::Get { .. }) => Ok((schema.name.clone(), params)),
            Ok(SearchRoute::Post { .. }) => Err(t!("ai.too_long").to_string().into()),
            Err(e) => Err(error_text(&e, &self.connection.read(cx).profile).into()),
        }
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_blocked(cx) {
            return;
        }
        let (name, params) = match self.params(cx) {
            Ok(run) => run,
            Err(text) => {
                self.error = Some(text);
                return cx.notify();
            }
        };
        match self.kind {
            ModelKind::NlSearch => self.send(name, params, window, cx),
            ModelKind::Conversation => {
                let view = cx.entity();
                self.connection.update(cx, |c, cx| {
                    c.ensure_writable(window, cx, move |window, cx| {
                        view.update(cx, |v, cx| v.send(name, params, window, cx));
                    });
                });
            }
        }
    }

    /// Runs detached: a closed tab lets the turn finish and drops the answer.
    fn send(
        &mut self,
        name: String,
        params: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = client_of(&self.connection, cx) else {
            self.error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        self.running = true;
        self.drop_result(cx);
        let search = cx
            .global::<AdminBackendGlobal>()
            .0
            .ai_search(client, name, params);
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let answer = search.await;
            this.update_in(cx, |this, window, cx| {
                this.running = false;
                if generation == this.generation {
                    match answer {
                        Ok(text) => this.take_answer(&text, window, cx),
                        Err(e) => this.take_error(&e, cx),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn take_error(&mut self, error: &Error, cx: &App) {
        let text = error_text(error, &self.connection.read(cx).profile);
        self.error = Some(match error {
            Error::Timeout => format!("{text} {}", t!("ai.timeout_note")).into(),
            _ => text.into(),
        });
    }

    fn take_answer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match parse_result(text, ("<mark>", "</mark>")) {
            Ok(result) => {
                self.found = Some(result.found);
                self.hits.update(cx, |h, cx| h.set_result(&result, cx));
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        match self.kind {
            ModelKind::Conversation => match chat_answer(text) {
                Some(answer) => {
                    self.conversation_id = Some(answer.conversation_id);
                    self.turns = answer.turns;
                }
                None => {
                    let no_answer = t!("ai.no_answer").to_string();
                    self.error = Some(match self.error.take() {
                        Some(parse) => format!("{no_answer} {parse}").into(),
                        None => no_answer.into(),
                    });
                }
            },
            ModelKind::NlSearch => match nl_outcome(text) {
                NlOutcome::Parsed(parsed) => {
                    let llm = parsed.llm.clone().unwrap_or_default();
                    self.llm.update(cx, |j, cx| j.set_text(&llm, window, cx));
                    self.parsed = Some(parsed);
                }
                NlOutcome::Failed(message) => self.error = Some(message.into()),
                NlOutcome::FellBack => self.fell_back = true,
            },
        }
    }

    fn new_conversation(&mut self, cx: &mut Context<Self>) {
        self.start_over(cx);
    }

    fn turn_rows(&self, cx: &App) -> Vec<impl IntoElement + use<>> {
        let muted = cx.theme().muted_foreground;
        self.turns
            .iter()
            .enumerate()
            .map(|(i, (role, text))| {
                let role = match role {
                    ChatRole::User => t!("ai.role.user"),
                    ChatRole::Assistant => t!("ai.role.assistant"),
                };
                let label = SharedString::from(format!("{role}: {text}"));
                div()
                    .id(SharedString::from(format!("ai.test.turn.{i}")))
                    .test_support()
                    .aria_label(label.clone())
                    .px_2()
                    .text_sm()
                    .text_color(muted)
                    .child(label)
            })
            .collect()
    }

    /// `q`, `filter_by` and `sort_by`, as written and as searched.
    fn parsed_rows(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let parsed = self.parsed.as_ref()?;
        let muted = cx.theme().muted_foreground;
        let cell = |id: String, text: String| {
            let text = SharedString::from(text);
            div()
                .id(SharedString::from(id))
                .test_support()
                .aria_label(text.clone())
                .flex_1()
                .min_w_0()
                .text_sm()
                .child(text)
        };
        let head = h_flex()
            .gap_2()
            .text_sm()
            .text_color(muted)
            .child(div().w(px(80.)))
            .child(div().flex_1().child(t!("ai.generated").to_string()))
            .child(div().flex_1().child(t!("ai.augmented").to_string()));
        let rows = ["q", "filter_by", "sort_by"].map(|key| {
            h_flex()
                .gap_2()
                .child(div().w(px(80.)).text_sm().child(key))
                .child(cell(
                    format!("ai.test.generated.{key}"),
                    param(&parsed.generated, key),
                ))
                .child(cell(
                    format!("ai.test.augmented.{key}"),
                    param(&parsed.augmented, key),
                ))
        });
        let time = t!("ai.parse_time", ms = parsed.parse_ms).to_string();
        Some(
            v_flex()
                .gap_1()
                .p_2()
                .child(head)
                .children(rows)
                .child(notice("ai.test.parse_time", time.into(), muted))
                .child(div().h(px(LLM_HEIGHT)).child(self.llm.clone())),
        )
    }
}

impl Render for AiTestPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, warning, muted) = (theme.danger, theme.warning, theme.muted_foreground);
        let conversation = self.kind == ModelKind::Conversation;
        let locked = conversation && self.connection.read(cx).writes_locked();
        let question_label = t!("ai.question").to_string();
        let run_label = if conversation {
            t!("ai.send").to_string()
        } else {
            t!("ai.run").to_string()
        };
        let turns = self.turn_rows(cx);
        let parsed = self.parsed_rows(cx);
        let found = self.found.map(|found| {
            let text = t!("ai.found", found = found).to_string();
            notice("ai.test.found", text.into(), muted)
        });
        v_flex()
            .key_context(TEST_PANEL)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &RunTest, window, cx| this.run(window, cx)))
            .on_action(cx.listener(|this, _: &NewConversation, _, cx| {
                this.new_conversation(cx);
            }))
            .gap_2()
            .p_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(200.)).child(
                            Select::new(&self.collection)
                                .id("ai.test.collection")
                                .small()
                                .accessibility_label(t!("test.collection").to_string())
                                .placeholder(t!("test.collection").to_string()),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.query_by)
                                .small()
                                // i18n-ok: Typesense parameter name
                                .prefix(div().text_sm().child("query_by"))
                                .id("ai.test.query_by")
                                // i18n-ok: Typesense parameter name
                                .aria_label("query_by"),
                        ),
                    ),
            )
            .child(question_label.clone())
            .child(
                div().id("ai.test.question").test_support().child(
                    Textarea::new(&self.question)
                        .aria_label(question_label)
                        .h(px(56.)),
                ),
            )
            .when(conversation, |el| {
                el.child(notice(
                    "ai.test.chat_note",
                    t!("ai.chat_note").to_string().into(),
                    muted,
                ))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .when(conversation, |el| {
                        let focus = self.focus_handle.clone();
                        el.child(
                            Button::new("ai.test.new_conversation")
                                .small()
                                .label(t!("ai.new_conversation").to_string())
                                .children(Kbd::binding_for_action(
                                    &NewConversation,
                                    Some(TEST_PANEL),
                                    window,
                                ))
                                .on_click(move |_, window, cx| {
                                    focus.dispatch_action(&NewConversation, window, cx);
                                }),
                        )
                    })
                    .child(
                        Button::new("ai.test.run")
                            .primary()
                            .small()
                            .label(run_label)
                            .when(locked, |b| {
                                b.child(super::env_badge::lock("ai.test.run", false, cx))
                            })
                            .children(Kbd::binding_for_action(&RunTest, Some(TEST_PANEL), window))
                            .loading(self.running)
                            .disabled(self.run_blocked(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("ai.test.error", e, danger)),
            )
            .when(self.fell_back, |el| {
                el.child(notice(
                    "ai.test.fell_back",
                    t!("ai.fell_back").to_string().into(),
                    warning,
                ))
            })
            .children(turns)
            .children(parsed)
            .children(found)
            .when(self.found.is_some(), |el| {
                el.child(div().h(px(HITS_HEIGHT)).child(self.hits.clone()))
            })
    }
}
