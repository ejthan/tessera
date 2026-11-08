// SPDX-License-Identifier: GPL-3.0-or-later

//! A read-only view of a response body that stays usable at any size.

use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window, div, prelude::*,
};
use rust_i18n::t;

use crate::JsonEditor;

/// Most bytes shown highlighted.
pub const HIGHLIGHT_MAX: usize = 1 << 20;
/// Most bytes shown at all, since the editor builds its rope on the UI thread.
pub const VIEW_MAX: usize = 8 << 20;

/// What [`BodyView`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// Nothing.
    Empty,
    /// All of the text, highlighted.
    Highlighted,
    /// All of the text, without highlighting.
    Plain,
    /// The first `shown` of `total` bytes, without highlighting.
    Cut {
        /// Bytes on screen.
        shown: usize,
        /// Bytes of the body.
        total: usize,
    },
}

/// Pretty JSON when `text` is JSON small enough to show, else `text`.
fn pretty(text: &str) -> Option<String> {
    if text.len() >= VIEW_MAX {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    serde_json::to_string_pretty(&value)
        .ok()
        .filter(|pretty| pretty.len() <= VIEW_MAX)
}

fn megabytes(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / (1 << 20) as f64)
}

/// A response body: pretty JSON with highlighting while small, plain text above
/// [`HIGHLIGHT_MAX`], and the start of it above [`VIEW_MAX`], with the rest behind Copy all.
pub struct BodyView {
    editor: Option<Entity<JsonEditor>>,
    full: Option<Arc<str>>,
    shown: Shown,
    /// The formatting running; a newer body replaces it.
    format_task: Option<Task<()>>,
}

impl BodyView {
    /// An empty view.
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            editor: None,
            full: None,
            shown: Shown::Empty,
            format_task: None,
        }
    }

    /// Shows `text`, formatted off the UI thread.
    pub fn set_body(&mut self, text: Arc<str>, window: &mut Window, cx: &mut Context<Self>) {
        self.full = Some(text.clone());
        let format = cx.background_spawn(async move {
            let shown = pretty(&text)
                .unwrap_or_else(|| text[..text.floor_char_boundary(VIEW_MAX)].to_string());
            (shown, text.len())
        });
        self.format_task = Some(cx.spawn_in(window, async move |this, cx| {
            let (text, total) = format.await;
            this.update_in(cx, |this, window, cx| {
                let (shown, editor) = if text.len() <= HIGHLIGHT_MAX {
                    let editor = cx.new(|cx| JsonEditor::new(&text, true, window, cx));
                    (Shown::Highlighted, editor)
                } else {
                    let shown = if total > VIEW_MAX {
                        Shown::Cut {
                            shown: text.len(),
                            total,
                        }
                    } else {
                        Shown::Plain
                    };
                    (shown, cx.new(|cx| JsonEditor::plain(&text, window, cx)))
                };
                this.shown = shown;
                this.editor = Some(editor);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Shows nothing.
    pub fn clear(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.format_task = None;
        self.editor = None;
        self.full = None;
        self.shown = Shown::Empty;
        cx.notify();
    }

    /// What is on screen.
    pub fn shown(&self) -> Shown {
        self.shown
    }

    /// The text on screen.
    pub fn text(&self, cx: &gpui_kit::App) -> Option<String> {
        Some(self.editor.as_ref()?.read(cx).text(cx))
    }

    /// Puts the whole body on the clipboard.
    pub fn copy_all(&self, cx: &mut Context<Self>) {
        if let Some(full) = &self.full {
            cx.write_to_clipboard(ClipboardItem::new_string(full.to_string()));
        }
    }
}

impl Render for BodyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cut = match self.shown {
            Shown::Cut { shown, total } => Some(SharedString::from(
                t!(
                    "body.cut",
                    shown = megabytes(shown),
                    total = megabytes(total)
                )
                .to_string(),
            )),
            _ => None,
        };
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .children(cut.map(|note| {
                h_flex()
                    .flex_none()
                    .gap_2()
                    .p_1()
                    .items_center()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        div()
                            .id("body.cut")
                            .test_support()
                            .aria_label(note.clone())
                            .flex_1()
                            .child(note),
                    )
                    .child(
                        Button::new("body.copy_all")
                            .ghost()
                            .small()
                            .label(t!("body.copy_all").to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.copy_all(cx))),
                    )
            }))
            .child(div().flex_1().min_h_0().children(self.editor.clone()))
    }
}
