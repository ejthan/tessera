// SPDX-License-Identifier: GPL-3.0-or-later

//! The hit list of a result, with the selected document beside it.

use std::ops::Range;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::actions::{SelectDown, SelectUp};
use gpui_kit::component::{ActiveTheme as _, h_flex, h_resizable, resizable_panel, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    HighlightStyle, IntoElement, ParentElement as _, Render, ScrollStrategy, SharedString,
    Styled as _, StyledText, Task, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};
use rust_i18n::t;
use tessera_core::search::results::{Hit, HitGroup, HitLine, SearchResult};
use tessera_widgets::JsonEditor;

use super::super::overview::grouped;
use crate::actions::SEARCH_HITS;

/// Height of a row of the list, headings included.
const ROW_HEIGHT: f32 = 72.;
/// Lines of a hit shown in its row.
const LINES: usize = 2;
/// Width of the document beside the list.
const DETAIL_WIDTH: f32 = 320.;

/// A row of the list.
enum Row {
    /// The heading of a group.
    Heading {
        key: SharedString,
        found: u64,
    },
    Hit(Hit),
}

fn group_rows(group: &HitGroup) -> impl Iterator<Item = Row> {
    let heading = Row::Heading {
        key: group.key.join(", ").into(),
        found: group.found,
    };
    std::iter::once(heading).chain(group.hits.iter().cloned().map(Row::Hit))
}

/// The hits of a result, one list with group headings among them.
pub struct HitsView {
    rows: Vec<Row>,
    selected: Option<usize>,
    detail: Option<Entity<JsonEditor>>,
    match_info: Option<SharedString>,
    /// The formatting of the selected document; a newer selection replaces it.
    detail_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl HitsView {
    /// An empty list.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            rows: Vec::new(),
            selected: None,
            detail: None,
            match_info: None,
            detail_task: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle().tab_stop(true),
        }
    }

    /// Shows `result`, with nothing selected.
    pub fn set_result(&mut self, result: &SearchResult, cx: &mut Context<Self>) {
        self.rows = result
            .groups
            .iter()
            .flat_map(group_rows)
            .chain(result.hits.iter().cloned().map(Row::Hit))
            .collect();
        self.selected = None;
        self.detail = None;
        self.match_info = None;
        self.detail_task = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// The editor showing the selected document.
    pub fn detail(&self) -> Option<&Entity<JsonEditor>> {
        self.detail.as_ref()
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Row::Hit(hit)) = self.rows.get(ix) else {
            return;
        };
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
        let (doc, info) = (hit.doc.clone(), hit.text_match_info.clone());
        let format = cx.background_spawn(async move {
            let document = serde_json::to_string_pretty(&*doc).unwrap_or_default();
            (document, info.map(|info| format!("{info:#}")))
        });
        self.detail_task = Some(cx.spawn_in(window, async move |this, cx| {
            let (document, info) = format.await;
            this.update_in(cx, |this, window, cx| {
                match &this.detail {
                    Some(detail) => detail.update(cx, |d, cx| d.set_text(&document, window, cx)),
                    None => {
                        let detail = cx.new(|cx| JsonEditor::new(&document, true, window, cx));
                        this.detail = Some(detail);
                    }
                }
                this.match_info = info.map(Into::into);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Selects the nearest hit at `step` from the selection, stepping over headings.
    fn step(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let start = match (self.selected, step) {
            (Some(ix), _) => ix as isize + step,
            (None, 1..) => 0,
            (None, _) => self.rows.len() as isize - 1,
        };
        let mut ix = start;
        while let Some(row) = usize::try_from(ix).ok().and_then(|ix| self.rows.get(ix)) {
            if matches!(row, Row::Hit(_)) {
                return self.select(ix as usize, window, cx);
            }
            ix += step;
        }
    }

    fn line(line: &HitLine, cx: &App) -> AnyElement {
        let style = HighlightStyle {
            background_color: Some(cx.theme().accent),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let marks = line.marks.iter().map(|mark| (mark.clone(), style));
        div()
            .truncate()
            .child(StyledText::new(line.text.clone()).with_highlights(marks))
            .into_any_element()
    }

    fn row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted, selected_bg) = (theme.border, theme.muted_foreground, theme.secondary);
        match &self.rows[ix] {
            Row::Heading { key, found } => {
                let label = SharedString::from(
                    t!("search.group", key = key, found = grouped(*found)).to_string(),
                );
                div()
                    .id(SharedString::from(format!("search.group.{ix}")))
                    .test_support()
                    .aria_label(label.clone())
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_end()
                    .p_2()
                    .border_b_1()
                    .border_color(border)
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label)
                    .into_any_element()
            }
            Row::Hit(hit) => {
                let lines = &hit.lines[..hit.lines.len().min(LINES)];
                let plain: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
                let curated = hit.curated.then(|| t!("search.hit.curated").to_string());
                let mut label = format!("{} {}", hit.id, plain.join(" "));
                if let Some(score) = hit.text_match {
                    label.push_str(&format!(" {score}"));
                }
                if let Some(curated) = &curated {
                    label.push_str(&format!(" {curated}"));
                }
                if let Some(collection) = &hit.collection {
                    label.push_str(&format!(" {collection}"));
                }
                v_flex()
                    .id(SharedString::from(format!("search.hit.{ix}")))
                    .test_support()
                    .aria_label(SharedString::from(label))
                    .h(px(ROW_HEIGHT))
                    .overflow_hidden()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .text_sm()
                    .when(self.selected == Some(ix), |el| el.bg(selected_bg))
                    .hover(|s| s.bg(selected_bg))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.select(ix, window, cx);
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(muted)
                            .child(hit.id.to_string())
                            .children(hit.text_match.map(|score| score.to_string()))
                            .children(curated)
                            .children(hit.collection.clone()),
                    )
                    .children(lines.iter().map(|line| Self::line(line, cx)))
                    .into_any_element()
            }
        }
    }
}

impl Focusable for HitsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HitsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = uniform_list(
            "search.hits",
            self.rows.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                range.map(|ix| this.row(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        let muted = cx.theme().muted_foreground;
        let info = self.match_info.clone();
        let open = self.detail.is_some();
        div()
            .key_context(SEARCH_HITS)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SelectUp, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, window, cx| this.step(1, window, cx)))
            .size_full()
            .child(
                h_resizable("search.hits.split")
                    .child(resizable_panel().child(rows))
                    .child(
                        resizable_panel()
                            .visible(open)
                            .size(px(DETAIL_WIDTH))
                            .border_l_1()
                            .border_color(cx.theme().border)
                            .child(
                                v_flex()
                                    .size_full()
                                    .child(div().flex_1().min_h_0().children(self.detail.clone()))
                                    .children(info.map(|info| {
                                        div()
                                            .id("search.match_info")
                                            .test_support()
                                            .aria_label(info.clone())
                                            .flex_none()
                                            .max_h(px(120.))
                                            .overflow_y_scroll()
                                            .p_1()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(info)
                                    })),
                            ),
                    ),
            )
    }
}
