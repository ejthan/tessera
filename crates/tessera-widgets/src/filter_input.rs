// SPDX-License-Identifier: GPL-3.0-or-later

//! A one-line `filter_by` input with completion of fields, operators and values.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{
    CompletionProvider, Editor, EditorState, IndentInline, InputEvent, OutdentInline, Rope,
    RopeExt as _,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Range, TextEdit,
};
use rust_i18n::t;
use tessera_core::documents::filter::{FilterContext, context_at, operators_for, quote_value};

/// How long a value waits for the next keystroke before it asks the server.
pub const FACET_DELAY: Duration = Duration::from_millis(150);
/// Most facet values offered.
pub const FACET_LIMIT: u32 = 20;

/// A field the filter can name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterField {
    /// Name, dotted for a nested field.
    pub name: String,
    /// Typesense type.
    pub r#type: String,
    /// Its values can be fetched as facets.
    pub facet: bool,
}

/// Emitted by [`FilterInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterInputEvent {
    /// Enter outside the completion menu, with the trimmed text.
    Submitted(String),
    /// The text changed.
    Changed(String),
}

/// Values of a facet field: `(field, prefix)`. Fails as an empty list, because autocompletion
/// must not surface errors.
pub type FacetFetch = Rc<dyn Fn(String, String, &mut App) -> Task<Vec<String>>>;

struct Provider {
    fields: Rc<RefCell<Vec<FilterField>>>,
    fetch: FacetFetch,
}

/// Completions showing `label` and replacing `range` with `new_text`.
pub(crate) fn response(range: Range, candidates: Vec<(String, String)>) -> CompletionResponse {
    let item = |(label, new_text)| CompletionItem {
        label,
        text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text })),
        ..Default::default()
    };
    CompletionResponse::Array(candidates.into_iter().map(item).collect())
}

/// The `options` starting with `typed`, inserted as they read.
fn matching(options: &[&str], typed: &str) -> Vec<(String, String)> {
    let matches = options.iter().filter(|option| option.starts_with(typed));
    matches.map(|o| (o.to_string(), o.to_string())).collect()
}

/// Nothing for a value already written as its only candidate: a menu there would make Enter
/// pick instead of apply.
fn unfinished(candidates: Vec<(String, String)>, typed: &str) -> Vec<(String, String)> {
    match candidates.as_slice() {
        [(_, new_text)] if new_text == typed => Vec::new(),
        _ => candidates,
    }
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
        let filter = text.to_string();
        // A menu here would turn Enter on an emptied filter into a pick.
        if filter.trim().is_empty() {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        }
        let context = context_at(&filter, offset);
        let (FilterContext::Field { start, .. }
        | FilterContext::Operator { start, .. }
        | FilterContext::Value { start, .. }
        | FilterContext::Join { start }) = context;
        let range = Range::new(
            text.offset_to_position(start),
            text.offset_to_position(offset),
        );
        let fields = self.fields.borrow();
        let field = |name: &str| fields.iter().find(|f| f.name == name);
        let candidates = match &context {
            FilterContext::Field { prefix, .. } => fields
                .iter()
                .filter(|f| f.name.starts_with(prefix.as_str()))
                .map(|f| (f.name.clone(), f.name.clone()))
                .collect(),
            FilterContext::Operator { field: name, .. } => field(name)
                .map(|f| matching(operators_for(&f.r#type), &filter[start..offset]))
                .unwrap_or_default(),
            // The condition may be the whole filter, so Enter stays free to apply it.
            FilterContext::Join { .. } => Vec::new(),
            FilterContext::Value {
                field: name,
                prefix,
                ..
            } => match field(name) {
                Some(f) if f.r#type.trim_end_matches("[]") == "bool" => {
                    unfinished(matching(&["true", "false"], prefix), &filter[start..offset])
                }
                Some(f) if f.facet => {
                    let (fetch, name, prefix) = (self.fetch.clone(), name.clone(), prefix.clone());
                    let typed = filter[start..offset].to_string();
                    // A newer keystroke drops this task, so only the last one reaches the server.
                    return cx.spawn(async move |cx| {
                        cx.background_executor().timer(FACET_DELAY).await;
                        let values = cx.update(|cx| fetch(name, prefix, cx)).await;
                        let candidates = values.into_iter().filter_map(|value| {
                            let quoted = quote_value(&value)?;
                            Some((value, quoted))
                        });
                        Ok(response(range, unfinished(candidates.collect(), &typed)))
                    });
                }
                _ => Vec::new(),
            },
        };
        Task::ready(Ok(response(range, candidates)))
    }

    fn is_completion_trigger(&self, _: usize, _: &str, _: &mut App) -> bool {
        true
    }
}

/// A `filter_by` input: Enter picks from an open menu and submits otherwise.
pub struct FilterInput {
    state: Entity<EditorState>,
    fields: Rc<RefCell<Vec<FilterField>>>,
    _subscription: Subscription,
}

impl EventEmitter<FilterInputEvent> for FilterInput {}

impl FilterInput {
    /// An empty input completing `fields`, with facet values from `fetch`.
    pub fn new(
        fields: Vec<FilterField>,
        fetch: FacetFetch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields = Rc::new(RefCell::new(fields));
        let provider = Rc::new(Provider {
            fields: fields.clone(),
            fetch,
        });
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .line_number(false)
                .folding(false)
                .searchable(false)
                .submit_on_enter(true)
                .scroll_beyond_last_line(None)
                .soft_wrap(false)
                .placeholder(t!("filter.placeholder").to_string());
            state.lsp_mut().completion_provider = Some(provider);
            state
        });
        let subscription = cx.subscribe_in(&state, window, Self::on_input);
        Self {
            state,
            fields,
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
                // `filter_by` is one line, and Shift+Enter or a paste can bring a break in.
                let line = text.replace(['\r', '\n'], " ");
                if line != text {
                    state.update(cx, |state, cx| {
                        let cursor = state.cursor();
                        state.set_value(line.clone(), window, cx);
                        // Each break became one space, so the offset still holds.
                        let at = state.text().offset_to_position(cursor);
                        state.set_cursor_position(at, window, cx);
                    });
                }
                cx.emit(FilterInputEvent::Changed(line));
            }
            // Shift+Enter comes here too, after inserting its line break.
            InputEvent::PressEnter { shift: false, .. } => {
                // Also drops a fetch still waiting, which would open a menu over the result.
                state.update(cx, |state, cx| state.dismiss_lsp_overlays(cx));
                cx.emit(FilterInputEvent::Submitted(
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
        cx.emit(FilterInputEvent::Changed(text.to_string()));
    }

    /// Replaces the fields offered.
    pub fn set_fields(&mut self, fields: Vec<FilterField>, _: &mut Context<Self>) {
        *self.fields.borrow_mut() = fields;
    }

    /// Puts the keyboard focus in the input.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Puts the keyboard focus in the input, with the cursor behind the text.
    pub fn focus_end(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            let end = state.text().offset_to_position(state.value().len());
            state.set_cursor_position(end, window, cx);
        });
    }

    /// The editor underneath, for reading its completion menu.
    pub fn state(&self) -> &Entity<EditorState> {
        &self.state
    }
}

impl Render for FilterInput {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("filter.input")
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
                    .aria_label(t!("filter.input").to_string()),
            )
    }
}
