// SPDX-License-Identifier: GPL-3.0-or-later

//! The console's list of favorites and past requests.

use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, IntoElement,
    ParentElement as _, ScrollStrategy, SharedString, Styled as _, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use rust_i18n::t;
use tessera_core::search::history::{HistoryEntry, matches};

use super::ConsoleView;
use crate::actions::{CONSOLE, OpenHistory, QUERY_LIST};

/// Height of a row of the list.
const ROW_HEIGHT: f32 = 44.;
/// Rows shown without scrolling.
const ROWS_SHOWN: usize = 8;
/// Width of the list.
const LIST_WIDTH: f32 = 460.;
/// Marks a favorite.
const STAR: &str = "★";

/// The list's state in the console.
pub(super) struct RequestList {
    open: bool,
    filter: Entity<InputState>,
    selected: usize,
    scroll: UniformListScrollHandle,
}

impl RequestList {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ConsoleView>) -> (Self, Subscription) {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("search.queries.filter").to_string())
        });
        let changed = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.list.selected = 0;
                cx.notify();
            }
        });
        let list = Self {
            open: false,
            filter,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
        };
        (list, changed)
    }
}

impl ConsoleView {
    /// Favorites first, then the history, each newest first, for the filter text.
    fn entries(&self, cx: &App) -> Vec<(HistoryEntry, bool)> {
        let needle = self.list.filter.read(cx).value().to_lowercase();
        let profile = self.profile_id(cx);
        let state = self.state.read(cx);
        let favorites = state.saved.favorites.iter().rev();
        let favorites = favorites
            .filter(|item| item.profile == profile && matches(item, &needle))
            .map(|item| (item.entry.clone(), true));
        let history = state.history.raw(profile, &needle);
        let history = history.into_iter().map(|item| (item.entry.clone(), false));
        favorites.chain(history).collect()
    }

    pub(super) fn toggle_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_list(!self.list.open, window, cx);
    }

    fn set_list(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.list.open = open;
        self.list.selected = 0;
        self.list
            .filter
            .update(cx, |f, cx| f.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn step_list(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.entries(cx).len();
        if !self.list.open || count == 0 {
            return;
        }
        let next = self.list.selected.saturating_add_signed(by).min(count - 1);
        self.list.selected = next;
        self.list
            .scroll
            .scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    pub(super) fn load_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.entries(cx).into_iter().nth(self.list.selected);
        let Some((entry, _)) = entry else {
            return;
        };
        self.set_list(false, window, cx);
        self.focus_handle.focus(window, cx);
        self.load(&entry, window, cx);
    }

    /// The trigger of the list with the list.
    pub(super) fn list_button(&self, window: &Window, cx: &mut Context<Self>) -> Popover {
        let (on_open, on_content) = (cx.entity(), cx.entity());
        let trigger = Button::new("console.history")
            .ghost()
            .small()
            .label(t!("console.history").to_string())
            .children(Kbd::binding_for_action(&OpenHistory, Some(CONSOLE), window));
        Popover::new("console.history.popover")
            .open(self.list.open)
            .track_focus(&self.list.filter.read(cx).focus_handle(cx))
            .on_open_change(move |open, window, cx| {
                on_open.update(cx, |this, cx| {
                    if *open != this.list.open {
                        this.set_list(*open, window, cx);
                    }
                });
            })
            .trigger(trigger)
            .content(move |_, _, cx| on_content.update(cx, |this, cx| this.list_content(cx)))
    }

    fn list_content(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .key_context(QUERY_LIST)
            .w(px(LIST_WIDTH))
            .gap_2()
            .p_1()
            .child(
                Input::new(&self.list.filter)
                    .id("console.history.filter")
                    .aria_label(t!("search.queries.filter").to_string())
                    .small(),
            )
            .child(self.list_rows(cx))
            .into_any_element()
    }

    fn list_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<(String, String)> = self
            .entries(cx)
            .iter()
            .filter_map(|(entry, favorite)| row(entry, *favorite))
            .collect();
        if rows.is_empty() {
            return div()
                .p_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t!("search.queries.empty").to_string())
                .into_any_element();
        }
        let count = rows.len();
        let selected = self.list.selected;
        let view = cx.entity();
        let rows = Rc::new(rows);
        let (muted, selected_bg) = (cx.theme().muted_foreground, cx.theme().secondary);
        uniform_list("console.history.list", count, move |range, _, _| {
            range
                .map(|ix| {
                    let (title, note) = rows[ix].clone();
                    let view = view.clone();
                    v_flex()
                        .id(SharedString::from(format!("console.history.row.{ix}")))
                        .test_support()
                        .aria_label(SharedString::from(format!("{title} {note}")))
                        .h(px(ROW_HEIGHT))
                        .px_2()
                        .justify_center()
                        .overflow_hidden()
                        .cursor_pointer()
                        .text_sm()
                        .when(ix == selected, |el| el.bg(selected_bg))
                        .on_click(move |_, window, cx| {
                            view.update(cx, |this, cx| {
                                this.list.selected = ix;
                                this.load_selected(window, cx);
                            });
                        })
                        .child(div().truncate().child(title))
                        .child(div().truncate().text_xs().text_color(muted).child(note))
                })
                .collect()
        })
        .track_scroll(&self.list.scroll)
        .h(px(ROW_HEIGHT * count.min(ROWS_SHOWN) as f32))
        .into_any_element()
    }
}

/// Title and note of a row.
fn row(entry: &HistoryEntry, favorite: bool) -> Option<(String, String)> {
    let HistoryEntry::Raw {
        method,
        path,
        query,
        body,
        body_dropped,
    } = entry
    else {
        return None;
    };
    let query = query.as_ref().map_or(String::new(), |q| format!("?{q}"));
    let star = if favorite { STAR } else { "" };
    let title = format!("{star} {method} {path}{query}");
    let note = match body {
        Some(body) => body.clone(),
        None if *body_dropped => t!("console.body_dropped").to_string(),
        None => String::new(),
    };
    Some((title.trim_start().to_string(), note))
}
