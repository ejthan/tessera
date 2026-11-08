// SPDX-License-Identifier: GPL-3.0-or-later

//! The inspector beside the table: one whole document in a read-only JSON editor.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, Window, div, prelude::*,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::Error;
use tessera_widgets::JsonEditor;

use super::copy::pretty;
use crate::connection::{ConnectionState, error_text};
use crate::documents::DocumentsBackendGlobal;

/// How a go to ID ended. A failure leaves the inspector as it was.
pub enum GoToResult {
    /// The inspector shows the document.
    Found,
    /// Why the document could not be loaded.
    Failed(SharedString),
}

impl EventEmitter<GoToResult> for Inspector {}

/// One document of a collection, loaded on request.
pub struct Inspector {
    connection: Entity<ConnectionState>,
    name: String,
    editor: Entity<JsonEditor>,
    id: Option<String>,
    base: Option<Value>,
    error: Option<SharedString>,
    /// The load running; a newer one replaces it.
    task: Option<Task<()>>,
    focus_handle: FocusHandle,
}

impl Inspector {
    /// An empty inspector over collection `name` of `connection`.
    pub fn new(
        connection: Entity<ConnectionState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            connection,
            name,
            editor: cx.new(|cx| JsonEditor::new("", true, window, cx)),
            id: None,
            base: None,
            error: None,
            task: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Loads document `id` and shows it; the old one stays until the answer.
    pub fn show(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.id = Some(id.clone());
        self.error = None;
        self.load(id, false, window, cx);
    }

    /// Like [`Self::show`], for an id typed by hand: reports through [`GoToResult`].
    pub fn go_to(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.load(id, true, window, cx);
    }

    fn load(&mut self, id: String, go_to: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connected) = &self.connection.read(cx).connected else {
            return;
        };
        let load = cx.global::<DocumentsBackendGlobal>().0.document(
            connected.client.clone(),
            self.name.clone(),
            id.clone(),
        );
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            this.update_in(cx, |this, window, cx| {
                this.finish(id, go_to, result, window, cx)
            })
            .ok();
        }));
    }

    fn finish(
        &mut self,
        id: String,
        go_to: bool,
        result: Result<Value, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        match result {
            Ok(document) => {
                let text = pretty(&document);
                self.editor
                    .update(cx, |e, cx| e.set_text(&text, window, cx));
                self.id = Some(id);
                self.base = Some(document);
                self.error = None;
                if go_to {
                    cx.emit(GoToResult::Found);
                }
            }
            Err(e) => {
                let text = error_text(&e, &self.connection.read(cx).profile).into();
                if go_to {
                    cx.emit(GoToResult::Failed(text));
                } else {
                    self.base = None;
                    self.error = Some(text);
                }
            }
        }
        cx.notify();
    }

    /// Whether the editor holds unsaved changes; it cannot while it is read-only.
    pub fn is_dirty(&self, _: &App) -> bool {
        false
    }

    /// The id of the document shown or loading.
    pub fn current_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The editor.
    pub fn editor(&self) -> &Entity<JsonEditor> {
        &self.editor
    }

    /// Whether the keyboard is inside the inspector.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }
}

impl Render for Inspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let body = if let Some(error) = self.error.clone() {
            div()
                .id("documents.inspector.error")
                .test_support()
                .aria_label(error.clone())
                .text_sm()
                .text_color(danger)
                .child(error)
                .into_any_element()
        } else if self.base.is_some() {
            div()
                .flex_1()
                .min_h_0()
                .child(self.editor.clone())
                .into_any_element()
        } else {
            let key = if self.task.is_some() {
                "documents.inspector.loading"
            } else {
                "documents.inspector.empty"
            };
            let text = SharedString::from(t!(key).to_string());
            div()
                .id("documents.inspector.body")
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        v_flex()
            .id("documents.inspector")
            .test_support()
            .track_focus(&self.focus_handle)
            .size_full()
            .p_2()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t!("documents.inspector.title").to_string()),
            )
            .child(body)
    }
}
