// SPDX-License-Identifier: GPL-3.0-or-later

//! The collections tab of a connection (spec §7.3): search, a sortable table of the
//! collections with their counts and aliases, favorites first.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, Keystroke,
    ParentElement as _, Render, SharedString, Stateful, Styled as _, Subscription, Task, Window,
    div, prelude::*, px, transparent_black,
};
use rust_i18n::t;
use tessera_client::{Alias, Collection};
use tessera_core::profile::Node;

use super::overview::grouped;
use crate::actions::{
    COLLECTIONS, COLLECTIONS_HEADER, FocusSearch, OpenCollectionRow, Refresh, ToggleFavorite,
};
use crate::collections::aliases_of;
use crate::connection::ConnectionState;
use crate::state::AppState;

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/collections.html";

/// What a column sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Collection name.
    Name,
    /// Number of documents.
    Documents,
    /// Number of fields.
    Fields,
    /// Creation time.
    Created,
    /// Alias names.
    Aliases,
    /// Number of synonym sets.
    SynonymSets,
    /// Number of curation sets.
    CurationSets,
}

/// One collection as the table shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Collection name.
    pub name: String,
    /// Number of documents.
    pub documents: u64,
    /// Number of fields.
    pub fields: usize,
    /// Creation time, Unix seconds.
    pub created_at: i64,
    /// Names of the aliases pointing here.
    pub aliases: Vec<String>,
    /// Number of synonym sets.
    pub synonym_sets: usize,
    /// Number of curation sets.
    pub curation_sets: usize,
}

impl Row {
    /// The row of `collection`.
    pub fn new(collection: &Collection, aliases: &[Alias]) -> Self {
        Self {
            name: collection.name.clone(),
            documents: collection.num_documents,
            fields: collection.fields.len(),
            created_at: collection.created_at,
            aliases: aliases_of(aliases, &collection.name)
                .map(str::to_owned)
                .collect(),
            synonym_sets: collection.synonym_sets.len(),
            curation_sets: collection.curation_sets.len(),
        }
    }

    /// Whether `query` (lowercase) occurs in the name or an alias.
    fn matches(&self, query: &str) -> bool {
        self.name.to_lowercase().contains(query)
            || self
                .aliases
                .iter()
                .any(|a| a.to_lowercase().contains(query))
    }
}

/// Sorts `rows` by `key`, favorites always first.
pub fn sort_rows(rows: &mut [Row], key: SortKey, ascending: bool, favorites: &[String]) {
    rows.sort_by(|a, b| {
        let favorite = |r: &Row| favorites.contains(&r.name);
        let order = match key {
            SortKey::Name => a.name.cmp(&b.name),
            SortKey::Documents => a.documents.cmp(&b.documents),
            SortKey::Fields => a.fields.cmp(&b.fields),
            SortKey::Created => a.created_at.cmp(&b.created_at),
            SortKey::Aliases => a.aliases.cmp(&b.aliases),
            SortKey::SynonymSets => a.synonym_sets.cmp(&b.synonym_sets),
            SortKey::CurationSets => a.curation_sets.cmp(&b.curation_sets),
        };
        favorite(b)
            .cmp(&favorite(a))
            .then(if ascending { order } else { order.reverse() })
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The columns, in order.
#[derive(Clone, Copy, PartialEq)]
enum Col {
    Favorite,
    Sorted(SortKey),
}

const COLUMNS: [Col; 8] = [
    Col::Favorite,
    Col::Sorted(SortKey::Name),
    Col::Sorted(SortKey::Documents),
    Col::Sorted(SortKey::Fields),
    Col::Sorted(SortKey::Created),
    Col::Sorted(SortKey::Aliases),
    Col::Sorted(SortKey::SynonymSets),
    Col::Sorted(SortKey::CurationSets),
];

impl Col {
    /// The suffix of the header's test id and locale key.
    fn key(self) -> &'static str {
        match self {
            Col::Favorite => "favorite",
            Col::Sorted(SortKey::Name) => "name",
            Col::Sorted(SortKey::Documents) => "documents",
            Col::Sorted(SortKey::Fields) => "fields",
            Col::Sorted(SortKey::Created) => "created",
            Col::Sorted(SortKey::Aliases) => "aliases",
            Col::Sorted(SortKey::SynonymSets) => "synonym_sets",
            Col::Sorted(SortKey::CurationSets) => "curation_sets",
        }
    }

    fn width(self) -> f32 {
        match self {
            Col::Favorite => 44.,
            Col::Sorted(SortKey::Name) => 240.,
            Col::Sorted(SortKey::Created) => 150.,
            Col::Sorted(SortKey::Aliases) => 220.,
            Col::Sorted(SortKey::SynonymSets | SortKey::CurationSets) => 120.,
            Col::Sorted(_) => 100.,
        }
    }

    fn numeric(self) -> bool {
        matches!(
            self,
            Col::Sorted(
                SortKey::Documents | SortKey::Fields | SortKey::SynonymSets | SortKey::CurationSets
            )
        )
    }

    fn title(self) -> SharedString {
        t!(format!("collections.col.{}", self.key()))
            .to_string()
            .into()
    }
}

/// `created_at` in local time, or "–" when the server sent none.
pub(super) fn created_text(secs: i64) -> String {
    let local = jiff::Timestamp::from_second(secs)
        .ok()
        .filter(|_| secs > 0)
        .map(|ts| ts.to_zoned(jiff::tz::TimeZone::system()));
    match local {
        Some(at) => at.strftime("%Y-%m-%d %H:%M").to_string(),
        None => t!("figure.missing").to_string(),
    }
}

/// The table's data: every collection, and the part that passes the search.
struct Rows {
    all: Vec<Row>,
    /// Indexes into `all` that pass `query`, in `all`'s order.
    visible: Vec<usize>,
    query: String,
    favorites: Vec<String>,
    sort: (SortKey, bool),
    headers: Vec<gpui_kit::FocusHandle>,
}

impl Rows {
    /// Sorts and filters again.
    fn rebuild(&mut self) {
        sort_rows(&mut self.all, self.sort.0, self.sort.1, &self.favorites);
        let query = self.query.trim().to_lowercase();
        self.visible = (0..self.all.len())
            .filter(|i| query.is_empty() || self.all[*i].matches(&query))
            .collect();
    }

    fn row(&self, row_ix: usize) -> Option<&Row> {
        self.all.get(*self.visible.get(row_ix)?)
    }

    /// Sorts by `key`: the same column flips, another starts A to Z (name) or largest first.
    fn sort_by(&mut self, key: SortKey) {
        self.sort = if self.sort.0 == key {
            (key, !self.sort.1)
        } else {
            (key, key == SortKey::Name)
        };
        self.rebuild();
    }

    fn cell(&self, row: &Row, col: Col) -> String {
        match col {
            Col::Favorite => String::new(),
            Col::Sorted(SortKey::Name) => row.name.clone(),
            Col::Sorted(SortKey::Documents) => grouped(row.documents),
            Col::Sorted(SortKey::Fields) => grouped(row.fields as u64),
            Col::Sorted(SortKey::Created) => created_text(row.created_at),
            Col::Sorted(SortKey::Aliases) => row.aliases.join(", "),
            Col::Sorted(SortKey::SynonymSets) => grouped(row.synonym_sets as u64),
            Col::Sorted(SortKey::CurationSets) => grouped(row.curation_sets as u64),
        }
    }
}

impl TableDelegate for Rows {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let col = COLUMNS[col_ix];
        let column = Column::new(col.key(), col.title())
            .width(px(col.width()))
            .resizable(false)
            .movable(false);
        if col.numeric() {
            column.text_right()
        } else {
            column
        }
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = COLUMNS[col_ix];
        let title = col.title();
        let id = SharedString::from(format!("collections.col.{}", col.key()));
        let Col::Sorted(key) = col else {
            // Favorites always come first, so there is nothing to sort by here.
            return div()
                .id(id)
                .test_support()
                .aria_label(title.clone())
                .size_full()
                .child(title)
                .into_any_element();
        };
        let focus = &self.headers[col_ix];
        let arrow = (self.sort.0 == key).then(|| {
            Icon::new(if self.sort.1 {
                IconName::ArrowUp
            } else {
                IconName::ArrowDown
            })
            .xsmall()
        });
        let enter = focus
            .is_focused(window)
            .then(|| Keystroke::parse("enter").ok().map(Kbd::new))
            .flatten();
        h_flex()
            .id(id)
            .test_support()
            .track_focus(focus)
            .key_context(COLLECTIONS_HEADER)
            .aria_label(title.clone())
            .size_full()
            .gap_1()
            .px_1()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(transparent_black())
            .when(col.numeric(), |el| el.justify_end())
            .focus(|s| s.border_color(cx.theme().ring))
            .on_click(cx.listener(move |table, _, _, cx| {
                table.delegate_mut().sort_by(key);
                cx.notify();
            }))
            .child(title)
            .children(arrow)
            .children(enter)
            .into_any_element()
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        let Some(row) = self.row(row_ix) else {
            return div().id(("collections.filler", row_ix));
        };
        let label = COLUMNS[1..]
            .iter()
            .map(|c| self.cell(row, *c))
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .id(SharedString::from(format!("collections.row.{}", row.name)))
            .aria_label(label)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = COLUMNS[col_ix];
        let Some(row) = self.row(row_ix) else {
            return div().into_any_element();
        };
        if col == Col::Favorite {
            let star = self.favorites.contains(&row.name).then(|| {
                Icon::new(gpui_kit::assets::IconName::Star)
                    .small()
                    .text_color(cx.theme().warning)
            });
            return div().children(star).into_any_element();
        }
        div()
            .truncate()
            .child(self.cell(row, col))
            .into_any_element()
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let col = COLUMNS[col_ix];
        let row = self.row(row_ix)?;
        Some(match col {
            Col::Favorite if self.favorites.contains(&row.name) => {
                t!("collections.col.favorite").to_string().into()
            }
            _ => self.cell(row, col).into(),
        })
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: gpui_kit::component::menu::PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::component::menu::PopupMenu {
        let Some(row) = self.row(row_ix) else {
            return menu;
        };
        let toggle = if self.favorites.contains(&row.name) {
            t!("collections.favorite_remove")
        } else {
            t!("collections.favorite_add")
        };
        menu.menu(
            t!("collections.open").to_string(),
            Box::new(OpenCollectionRow),
        )
        .menu(toggle.to_string(), Box::new(ToggleFavorite))
    }
}

/// The content of a collections tab.
pub struct CollectionsList {
    connection: Entity<ConnectionState>,
    state: Entity<AppState>,
    table: Entity<TableState<Rows>>,
    search: Entity<InputState>,
    /// Name of the selected row; survives re-sorting.
    selected: Option<String>,
    /// Why the last favorite save failed, shown under the toolbar.
    favorite_error: Option<SharedString>,
    save_task: Option<Task<()>>,
    /// What the rows were built from.
    loaded: Option<(Option<Instant>, Vec<String>)>,
    _subscriptions: Vec<Subscription>,
}

impl CollectionsList {
    /// The list of `connection`'s collections; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let headers = COLUMNS
            .iter()
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect();
        let rows = Rows {
            all: Vec::new(),
            visible: Vec::new(),
            query: String::new(),
            favorites: Vec::new(),
            sort: (SortKey::Name, true),
            headers,
        };
        let table = cx.new(|cx| {
            TableState::new(rows, window, cx)
                .sortable(false)
                .col_movable(false)
                .col_resizable(false)
                .col_selectable(false)
                .row_header(false)
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("collections.search").to_string())
        });
        let subscriptions = vec![
            cx.observe(&connection, |this, _, cx| this.sync(cx)),
            cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = search.read(cx).value().to_string();
                    this.table.update(cx, |t, cx| {
                        t.delegate_mut().query = query;
                        t.delegate_mut().rebuild();
                        cx.notify();
                    });
                    this.reselect(cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &table,
                window,
                |this, table, event: &TableEvent, window, cx| match event {
                    TableEvent::SelectRow(ix) => {
                        this.selected = table.read(cx).delegate().row(*ix).map(|r| r.name.clone());
                    }
                    TableEvent::RightClickedRow(Some(ix)) => {
                        // The menu's actions act on the selected row.
                        table.update(cx, |t, cx| t.set_selected_row(*ix, cx));
                        table.read(cx).focus_handle(cx).focus(window, cx);
                    }
                    TableEvent::DoubleClickedRow(_) => this.open_selected(cx),
                    _ => {}
                },
            ),
        ];
        let mut this = Self {
            connection,
            state,
            table,
            search,
            selected: None,
            favorite_error: None,
            save_task: None,
            loaded: None,
            _subscriptions: subscriptions,
        };
        this.connection
            .update(cx, |c, cx| c.load_collection_list(cx));
        this.sync(cx);
        this
    }

    /// Rebuilds the rows when the cache or the favorites changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let conn = self.connection.read(cx);
        let cache = &conn.collection_cache;
        let stamp = (cache.loaded_at, conn.profile.favorite_collections.clone());
        if self.loaded.as_ref() != Some(&stamp) {
            let rows: Vec<Row> = cache
                .collections
                .iter()
                .flatten()
                .map(|c| Row::new(c, &cache.aliases))
                .collect();
            self.table.update(cx, |t, cx| {
                let d = t.delegate_mut();
                d.all = rows;
                d.favorites = stamp.1.clone();
                d.rebuild();
                cx.notify();
            });
            self.loaded = Some(stamp);
            self.reselect(cx);
        }
        cx.notify();
    }

    /// Selects the row of the selected name again after the order changed.
    fn reselect(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        self.table.update(cx, |t, cx| {
            let d = t.delegate();
            let ix = d.visible.iter().position(|i| d.all[*i].name == name);
            match ix {
                Some(ix) if t.selected_row() != Some(ix) => t.set_selected_row(ix, cx),
                None => t.clear_selection(cx),
                _ => {}
            }
        });
    }

    /// The selected collection, only while its row is on screen: a search that hides it
    /// leaves nothing for the row actions to act on.
    fn visible_selected(&self, cx: &App) -> Option<String> {
        let table = self.table.read(cx);
        let ix = table.selected_row()?;
        table.delegate().row(ix).map(|r| r.name.clone())
    }

    /// Opens the tab of the selected collection.
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.visible_selected(cx) else {
            return;
        };
        let id = self.connection.read(cx).profile.id;
        self.state
            .update(cx, |s, cx| s.open_collection(id, name, cx));
    }

    /// Stars or unstars the selected collection; a failed save shows under the toolbar.
    pub fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.visible_selected(cx) else {
            return;
        };
        let id = self.connection.read(cx).profile.id;
        let saved = self
            .state
            .update(cx, |s, cx| s.toggle_favorite(id, &name, cx));
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let error = saved.await.err().map(|detail| {
                SharedString::from(t!("collections.favorite_error", detail = detail).to_string())
            });
            this.update(cx, |view, cx| {
                view.favorite_error = error;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Puts the cursor in the search field.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Loads the list again, keeping the old one on screen until the new one arrives.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.connection
            .update(cx, |c, cx| c.load_collection_list(cx));
    }

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let count = SharedString::from(
            t!(
                "collections.count",
                shown = delegate.visible.len(),
                total = delegate.all.len()
            )
            .to_string(),
        );
        h_flex()
            .gap_3()
            .p_2()
            .items_center()
            .child(
                div().w(px(320.)).child(
                    Input::new(&self.search)
                        .suffix(div().children(Kbd::binding_for_action(
                            &FocusSearch,
                            Some(COLLECTIONS),
                            window,
                        )))
                        .id("collections.search")
                        .aria_label(t!("collections.search").to_string()),
                ),
            )
            .child(
                div()
                    .id("collections.count")
                    .test_support()
                    .aria_label(count.clone())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
            .child(div().flex_1())
            .child(
                Button::new("collections.reload")
                    .ghost()
                    .small()
                    .label(t!("collections.reload").to_string())
                    .children(Kbd::binding_for_action(&Refresh, Some(COLLECTIONS), window))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }

    /// The explanation shown when the cluster has no collections.
    fn empty(&self, cx: &App) -> impl IntoElement {
        let local = self
            .connection
            .read(cx)
            .profile
            .nodes
            .iter()
            .any(Node::is_local);
        let sentence = t!("collections.empty").to_string();
        let seed = t!("collections.seed_hint").to_string();
        let label = if local {
            format!("{sentence} {seed}")
        } else {
            sentence.clone()
        };
        v_flex()
            .id("collections.empty")
            .test_support()
            .aria_label(SharedString::from(label))
            .gap_2()
            .p_4()
            .items_start()
            .text_sm()
            .child(sentence)
            .child(
                Button::new("collections.docs")
                    .link()
                    .label(t!("collections.docs").to_string())
                    .on_click(|_, _, cx| cx.open_url(DOCS_URL)),
            )
            .when(local, |el| {
                el.child(div().text_color(cx.theme().muted_foreground).child(seed))
            })
    }
}

impl Render for CollectionsList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let cache = &self.connection.read(cx).collection_cache;
        let error = cache.error.clone();
        let loaded = cache.collections.as_ref().map(Vec::len);
        let loading = cache.loading && loaded.is_none();
        let (shown, query) = {
            let d = self.table.read(cx).delegate();
            (d.visible.len(), d.query.trim().to_string())
        };
        let notice = |id: &'static str, text: SharedString, color| {
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .px_2()
                .py_1()
                .text_sm()
                .text_color(color)
                .child(text)
        };
        let body = match loaded {
            Some(0) => self.empty(cx).into_any_element(),
            Some(_) if shown == 0 => notice(
                "collections.no_match",
                t!("collections.no_match", query = query).to_string().into(),
                muted,
            )
            .into_any_element(),
            Some(_) => div()
                .flex_1()
                .min_h_0()
                .child(
                    DataTable::new(&self.table)
                        .bordered(false)
                        .accessibility_label(t!("sidebar.collections").to_string()),
                )
                .into_any_element(),
            None if loading => notice(
                "collections.loading",
                t!("collections.loading").to_string().into(),
                muted,
            )
            .into_any_element(),
            None => div().into_any_element(),
        };
        v_flex()
            .size_full()
            .child(self.toolbar(window, cx))
            .children(
                self.favorite_error
                    .clone()
                    .map(|e| notice("collections.favorite_error", e, danger)),
            )
            .children(error.map(|e| notice("collections.error", e, danger)))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, documents: u64, aliases: &[&str]) -> Row {
        Row {
            name: name.into(),
            documents,
            fields: 0,
            created_at: 0,
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            synonym_sets: 0,
            curation_sets: 0,
        }
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn favorites_come_first_in_either_direction() {
        let mut rows = vec![row("a", 1, &[]), row("b", 3, &[]), row("c", 2, &[])];
        let favorites = vec!["a".to_string()];
        sort_rows(&mut rows, SortKey::Documents, false, &favorites);
        assert_eq!(names(&rows), ["a", "b", "c"]);
        sort_rows(&mut rows, SortKey::Documents, true, &favorites);
        assert_eq!(names(&rows), ["a", "c", "b"]);
        sort_rows(&mut rows, SortKey::Name, false, &[]);
        assert_eq!(names(&rows), ["c", "b", "a"]);
    }

    #[test]
    fn search_matches_name_and_alias_ignoring_case() {
        let r = row("Books", 0, &["library"]);
        assert!(r.matches("book"));
        assert!(r.matches("lib"));
        assert!(!r.matches("cd"));
    }
}
