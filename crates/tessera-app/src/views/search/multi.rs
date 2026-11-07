// SPDX-License-Identifier: GPL-3.0-or-later

//! Several searches in one tab: the tabs above the toolbar, what they send and what comes back.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::{
    App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::*,
};
use rust_i18n::t;
use tessera_client::{MULTI_SEARCH_LIMIT, MultiSearch};
use tessera_core::search::params::{ParamError, SearchDraft, to_search};

use super::{SearchWorkbench, param_error_text, value_text};
use crate::actions::{AddSearch, NextSearch, PreviousSearch, RemoveSearch, ToggleUnion, WORKBENCH};

/// The request for `drafts`. A union's page is the first draft's and goes in the URL, because
/// the server ignores it inside a search.
pub(super) fn multi_request(
    drafts: &[SearchDraft],
    union: bool,
) -> Result<MultiSearch, ParamError> {
    let mut searches = drafts
        .iter()
        .map(to_search)
        .collect::<Result<Vec<_>, _>>()?;
    let number = |key: &str| match drafts.first().and_then(|d| d.params.get(key)) {
        Some(value) => value_text(Some(value))
            .parse()
            .map(Some)
            .map_err(|_| ParamError::BadNumber(key.to_string())),
        None => Ok(None),
    };
    let (page, per_page) = if union {
        (number("page")?, number("per_page")?)
    } else {
        (None, None)
    };
    if union {
        for search in &mut searches {
            search.shift_remove("page");
            search.shift_remove("per_page");
        }
    }
    Ok(MultiSearch {
        union,
        searches,
        page,
        per_page,
    })
}

impl SearchWorkbench {
    /// The index of `results` on screen.
    pub(super) fn shown(&self) -> usize {
        self.result_tab.min(self.results.len().saturating_sub(1))
    }

    /// The draft the result on screen belongs to; a union pages through the first.
    pub(super) fn shown_draft(&self) -> usize {
        if self.union {
            return 0;
        }
        self.shown().min(self.drafts.len() - 1)
    }

    pub(super) fn show_result_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.result_tab = ix;
        if let Some(Ok(result)) = self.results.get(self.shown()) {
            self.hits.update(cx, |h, cx| h.set_result(result, cx));
        }
        cx.notify();
    }

    /// Why draft `ix` cannot be sent.
    fn param_error(&self, ix: usize) -> Option<SharedString> {
        let error = to_search(&self.drafts[ix]).err()?;
        Some(param_error_text(&error).into())
    }

    /// Text that does not parse belongs to the active draft, so it stays until it does.
    fn select_draft(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.json_error.is_some() || ix >= self.drafts.len() {
            return;
        }
        self.active = ix;
        self.sync_draft(window, cx);
        self.show_result_tab(ix, cx);
    }

    pub(super) fn add_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drafts.len() >= MULTI_SEARCH_LIMIT || self.json_error.is_some() {
            return;
        }
        self.drafts.push(self.draft().clone());
        self.select_draft(self.drafts.len() - 1, window, cx);
        self.changed(window, cx);
    }

    pub(super) fn remove_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drafts.len() < 2 || self.json_error.is_some() {
            return;
        }
        let ix = self.active;
        self.drafts.remove(ix);
        // Its answer would land on the tabs after the one that went.
        self.drop_run();
        if !self.union_answer && ix < self.results.len() {
            drop(self.results.remove(ix));
        }
        self.union &= self.drafts.len() > 1;
        self.select_draft(ix.min(self.drafts.len() - 1), window, cx);
        self.changed(window, cx);
    }

    /// The next search at `step`, wrapping around.
    pub(super) fn step_search(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.active.cast_signed() + step;
        let ix = next
            .rem_euclid(self.drafts.len().cast_signed())
            .cast_unsigned();
        self.select_draft(ix, window, cx);
    }

    pub(super) fn toggle_union(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drafts.len() < 2 {
            return;
        }
        self.union = !self.union;
        if let Some(wrapper) = self.preset.as_mut().and_then(|p| p.wrapper.as_mut()) {
            *wrapper = self.union;
        }
        self.sync_paging_lock(cx);
        self.changed(window, cx);
    }

    /// Tabs of the drafts, with what edits them.
    pub(super) fn search_tabs(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let danger = cx.theme().danger;
        let count = self.drafts.len();
        let blocked = self.json_error.is_some();
        let full = count >= MULTI_SEARCH_LIMIT;
        let tabs = (0..count).map(|ix| {
            let label = SharedString::from(t!("search.tab", n = ix + 1).to_string());
            let error = self.param_error(ix).map(|error| {
                div()
                    .id(SharedString::from(format!("search.tab.{ix}.error")))
                    .test_support()
                    .aria_label(error)
                    .text_color(danger)
                    .child(IconName::TriangleAlert)
            });
            Tab::new()
                .aria_label(label.clone())
                .disabled(blocked)
                .child(
                    div()
                        .id(SharedString::from(format!("search.tab.{ix}")))
                        .test_support()
                        .aria_label(label.clone())
                        .child(label),
                )
                .suffix(div().children(error))
        });
        let tabs: Vec<Tab> = tabs.collect();
        let step_button = |id: &'static str, icon: IconName, key: &str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .accessibility_label(t!(key).to_string())
                .tooltip(t!(key).to_string())
                .disabled(count < 2 || blocked)
        };
        h_flex()
            .flex_none()
            .gap_2()
            .pr_2()
            .items_center()
            .child(
                div().flex_1().min_w_0().child(
                    TabBar::new("search.tabs")
                        .underline()
                        .selected_index(self.active)
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            this.select_draft(*ix, window, cx);
                        }))
                        .children(tabs),
                ),
            )
            .child(
                step_button("search.previous", IconName::ChevronLeft, "search.previous")
                    .children(Kbd::binding_for_action(
                        &PreviousSearch,
                        Some(WORKBENCH),
                        window,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.step_search(-1, window, cx);
                    })),
            )
            .child(
                step_button("search.next", IconName::ChevronRight, "search.next")
                    .children(Kbd::binding_for_action(
                        &NextSearch,
                        Some(WORKBENCH),
                        window,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.step_search(1, window, cx);
                    })),
            )
            .child(
                Button::new("search.add")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(t!("search.add").to_string())
                    .children(Kbd::binding_for_action(&AddSearch, Some(WORKBENCH), window))
                    .disabled(full || blocked)
                    .on_click(cx.listener(|this, _, window, cx| this.add_search(window, cx))),
            )
            .child(
                Button::new("search.remove")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .label(t!("search.remove").to_string())
                    .children(Kbd::binding_for_action(
                        &RemoveSearch,
                        Some(WORKBENCH),
                        window,
                    ))
                    .disabled(count < 2 || blocked)
                    .on_click(cx.listener(|this, _, window, cx| this.remove_search(window, cx))),
            )
            .child(
                Checkbox::new("search.union")
                    .checked(self.union)
                    .disabled(count < 2)
                    .label(t!("search.union.label").to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_union(window, cx))),
            )
            .children(Kbd::binding_for_action(
                &ToggleUnion,
                Some(WORKBENCH),
                window,
            ))
    }

    /// Why Add does nothing more.
    pub(super) fn limit_note(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        if self.drafts.len() < MULTI_SEARCH_LIMIT {
            return None;
        }
        let note = SharedString::from(t!("search.limit", n = MULTI_SEARCH_LIMIT).to_string());
        Some(
            div()
                .id("search.add.limit")
                .test_support()
                .aria_label(note.clone())
                .flex_none()
                .px_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(note),
        )
    }

    /// One tab per answered search, when there are several.
    pub(super) fn result_tabs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.results.len() < 2 {
            return None;
        }
        let danger = cx.theme().danger;
        let tabs = self.results.iter().enumerate().map(|(ix, result)| {
            let label = SharedString::from(t!("search.tab", n = ix + 1).to_string());
            Tab::new().aria_label(label.clone()).child(
                div()
                    .id(SharedString::from(format!("search.result_tab.{ix}")))
                    .test_support()
                    .aria_label(label.clone())
                    .when(result.is_err(), |el| el.text_color(danger))
                    .child(label),
            )
        });
        let tabs: Vec<Tab> = tabs.collect();
        Some(
            TabBar::new("search.result_tabs")
                .underline()
                .selected_index(self.shown())
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    this.show_result_tab(*ix, cx);
                }))
                .children(tabs),
        )
    }

    /// The failure of the search on screen, whose neighbours may have answered.
    pub(super) fn item_error(&self) -> Option<SharedString> {
        let error = self.results.get(self.shown())?.as_ref().err()?;
        let text = t!(
            "search.item_error",
            code = error.code,
            message = error.message
        );
        Some(text.to_string().into())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::{Value, json};

    use super::*;

    fn draft(params: Value) -> SearchDraft {
        SearchDraft {
            collection: "books".into(),
            params: params.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn federated_keeps_each_page_in_its_search() {
        let drafts = [draft(json!({"page": 2})), draft(json!({"page": 3}))];
        let request = multi_request(&drafts, false).unwrap();
        assert_eq!((request.page, request.per_page), (None, None));
        assert_eq!(request.searches[1]["page"], json!(3));
    }

    #[test]
    fn union_reads_a_page_written_as_text() {
        let drafts = [draft(json!({"page": "4"})), draft(json!({}))];
        let request = multi_request(&drafts, true).unwrap();
        assert_eq!((request.page, request.per_page), (Some(4), None));
    }

    #[test]
    fn a_union_refuses_a_page_that_is_no_number() {
        let drafts = [draft(json!({"page": "two"})), draft(json!({}))];
        assert_eq!(
            multi_request(&drafts, true).unwrap_err(),
            ParamError::BadNumber("page".into())
        );
        let drafts = [draft(json!({"per_page": 1.5})), draft(json!({}))];
        assert_eq!(
            multi_request(&drafts, true).unwrap_err(),
            ParamError::BadNumber("per_page".into())
        );
        assert!(multi_request(&drafts, false).is_ok());
    }

    #[test]
    fn a_bad_draft_fails_the_request() {
        let drafts = [draft(json!({})), draft(json!({"Bad-Key": 1}))];
        assert_eq!(
            multi_request(&drafts, false).unwrap_err(),
            ParamError::BadKey("Bad-Key".into())
        );
    }
}
