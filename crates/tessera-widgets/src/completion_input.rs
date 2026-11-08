// SPDX-License-Identifier: GPL-3.0-or-later

//! A one-line input completing from a plain list of candidates.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{
    CompletionProvider, Editor, EditorState, IndentInline, InputEvent, OutdentInline, Rope,
    RopeExt as _,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use lsp_types::{CompletionContext, CompletionResponse, Range};

use crate::filter_input::response;

/// Emitted by [`CompletionInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionInputEvent {
    /// The text changed.
    Changed(String),
    /// Enter outside the completion menu, with the trimmed text.
    Submitted(String),
}

struct Provider {
    candidates: Rc<RefCell<Vec<String>>>,
}

fn trimmed(text: &str) -> &str {
    text.trim_start_matches('/')
}

impl CompletionProvider for Provider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let typed = text.to_string();
        // A menu here would turn Enter on an emptied input into a pick.
        if typed.trim().is_empty() {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        }
        let range = Range::new(text.offset_to_position(0), text.offset_to_position(offset));
        let candidates = self.candidates.clone();
        // Spawned to run after this keystroke's `Changed`, which refills the list.
        cx.spawn(async move |_| {
            let candidates = candidates.borrow();
            let typed = trimmed(&typed);
            // A text already complete wants Enter to submit, not to pick.
            let matches = candidates
                .iter()
                .filter(|c| trimmed(c).starts_with(typed))
                .filter(|_| !candidates.iter().any(|c| trimmed(c) == typed))
                .map(|c| (c.clone(), c.clone()));
            Ok(response(range, matches.collect()))
        })
    }

    fn is_completion_trigger(&self, _: usize, _: &str, _: &mut App) -> bool {
        true
    }
}

/// One line with completion: Enter picks from an open menu and submits otherwise.
pub struct CompletionInput {
    state: Entity<EditorState>,
    candidates: Rc<RefCell<Vec<String>>>,
    label: SharedString,
    _subscription: Subscription,
}

impl EventEmitter<CompletionInputEvent> for CompletionInput {}

impl CompletionInput {
    /// An empty input showing `placeholder`, also its accessible name.
    pub fn new(
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let label = placeholder.into();
        let candidates = Rc::new(RefCell::new(Vec::new()));
        let provider = Rc::new(Provider {
            candidates: candidates.clone(),
        });
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .line_number(false)
                .folding(false)
                .searchable(false)
                .submit_on_enter(true)
                .scroll_beyond_last_line(None)
                .soft_wrap(false)
                .placeholder(label.clone());
            state.lsp_mut().completion_provider = Some(provider);
            state
        });
        let subscription = cx.subscribe_in(&state, window, Self::on_input);
        Self {
            state,
            candidates,
            label,
            _subscription: subscription,
        }
    }

    fn on_input(
        &mut self,
        state: &Entity<EditorState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = self.value(cx);
                // One line, and Shift+Enter or a paste can bring a break in.
                let line = text.replace(['\r', '\n'], "");
                if line != text {
                    state.update(cx, |state, cx| state.set_value(line.clone(), window, cx));
                }
                cx.emit(CompletionInputEvent::Changed(line));
            }
            // Shift+Enter comes here too, after inserting its line break.
            InputEvent::PressEnter { shift: false, .. } => {
                state.update(cx, |state, cx| state.dismiss_lsp_overlays(cx));
                cx.emit(CompletionInputEvent::Submitted(
                    self.value(cx).trim().to_string(),
                ));
            }
            _ => {}
        }
    }

    /// The current text.
    pub fn value(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    /// Replaces the text. `set_value` emits no input event, so this emits itself.
    pub fn set_value(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
        cx.emit(CompletionInputEvent::Changed(text.to_string()));
    }

    /// Replaces what the menu offers.
    pub fn set_candidates(&mut self, candidates: Vec<String>) {
        *self.candidates.borrow_mut() = candidates;
    }

    /// What the menu offers.
    pub fn candidates(&self) -> Vec<String> {
        self.candidates.borrow().clone()
    }

    /// Puts the keyboard focus in the input.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// The editor underneath, for reading its completion menu.
    pub fn state(&self) -> &Entity<EditorState> {
        &self.state
    }
}

impl Render for CompletionInput {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("completion.input")
            .test_support()
            .w_full()
            // An editor keeps Tab for indenting; in one line it has to leave the field.
            .capture_action(|_: &IndentInline, window, cx| {
                window.focus_next(cx);
                cx.stop_propagation();
            })
            .capture_action(|_: &OutdentInline, window, cx| {
                window.focus_prev(cx);
                cx.stop_propagation();
            })
            .child(
                Editor::new(&self.state)
                    .h(px(32.))
                    .aria_label(self.label.clone()),
            )
    }
}
