// SPDX-License-Identifier: GPL-3.0-or-later

//! The documents view of a collection tab.

pub mod columns;
pub mod copy;
pub mod inspector;
pub mod review;
pub mod table;

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{POPUP_PRIORITY, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu};
use gpui_kit::component::table::{DataTable, TableEvent, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, DismissEvent, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, Styled as _, Subscription, Task,
    Window, anchored, deferred, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::Collection;
use tessera_core::documents::columns::{columns_from_schema, exclude_fields, merge_keys};
use tessera_core::documents::template::template;
use tessera_widgets::{FacetFetch, FilterField, FilterInput, FilterInputEvent, confirm};
use uuid::Uuid;

use self::inspector::{DocumentSaved, GoToResult, Inspector};
use self::table::DocRows;
use super::overview::grouped;
use super::schema_view::COPIED_FOR;
use crate::actions::{
    ClearFilter, CopyCell, CopyRowJson, DOCUMENTS, FocusSearch, GoToId, NarrowerColumn,
    NewDocument, OpenDocument, SortByColumn, ToggleColumns, ToggleInspector, TogglePinColumn,
    WiderColumn,
};
use crate::connection::{CollectionChanged, ConnectionState};
use crate::documents::DocumentsBackendGlobal;
use crate::state::AppState;

/// What `key` being sorted again leads to: off, descending, ascending, off.
fn next_sort(current: Option<(String, bool)>, key: String) -> Option<(String, bool)> {
    match current {
        Some((k, false)) if k == key => Some((key, true)),
        Some((k, true)) if k == key => None,
        _ => Some((key, false)),
    }
}

/// What the filter bar offers: `id` and the indexed fields of the schema. Filtering a field
/// with `index: false` is a 400.
fn filter_fields(collection: &Collection) -> Vec<FilterField> {
    let id = FilterField {
        name: "id".into(),
        r#type: "string".into(),
        facet: false,
    };
    let fields = collection
        .fields
        .iter()
        .filter(|f| f.flag("index") && !f.name.contains('*') && f.name != "id")
        .map(|f| FilterField {
            name: f.name.clone(),
            r#type: f.r#type.clone(),
            facet: f.flag("facet"),
        });
    std::iter::once(id).chain(fields).collect()
}

/// How long a resize waits for the next one before it is saved, so a drag writes once.
const SAVE_DELAY: Duration = Duration::from_millis(300);

/// How long a selection waits for the next one before the inspector loads it, so arrowing
/// through rows fetches only where it stops.
const FOLLOW_DELAY: Duration = Duration::from_millis(120);
const INSPECTOR_WIDTH: f32 = 420.;
const GOTO_WIDTH: f32 = 200.;

/// The documents of one collection, as a table.
pub struct DocumentsView {
    state: Entity<AppState>,
    profile: Uuid,
    name: String,
    columns_open: bool,
    /// Why the last save of the column settings failed.
    save_error: Option<SharedString>,
    /// The save to come or running; a newer change replaces it.
    save_task: Option<Task<()>>,
    has_schema: bool,
    table: Entity<TableState<DocRows>>,
    filter: Entity<FilterInput>,
    inspector: Entity<Inspector>,
    inspector_open: bool,
    /// The wait before the inspector follows the selection; a newer selection replaces it.
    follow_task: Option<Task<()>>,
    goto: Entity<InputState>,
    goto_error: Option<SharedString>,
    /// Says "Copied" on the button for a moment.
    copied: bool,
    copied_reset: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    copy_error: Option<SharedString>,
    /// The open copy menu of a cell, where it was asked for, and its dismissal.
    context_menu: Option<(Entity<PopupMenu>, Point<Pixels>, Subscription)>,
    connection: Entity<ConnectionState>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 7],
}

impl DocumentsView {
    /// The view of collection `name`; it loads once the schema arrives.
    pub fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let profile = connection.read(cx).profile.id;
        let prefs = state.read(cx).document_view(profile, &name);
        let rows = DocRows::new(connection.clone(), name.clone(), prefs);
        let table = cx.new(|cx| {
            TableState::new(rows, window, cx)
                .cell_selectable(true)
                .row_header(false)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
        });
        let (facets, collection) = (connection.clone(), name.clone());
        let fetch: FacetFetch = Rc::new(move |field, prefix, cx: &mut App| {
            let Some(connected) = &facets.read(cx).connected else {
                return Task::ready(Vec::new());
            };
            let values = cx.global::<DocumentsBackendGlobal>().0.facet_values(
                connected.client.clone(),
                collection.clone(),
                field,
                prefix,
            );
            cx.spawn(async move |_| values.await.unwrap_or_default())
        });
        let filter = cx.new(|cx| FilterInput::new(Vec::new(), fetch, window, cx));
        let inspector = cx.new(|cx| Inspector::new(connection.clone(), name.clone(), window, cx));
        let goto = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("documents.goto.placeholder").to_string())
        });
        Self {
            _subscriptions: [
                cx.subscribe(&filter, |this, _, event: &FilterInputEvent, cx| {
                    match event {
                        FilterInputEvent::Submitted(text) => this.apply_filter(text, cx),
                        // The clear button follows the text.
                        FilterInputEvent::Changed(_) => cx.notify(),
                    }
                }),
                cx.observe(&table, |_, _, cx| cx.notify()),
                cx.subscribe_in(&table, window, |this, _, event: &TableEvent, window, cx| {
                    match event {
                        TableEvent::ColumnWidthsChanged(widths) => this.widths_changed(widths, cx),
                        TableEvent::SelectCell(row, _) => {
                            this.copy_error = None;
                            this.follow(*row, window, cx);
                            cx.notify();
                        }
                        TableEvent::DoubleClickedCell(row, _) => this.open_row(*row, window, cx),
                        TableEvent::RightClickedCell(row, col) => {
                            this.open_context_menu(*row, *col, window, cx);
                        }
                        // The kit opens the menu here; its copy actions need this row.
                        TableEvent::RightClickedRow(Some(row)) => {
                            let table = this.table.clone();
                            table.update(cx, |t, cx| {
                                let col = t.selected_cell().map_or(0, |(_, col)| col);
                                t.set_selected_cell(*row, col, cx);
                            });
                        }
                        _ => {}
                    }
                }),
                cx.subscribe_in(&goto, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.go_to(window, cx);
                    }
                }),
                cx.subscribe(&inspector, |this, _, event: &GoToResult, cx| {
                    match event {
                        GoToResult::Found => {
                            this.goto_error = None;
                            this.inspector_open = true;
                        }
                        GoToResult::Failed(error) => this.goto_error = Some(error.clone()),
                    }
                    cx.notify();
                }),
                cx.subscribe(&inspector, |this, _, _: &DocumentSaved, cx| this.reload(cx)),
                cx.subscribe(&connection, |this, _, CollectionChanged(name), cx| {
                    if *name == this.name {
                        this.reload(cx);
                    }
                }),
            ],
            state,
            profile,
            name,
            columns_open: false,
            save_error: None,
            save_task: None,
            has_schema: false,
            table,
            filter,
            inspector,
            inspector_open: false,
            follow_task: None,
            goto,
            goto_error: None,
            copied: false,
            copied_reset: None,
            copy_task: None,
            copy_error: None,
            context_menu: None,
            connection,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Takes the columns from `collection`. Loads when the query changes with it, and on the
    /// first call, because the table reports no visible rows while it has none.
    pub fn set_schema(&mut self, collection: Collection, _: &mut Window, cx: &mut Context<Self>) {
        let first = !std::mem::replace(&mut self.has_schema, true);
        let fields = filter_fields(&collection);
        self.filter.update(cx, |f, cx| f.set_fields(fields, cx));
        self.inspector
            .update(cx, |i, cx| i.set_schema(collection.clone(), cx));
        self.table.update(cx, |t, cx| {
            let rows = t.delegate_mut();
            let exclude = exclude_fields(&collection);
            // A vector field appears in pages asked for before this schema arrived.
            let excluded = |key: &str| {
                exclude
                    .as_deref()
                    .is_some_and(|fields| fields.split(',').any(|f| f == key))
            };
            let seen: Vec<String> = rows
                .all_columns
                .iter()
                .filter(|c| !c.in_schema && !excluded(&c.key))
                .map(|c| c.key.clone())
                .collect();
            rows.all_columns = columns_from_schema(&collection);
            merge_keys(&mut rows.all_columns, &seen);
            rows.apply_prefs();
            let exclude_changed = rows.exclude != exclude;
            rows.exclude = exclude;
            // Left in place, the server would refuse every query, with no column to clear it.
            let sort_gone = rows.sort.as_ref().is_some_and(|(key, _)| {
                !rows.all_columns.iter().any(|c| c.key == *key && c.sortable)
            });
            if sort_gone {
                rows.sort = None;
            }
            t.refresh(cx);
            if first || sort_gone || exclude_changed {
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
            rows.error = Some(error);
            cx.notify();
        });
    }

    /// Queries with `text` as `filter_by`; an empty one is no filter.
    fn apply_filter(&mut self, text: &str, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let rows = t.delegate_mut();
            rows.filter = (!text.is_empty()).then(|| text.to_string());
            rows.requery(cx);
            cx.notify();
        });
    }

    /// Puts the focus in the filter bar, which exists once the schema is known.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_schema {
            self.filter.update(cx, |f, cx| f.focus(window, cx));
        }
    }

    /// Empties the filter bar and, when a filter is applied, drops it.
    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| f.set_value("", window, cx));
        if self.table.read(cx).delegate().filter.is_some() {
            self.apply_filter("", cx);
        }
    }

    /// Opens or closes the column picker.
    pub fn set_columns_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.columns_open = open;
        cx.notify();
    }

    /// Hides or shows column `key`; the last visible one stays.
    pub fn toggle_hidden(&mut self, key: &str, cx: &mut Context<Self>) {
        self.edit_prefs(cx, |r| {
            columns::toggle_hidden(&r.all_columns, &mut r.prefs, key)
        });
    }

    /// Pins or unpins column `key`.
    pub fn toggle_pin(&mut self, key: &str, cx: &mut Context<Self>) {
        self.edit_prefs(cx, |r| {
            columns::toggle_pin(&mut r.prefs, key);
            true
        });
    }

    /// Back to the default columns and widths.
    pub fn reset_columns(&mut self, cx: &mut Context<Self>) {
        self.edit_prefs(cx, |r| {
            r.prefs = Default::default();
            true
        });
    }

    /// Applies `change` to the table's delegate; when it says so, redraws the columns and
    /// saves the settings.
    fn edit_prefs(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut DocRows) -> bool) {
        let changed = self.table.update(cx, |t, cx| {
            if !change(t.delegate_mut()) {
                return false;
            }
            t.delegate_mut().apply_prefs();
            t.refresh(cx);
            cx.notify();
            true
        });
        if changed {
            self.save(None, cx);
            cx.notify();
        }
    }

    /// Takes the widths a drag or a fit left in the table.
    fn widths_changed(&mut self, widths: &[Pixels], cx: &mut Context<Self>) {
        self.table.update(cx, |t, _| {
            let rows = t.delegate_mut();
            for (column, width) in rows.columns.iter().zip(widths) {
                rows.prefs
                    .widths
                    .insert(column.key.clone(), f32::from(*width));
            }
        });
        self.save(Some(SAVE_DELAY), cx);
    }

    /// Key and drawn width of the selected cell's column.
    fn selected_column(&self, cx: &App) -> Option<(String, f32)> {
        let t = self.table.read(cx);
        let (_, col) = t.selected_cell()?;
        let key = t.delegate().columns.get(col)?.key.clone();
        Some((key, f32::from(t.col_width(col)?)))
    }

    fn resize_selected(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some((key, width)) = self.selected_column(cx) else {
            return;
        };
        self.edit_prefs(cx, |r| {
            columns::set_width(&mut r.prefs, &key, width + delta);
            true
        });
    }

    /// Stores the settings in the app state at once, or after `delay`, and writes them. A
    /// newer change replaces a save that has not run yet.
    fn save(&mut self, delay: Option<Duration>, cx: &mut Context<Self>) {
        let now = delay.is_none().then(|| self.commit(cx));
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let saving = match (now, delay) {
                (Some(saving), _) => saving,
                (None, delay) => {
                    cx.background_executor()
                        .timer(delay.unwrap_or_default())
                        .await;
                    let Ok(saving) = this.update(cx, |this, cx| this.commit(cx)) else {
                        return;
                    };
                    saving
                }
            };
            let result = saving.await;
            this.update(cx, |this, cx| {
                this.save_error = result.err().map(|error| {
                    t!("documents.columns_save_failed", error = error)
                        .to_string()
                        .into()
                });
                cx.notify();
            })
            .ok();
        }));
    }

    fn commit(&self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let prefs = self.table.read(cx).delegate().prefs.clone();
        self.state.update(cx, |s, cx| {
            s.set_document_view(self.profile, &self.name, prefs, cx)
        })
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

    /// The filter bar.
    pub fn filter(&self) -> &Entity<FilterInput> {
        &self.filter
    }

    /// The inspector.
    pub fn inspector(&self) -> &Entity<Inspector> {
        &self.inspector
    }

    /// The table.
    pub fn table(&self) -> &Entity<TableState<DocRows>> {
        &self.table
    }

    /// The id of a loaded row.
    fn row_id(&self, row: usize, cx: &App) -> Option<String> {
        let rows = self.table.read(cx).delegate();
        rows.row(row).map(|r| r.id.to_string())
    }

    /// The id of the selected cell's row, if it is loaded.
    fn selected_id(&self, cx: &App) -> Option<String> {
        let (row, _) = self.table.read(cx).selected_cell()?;
        self.row_id(row, cx)
    }

    /// The inspector, while open, moves to `row` once the selection rests.
    fn follow(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_task = None;
        let inspector = self.inspector.read(cx);
        if !self.inspector_open || inspector.is_dirty(cx) {
            return;
        }
        let Some(id) = self.row_id(row, cx) else {
            return;
        };
        if inspector.current_id() == Some(&id) {
            return;
        }
        self.follow_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FOLLOW_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                this.inspector.update(cx, |i, cx| i.show(id, window, cx));
            })
            .ok();
        }));
    }

    /// Opens the inspector on `row`.
    fn open_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_task = None;
        self.inspector_open = true;
        if let Some(id) = self.row_id(row, cx)
            && !self.inspector.read(cx).is_dirty(cx)
        {
            self.inspector.update(cx, |i, cx| i.show(id, window, cx));
        }
        cx.notify();
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, _)) = self.table.read(cx).selected_cell() {
            self.open_row(row, window, cx);
        }
    }

    fn toggle_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inspector_open {
            self.inspector_open = true;
            self.open_selected(window, cx);
            return cx.notify();
        }
        self.after_discard(window, cx, |this, window, cx| {
            this.close_inspector(window, cx)
        });
    }

    fn close_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_task = None;
        self.inspector_open = false;
        // The closed panel is not drawn, and with it would go every shortcut of this view.
        if self.inspector.read(cx).has_focus(window, cx) {
            self.table.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Runs `then` now, or once the user agreed to drop the inspector's unsaved changes.
    fn after_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !self.inspector.read(cx).is_dirty(cx) {
            return then(self, window, cx);
        }
        let view = cx.entity().downgrade();
        confirm::open(
            t!("documents.inspector.discard.title").to_string(),
            t!("documents.inspector.discard.body").to_string(),
            t!("documents.inspector.discard.confirm").to_string(),
            move |window, cx| {
                view.update(cx, |this, cx| {
                    this.inspector.update(cx, |i, cx| i.revert(window, cx));
                    then(this, window, cx);
                })
                .ok();
            },
            window,
            cx,
        );
    }

    /// Opens the inspector on a new document from the schema's template.
    fn new_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.after_discard(window, cx, |this, window, cx| {
            let Some(schema) = this.inspector.read(cx).schema() else {
                return;
            };
            let document = template(schema);
            this.follow_task = None;
            this.inspector_open = true;
            this.inspector
                .update(cx, |i, cx| i.new_document(document, window, cx));
            cx.notify();
        });
    }

    /// Selects the cell and opens the copy menu at the pointer. The kit never shows the
    /// table's context menu for a click on a cell. An unloaded row has nothing to copy.
    fn open_context_menu(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table
            .update(cx, |t, cx| t.set_selected_cell(row, col, cx));
        if self.row_id(row, cx).is_none() {
            return;
        }
        let target = self.table.read(cx).focus_handle(cx);
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            copy::entries(menu.action_context(target))
        });
        menu.focus_handle(cx).focus(window, cx);
        let closed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some((menu, window.mouse_position(), closed));
        cx.notify();
    }

    /// Puts the focus in the Go to ID input, which exists once the schema is known.
    fn focus_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_schema {
            self.goto.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Loads the document whose id is in the Go to ID input.
    fn go_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.goto.read(cx).value().trim().to_string();
        if !id.is_empty() {
            self.after_discard(window, cx, move |this, window, cx| {
                this.follow_task = None;
                let id = id.clone();
                this.inspector.update(cx, |i, cx| i.go_to(id, window, cx));
            });
        }
    }

    /// Says "Copied" on the button for a moment.
    fn copied(&mut self, cx: &mut Context<Self>) {
        self.copied = true;
        self.copy_error = None;
        cx.notify();
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |view, cx| {
                view.copied = false;
                cx.notify();
            })
            .ok();
        }));
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

    fn pin_selected(&mut self, cx: &mut Context<Self>) {
        if let Some((key, _)) = self.selected_column(cx) {
            self.toggle_pin(&key, cx);
        }
    }

    fn filter_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let applied = self.table.read(cx).delegate().filter.is_some();
        let clear = (applied || !self.filter.read(cx).value(cx).is_empty()).then(|| {
            Button::new("documents.filter.clear")
                .ghost()
                .label(t!("documents.filter_clear").to_string())
                .children(Kbd::binding_for_action(
                    &ClearFilter,
                    Some(DOCUMENTS),
                    window,
                ))
                .on_click(cx.listener(|this, _, window, cx| this.clear_filter(window, cx)))
        });
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("documents.filter")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .child(self.filter.clone()),
            )
            .children(Kbd::binding_for_action(
                &FocusSearch,
                Some(DOCUMENTS),
                window,
            ))
            .children(clear)
            .child(
                div().w(px(GOTO_WIDTH)).child(
                    Input::new(&self.goto)
                        .suffix(div().children(Kbd::binding_for_action(
                            &GoToId,
                            Some(DOCUMENTS),
                            window,
                        )))
                        .id("documents.goto")
                        .aria_label(t!("documents.goto.placeholder").to_string()),
                ),
            )
    }

    /// The Copy dropdown and the inspector's toggle.
    fn actions(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let label = if self.copied {
            t!("documents.copy.copied")
        } else {
            t!("documents.copy")
        };
        // Its shortcuts and actions are the table's, whatever holds the focus.
        let target = self.table.read(cx).focus_handle(cx);
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("documents.copy")
                    .ghost()
                    .small()
                    .dropdown_caret(true)
                    .label(label.to_string())
                    .dropdown_menu(move |menu, _, _| {
                        copy::entries(menu.action_context(target.clone()))
                    }),
            )
            .child(
                Button::new("documents.new")
                    .ghost()
                    .small()
                    .label(t!("documents.new").to_string())
                    .children(Kbd::binding_for_action(
                        &NewDocument,
                        Some(DOCUMENTS),
                        window,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.new_document(window, cx))),
            )
            .child(
                Button::new("documents.toggle_inspector")
                    .ghost()
                    .small()
                    .selected(self.inspector_open)
                    .label(t!("documents.inspector.title").to_string())
                    .children(Kbd::binding_for_action(
                        &ToggleInspector,
                        Some(DOCUMENTS),
                        window,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_inspector(window, cx))),
            )
    }

    fn status(&self, window: &Window, cx: &App) -> Option<impl IntoElement + use<>> {
        let rows = self.table.read(cx).delegate();
        if !rows.answered {
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
        let query_error = rows.query_error.clone();
        let error = rows.error.clone().or_else(|| self.copy_error.clone());
        let picker = self.has_schema.then(|| {
            columns::picker(
                cx.entity(),
                self.columns_open,
                rows.all_columns.clone(),
                rows.prefs.clone(),
                window,
            )
        });
        let save_error = self.save_error.clone();
        let dimmed = query_error.is_some();
        let bar = self.has_schema.then(|| self.filter_bar(window, cx));
        let actions = self.has_schema.then(|| self.actions(window, cx));
        let goto_error = self.goto_error.clone();
        let context_menu = self.context_menu.as_ref().map(|(menu, position, _)| {
            // As the kit's own context menu: the rows do not scroll under it.
            deferred(
                anchored().child(
                    div()
                        .w(window.bounds().size.width)
                        .h(window.bounds().size.height)
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .child(
                            anchored()
                                .position(*position)
                                .snap_to_window_with_margin(px(8.))
                                .child(menu.clone()),
                        ),
                ),
            )
            .with_priority(POPUP_PRIORITY)
        });
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(DOCUMENTS)
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ClearFilter, window, cx| this.clear_filter(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleInspector, window, cx| {
                this.toggle_inspector(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenDocument, window, cx| this.open_selected(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewDocument, window, cx| this.new_document(window, cx)),
            )
            .on_action(cx.listener(|this, _: &GoToId, window, cx| this.focus_goto(window, cx)))
            .on_action(cx.listener(|this, _: &CopyCell, _, cx| copy::copy_cell(this, cx)))
            .on_action(
                cx.listener(|this, _: &CopyRowJson, window, cx| copy::copy_row(this, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SortByColumn, _, cx| this.sort_selected(cx)))
            .on_action(cx.listener(|this, _: &ToggleColumns, _, cx| {
                this.set_columns_open(!this.columns_open, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePinColumn, _, cx| this.pin_selected(cx)))
            .on_action(cx.listener(|this, _: &NarrowerColumn, _, cx| {
                this.resize_selected(-columns::STEP, cx)
            }))
            .on_action(
                cx.listener(|this, _: &WiderColumn, _, cx| this.resize_selected(columns::STEP, cx)),
            )
            .size_full()
            .p_3()
            .gap_2()
            .children(bar)
            .children(query_error.map(|e| {
                div()
                    .id("documents.filter.error")
                    .test_support()
                    .aria_label(e.clone())
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(e)
            }))
            .children(goto_error.map(|e| {
                div()
                    .id("documents.goto.error")
                    .test_support()
                    .aria_label(e.clone())
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(e)
            }))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().children(self.status(window, cx)))
                    .child(h_flex().gap_2().children(actions).children(picker)),
            )
            .children(save_error.map(|e| {
                div()
                    .id("documents.columns_error")
                    .test_support()
                    .aria_label(e.clone())
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(e)
            }))
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
                div().flex_1().min_h_0().child(
                    h_resizable("documents.split")
                        .child(
                            resizable_panel().child(
                                div()
                                    .id("documents.table")
                                    .test_support()
                                    .size_full()
                                    .when(dimmed, |table| table.opacity(0.5))
                                    .child(DataTable::new(&self.table).bordered(true)),
                            ),
                        )
                        .child(
                            resizable_panel()
                                .visible(self.inspector_open)
                                .size(px(INSPECTOR_WIDTH))
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .child(self.inspector.clone()),
                        ),
                ),
            )
            .children(context_menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_fields_are_id_and_the_indexed_ones() {
        let collection: Collection = serde_json::from_value(serde_json::json!({
            "name": "c",
            "fields": [
                {"name": ".*", "type": "auto"},
                {"name": "title", "type": "string"},
                {"name": "secret", "type": "string", "index": false},
                {"name": "dimensions.width", "type": "int32", "facet": true},
            ],
        }))
        .unwrap();
        let fields = filter_fields(&collection);
        let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["id", "title", "dimensions.width"]);
        assert!(fields[2].facet);
    }
}
