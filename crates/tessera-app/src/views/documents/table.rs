// SPDX-License-Identifier: GPL-3.0-or-later

//! The documents table: pages load as they become visible.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    App, ClickEvent, Context, Div, Entity, IntoElement, ParentElement as _, SharedString, Stateful,
    Styled as _, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{DocumentQuery, Error};
use tessera_core::documents::columns::{ColumnDef, merge_keys, sort_key};
use tessera_core::documents::page::{DocumentPage, PageCache, Row, row_position, wanted_pages};
use tessera_core::documents::views::CollectionView;
use tessera_core::documents::{PAGE_CACHE, PER_PAGE};
use tessera_core::profile::Node;

use super::columns::{MIN_WIDTH, visible_columns};
use super::copy;
use super::selection::Selection;
use crate::connection::{ConnectionState, error_text};
use crate::documents::{DocumentsBackendGlobal, PageResult};

const ROWS_PER_PAGE: usize = PER_PAGE as usize;
/// How often a scroll may start fetches; a running timer is left alone, so continuous
/// scrolling still fetches.
const THROTTLE: Duration = Duration::from_millis(80);
const MAX_INFLIGHT: usize = 4;
const DEFAULT_WIDTH: f32 = 160.;
const ID_WIDTH: f32 = 120.;
const NUMERIC_WIDTH: f32 = 100.;
/// Column 0 holds the checkboxes; `columns` starts at table column 1.
const SELECT_COLUMN: usize = 0;
const SELECT_WIDTH: f32 = 36.;
/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/documents.html";

/// Where pages come from. The client is read per request, so a reconnect needs no new table.
struct Target {
    connection: Entity<ConnectionState>,
    name: String,
}

/// The table's data: a window of pages over `found` rows.
pub struct DocRows {
    /// Visible columns, pinned ones first: `all_columns` seen through `prefs`.
    pub columns: Vec<ColumnDef>,
    /// Every column of the schema and every key seen in documents.
    pub all_columns: Vec<ColumnDef>,
    /// Hidden, pinned and sized columns of this collection.
    pub prefs: CollectionView,
    /// Sort key and whether it ascends.
    pub sort: Option<(String, bool)>,
    /// Documents matching the query.
    pub found: u64,
    /// Documents in the collection.
    pub out_of: u64,
    /// Server-side time of the last answer.
    pub search_time_ms: u64,
    /// `filter_by` of the query.
    pub filter: Option<String>,
    /// `exclude_fields` of the query.
    pub exclude: Option<String>,
    /// A page failed to load.
    pub error: Option<SharedString>,
    /// No answer has arrived yet.
    pub first_load: bool,
    /// A page has loaded, so the figures mean something.
    pub answered: bool,
    /// A new filter or sort failed; the old rows stay.
    pub query_error: Option<SharedString>,
    /// The ticked rows.
    pub selection: Selection,
    /// How the last delete failed.
    pub delete_error: Option<SharedString>,
    /// The last tick ran into `MAX_SELECTION`.
    pub selection_limit: bool,
    /// The last range passed over rows that are not loaded.
    pub selection_unloaded: bool,
    /// The query changed: what gets ticked on the old rows goes with them.
    query_changed: bool,
    /// The cached pages belong to the previous query.
    replacing: bool,
    generation: u64,
    pages: PageCache<Arc<DocumentPage>>,
    visible: Range<usize>,
    inflight: HashMap<u32, Task<()>>,
    throttle: Option<Task<()>>,
    target: Target,
}

impl DocRows {
    /// An empty table over collection `name` of `connection`, laid out by `prefs`.
    pub fn new(connection: Entity<ConnectionState>, name: String, prefs: CollectionView) -> Self {
        Self {
            columns: Vec::new(),
            all_columns: Vec::new(),
            prefs,
            sort: None,
            found: 0,
            out_of: 0,
            search_time_ms: 0,
            filter: None,
            exclude: None,
            error: None,
            first_load: true,
            answered: false,
            query_error: None,
            selection: Selection::default(),
            delete_error: None,
            selection_limit: false,
            selection_unloaded: false,
            query_changed: false,
            replacing: false,
            generation: 0,
            pages: PageCache::new(PAGE_CACHE),
            visible: 0..0,
            inflight: HashMap::new(),
            throttle: None,
            target: Target { connection, name },
        }
    }

    /// Pages held in memory.
    pub fn cached_pages(&self) -> usize {
        self.pages.len()
    }

    /// Shows `all_columns` as `prefs` say; the caller refreshes the table.
    pub fn apply_prefs(&mut self) {
        self.columns = visible_columns(&self.all_columns, &self.prefs);
    }

    /// The number of the current query.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A new query has not answered yet, so the rows on screen are not its rows.
    pub fn query_pending(&self) -> bool {
        self.query_changed
    }

    /// Column `col_ix` of the table, unless it is the checkboxes'.
    pub fn column_at(&self, col_ix: usize) -> Option<&ColumnDef> {
        self.columns.get(col_ix.checked_sub(1)?)
    }

    /// Ticks or unticks row `row_ix`, or with `extend` ticks up to it from the anchor.
    pub fn check(&mut self, row_ix: usize, extend: bool) {
        self.selection_unloaded = false;
        if extend {
            let skipped = Cell::new(false);
            let mut selection = std::mem::take(&mut self.selection);
            self.selection_limit = selection.extend(row_ix, |row| {
                let id = self.row(row).map(|r| r.id.to_string());
                skipped.set(skipped.get() || id.is_none());
                id
            });
            self.selection = selection;
            self.selection_unloaded = skipped.get();
        } else if let Some(id) = self.row(row_ix).map(|r| r.id.clone()) {
            self.selection_limit = self.selection.toggle(&id, row_ix);
        }
    }

    /// Ticks every row of the loaded pages.
    pub fn check_loaded(&mut self) {
        let last = (self.found as usize).div_ceil(ROWS_PER_PAGE) as u32;
        let pages = (1..=last).filter_map(|page| self.pages.get(page));
        let ids = pages.flat_map(|page| page.rows.iter().map(|r| r.id.to_string()));
        self.selection_limit = self.selection.add_all(ids);
        self.selection_unloaded = false;
    }

    /// Unticks everything.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.selection_limit = false;
        self.selection_unloaded = false;
    }

    /// Starts over with a new query, whose rows the selection does not apply to. The old rows
    /// stay until it answers.
    pub fn requery(&mut self, cx: &mut Context<TableState<Self>>) {
        self.clear_selection();
        self.delete_error = None;
        self.query_changed = true;
        self.reload(cx);
    }

    /// Asks the same query again; the rows stay until it answers.
    pub fn reload(&mut self, cx: &mut Context<TableState<Self>>) {
        self.generation += 1;
        self.inflight.clear();
        self.throttle = None;
        self.replacing = true;
        self.fetch(1, cx);
    }

    fn fetch_wanted(&mut self, cx: &mut Context<TableState<Self>>) {
        // The query was refused; only a new one can answer.
        if self.query_error.is_some() {
            return;
        }
        let wanted = wanted_pages(self.visible.clone(), ROWS_PER_PAGE, self.found);
        // Page 1 of a new query answers `replacing`, wherever the table scrolled meanwhile.
        let replacing = self.replacing;
        self.inflight
            .retain(|page, _| wanted.contains(page) || (replacing && *page == 1));
        for page in wanted {
            if self.inflight.len() >= MAX_INFLIGHT {
                break;
            }
            if self.pages.get(page).is_none() && !self.inflight.contains_key(&page) {
                self.fetch(page, cx);
            }
        }
    }

    fn fetch(&mut self, page: u32, cx: &mut Context<TableState<Self>>) {
        let Some(connected) = &self.target.connection.read(cx).connected else {
            return;
        };
        let query = DocumentQuery {
            page,
            per_page: PER_PAGE,
            filter_by: self.filter.clone(),
            sort_by: self.sort.as_ref().map(|(k, a)| sort_key(k, *a)),
            exclude_fields: self.exclude.clone(),
        };
        let load = cx.global::<DocumentsBackendGlobal>().0.page(
            connected.client.clone(),
            self.target.name.clone(),
            query,
        );
        let generation = self.generation;
        let task = cx.spawn(async move |table, cx| {
            let result = load.await;
            table
                .update(cx, |t, cx| {
                    if t.delegate_mut().finish(page, generation, result, cx) {
                        t.refresh(cx);
                    }
                    cx.notify();
                })
                .ok();
        });
        self.inflight.insert(page, task);
    }

    /// Takes in an answer; an answer to an older query is dropped. True when a new column
    /// appeared, which the caller shows through `TableState::refresh`.
    pub fn finish(
        &mut self,
        page: u32,
        generation: u64,
        result: Result<PageResult, Error>,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.inflight.remove(&page);
        self.first_load = false;
        // A refused query answers too: the old rows are then all there is.
        if std::mem::take(&mut self.query_changed) {
            self.clear_selection();
        }
        match result {
            Ok(answer) => {
                self.answered = true;
                let replaced = std::mem::take(&mut self.replacing);
                if replaced {
                    self.pages.clear();
                }
                let shown = self.columns.len();
                merge_keys(&mut self.all_columns, &answer.page.keys);
                self.apply_prefs();
                let new_column = self.columns.len() != shown;
                self.pages.insert(page, Arc::new(answer.page));
                self.found = answer.found;
                self.out_of = answer.out_of;
                self.search_time_ms = answer.search_time_ms;
                self.error = None;
                self.query_error = None;
                if replaced {
                    // The scroll position stays, and it may be far from page 1.
                    self.fetch_wanted(cx);
                }
                new_column
            }
            Err(e) => {
                let text = error_text(&e, &self.target.connection.read(cx).profile).into();
                // With nothing on screen yet there are no old rows for it to stand beside.
                if self.replacing && self.answered {
                    self.query_error = Some(text);
                } else {
                    self.error = Some(text);
                }
                false
            }
        }
    }

    /// Row `row_ix`, if its page is loaded.
    pub fn row(&self, row_ix: usize) -> Option<&Row> {
        let (page, offset) = row_position(row_ix, ROWS_PER_PAGE);
        self.pages.get(page)?.rows.get(offset)
    }

    fn text(&self, row_ix: usize, col_ix: usize) -> SharedString {
        if col_ix == SELECT_COLUMN {
            return SharedString::default();
        }
        let Some(row) = self.row(row_ix) else {
            return t!("documents.loading_row").to_string().into();
        };
        self.column_at(col_ix)
            .and_then(|c| row.cells.get(&c.key))
            .map(|cell| SharedString::from(cell.to_string()))
            .unwrap_or_default()
    }
}

impl DocRows {
    /// The checkbox of a loaded row. The click is the wrapper's: the checkbox gives no shift
    /// state, and the cell must not take it.
    fn render_check(
        &self,
        row_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement + use<> {
        let check = self.row(row_ix).map(|row| {
            Checkbox::new(SharedString::from(format!("documents.check.{}", row.id)))
                .accessibility_label(row.id.to_string())
                .checked(self.selection.contains(&row.id))
        });
        div()
            .id(("documents.select", row_ix))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(move |t, event: &ClickEvent, _, cx| {
                cx.stop_propagation();
                t.delegate_mut().check(row_ix, event.modifiers().shift);
                cx.notify();
            }))
            .children(check)
    }
}

impl TableDelegate for DocRows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len() + 1
    }

    fn rows_count(&self, _: &App) -> usize {
        self.found as usize
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        if col_ix == SELECT_COLUMN {
            return Column::new("__select", "")
                .width(px(SELECT_WIDTH))
                .fixed_left()
                .selectable(false)
                .resizable(false)
                .p_0();
        }
        let Some(def) = self.column_at(col_ix) else {
            return Column::default();
        };
        let default = match (def.key.as_str(), def.numeric) {
            ("id", _) => ID_WIDTH,
            (_, true) => NUMERIC_WIDTH,
            _ => DEFAULT_WIDTH,
        };
        let width = self.prefs.widths.get(&def.key).copied().unwrap_or(default);
        let column = Column::new(def.key.clone(), def.key.clone())
            .width(px(width))
            .min_width(px(MIN_WIDTH));
        let column = if def.numeric {
            column.text_right()
        } else {
            column
        };
        let column = if self.prefs.pinned.contains(&def.key) {
            column.fixed_left()
        } else {
            column
        };
        match &self.sort {
            Some((key, true)) if *key == def.key => column.ascending(),
            Some((key, false)) if *key == def.key => column.descending(),
            _ if def.sortable => column.sortable(),
            _ => column,
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(key) = self.column_at(col_ix).map(|c| c.key.clone()) else {
            return;
        };
        self.sort = match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some((key, true)),
            ColumnSort::Descending => Some((key, false)),
        };
        self.requery(cx);
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        match self.row(row_ix) {
            Some(row) => div()
                .id(SharedString::from(format!("documents.row.{}", row.id)))
                .when(self.selection.contains(&row.id), |tr| {
                    tr.bg(cx.theme().tokens.table_active)
                }),
            None => div().id(("documents.placeholder", row_ix)),
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if col_ix == SELECT_COLUMN {
            return self.render_check(row_ix, cx).into_any_element();
        }
        let text = self.text(row_ix, col_ix);
        let (Some(row), Some(column)) = (self.row(row_ix), self.column_at(col_ix)) else {
            return div()
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element();
        };
        div()
            .id(SharedString::from(format!(
                "documents.cell.{}.{}",
                row.id, column.key
            )))
            .test_support()
            .aria_label(text.clone())
            .truncate()
            .child(text)
            .into_any_element()
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.text(row_ix, col_ix))
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.text(row_ix, col_ix).to_string()
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // A failed load is not an empty collection; the view shows the error.
        if self.error.is_some() || self.query_error.is_some() {
            return div().into_any_element();
        }
        let filtered = self.filter.is_some();
        let seed = !filtered
            && self
                .target
                .connection
                .read(cx)
                .profile
                .nodes
                .iter()
                .any(Node::is_local);
        let key = if filtered {
            "documents.no_match"
        } else {
            "documents.empty"
        };
        let title = t!(key).to_string();
        let seed_hint = t!("documents.seed_hint").to_string();
        let label = if seed {
            format!("{title} {seed_hint}")
        } else {
            title.clone()
        };
        v_flex()
            .id("documents.empty")
            .test_support()
            .aria_label(SharedString::from(label))
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(title)
            .when(!filtered, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("documents.empty_hint").to_string()),
                )
                .child(
                    Button::new("documents.docs")
                        .link()
                        .label(t!("documents.docs").to_string())
                        .on_click(|_, _, cx| cx.open_url(DOCS_URL)),
                )
            })
            .when(seed, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(seed_hint),
                )
            })
            .into_any_element()
    }

    // Only for a click beside the last column: on a cell the kit stops the click before its
    // context menu sees it, and `DocumentsView` opens the same list itself.
    fn context_menu(
        &mut self,
        _: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        copy::entries(menu)
    }

    fn loading(&self, _: &App) -> bool {
        self.first_load
    }

    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if !visible_range.is_empty() {
            let (first, _) = row_position(visible_range.start, ROWS_PER_PAGE);
            let (last, _) = row_position(visible_range.end - 1, ROWS_PER_PAGE);
            for page in first..=last {
                self.pages.touch(page);
            }
        }
        self.visible = visible_range;
        if self.throttle.is_none() {
            self.throttle = Some(cx.spawn(async move |table, cx| {
                cx.background_executor().timer(THROTTLE).await;
                table
                    .update(cx, |t, cx| {
                        let rows = t.delegate_mut();
                        rows.throttle = None;
                        rows.fetch_wanted(cx);
                    })
                    .ok();
            }));
        }
    }
}
