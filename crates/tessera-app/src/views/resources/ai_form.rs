// SPDX-License-Identifier: GPL-3.0-or-later

//! The form of an AI model. The API key field is the only place a typed key lives, and it never
//! holds what the server stored, not even the mask.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::ModelKind;
use tessera_core::ai::{ModelDraft, endpoint_key};
use tessera_widgets::completion_input::{CompletionInput, CompletionInputEvent};
use tessera_widgets::json_editor::JsonEditor;

use crate::connection::ConnectionState;

/// Model names worth completing.
fn model_names(kind: ModelKind) -> Vec<String> {
    let names: &[&str] = match kind {
        ModelKind::Conversation => &[
            "openai/gpt-4o",
            "azure/",
            "google/gemini-2.5-flash",
            "cloudflare/@cf/mistral/mistral-7b-instruct-v0.1",
            "vllm/",
        ],
        ModelKind::NlSearch => &[
            "openai/gpt-4.1",
            "azure/",
            "google/gemini-2.5-flash",
            "gcp/gemini-2.5-flash",
            "cloudflare/",
            "vllm/",
        ],
    };
    names.iter().map(ToString::to_string).collect()
}

/// The inputs of one [`ModelsView`](super::ai::ModelsView).
pub struct ModelForm {
    kind: ModelKind,
    connection: Entity<ConnectionState>,
    new: bool,
    id: Entity<InputState>,
    model: Entity<CompletionInput>,
    key: Entity<InputState>,
    endpoint: Entity<InputState>,
    account: Entity<InputState>,
    system_prompt: Entity<TextareaState>,
    max_bytes: Entity<InputState>,
    history: Entity<SelectState<Vec<String>>>,
    ttl: Entity<InputState>,
    temperature: Entity<InputState>,
    more: Entity<JsonEditor>,
    /// When the collections in the select were read.
    names_from: Option<Instant>,
    _subscriptions: [Subscription; 2],
}

impl ModelForm {
    /// An empty form.
    pub(super) fn new(
        kind: ModelKind,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let label = t!("ai.form.model").to_string();
        let model = cx.new(|cx| {
            let mut input = CompletionInput::new(label, window, cx);
            input.set_candidates(model_names(kind));
            input
        });
        let subscriptions = [
            cx.subscribe(&model, |_, _, _: &CompletionInputEvent, cx| cx.notify()),
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.sync_names(window, cx);
            }),
        ];
        let mut this = Self {
            kind,
            connection,
            new: false,
            id: cx.new(|cx| InputState::new(window, cx)),
            model,
            key: cx.new(|cx| InputState::new(window, cx).masked(true)),
            endpoint: cx.new(|cx| InputState::new(window, cx)),
            account: cx.new(|cx| InputState::new(window, cx)),
            system_prompt: cx.new(|cx| TextareaState::new(window, cx).rows(3)),
            max_bytes: cx.new(|cx| InputState::new(window, cx)),
            history: cx.new(|cx| SelectState::new(Vec::new(), None, window, cx).searchable(true)),
            ttl: cx.new(|cx| InputState::new(window, cx)),
            temperature: cx.new(|cx| InputState::new(window, cx)),
            more: cx.new(|cx| JsonEditor::new("", false, window, cx)),
            names_from: None,
            _subscriptions: subscriptions,
        };
        this.sync_names(window, cx);
        this
    }

    fn collection_names(&self, cx: &App) -> Vec<String> {
        let cache = &self.connection.read(cx).collection_cache;
        let collections = cache.collections.iter().flatten();
        collections.map(|c| c.name.clone()).collect()
    }

    /// New collections reach the select, which keeps what it holds.
    fn sync_names(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.connection.read(cx).collection_cache.loaded_at;
        if from == self.names_from {
            return;
        }
        self.names_from = from;
        let held = self.history.read(cx).selected_value().cloned();
        self.fill_history(held.as_deref().unwrap_or_default(), window, cx);
    }

    fn fill_history(&self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut names = self.collection_names(cx);
        // One that is gone from the server still shows what the model holds.
        if !value.is_empty() && !names.iter().any(|n| n == value) {
            names.push(value.to_string());
        }
        let value = value.to_string();
        self.history.update(cx, |s, cx| {
            s.set_items(names, window, cx);
            s.set_selected_value(&value, window, cx);
        });
    }

    /// Shows `draft`, of a model not sent yet when `new`. The key field starts empty.
    pub(super) fn show(
        &mut self,
        draft: &ModelDraft,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new = new;
        self.clear_key(window, cx);
        let placeholder = if new {
            t!("ai.key_new")
        } else {
            t!("ai.key_keep")
        };
        self.key.update(cx, |k, cx| {
            k.set_placeholder(placeholder.to_string(), window, cx)
        });
        let inputs = [
            (&self.id, &draft.id),
            (&self.endpoint, &draft.endpoint),
            (&self.account, &draft.account_id),
            (&self.max_bytes, &draft.max_bytes),
            (&self.ttl, &draft.ttl),
            (&self.temperature, &draft.temperature),
        ];
        for (input, text) in inputs {
            input.update(cx, |i, cx| i.set_value(text.clone(), window, cx));
        }
        self.model
            .update(cx, |m, cx| m.set_value(&draft.model_name, window, cx));
        let prompt = draft.system_prompt.clone();
        self.system_prompt
            .update(cx, |p, cx| p.set_value(prompt, window, cx));
        self.fill_history(&draft.history_collection, window, cx);
        let more = if draft.more.is_empty() {
            String::new()
        } else {
            serde_json::to_string_pretty(&draft.more).unwrap_or_default()
        };
        self.more.update(cx, |j, cx| j.set_text(&more, window, cx));
        cx.notify();
    }

    /// Empties the key field.
    pub(super) fn clear_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.key.update(cx, |k, cx| k.set_value("", window, cx));
    }

    /// What the key field holds.
    pub fn key_text(&self, cx: &App) -> String {
        self.key.read(cx).value().to_string()
    }

    /// What "More settings" holds.
    pub fn more_text(&self, cx: &App) -> String {
        self.more.read(cx).text(cx)
    }

    /// The collections on offer for the history.
    pub fn history_names(&self, cx: &App) -> Vec<String> {
        self.collection_names(cx)
    }

    /// Types a model name.
    pub fn set_model(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.set_value(name, window, cx));
        cx.notify();
    }

    /// Picks the history collection.
    pub fn pick_history(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.history
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
    }

    /// Replaces "More settings".
    pub fn set_more(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.more.update(cx, |j, cx| j.set_text(text, window, cx));
    }

    /// The form's values, or `None` when "More settings" is no JSON object.
    pub(super) fn draft(&self, cx: &App) -> Option<ModelDraft> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let more = self.more.read(cx).text(cx);
        let more = if more.trim().is_empty() {
            Map::new()
        } else {
            match serde_json::from_str::<Value>(&more) {
                Ok(Value::Object(map)) => map,
                _ => return None,
            }
        };
        Some(ModelDraft {
            id: text(&self.id),
            model_name: self.model.read(cx).value(cx).trim().to_string(),
            endpoint: text(&self.endpoint),
            account_id: text(&self.account),
            system_prompt: self.system_prompt.read(cx).value().to_string(),
            max_bytes: text(&self.max_bytes),
            history_collection: self
                .history
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default(),
            ttl: text(&self.ttl),
            temperature: text(&self.temperature),
            more,
        })
    }

    fn field(
        id: &'static str,
        label: String,
        state: &Entity<InputState>,
    ) -> impl IntoElement + use<> {
        v_flex()
            .gap_1()
            .child(label.clone())
            .child(Input::new(state).id(id).aria_label(label))
    }
}

impl Render for ModelForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let model = self.model.read(cx).value(cx);
        let conversation = self.kind == ModelKind::Conversation;
        let has_endpoint = endpoint_key(self.kind, &model).is_some();
        let key_label = t!("ai.form.key").to_string();
        let prompt_label = t!("ai.form.system_prompt").to_string();
        let history_label = t!("ai.form.history").to_string();
        v_flex()
            .gap_2()
            .p_2()
            .when(self.new, |el| {
                el.child(Self::field(
                    "ai.form.id",
                    t!("ai.form.id").to_string(),
                    &self.id,
                ))
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(t!("ai.form.model").to_string())
                    .child(
                        div()
                            .id("ai.form.model")
                            .test_support()
                            .child(self.model.clone()),
                    ),
            )
            .child(
                v_flex().gap_1().child(key_label.clone()).child(
                    Input::new(&self.key)
                        .mask_toggle()
                        .id("ai.form.key")
                        .aria_label(key_label),
                ),
            )
            .when(has_endpoint, |el| {
                el.child(Self::field(
                    "ai.form.endpoint",
                    t!("ai.form.endpoint").to_string(),
                    &self.endpoint,
                ))
            })
            .when(model.starts_with("cloudflare/"), |el| {
                el.child(Self::field(
                    "ai.form.account",
                    t!("ai.form.account").to_string(),
                    &self.account,
                ))
            })
            .child(
                v_flex().gap_1().child(prompt_label.clone()).child(
                    div().id("ai.form.system_prompt").test_support().child(
                        Textarea::new(&self.system_prompt)
                            .aria_label(prompt_label)
                            .h(px(64.)),
                    ),
                ),
            )
            .child(Self::field(
                "ai.form.max_bytes",
                t!("ai.form.max_bytes").to_string(),
                &self.max_bytes,
            ))
            .when(conversation, |el| {
                el.child(
                    v_flex().gap_1().child(history_label.clone()).child(
                        Select::new(&self.history)
                            .id("ai.form.history")
                            .small()
                            .accessibility_label(history_label.clone())
                            .placeholder(history_label),
                    ),
                )
                .child(Self::field(
                    "ai.form.ttl",
                    t!("ai.form.ttl").to_string(),
                    &self.ttl,
                ))
            })
            .when(!conversation, |el| {
                el.child(Self::field(
                    "ai.form.temperature",
                    t!("ai.form.temperature").to_string(),
                    &self.temperature,
                ))
            })
            .child(t!("ai.form.more").to_string())
            .child(div().h(px(96.)).child(self.more.clone()))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(SharedString::from(t!("ai.form.more_note").to_string())),
            )
    }
}
