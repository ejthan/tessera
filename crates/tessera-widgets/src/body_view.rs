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

/// Most bytes shown highlighted. The kit highlights on the UI thread: measured on an M3 Max,
/// 32 KiB take 11 ms before the first frame, 64 KiB 20 ms and 1 MiB 109 ms (design §4.4).
pub const HIGHLIGHT_MAX: usize = 32 << 10;
/// Most bytes shown at all. The kit lays the text out on the UI thread before the first frame:
/// 512 KiB take 12 ms, 1 MiB 21 ms and 8 MiB 91 ms (design §4.4).
pub const VIEW_MAX: usize = 512 << 10;
/// Most bytes pretty printed, off the UI thread, so a compact body is cut as lines.
const PRETTY_MAX: usize = 32 << 20;

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

/// Pretty JSON when `text` is JSON that is not too large to format.
fn pretty(text: &str) -> Option<String> {
    if text.len() > PRETTY_MAX {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

/// The start of `text` to show, formatted if it can be, and the length it was cut from.
fn to_show(text: &str) -> (String, usize) {
    let formatted = pretty(text);
    let whole = formatted.as_deref().unwrap_or(text);
    let shown = &whole[..whole.floor_char_boundary(VIEW_MAX)];
    (shown.to_string(), whole.len())
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
        let format = cx.background_spawn(async move { to_show(&text) });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compact_body_is_cut_as_lines() {
        let compact = format!("[{}1]", "1,".repeat(VIEW_MAX));
        let (shown, total) = to_show(&compact);
        assert!(shown.len() <= VIEW_MAX);
        assert!(shown.lines().count() > 1000);
        assert!(total > compact.len());
    }

    #[test]
    fn text_that_is_not_json_is_cut_on_a_char_boundary() {
        let (shown, total) = to_show(&"é".repeat(VIEW_MAX));
        assert!(shown.len() <= VIEW_MAX && shown.chars().all(|c| c == 'é'));
        assert_eq!(total, 2 * VIEW_MAX);
    }
}
