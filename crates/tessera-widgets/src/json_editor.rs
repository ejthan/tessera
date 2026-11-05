// SPDX-License-Identifier: GPL-3.0-or-later

//! A JSON code editor that parses its text on every edit and shows the first error inline.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::v_flex;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, prelude::*,
};
use rust_i18n::t;

/// Where and why the text is not valid JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    /// 1-based line.
    pub line: usize,
    /// 1-based column.
    pub column: usize,
    /// The parser's message, without position.
    pub message: String,
}

/// Emitted by [`JsonEditor`].
pub enum JsonEditorEvent {
    /// The text changed; [`JsonEditor::error`] is up to date.
    Changed,
}

/// A JSON editor with highlighting, line numbers, folding and an inline error line.
pub struct JsonEditor {
    state: Entity<EditorState>,
    read_only: bool,
    error: Option<JsonError>,
    _subscription: Subscription,
}

impl EventEmitter<JsonEditorEvent> for JsonEditor {}

impl JsonEditor {
    /// An editor holding `text`; a read-only one ignores typing.
    pub fn new(text: &str, read_only: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
                .folding(true)
                .default_value(text.to_string())
        });
        let subscription = cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.reparse(cx);
                cx.emit(JsonEditorEvent::Changed);
            }
        });
        let mut editor = Self {
            state,
            read_only,
            error: None,
            _subscription: subscription,
        };
        editor.reparse(cx);
        editor
    }

    /// The current text.
    pub fn text(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    /// Replaces the text. `set_value` emits no input event, so this parses and emits itself.
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
        self.reparse(cx);
        cx.emit(JsonEditorEvent::Changed);
    }

    /// The parse error of the current text, if any.
    pub fn error(&self) -> Option<&JsonError> {
        self.error.as_ref()
    }

    fn reparse(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.error = serde_json::from_str::<serde_json::Value>(&text)
            .err()
            .map(|e| JsonError {
                line: e.line(),
                column: e.column(),
                message: e
                    .to_string()
                    .rsplit_once(" at line ")
                    .map_or_else(|| e.to_string(), |(m, _)| m.to_string()),
            });
        cx.notify();
    }
}

impl Render for JsonEditor {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_1()
            .child(
                div().id("json.editor").test_support().flex_1().child(
                    Editor::new(&self.state)
                        .readonly(self.read_only)
                        .h_full()
                        .aria_label(t!("json.editor").to_string()),
                ),
            )
            .children(self.error.as_ref().map(|e| {
                let text = t!(
                    "json.error",
                    line = e.line,
                    column = e.column,
                    message = e.message.clone()
                )
                .to_string();
                div()
                    .id("json.error")
                    .test_support()
                    .aria_label(text.clone())
                    .text_sm()
                    .child(text)
            }))
    }
}
