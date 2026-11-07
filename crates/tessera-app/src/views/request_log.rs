// SPDX-License-Identifier: GPL-3.0-or-later

//! The request log panel in the bottom dock: filters, the table of requests and the detail
//! of the selected one.

use std::cell::Cell;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Stateful, Styled as _, Subscription,
    Task, WeakEntity, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{FULL_BODY_MAX, Method};
use tessera_core::log::{BodyPart, Filter, LogEntry};
use tessera_core::{curl, redact};
use tessera_widgets::BodyView;
use uuid::Uuid;

use super::env_badge;
use super::request_detail::{self, Detail};
use crate::actions::{CopyAsCurl, REQUEST_LOG, ShowFullBody, gated};
use crate::state::AppState;

/// Part of the window a full body's dialog takes.
const FULL_VIEW_SHARE: f32 = 0.8;
/// Initial width of the detail split.
const DETAIL_WIDTH: f32 = 420.;
/// The methods the method menu offers.
const METHODS: [Method; 6] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
    Method::HEAD,
];

/// The columns of the table, in order. `Connection` shows only with several connections.
#[derive(Clone, Copy, PartialEq)]
enum Col {
    Time,
    Connection,
    Method,
    Path,
    Node,
    Status,
    Duration,
    Size,
}

const COLUMNS: [Col; 8] = [
    Col::Time,
    Col::Connection,
    Col::Method,
    Col::Path,
    Col::Node,
    Col::Status,
    Col::Duration,
    Col::Size,
];

impl Col {
    fn key(self) -> &'static str {
        match self {
            Col::Time => "time",
            Col::Connection => "connection",
            Col::Method => "method",
            Col::Path => "path",
            Col::Node => "node",
            Col::Status => "status",
            Col::Duration => "duration",
            Col::Size => "size",
        }
    }

    fn width(self) -> f32 {
        match self {
            Col::Path => 260.,
            Col::Node | Col::Connection => 140.,
            Col::Time => 100.,
            _ => 70.,
        }
    }
}

/// One table row: the cell texts of a log entry, built once when the entry arrives.
struct Row {
    id: u64,
    failed: bool,
    /// One text per entry of [`COLUMNS`], in that order.
    cells: [SharedString; COLUMNS.len()],
}

impl Row {
    fn new(entry: &LogEntry) -> Self {
        let e = &entry.event;
        let node = match (e.node.host_str(), e.node.port_or_known_default()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            _ => e.node.to_string(),
        };
        Self {
            id: e.id,
            failed: entry.is_error(),
            cells: COLUMNS.map(|col| {
                match col {
                    Col::Time => local_time(e.started_at),
                    Col::Connection => entry.connection_name.to_string(),
                    Col::Method => e.method.to_string(),
                    Col::Path => path_and_query(&e.path, e.query.as_deref()),
                    Col::Node => node.clone(),
                    Col::Status => e
                        .status
                        .map_or_else(|| t!("log.status.failed").to_string(), |s| s.to_string()),
                    Col::Duration => t!("units.ms", n = e.duration.as_millis()).to_string(),
                    Col::Size => e.size.map(request_detail::size).unwrap_or_default(),
                }
                .into()
            }),
        }
    }
}

/// `path?query`, with a key in the query masked.
pub(crate) fn path_and_query(path: &str, query: Option<&str>) -> String {
    match query {
        Some(q) => format!("{path}?{}", redact::query(q)),
        None => path.to_string(),
    }
}

/// The rows the table shows: the log entries that pass the filter, oldest first.
#[derive(Default)]
struct LogRows {
    rows: Vec<Row>,
    show_connection: bool,
}

impl LogRows {
    fn columns(&self) -> impl Iterator<Item = Col> + '_ {
        COLUMNS
            .into_iter()
            .filter(|c| self.show_connection || *c != Col::Connection)
    }

    fn col(&self, col_ix: usize) -> Col {
        self.columns().nth(col_ix).unwrap_or(Col::Path)
    }

    fn text(&self, row_ix: usize, col: Col) -> SharedString {
        self.rows
            .get(row_ix)
            .map(|r| r.cells[col as usize].clone())
            .unwrap_or_default()
    }
}

/// `HH:MM:SS.mmm` in the system time zone; empty for a time jiff cannot represent.
fn local_time(at: std::time::SystemTime) -> String {
    jiff::Timestamp::try_from(at)
        .map(|ts| {
            ts.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_default()
}

impl TableDelegate for LogRows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns().count()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let col = self.col(col_ix);
        let column = Column::new(col.key(), t!(format!("log.col.{}", col.key())).to_string())
            .width(px(col.width()));
        match col {
            Col::Status | Col::Duration | Col::Size => column.text_right(),
            _ => column,
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        let Some(row) = self.rows.get(row_ix) else {
            return div().id(("log.filler", row_ix));
        };
        let label = self
            .columns()
            .map(|c| row.cells[c as usize].as_ref())
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .id(SharedString::from(format!("log.row.{}", row.id)))
            .aria_label(label)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = self.col(col_ix);
        let failed = col == Col::Status && self.rows.get(row_ix).is_some_and(|r| r.failed);
        div()
            .truncate()
            .when(failed, |el| el.text_color(cx.theme().danger))
            .child(self.text(row_ix, col))
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.text(row_ix, self.col(col_ix)))
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.text(row_ix, self.col(col_ix)).to_string()
    }
}

/// Which of All, Errors and Writes is chosen.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    All,
    Errors,
    Writes,
}

/// The bottom dock: the request log of all connections.
pub struct RequestLogPanel {
    state: Entity<AppState>,
    table: Entity<TableState<LogRows>>,
    filter: Filter,
    /// Event id of the selected entry.
    selected: Option<u64>,
    detail: Option<Detail>,
    detail_task: Option<Task<()>>,
    /// The dialog's view of a full body. The dialog owns it, so closing frees the text.
    full_view: Option<WeakEntity<BodyView>>,
    full_task: Option<Task<()>>,
    /// What the rows were built from; an unchanged stamp skips the rebuild.
    stamp: Option<Stamp>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// Log length, last event id, filter and open connections at the last refresh.
type Stamp = (usize, Option<u64>, Filter, Vec<(Uuid, String)>);

impl RequestLogPanel {
    /// The log of `state`, refreshed whenever new entries arrive.
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            crate::settings::follow_language(cx);
            TableState::new(LogRows::default(), window, cx)
                .sortable(false)
                .col_movable(false)
                .col_selectable(false)
        });
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.refresh(cx)),
            cx.subscribe(&table, |this, _, event: &TableEvent, cx| match event {
                TableEvent::SelectRow(ix) => this.select(*ix, cx),
                TableEvent::ClearSelection => this.deselect(cx),
                _ => {}
            }),
        ];
        let mut this = Self {
            state,
            table,
            filter: Filter::default(),
            selected: None,
            detail: None,
            detail_task: None,
            full_view: None,
            full_task: None,
            stamp: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// Puts the selected entry on the clipboard as a curl command with a key placeholder.
    pub fn copy_as_curl(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            return;
        };
        let command = self
            .state
            .read(cx)
            .log()
            .get(id)
            .filter(|entry| !request_detail::is_import_batch(&entry.event))
            .map(curl::command);
        if let Some(command) = command {
            cx.write_to_clipboard(ClipboardItem::new_string(command));
        }
    }

    /// Whether the selected entry can be copied as curl: an import batch cannot, its body is
    /// not kept.
    pub fn has_selection(&self, cx: &App) -> bool {
        self.selected.is_some_and(|id| {
            let state = self.state.read(cx);
            state
                .log()
                .get(id)
                .is_some_and(|entry| !request_detail::is_import_batch(&entry.event))
        })
    }

    /// The first cut body of the selected entry whose whole text is still kept.
    fn kept_part(&self, cx: &App) -> Option<BodyPart> {
        let detail = self.detail.as_ref()?;
        let log = self.state.read(cx).log();
        detail
            .cut
            .iter()
            .map(|(part, _)| *part)
            .find(|part| log.full_body(detail.id, *part).is_some())
    }

    /// Open connections with their names, while there are several.
    fn connections(&self, cx: &App) -> Vec<(Uuid, String)> {
        let open = &self.state.read(cx).connections;
        if open.len() < 2 {
            return Vec::new();
        }
        open.iter()
            .map(|c| {
                let p = &c.read(cx).profile;
                (p.id, p.name.clone())
            })
            .collect()
    }

    /// Reloads the rows from the log, keeping the selection and following new entries
    /// while scrolled to the bottom.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let connections = self.connections(cx);
        // A connection menu that is hidden must not filter.
        self.filter
            .connections
            .retain(|id| connections.iter().any(|(c, _)| c == id));
        let log = self.state.read(cx).log();
        let stamp = (
            log.len(),
            log.last_id(),
            self.filter.clone(),
            connections.clone(),
        );
        // AppState also notifies for changes that leave the log as it was.
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        self.stamp = Some(stamp);
        let mut old = self
            .table
            .update(cx, |t, _| std::mem::take(&mut t.delegate_mut().rows))
            .into_iter()
            .peekable();
        // Both lists are ordered by id, so a row built earlier is found by walking along.
        let rows: Vec<Row> = self
            .state
            .read(cx)
            .log()
            .entries(&self.filter)
            .into_iter()
            .map(|e| {
                while old.next_if(|r| r.id < e.event.id).is_some() {}
                old.next_if(|r| r.id == e.event.id)
                    .unwrap_or_else(|| Row::new(e))
            })
            .collect();
        let selected = self
            .selected
            .and_then(|id| rows.iter().position(|r| r.id == id));
        if selected.is_none() {
            self.deselect(cx);
        }
        let show_connection = !connections.is_empty();
        self.table.update(cx, |table, cx| {
            let follow = table
                .vertical_scroll_handle
                .is_scrolled_to_end()
                .unwrap_or(true);
            let rows_delegate = table.delegate_mut();
            let columns_changed = rows_delegate.show_connection != show_connection;
            rows_delegate.rows = rows;
            rows_delegate.show_connection = show_connection;
            if columns_changed {
                table.refresh(cx);
            }
            match selected {
                Some(ix) if table.selected_row() != Some(ix) => table.set_selected_row(ix, cx),
                None if table.selected_row().is_some() => table.clear_selection(cx),
                _ => {}
            }
            if follow {
                table.vertical_scroll_handle.scroll_to_bottom();
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Selects row `ix` and builds its detail off the GPUI thread.
    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.table.read(cx).delegate().rows.get(ix).map(|r| r.id) else {
            return;
        };
        if self.selected == Some(id) {
            return;
        }
        let Some(entry) = self.state.read(cx).log().get(id).cloned() else {
            return;
        };
        self.selected = Some(entry.event.id);
        self.detail = None;
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let detail = cx
                .background_spawn(async move { Detail::new(&entry) })
                .await;
            this.update(cx, |this, cx| {
                if this.selected == Some(detail.id) {
                    this.detail = Some(detail);
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// The dialog's view of a full body, while the dialog is open.
    pub fn full_view(&self) -> Option<Entity<BodyView>> {
        self.full_view.as_ref()?.upgrade()
    }

    /// Opens the selected entry's response if its full text is kept, else its request.
    pub fn show_full_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(part) = self.kept_part(cx) {
            self.open_full(part, window, cx);
        }
    }

    /// Shows `part` of the selected entry in a dialog, masked off the UI thread. An evicted body
    /// opens nothing: its place in the detail says so.
    fn open_full(&mut self, part: BodyPart, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            return;
        };
        let state = self.state.read(cx);
        let (Some(text), Some(entry)) = (state.log().full_body(id, part), state.log().get(id))
        else {
            return;
        };
        let name = entry.connection_name.to_string();
        let open = state.connections.iter().map(|c| &c.read(cx).profile);
        let env = open
            .filter(|p| p.id == entry.connection)
            .map(|p| env_badge::label(&p.environment))
            .next();
        let connection = env.map_or_else(|| name.clone(), |env| format!("{name} · {env}"));
        let title = SharedString::from(
            match part {
                BodyPart::Response => t!("log.detail.full_response", connection = connection),
                BodyPart::Request => t!("log.detail.full_request", connection = connection),
            }
            .to_string(),
        );
        let view = cx.new(|cx| BodyView::new(window, cx));
        self.full_view = Some(view.downgrade());
        let target = view.downgrade();
        // Once: paging walks this text and masks nothing again.
        self.full_task = Some(cx.spawn_in(window, async move |_, cx| {
            let masked = cx
                .background_spawn(async move { redact::json(&text) })
                .await;
            target
                .update_in(cx, |v, window, cx| v.set_body(masked.into(), window, cx))
                .ok();
        }));
        let viewport = window.viewport_size();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                // So its paging shortcuts work from the start.
                view.read(cx).focus_handle(cx).focus(window, cx);
            }
            dialog
                .title(
                    div()
                        .id("log.full.title")
                        .test_support()
                        .aria_label(title.clone())
                        .child(title.clone()),
                )
                .w(viewport.width * FULL_VIEW_SHARE)
                .child(
                    div()
                        .h(viewport.height * FULL_VIEW_SHARE)
                        .child(view.clone()),
                )
        });
    }

    /// The Full body buttons of the selected entry, or why a text is not there.
    fn full_body_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let detail = self.detail.as_ref()?;
        if detail.cut.is_empty() {
            return None;
        }
        let log = self.state.read(cx).log();
        let kept: Vec<BodyPart> = detail
            .cut
            .iter()
            .map(|(part, _)| *part)
            .filter(|part| log.full_body(detail.id, *part).is_some())
            .collect();
        let missing = detail.cut.iter().filter(|(part, _)| !kept.contains(part));
        let too_large = missing.clone().any(|(_, len)| *len > FULL_BODY_MAX);
        let gone = missing.into_iter().any(|(_, len)| *len <= FULL_BODY_MAX);
        let muted = cx.theme().muted_foreground;
        let note = move |id: &'static str, text: String| {
            let text = SharedString::from(text);
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .text_color(muted)
                .child(text)
        };
        let buttons: Vec<_> = kept
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let part = *part;
                let (id, label) = match part {
                    BodyPart::Response => {
                        ("log.detail.full.response", t!("log.detail.show_response"))
                    }
                    BodyPart::Request => ("log.detail.full.request", t!("log.detail.show_request")),
                };
                // The shortcut opens the first one, so only that one shows it.
                let kbd = (index == 0)
                    .then(|| Kbd::binding_for_action(&ShowFullBody, Some(REQUEST_LOG), window))
                    .flatten();
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .label(label.to_string())
                    .children(kbd)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_full(part, window, cx);
                    }))
            })
            .collect();
        Some(
            h_flex()
                .flex_none()
                .flex_wrap()
                .gap_2()
                .px_3()
                .pt_1()
                .items_center()
                .text_xs()
                .children(buttons)
                .when(gone, |el| {
                    el.child(note(
                        "log.detail.full_gone",
                        t!("log.detail.full_gone").to_string(),
                    ))
                })
                .when(too_large, |el| {
                    el.child(note(
                        "log.detail.full_too_large",
                        t!("log.detail.full_too_large").to_string(),
                    ))
                }),
        )
    }

    fn deselect(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.detail = None;
        self.detail_task = None;
        cx.notify();
    }

    fn set_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.filter.errors_only = kind == Kind::Errors;
        self.filter.writes_only = kind == Kind::Writes;
        self.refresh(cx);
    }

    fn kind(&self) -> Kind {
        if self.filter.errors_only {
            Kind::Errors
        } else if self.filter.writes_only {
            Kind::Writes
        } else {
            Kind::All
        }
    }

    /// Adds `item` to `list`, or removes it; `None` empties the list (all).
    fn toggle<T: PartialEq>(list: &mut Vec<T>, item: Option<T>) {
        match item {
            None => list.clear(),
            Some(item) => match list.iter().position(|i| *i == item) {
                Some(ix) => {
                    list.remove(ix);
                }
                None => list.push(item),
            },
        }
    }

    fn render_filters(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let import_selected = self.selected.is_some_and(|id| {
            self.state
                .read(cx)
                .log()
                .get(id)
                .is_some_and(|entry| request_detail::is_import_batch(&entry.event))
        });
        let kind = self.kind();
        let kinds = ButtonGroup::new("log.filter").outline().xsmall().children(
            [
                (Kind::All, "all"),
                (Kind::Errors, "errors"),
                (Kind::Writes, "writes"),
            ]
            .map(|(k, name)| {
                Button::new(SharedString::from(format!("log.filter.{name}")))
                    .label(t!(format!("log.filter.{name}")).to_string())
                    .selected(kind == k)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_kind(k, cx)))
            }),
        );
        let view = cx.entity().downgrade();
        let methods = self.filter.methods.clone();
        let methods_label = if methods.is_empty() {
            t!("log.filter.all_methods").to_string()
        } else {
            methods
                .iter()
                .map(Method::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let method_menu = Button::new("log.filter.methods")
            .outline()
            .xsmall()
            .dropdown_caret(true)
            .label(methods_label)
            .dropdown_menu({
                let view = view.clone();
                move |menu, _, _| {
                    let all = methods.is_empty();
                    let items = std::iter::once((t!("log.filter.all_methods").to_string(), None))
                        .chain(METHODS.iter().map(|m| (m.to_string(), Some(m.clone()))));
                    items.fold(menu, |menu, (label, method)| {
                        let checked = method.as_ref().map_or(all, |m| methods.contains(m));
                        let view = view.clone();
                        menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                            move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    Self::toggle(&mut this.filter.methods, method.clone());
                                    this.refresh(cx);
                                })
                                .ok();
                            },
                        ))
                    })
                }
            });
        let connections = self.connections(cx);
        let chosen = self.filter.connections.clone();
        let connection_menu = (!connections.is_empty()).then(|| {
            let label = if chosen.is_empty() {
                t!("log.filter.all_connections").to_string()
            } else {
                connections
                    .iter()
                    .filter(|(id, _)| chosen.contains(id))
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            Button::new("log.filter.connections")
                .outline()
                .xsmall()
                .dropdown_caret(true)
                .label(label)
                .dropdown_menu(move |menu, _, _| {
                    let items =
                        std::iter::once((t!("log.filter.all_connections").to_string(), None))
                            .chain(
                                connections
                                    .iter()
                                    .map(|(id, name)| (name.clone(), Some(*id))),
                            );
                    items.fold(menu, |menu, (label, id)| {
                        let checked = id.map_or(chosen.is_empty(), |id| chosen.contains(&id));
                        let view = view.clone();
                        menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                            move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    Self::toggle(&mut this.filter.connections, id);
                                    this.refresh(cx);
                                })
                                .ok();
                            },
                        ))
                    })
                })
        });
        h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .child(kinds)
            .child(method_menu)
            .children(connection_menu)
            .child(
                Switch::new("log.filter.polling")
                    .xsmall()
                    .checked(self.filter.show_polling)
                    .label(t!("log.filter.polling").to_string())
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.filter.show_polling = *checked;
                        this.refresh(cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("log.copy_curl")
                    .ghost()
                    .xsmall()
                    .disabled(self.selected.is_none() || import_selected)
                    .when(import_selected, |b| {
                        b.tooltip(t!("log.detail.no_curl_import").to_string())
                    })
                    .label(t!("log.copy_curl").to_string())
                    .children(Kbd::global_binding_for_action(&CopyAsCurl, window))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_as_curl(cx))),
            )
    }
}

impl EventEmitter<PanelEvent> for RequestLogPanel {}

impl Focusable for RequestLogPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for RequestLogPanel {
    fn panel_name(&self) -> &'static str {
        "RequestLog"
    }

    // ⌘J hides it; closing would leave nothing to toggle.
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for RequestLogPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("log.title").to_string()
    }
}

impl Render for RequestLogPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let full_body = self.full_body_bar(window, cx);
        let no_full_body = self.kept_part(cx).is_none();
        let filters = self.render_filters(window, cx);
        let table = DataTable::new(&self.table)
            .xsmall()
            .bordered(false)
            .accessibility_label(t!("log.title").to_string());
        v_flex()
            .id("dock.bottom")
            .test_support()
            .track_focus(&self.focus_handle)
            .key_context(gated(REQUEST_LOG, &[(no_full_body, &ShowFullBody)]))
            .size_full()
            .child(filters)
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("log.split")
                        .child(resizable_panel().child(table))
                        .child(
                            resizable_panel()
                                .visible(self.selected.is_some())
                                .size(px(DETAIL_WIDTH))
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .child(
                                    v_flex().size_full().children(full_body).child(
                                        div().flex_1().min_h_0().child(request_detail::render(
                                            self.detail.as_ref(),
                                            cx,
                                        )),
                                    ),
                                ),
                        ),
                ),
            )
            .on_action(cx.listener(|this, _: &ShowFullBody, window, cx| {
                this.show_full_body(window, cx);
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::local_time;

    #[test]
    fn local_time_has_milliseconds() {
        let t = local_time(SystemTime::UNIX_EPOCH + Duration::from_millis(1_234));
        let digits: String = t.chars().filter(char::is_ascii_digit).collect();
        assert_eq!(t.len(), "00:00:01.234".len(), "{t}");
        assert!(t.ends_with("01.234"), "{t}");
        assert_eq!(digits.len(), 9, "{t}");
    }
}
