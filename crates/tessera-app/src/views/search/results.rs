// SPDX-License-Identifier: GPL-3.0-or-later

//! What surrounds the hit list: status line, facets, paging and the empty result.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_core::search::facet::{facet_active, facet_value_ok, toggle_facet};
use tessera_core::search::results::{Facet, SearchResult};

use super::super::overview::grouped;
use super::{SearchWorkbench, value_text};
use crate::actions::{NextResultPage, PreviousResultPage, WORKBENCH};

/// The server's `per_page` when a search does not set one.
const DEFAULT_PER_PAGE: u64 = 10;
const FACETS_WIDTH: f32 = 220.;

/// Pages of `found` results at `per_page`.
fn page_count(found: u64, per_page: u64) -> u64 {
    found.div_ceil(per_page.max(1))
}

/// The status line of `result`. A union's `found` counts a document once per search and moves
/// with the page, so it is only ever approximate.
pub(super) fn status(result: &SearchResult, union: bool) -> SharedString {
    let (found, out_of, ms) = (
        grouped(result.found),
        grouped(result.out_of),
        result.search_time_ms,
    );
    let line = match result.found_docs {
        Some(docs) => t!(
            "search.status_groups",
            found = found,
            docs = grouped(docs),
            out_of = out_of,
            ms = ms
        ),
        None => t!("search.status", found = found, out_of = out_of, ms = ms),
    };
    if union {
        let note = t!("search.union.before_duplicates");
        return format!("{line} · {note}").into();
    }
    line.to_string().into()
}

impl SearchWorkbench {
    fn shown_param(&self, key: &str) -> String {
        value_text(self.drafts[self.shown_draft()].params.get(key))
    }

    fn per_page(&self) -> u64 {
        let per_page = self.shown_param("per_page").parse();
        per_page.unwrap_or(DEFAULT_PER_PAGE)
    }

    /// The draft's page, not the result's: a quick second Next goes on from the first.
    fn page(&self) -> u64 {
        let page = self.shown_param("page").parse();
        page.unwrap_or(1)
    }

    /// The last page of `result`. A union's `found` overstates, so a short page ends it.
    fn last_page(&self, result: &SearchResult) -> u64 {
        let per_page = self.per_page();
        let pages = page_count(result.found, per_page);
        if pages > 0 && self.union_answer && (result.hits.len() as u64) < per_page {
            return self.page();
        }
        pages
    }

    /// Writes `page` and runs, when `step` stays within the result.
    pub(super) fn turn_page(&mut self, step: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(result) = self.result() else {
            return;
        };
        let pages = self.last_page(result);
        // Clamped: an edit in JSON can leave the draft past the end.
        let page = self
            .page()
            .saturating_add_signed(step)
            .clamp(1, pages.max(1));
        if page != self.page() {
            self.write_and_run(&[("page", Some(page.into()))], window, cx);
        }
    }

    fn field_type(&self, field: &str, cx: &App) -> String {
        let collection = &self.drafts[self.shown_draft()].collection;
        let schema = self.schema_of(collection, cx).into_iter();
        let found = schema.flat_map(|c| &c.fields).find(|f| f.name == field);
        found.map_or_else(|| "string".to_string(), |f| f.r#type.clone())
    }

    fn filter_text(&self) -> String {
        self.shown_param("filter_by")
    }

    /// Adds `value` to the filter or takes it out, from the first page on, and runs.
    fn toggle_facet(
        &mut self,
        field: &str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field_type = self.field_type(field, cx);
        let Some(filter) = toggle_facet(&self.filter_text(), field, &field_type, value) else {
            return;
        };
        let filter = (!filter.is_empty()).then(|| Value::from(filter));
        self.write_and_run(&[("filter_by", filter), ("page", None)], window, cx);
    }

    fn facet_section(&self, facet: &Facet, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let field = facet.field.clone();
        let field_type = self.field_type(&field, cx);
        let filter = self.filter_text();
        let blocked = self.json_error.is_some();
        let sampled = facet.sampled.then(|| {
            let note = SharedString::from(t!("search.facet.sampled").to_string());
            div()
                .id(SharedString::from(format!("search.facet.{field}.sampled")))
                .test_support()
                .aria_label(note.clone())
                .text_xs()
                .text_color(muted)
                .child(note)
        });
        let values: Vec<_> = facet
            .counts
            .iter()
            .map(|(value, count)| {
                let id = format!("search.facet.{field}.{value}");
                let writable = facet_value_ok(&field_type, value);
                let note = (!writable).then(|| {
                    let note = SharedString::from(t!("search.facet.unwritable").to_string());
                    div()
                        .id(SharedString::from(format!("{id}.note")))
                        .test_support()
                        .aria_label(note.clone())
                        .text_xs()
                        .text_color(muted)
                        .child(note)
                });
                let (field, name) = (field.clone(), value.clone());
                v_flex()
                    .child(
                        Checkbox::new(SharedString::from(id))
                            .checked(facet_active(&filter, &field, value))
                            .disabled(!writable || blocked)
                            .label(format!("{value} ({})", grouped(*count)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_facet(&field, &name, window, cx);
                            })),
                    )
                    .children(note)
            })
            .collect();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(facet.field.clone()),
            )
            .children(sampled)
            .children(values)
    }

    /// The facet panel; `None` for a result without facets.
    pub(super) fn facet_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        // A union's counts mix searches, and a click could only reach one of them.
        let facets = self.result()?.facets.clone();
        if facets.is_empty() || self.union || self.union_answer {
            return None;
        }
        let sections: Vec<_> = facets.iter().map(|f| self.facet_section(f, cx)).collect();
        Some(
            v_flex()
                .id("search.facets")
                .flex_none()
                .w(px(FACETS_WIDTH))
                .h_full()
                .overflow_y_scroll()
                .gap_3()
                .p_2()
                .border_l_1()
                .border_color(cx.theme().border)
                .children(sections),
        )
    }

    /// Previous and Next, under the list.
    pub(super) fn pager(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let result = self.result()?;
        let page = self.page();
        let pages = self.last_page(result);
        if pages == 0 {
            return None;
        }
        let blocked = self.json_error.is_some();
        Some(
            h_flex()
                .flex_none()
                .gap_2()
                .p_1()
                .items_center()
                .text_sm()
                .child(
                    Button::new("search.page.previous")
                        .ghost()
                        .small()
                        .label(t!("search.page.previous").to_string())
                        .children(Kbd::binding_for_action(
                            &PreviousResultPage,
                            Some(WORKBENCH),
                            window,
                        ))
                        .disabled(page <= 1 || blocked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.turn_page(-1, window, cx);
                        })),
                )
                .child(t!("search.page.of", page = page, pages = pages).to_string())
                .child(
                    Button::new("search.page.next")
                        .ghost()
                        .small()
                        .label(t!("search.page.next").to_string())
                        .children(Kbd::binding_for_action(
                            &NextResultPage,
                            Some(WORKBENCH),
                            window,
                        ))
                        .disabled(page >= pages || blocked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.turn_page(1, window, cx);
                        })),
                ),
        )
    }

    /// Why the list is empty: nothing matched, or the page lies past the end.
    pub(super) fn no_hits(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let result = self.result()?;
        if !result.hits.is_empty() || !result.groups.is_empty() {
            return None;
        }
        let q = self.shown_param("q");
        let filter = self.filter_text();
        let (id, text) = if result.found > 0 && self.union_answer {
            ("search.end", t!("search.union.end"))
        } else if result.found > 0 {
            let found = grouped(result.found);
            ("search.past_end", t!("search.past_end", found = found))
        } else if filter.is_empty() {
            ("search.no_hits", t!("search.no_hits", q = q))
        } else {
            let text = t!("search.no_hits_filter", q = q, filter = filter);
            ("search.no_hits", text)
        };
        let text = SharedString::from(text.to_string());
        Some(
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .p_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
    }
}
