// SPDX-License-Identifier: GPL-3.0-or-later

//! The documents view of a collection tab.

pub mod table;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*,
};
use rust_i18n::t;
use tessera_client::Collection;
use tessera_core::documents::columns::{columns_from_schema, exclude_fields, merge_keys};

use self::table::DocRows;
use super::overview::grouped;
use crate::actions::{DOCUMENTS, SortByColumn};
use crate::connection::{CollectionChanged, ConnectionState};
use crate::state::AppState;

/// What `key` being sorted again leads to: off, descending, ascending, off.
fn next_sort(current: Option<(String, bool)>, key: String) -> Option<(String, bool)> {
    match current {
        Some((k, false)) if k == key => Some((key, true)),
        Some((k, true)) if k == key => None,
        _ => Some((key, false)),
    }
}

/// The documents of one collection, as a table.
pub struct DocumentsView {
    name: String,
    has_schema: bool,
    table: Entity<TableState<DocRows>>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl DocumentsView {
    /// The view of collection `name`; it loads once the schema arrives.
    pub fn new(
        _state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = DocRows::new(connection.clone(), name.clone());
        let table = cx.new(|cx| {
            TableState::new(rows, window, cx)
                .cell_selectable(true)
                .row_header(false)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
        });
        Self {
            _subscriptions: [
                cx.observe(&table, |_, _, cx| cx.notify()),
                cx.subscribe(&connection, |this, _, CollectionChanged(name), cx| {
                    if *name == this.name {
                        this.reload(cx);
                    }
                }),
            ],
            name,
            has_schema: false,
            table,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Takes the columns from `collection`; the first call loads page 1, because the table
    /// reports no visible rows while it has none.
    pub fn set_schema(&mut self, collection: Collection, _: &mut Window, cx: &mut Context<Self>) {
        let first = !std::mem::replace(&mut self.has_schema, true);
        self.table.update(cx, |t, cx| {
            let rows = t.delegate_mut();
            let seen: Vec<String> = rows
                .columns
                .iter()
                .filter(|c| !c.in_schema)
                .map(|c| c.key.clone())
                .collect();
            rows.columns = columns_from_schema(&collection);
            merge_keys(&mut rows.columns, &seen);
            rows.exclude = exclude_fields(&collection);
            t.refresh(cx);
            if first {
                t.delegate_mut().requery(cx);
            }
            cx.notify();
        });
    }

    /// The schema did not load, so no page will: says why instead of loading forever.
    pub fn schema_failed(&mut self, error: SharedString, cx: &mut Context<Self>) {
        if self.has_schema {
            return;
        }
        self.table.update(cx, |t, cx| {
            let rows = t.delegate_mut();
            rows.first_load = false;
            rows.query_error = Some(error);
            cx.notify();
        });
    }

    /// Whether the columns are known.
    pub fn has_schema(&self) -> bool {
        self.has_schema
    }

    /// Drops the pages and loads them again; the rows and the scroll position stay until
    /// the answer.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.has_schema {
            self.table.update(cx, |t, cx| {
                t.delegate_mut().requery(cx);
                cx.notify();
            });
        }
    }

    /// Documents matching the query.
    pub fn found(&self, cx: &App) -> u64 {
        self.table.read(cx).delegate().found
    }

    /// The table.
    pub fn table(&self) -> &Entity<TableState<DocRows>> {
        &self.table
    }

    /// Sorts by the selected cell's column: the keyboard path to the header's sort icon.
    fn sort_selected(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let Some((_, col)) = t.selected_cell() else {
                return;
            };
            let rows = t.delegate_mut();
            let Some(key) = rows
                .columns
                .get(col)
                .filter(|c| c.sortable)
                .map(|c| c.key.clone())
            else {
                return;
            };
            rows.sort = next_sort(rows.sort.take(), key);
            t.refresh(cx);
            t.delegate_mut().requery(cx);
            cx.notify();
        });
    }

    fn status(&self, window: &Window, cx: &App) -> Option<impl IntoElement + use<>> {
        let rows = self.table.read(cx).delegate();
        if rows.first_load || !self.has_schema {
            return None;
        }
        let mut parts = vec![
            t!(
                "documents.status_count",
                found = grouped(rows.found),
                total = grouped(rows.out_of)
            )
            .to_string(),
            t!("documents.status_time", ms = rows.search_time_ms).to_string(),
        ];
        if let Some((key, ascending)) = &rows.sort {
            let arrow = if *ascending { "↑" } else { "↓" };
            parts.push(t!("documents.status_sort", field = key, arrow = arrow).to_string());
        }
        let text = SharedString::from(parts.join(" · "));
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .id("documents.status")
                        .test_support()
                        .aria_label(text.clone())
                        .child(text),
                )
                .children(Kbd::binding_for_action(
                    &SortByColumn,
                    Some(DOCUMENTS),
                    window,
                )),
        )
    }
}

impl Render for DocumentsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.table.read(cx).delegate();
        let error = rows.query_error.clone().or_else(|| rows.error.clone());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(DOCUMENTS)
            .on_action(cx.listener(|this, _: &SortByColumn, _, cx| this.sort_selected(cx)))
            .size_full()
            .p_3()
            .gap_2()
            .children(self.status(window, cx))
            .children(error.map(|e| {
                div()
                    .id("documents.error")
                    .test_support()
                    .aria_label(e.clone())
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(e)
            }))
            .child(
                div()
                    .id("documents.table")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(&self.table).bordered(true)),
            )
    }
}
