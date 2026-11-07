// SPDX-License-Identifier: GPL-3.0-or-later

//! The views of the relevance resources, one tab per connection and kind.

pub mod ai;
pub mod ai_form;
pub mod ai_test;
pub mod aliases;
pub mod analytics;
pub mod attachment;
pub mod backup;
pub mod curation_dialog;
pub mod curations;
pub(crate) mod item_save;
pub mod key_dialog;
pub mod keys;
pub mod name_dialog;
pub mod operations;
pub mod presets;
pub mod reindex;
pub mod restore;
pub mod scoped_keys;
pub mod set_file;
pub mod sets;
pub mod stemming;
pub mod stemming_dialog;
pub mod stopwords;
pub mod synonyms;
pub mod test_panel;

use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Action, AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Hsla,
    IntoElement, ParentElement as _, Render, SharedString, Stateful, Styled as _, Subscription,
    Task, WeakEntity, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{Client, Error, SetKind};
use tessera_core::profile::Profile;
use tessera_widgets::confirm;
use tessera_widgets::confirm_danger::{self, DangerOptions};

use self::ai::AiView;
use self::aliases::AliasesView;
use self::analytics::AnalyticsView;
use self::backup::BackupView;
use self::keys::KeysView;
use self::operations::OperationsView;
use self::presets::PresetsView;
use self::sets::SetsView;
use self::stemming::StemmingView;
use self::stopwords::StopwordsView;
use super::env_badge;
use crate::actions::{FocusResourceFilter, NewResource, RESOURCES, ReloadResource};
use crate::connection::{ConnectionState, error_text};
use crate::state::{AppState, ResourceKind};

/// The view inside a resource tab.
pub enum ResourceView {
    /// See [`AliasesView`].
    Aliases(Entity<AliasesView>),
    /// See [`StopwordsView`].
    Stopwords(Entity<StopwordsView>),
    /// See [`PresetsView`].
    Presets(Entity<PresetsView>),
    /// See [`SetsView`].
    Sets(Entity<SetsView>),
    /// See [`KeysView`].
    Keys(Entity<KeysView>),
    /// See [`AnalyticsView`].
    Analytics(Entity<AnalyticsView>),
    /// See [`StemmingView`].
    Stemming(Entity<StemmingView>),
    /// See [`AiView`].
    Ai(Entity<AiView>),
    /// See [`OperationsView`].
    Operations(Entity<OperationsView>),
    /// See [`BackupView`].
    Backup(Entity<BackupView>),
}

impl ResourceView {
    /// The view of `kind`.
    pub fn new(
        kind: ResourceKind,
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        match kind {
            ResourceKind::Aliases => {
                Self::Aliases(cx.new(|cx| AliasesView::new(connection, state, window, cx)))
            }
            ResourceKind::Stopwords => {
                Self::Stopwords(cx.new(|cx| StopwordsView::new(connection, window, cx)))
            }
            ResourceKind::Presets => {
                Self::Presets(cx.new(|cx| PresetsView::new(connection, state, window, cx)))
            }
            ResourceKind::Synonyms => {
                Self::Sets(cx.new(|cx| SetsView::new(SetKind::Synonym, connection, window, cx)))
            }
            ResourceKind::Curations => {
                Self::Sets(cx.new(|cx| SetsView::new(SetKind::Curation, connection, window, cx)))
            }
            ResourceKind::Keys => {
                Self::Keys(cx.new(|cx| KeysView::new(connection, state, window, cx)))
            }
            ResourceKind::Analytics => {
                Self::Analytics(cx.new(|cx| AnalyticsView::new(connection, state, window, cx)))
            }
            ResourceKind::Stemming => {
                Self::Stemming(cx.new(|cx| StemmingView::new(connection, window, cx)))
            }
            ResourceKind::Ai => Self::Ai(cx.new(|cx| AiView::new(connection, window, cx))),
            ResourceKind::Operations => {
                Self::Operations(cx.new(|cx| OperationsView::new(connection, window, cx)))
            }
            ResourceKind::Backup => Self::Backup(cx.new(|cx| BackupView::new(connection, cx))),
        }
    }

    /// What it draws.
    pub fn view(&self) -> AnyView {
        match self {
            Self::Aliases(view) => view.clone().into(),
            Self::Stopwords(view) => view.clone().into(),
            Self::Presets(view) => view.clone().into(),
            Self::Sets(view) => view.clone().into(),
            Self::Keys(view) => view.clone().into(),
            Self::Analytics(view) => view.clone().into(),
            Self::Stemming(view) => view.clone().into(),
            Self::Ai(view) => view.clone().into(),
            Self::Operations(view) => view.clone().into(),
            Self::Backup(view) => view.clone().into(),
        }
    }

    /// Its focus: its shortcuts work from the moment the tab shows.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Aliases(view) => view.focus_handle(cx),
            Self::Stopwords(view) => view.focus_handle(cx),
            Self::Presets(view) => view.focus_handle(cx),
            Self::Sets(view) => view.focus_handle(cx),
            Self::Keys(view) => view.focus_handle(cx),
            Self::Analytics(view) => view.focus_handle(cx),
            Self::Stemming(view) => view.focus_handle(cx),
            Self::Ai(view) => view.focus_handle(cx),
            Self::Operations(view) => view.focus_handle(cx),
            Self::Backup(view) => view.focus_handle(cx),
        }
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        match self {
            Self::Aliases(view) => view.read(cx).gates(cx),
            Self::Stopwords(view) => view.read(cx).gates(cx),
            Self::Presets(view) => view.read(cx).gates(cx),
            Self::Sets(view) => view.read(cx).gates(cx),
            Self::Keys(view) => view.read(cx).gates(cx),
            Self::Analytics(view) => view.read(cx).gates(cx),
            Self::Stemming(view) => view.read(cx).gates(cx),
            Self::Ai(view) => view.read(cx).gates(cx),
            Self::Operations(view) => view.read(cx).gates(cx),
            Self::Backup(view) => view.read(cx).gates(cx),
        }
    }
}

/// One column: the suffix of its locale key and test id, its width, and whether it is a number.
pub(super) type ResourceColumn = (&'static str, f32, bool);

/// One row of a [`ResourceList`].
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    /// The resource's name, which identifies the row.
    pub key: String,
    /// The cells, as text. The first is matched by the filter, like all of them.
    pub cells: Vec<String>,
    /// The cell that states a problem.
    pub warning: Option<usize>,
}

/// The table's data: every row, and the part that passes the filter.
pub struct ListRows {
    /// Prefix of the locale keys and test ids.
    prefix: &'static str,
    columns: &'static [ResourceColumn],
    all: Vec<ListRow>,
    visible: Vec<usize>,
    query: String,
}

impl ListRows {
    fn rebuild(&mut self) {
        let query = self.query.trim().to_lowercase();
        let hit = |r: &ListRow| r.cells.iter().any(|c| c.to_lowercase().contains(&query));
        self.visible = (0..self.all.len()).filter(|i| hit(&self.all[*i])).collect();
    }

    fn row(&self, row_ix: usize) -> Option<&ListRow> {
        self.all.get(*self.visible.get(row_ix)?)
    }
}

impl TableDelegate for ListRows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, width, numeric) = self.columns[col_ix];
        let title = t!(format!("{}.col.{key}", self.prefix)).to_string();
        let column = Column::new(key, title)
            .width(px(width))
            .resizable(false)
            .movable(false);
        if numeric { column.text_right() } else { column }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        let Some(row) = self.row(row_ix) else {
            return div().id(("resources.filler", row_ix));
        };
        div()
            .id(SharedString::from(format!(
                "{}.row.{}",
                self.prefix, row.key
            )))
            .aria_label(row.cells.join(" · "))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (prefix, column) = (self.prefix, self.columns[col_ix].0);
        let warning = cx.theme().warning;
        let Some(row) = self.row(row_ix) else {
            return div().into_any_element();
        };
        let text = SharedString::from(row.cells[col_ix].clone());
        div()
            .id(SharedString::from(format!("{prefix}.{column}.{}", row.key)))
            .test_support()
            .aria_label(text.clone())
            .truncate()
            .when(row.warning == Some(col_ix), |el| el.text_color(warning))
            .child(text)
            .into_any_element()
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.row(row_ix)?.cells[col_ix].clone().into())
    }
}

/// A filter and a table of named resources, with the selection that survives a reload.
pub struct ResourceList {
    table: Entity<TableState<ListRows>>,
    filter: Entity<InputState>,
    selected: Option<String>,
    _subscriptions: [Subscription; 2],
}

impl ResourceList {
    /// An empty list.
    pub(super) fn new(
        prefix: &'static str,
        columns: &'static [ResourceColumn],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = ListRows {
            prefix,
            columns,
            all: Vec::new(),
            visible: Vec::new(),
            query: String::new(),
        };
        let table = cx.new(|cx| {
            crate::settings::follow_language(cx);
            TableState::new(rows, window, cx)
                .sortable(false)
                .col_movable(false)
                .col_resizable(false)
                .col_selectable(false)
                .row_header(false)
        });
        let placeholder = t!(format!("{prefix}.filter")).to_string();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscriptions = [
            cx.subscribe(&filter, |this, filter, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = filter.read(cx).value().to_string();
                    this.table.update(cx, |t, cx| {
                        t.delegate_mut().query = query;
                        t.delegate_mut().rebuild();
                        cx.notify();
                    });
                    this.reselect(cx);
                }
            }),
            cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
                if let TableEvent::SelectRow(ix) = event {
                    this.selected = table.read(cx).delegate().row(*ix).map(|r| r.key.clone());
                    cx.notify();
                }
            }),
        ];
        Self {
            table,
            filter,
            selected: None,
            _subscriptions: subscriptions,
        }
    }

    /// Replaces the rows.
    pub(super) fn set_rows(&mut self, rows: Vec<ListRow>, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            t.delegate_mut().all = rows;
            t.delegate_mut().rebuild();
            cx.notify();
        });
        self.reselect(cx);
    }

    /// Selects the row of `name`, or none. A row not loaded yet is selected when it arrives.
    pub(super) fn select(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.selected = name;
        if self.selected.is_none() {
            self.table.update(cx, |t, cx| t.clear_selection(cx));
        }
        self.reselect(cx);
    }

    /// Selects the row of the selected name again after the rows changed.
    fn reselect(&mut self, cx: &mut Context<Self>) {
        if let Some(name) = self.selected.clone() {
            self.table.update(cx, |t, cx| {
                let d = t.delegate();
                let ix = d.visible.iter().position(|i| d.all[*i].key == name);
                match ix {
                    Some(ix) if t.selected_row() != Some(ix) => t.set_selected_row(ix, cx),
                    None => t.clear_selection(cx),
                    _ => {}
                }
            });
        }
        cx.notify();
    }

    /// The selected row's name, only while the row is on screen.
    pub(super) fn selected(&self, cx: &App) -> Option<String> {
        let table = self.table.read(cx);
        let row = table.delegate().row(table.selected_row()?)?;
        Some(row.key.clone())
    }

    /// Whether there are no rows at all.
    pub(super) fn is_empty(&self, cx: &App) -> bool {
        self.table.read(cx).delegate().all.is_empty()
    }

    /// The filter alone, for a list beside another list's toolbar.
    pub(super) fn filter_input(
        this: &Entity<Self>,
        prefix: &'static str,
        cx: &App,
    ) -> impl IntoElement + use<> {
        Input::new(&this.read(cx).filter)
            .id(SharedString::from(format!("{prefix}.filter")))
            .aria_label(t!(format!("{prefix}.filter")).to_string())
    }

    /// Puts the cursor in the filter.
    pub(super) fn focus_filter(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let filter = this.read(cx).filter.read(cx).focus_handle(cx);
        filter.focus(window, cx);
    }
}

impl Render for ResourceList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefix = self.table.read(cx).delegate().prefix;
        div().size_full().child(
            DataTable::new(&self.table)
                .with_size(crate::settings::table_size(cx))
                .bordered(false)
                .accessibility_label(t!(format!("sidebar.{prefix}")).to_string()),
        )
    }
}

/// The toolbar of a list: filter, New and Reload.
pub(super) fn toolbar(
    prefix: &'static str,
    list: &Entity<ResourceList>,
    focus: &FocusHandle,
    locked: bool,
    window: &Window,
    cx: &App,
) -> impl IntoElement + use<> {
    let (on_new, on_reload) = (focus.clone(), focus.clone());
    let filter_label = t!(format!("{prefix}.filter")).to_string();
    let new_id = SharedString::from(format!("{prefix}.new"));
    h_flex()
        .flex_none()
        .flex_wrap()
        .gap_3()
        .p_2()
        .items_center()
        .child(
            div().w(px(280.)).child(
                Input::new(&list.read(cx).filter)
                    .suffix(div().children(Kbd::binding_for_action(
                        &FocusResourceFilter,
                        Some(RESOURCES),
                        window,
                    )))
                    .id(SharedString::from(format!("{prefix}.filter")))
                    .aria_label(filter_label),
            ),
        )
        .child(div().flex_1())
        .child(
            Button::new(new_id.clone())
                .primary()
                .small()
                .label(t!("resources.new").to_string())
                .when(locked, |b| b.child(env_badge::lock(&new_id, false, cx)))
                .children(Kbd::binding_for_action(
                    &NewResource,
                    Some(RESOURCES),
                    window,
                ))
                .on_click(move |_, window, cx| on_new.dispatch_action(&NewResource, window, cx)),
        )
        .child(
            Button::new(SharedString::from(format!("{prefix}.reload")))
                .ghost()
                .small()
                .label(t!("resources.reload").to_string())
                .children(Kbd::binding_for_action(
                    &ReloadResource,
                    Some(RESOURCES),
                    window,
                ))
                .on_click(move |_, window, cx| {
                    on_reload.dispatch_action(&ReloadResource, window, cx);
                }),
        )
}

/// A button under a list that acts on the selected row, through `action`.
pub(super) fn row_button(
    id: String,
    label: String,
    action: &'static dyn Action,
    (disabled, locked): (bool, bool),
    focus: &FocusHandle,
    window: &Window,
    cx: &App,
) -> Button {
    let focus = focus.clone();
    Button::new(SharedString::from(id.clone()))
        .small()
        .label(label)
        .when(locked, |b| b.child(env_badge::lock(&id, false, cx)))
        .children(Kbd::binding_for_action(action, Some(RESOURCES), window))
        .disabled(disabled)
        .on_click(move |_, window, cx| focus.dispatch_action(action, window, cx))
}

/// The explanation of an empty list, with its docs link.
pub(super) fn empty_state(prefix: &'static str, docs: &'static str) -> impl IntoElement {
    let sentence = t!(format!("{prefix}.empty")).to_string();
    v_flex()
        .id(SharedString::from(format!("{prefix}.empty")))
        .test_support()
        .aria_label(SharedString::from(sentence.clone()))
        .gap_2()
        .p_4()
        .items_start()
        .text_sm()
        .child(sentence)
        .child(
            Button::new(SharedString::from(format!("{prefix}.docs")))
                .link()
                .label(t!("resources.docs").to_string())
                .on_click(move |_, _, cx| cx.open_url(docs)),
        )
}

/// One inline line of text, in full.
pub(super) fn notice(
    id: impl Into<SharedString>,
    text: SharedString,
    color: Hsla,
) -> impl IntoElement {
    div()
        .id(id.into())
        .test_support()
        .aria_label(text.clone())
        .px_2()
        .py_1()
        .text_sm()
        .text_color(color)
        .child(text)
}

/// The client of `connection`, while it is connected.
pub(super) fn client_of(connection: &Entity<ConnectionState>, cx: &App) -> Option<Client> {
    let connected = connection.read(cx).connected.as_ref();
    connected.map(|c| c.client.clone())
}

/// Why a list failed to load. A key that may not read it says so.
pub(super) fn list_error_text(error: &Error, profile: &Profile) -> SharedString {
    let detail = error_text(error, profile);
    match error {
        Error::Http {
            status: 401 | 403, ..
        } => t!("resources.forbidden", detail = detail)
            .to_string()
            .into(),
        _ => detail.into(),
    }
}

/// `title` with the connection and environment, for a title that holds only text.
pub(crate) fn titled(title: &str, profile: &Profile) -> String {
    let environment = env_badge::label(&profile.environment);
    format!("{title} · {} · {environment}", profile.name)
}

/// A dialog's title with the connection and environment (UX rule 1).
pub(super) fn dialog_title(
    title: String,
    connection: &Entity<ConnectionState>,
    cx: &App,
) -> impl IntoElement + use<> {
    let profile = &connection.read(cx).profile;
    h_flex()
        .gap_2()
        .items_center()
        .child(title)
        .child(div().text_sm().child(profile.name.clone()))
        .child(env_badge::render(&profile.environment, cx))
}

/// A view that shows why a write failed.
pub(super) trait ErrorSlot: 'static {
    /// Where the text goes.
    fn error_slot(&mut self) -> &mut Option<SharedString>;
}

/// Shows a failed write on the view that started it.
pub(super) fn report_to<V: ErrorSlot>(
    view: WeakEntity<V>,
) -> impl Fn(SharedString, &mut App) + 'static {
    move |text, cx| {
        view.update(cx, |view, cx| {
            *view.error_slot() = Some(text);
            cx.notify();
        })
        .ok();
    }
}

/// What a destructive write asks (D62).
pub(super) struct Danger {
    pub title: String,
    /// The concrete consequences.
    pub lines: Vec<String>,
    /// What to type where `typed` is set.
    pub expected: String,
    pub action: String,
    /// Asks for the name instead of a plain confirmation.
    pub typed: bool,
}

/// Asks, then runs `run`. Cancelling sends nothing. A typed confirmation shows a failure in its
/// dialog; after a plain one, which is closed by then, `on_error` gets it.
pub(super) fn confirm_write(
    danger: Danger,
    connection: &Entity<ConnectionState>,
    run: impl Fn(&mut Window, &mut App) -> Task<Result<(), SharedString>> + 'static,
    on_error: impl Fn(SharedString, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let profile = &connection.read(cx).profile;
    let (title, prod) = (
        titled(&danger.title, profile),
        profile.environment.is_prod(),
    );
    if danger.typed {
        let options = DangerOptions {
            title: title.into(),
            consequences: danger.lines.into_iter().map(Into::into).collect(),
            expected: danger.expected.into(),
            action_label: danger.action.into(),
            prod,
        };
        return confirm_danger::open(options, run, window, cx);
    }
    let on_error = Rc::new(on_error);
    confirm::open(
        title,
        danger.lines.join(" "),
        danger.action,
        move |window, cx| {
            let task = run(window, cx);
            let on_error = on_error.clone();
            cx.spawn(async move |cx| {
                if let Err(text) = task.await {
                    cx.update(|cx| on_error(text, cx));
                }
            })
            .detach();
        },
        window,
        cx,
    );
}
