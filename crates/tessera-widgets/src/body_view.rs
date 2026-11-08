// SPDX-License-Identifier: GPL-3.0-or-later

//! A read-only view of a response body that stays usable at any size.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Action, AppContext as _, ClipboardItem, Context, Div, Entity, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, Window, div,
    prelude::*,
};
use rust_i18n::t;

use crate::JsonEditor;

/// Most bytes shown highlighted. The kit highlights on the UI thread: measured on an M3 Max,
/// 32 KiB take 11 ms before the first frame, 64 KiB 20 ms and 1 MiB 109 ms (design §4.4).
pub const HIGHLIGHT_MAX: usize = 32 << 10;
/// Most bytes in the editor at a time. The kit lays the text out on the UI thread before the first frame:
/// 512 KiB take 12 ms, 1 MiB 21 ms and 8 MiB 91 ms (design §4.4).
pub const VIEW_MAX: usize = 512 << 10;
/// Most bytes pretty printed, off the UI thread, so a compact body is paged by lines.
const PRETTY_MAX: usize = 32 << 20;

gpui_kit::actions!(
    body_view,
    [
        /// Shows the chunk before the one on screen.
        PreviousChunk,
        /// Shows the chunk after the one on screen.
        NextChunk
    ]
);

/// Key context of a [`BodyView`].
pub const BODY_VIEW: &str = "BodyView";

/// What [`BodyView`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// Nothing.
    Empty,
    /// All of the text, highlighted.
    Highlighted,
    /// All of the text, without highlighting.
    Plain,
    /// One chunk of a text too long for the editor, without highlighting.
    Chunk {
        /// 1-based.
        page: usize,
        /// Chunks of the text.
        pages: usize,
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

/// `text` as chunks of at most [`VIEW_MAX`] bytes that cover all of it, each ending on a line end
/// where it holds one, else on a char boundary.
fn chunks(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    while text.len() - start > VIEW_MAX {
        let end = text.floor_char_boundary(start + VIEW_MAX);
        let line_end = text[start..end].rfind('\n').map(|i| start + i + 1);
        let end = line_end.unwrap_or(end);
        out.push(start..end);
        start = end;
    }
    out.push(start..text.len());
    out
}

fn megabytes(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / (1 << 20) as f64)
}

/// The text a view pages through: formatted once, off the UI thread.
struct Paged {
    text: Arc<str>,
    chunks: Vec<Range<usize>>,
    page: usize,
}

/// A response body: pretty JSON with highlighting while small, plain text above
/// [`HIGHLIGHT_MAX`], and in chunks of [`VIEW_MAX`] above that, with Copy all for the original.
pub struct BodyView {
    editor: Option<Entity<JsonEditor>>,
    full: Option<Arc<str>>,
    paged: Option<Paged>,
    shown: Shown,
    /// The formatting running; a newer body replaces it.
    format_task: Option<Task<()>>,
    focus_handle: FocusHandle,
}

impl BodyView {
    /// An empty view.
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            editor: None,
            full: None,
            paged: None,
            shown: Shown::Empty,
            format_task: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Shows `text`, formatted off the UI thread.
    pub fn set_body(&mut self, text: Arc<str>, window: &mut Window, cx: &mut Context<Self>) {
        self.full = Some(text.clone());
        let format = cx.background_spawn(async move {
            let formatted: Arc<str> = pretty(&text).map_or(text, Into::into);
            let chunks = chunks(&formatted);
            (formatted, chunks)
        });
        self.format_task = Some(cx.spawn_in(window, async move |this, cx| {
            let (text, chunks) = format.await;
            this.update_in(cx, |this, window, cx| {
                let highlighted = text.len() <= HIGHLIGHT_MAX;
                let page_count = chunks.len();
                this.paged = Some(Paged {
                    text,
                    chunks,
                    page: 0,
                });
                this.show_page(0, highlighted, window, cx);
                if page_count == 1 && !highlighted {
                    this.shown = Shown::Plain;
                }
            })
            .ok();
        }));
    }

    /// Builds the editor of chunk `page`, the one step that takes the UI thread.
    fn show_page(
        &mut self,
        page: usize,
        highlighted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(paged) = &mut self.paged else {
            return;
        };
        paged.page = page;
        let text = &paged.text[paged.chunks[page].clone()];
        let editor = if highlighted {
            cx.new(|cx| JsonEditor::new(text, true, window, cx))
        } else {
            cx.new(|cx| JsonEditor::plain(text, window, cx))
        };
        let pages = paged.chunks.len();
        self.shown = if highlighted {
            Shown::Highlighted
        } else {
            Shown::Chunk {
                page: page + 1,
                pages,
            }
        };
        // The old editor goes, and focus in it would leave no key context behind.
        let refocus = self.focus_handle.contains_focused(window, cx);
        self.editor = Some(editor);
        if refocus {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn step(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(paged) = &self.paged else {
            return;
        };
        let last = paged.chunks.len() - 1;
        let page = paged.page.saturating_add_signed(by).min(last);
        if page != paged.page {
            self.show_page(page, false, window, cx);
        }
    }

    /// Shows the next chunk.
    pub fn next_chunk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    /// Shows the previous chunk.
    pub fn previous_chunk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, window, cx);
    }

    /// Shows nothing.
    pub fn clear(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.format_task = None;
        self.editor = None;
        self.full = None;
        self.paged = None;
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

    fn pager(&self, page: usize, pages: usize, window: &Window, cx: &mut Context<Self>) -> Div {
        let total = self.paged.as_ref().map_or(0, |p| p.text.len());
        let note = SharedString::from(
            t!(
                "body.chunk",
                page = page,
                pages = pages,
                total = megabytes(total)
            )
            .to_string(),
        );
        let kbd = |action: &dyn Action| Kbd::binding_for_action(action, Some(BODY_VIEW), window);
        h_flex()
            .flex_none()
            .gap_2()
            .p_1()
            .items_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .id("body.chunk")
                    .test_support()
                    .aria_label(note.clone())
                    .flex_1()
                    .child(note),
            )
            .child(
                Button::new("body.previous")
                    .ghost()
                    .small()
                    .label(t!("body.previous").to_string())
                    .children(kbd(&PreviousChunk))
                    .disabled(page == 1)
                    .on_click(cx.listener(|this, _, window, cx| this.previous_chunk(window, cx))),
            )
            .child(
                Button::new("body.next")
                    .ghost()
                    .small()
                    .label(t!("body.next").to_string())
                    .children(kbd(&NextChunk))
                    .disabled(page == pages)
                    .on_click(cx.listener(|this, _, window, cx| this.next_chunk(window, cx))),
            )
            .child(
                Button::new("body.copy_all")
                    .ghost()
                    .small()
                    .label(t!("body.copy_all").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.copy_all(cx))),
            )
    }
}

impl Focusable for BodyView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BodyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pager = match self.shown {
            Shown::Chunk { page, pages } => Some(self.pager(page, pages, window, cx)),
            _ => None,
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(BODY_VIEW)
            .on_action(cx.listener(|this, _: &PreviousChunk, window, cx| {
                this.previous_chunk(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextChunk, window, cx| this.next_chunk(window, cx)))
            .size_full()
            .children(pager)
            .child(div().flex_1().min_h_0().children(self.editor.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(text: &str) -> Vec<&str> {
        chunks(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn chunks_cover_the_text_and_end_on_lines() {
        let text = "a line of text\n".repeat(VIEW_MAX / 4);
        let parts = pieces(&text);
        assert!(parts.len() > 1);
        assert_eq!(parts.concat(), text);
        for part in &parts[..parts.len() - 1] {
            assert!(part.len() <= VIEW_MAX && part.ends_with('\n'));
        }
    }

    #[test]
    fn a_long_line_is_cut_on_a_char_boundary() {
        let text = "é".repeat(VIEW_MAX + 1);
        let parts = pieces(&text);
        assert_eq!(parts.concat(), text);
        assert!(parts.iter().all(|p| p.len() <= VIEW_MAX));
    }

    #[test]
    fn a_short_text_is_one_chunk() {
        assert_eq!(pieces("a"), ["a"]);
        assert_eq!(pieces(""), [""]);
        assert_eq!(pieces(&"a".repeat(VIEW_MAX)).len(), 1);
    }

    #[test]
    fn a_compact_body_formats_before_it_is_cut() {
        let compact = format!("[{}1]", "1,".repeat(VIEW_MAX));
        let formatted = pretty(&compact).unwrap();
        assert!(pieces(&formatted).len() > 1);
        assert!(pieces(&formatted)[0].lines().count() > 1000);
    }
}
