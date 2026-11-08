// SPDX-License-Identifier: GPL-3.0-or-later

//! A JSON code editor that parses its text on every edit and shows the first error inline.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position};
use gpui_kit::component::{ActiveTheme as _, v_flex};
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

/// How serious a [`JsonMark`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkSeverity {
    /// Drawn as an error.
    Error,
    /// Drawn as a warning.
    Warning,
    /// Drawn as a note.
    Info,
}

/// An underlined stretch of one line, with its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonMark {
    /// 0-based line.
    pub line: u32,
    /// 0-based column of the first marked character.
    pub start: u32,
    /// 0-based column just past the last marked character.
    pub end: u32,
    /// What is wrong there.
    pub message: String,
    /// How serious it is.
    pub severity: MarkSeverity,
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
    /// Shows any text: no highlighting and nothing to report about its syntax.
    plain: bool,
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
        Self::build(state, read_only, false, cx)
    }

    /// A read-only view of `text` of any size and syntax, without highlighting.
    pub fn plain(text: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .soft_wrap(false)
                .default_value(text.to_string())
        });
        Self::build(state, true, true, cx)
    }

    fn build(
        state: Entity<EditorState>,
        read_only: bool,
        plain: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.reparse(cx);
                cx.emit(JsonEditorEvent::Changed);
            }
        });
        let mut editor = Self {
            state,
            read_only,
            plain,
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

    /// The parse error of the current text, if any. An empty text is an error too.
    pub fn error(&self) -> Option<&JsonError> {
        self.error.as_ref()
    }

    /// Stops or allows typing.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    /// Replaces the marks. Any text change clears them, so they always fit the text.
    pub fn set_marks(&mut self, mut marks: Vec<JsonMark>, cx: &mut Context<Self>) {
        // The set keeps entries in the order they arrive and seeks through them by offset.
        marks.sort_by_key(|m| (m.line, m.start));
        self.state.update(cx, |state, cx| {
            if let Some(set) = state.diagnostics_mut() {
                set.clear();
                set.extend(marks.into_iter().map(|m| {
                    let range = Position::new(m.line, m.start)..Position::new(m.line, m.end);
                    Diagnostic::new(range, m.message).with_severity(match m.severity {
                        MarkSeverity::Error => DiagnosticSeverity::Error,
                        MarkSeverity::Warning => DiagnosticSeverity::Warning,
                        MarkSeverity::Info => DiagnosticSeverity::Info,
                    })
                }));
            }
            cx.notify();
        });
    }

    /// How many marks the text carries.
    pub fn mark_count(&self, cx: &App) -> usize {
        self.state.read(cx).diagnostics().map_or(0, |set| set.len())
    }

    /// Puts the keyboard focus in the editor.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Moves the cursor to the parse error and focuses the editor (design §6); without an
    /// error it only focuses.
    pub fn jump_to_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = &self.error else {
            return self.focus(window, cx);
        };
        // serde_json counts from 1 (column 0 at the very start of a line); `Position` from 0.
        let at = Position::new(
            e.line.saturating_sub(1) as u32,
            e.column.saturating_sub(1) as u32,
        );
        self.state
            .update(cx, |state, cx| state.set_cursor_position(at, window, cx));
    }

    fn reparse(&mut self, cx: &mut Context<Self>) {
        if self.plain {
            return;
        }
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
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
                    .text_color(danger)
                    .child(text)
            }))
    }
}
