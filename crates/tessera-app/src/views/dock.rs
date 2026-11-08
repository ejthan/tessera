// SPDX-License-Identifier: GPL-3.0-or-later

//! The panels of the window's dock: sidebar (left), overview tabs and the welcome screen
//! (center). The request log (bottom) lives in [`super::request_log`].

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::dock::{BasePanel, ClosePanel, Panel, PanelEvent};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Action, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Task, Window, div, prelude::*,
};
use rust_i18n::t;

use super::collections_list::CollectionsList;
use super::connect_dialog::ConnectDialog;
use super::documents::DocumentsView;
use super::import_dialog::ImportDialog;
use super::overview::Overview;
use super::schema_view::{SchemaFailed, SchemaLoaded, SchemaView};
use super::search::SearchWorkbench;
use super::sidebar::Sidebar;
use super::{env_badge, unlock, welcome};
use crate::actions::{
    COLLECTION, COLLECTIONS, CloneCollection, DeleteCollection, EditSchema, FocusSearch, LockAgain,
    NewCollection, OVERVIEW, OpenCollectionRow, Refresh, ShowDocuments, ShowSchema, ToggleFavorite,
    TogglePolling, TruncateCollection, WORKBENCH,
};
use crate::connection::ConnectionState;
use crate::poller::Poller;
use crate::state::AppState;

/// How long the Copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_secs(2);

/// Implements the traits every panel needs that carry no behavior of their own.
macro_rules! panel_basics {
    ($panel:ty) => {
        impl EventEmitter<PanelEvent> for $panel {}

        impl Focusable for $panel {
            fn focus_handle(&self, _: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }
    };
}

/// The left dock: the connection tree.
pub struct SidebarPanel {
    sidebar: Entity<Sidebar>,
    focus_handle: FocusHandle,
}

impl SidebarPanel {
    /// The sidebar over `state`.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            sidebar: cx.new(|cx| Sidebar::new(state, cx)),
            focus_handle: cx.focus_handle(),
        }
    }
}

panel_basics!(SidebarPanel);

impl BasePanel for SidebarPanel {
    fn panel_name(&self) -> &'static str {
        "Sidebar"
    }

    // Nothing could bring it back.
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for SidebarPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("sidebar.connections").to_string()
    }

    // The sidebar draws its own header.
    fn title_bar(&self, _: &App) -> bool {
        false
    }
}

impl Render for SidebarPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id("sidebar")
            .test_support()
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(self.sidebar.clone())
    }
}

/// A center tab with the overview of one connection.
pub struct OverviewPanel {
    connection: Entity<ConnectionState>,
    overview: Entity<Overview>,
    focus_handle: FocusHandle,
    _poller: Entity<Poller>,
    _subscription: Subscription,
}

impl OverviewPanel {
    /// The overview of `connection`, redrawn whenever it changes. Polls while it is open.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _poller: Poller::start(connection.clone(), window, cx),
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
            overview: cx.new(|cx| Overview::new(connection.clone(), window, cx)),
            connection,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The connection this tab shows.
    pub fn connection(&self) -> &Entity<ConnectionState> {
        &self.connection
    }
}

panel_basics!(OverviewPanel);

impl BasePanel for OverviewPanel {
    fn panel_name(&self) -> &'static str {
        "Overview"
    }
}

impl Panel for OverviewPanel {
    /// Color dot, name and environment badge (UX rule 1).
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        let (profile, unlocked) = (&conn.profile, conn.writes_unlocked);
        let tab_id = format!("tab.{}.overview", profile.id);
        let name = SharedString::from(profile.name.clone());
        h_flex()
            .id(SharedString::from(tab_id.clone()))
            .test_support()
            .aria_label(name.clone())
            .gap_2()
            .items_center()
            .child(env_badge::dot(profile.color, cx))
            .child(name)
            .child(env_badge::render(&profile.environment, cx))
            .when(profile.read_only, |el| {
                el.child(env_badge::lock(&tab_id, unlocked, cx))
            })
    }
}

impl Render for OverviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        // The tab chrome cannot be styled per panel, so the profile color edges the content.
        let edge = env_badge::color(conn.profile.color, cx.theme());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(OVERVIEW)
            .on_action(cx.listener(|this, _: &LockAgain, _, cx| {
                this.connection.update(cx, |c, cx| c.lock(cx));
            }))
            .on_action(cx.listener(|this, _: &TogglePolling, _, cx| {
                this.connection
                    .update(cx, |c, cx| c.set_paused(!c.paused, cx));
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                this.connection
                    .update(cx, |c, cx| c.refresh_collections(cx));
            }))
            .size_full()
            .border_t_2()
            .border_color(edge)
            .children(unlock::banner(&self.connection, OVERVIEW, window, cx))
            .child(self.overview.clone())
    }
}

/// A center tab with the collections of one connection.
pub struct CollectionsPanel {
    connection: Entity<ConnectionState>,
    list: Entity<CollectionsList>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl CollectionsPanel {
    /// The collections of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
            list: cx.new(|cx| CollectionsList::new(connection.clone(), state, window, cx)),
            connection,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The connection this tab shows.
    pub fn connection(&self) -> &Entity<ConnectionState> {
        &self.connection
    }
}

panel_basics!(CollectionsPanel);

impl BasePanel for CollectionsPanel {
    fn panel_name(&self) -> &'static str {
        "Collections"
    }
}

impl Panel for CollectionsPanel {
    /// Color dot, "Collections" and environment badge (UX rule 1).
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        let (profile, unlocked) = (&conn.profile, conn.writes_unlocked);
        let tab_id = format!("tab.{}.collections", profile.id);
        let name = SharedString::from(t!("sidebar.collections").to_string());
        h_flex()
            .id(SharedString::from(tab_id.clone()))
            .test_support()
            .aria_label(name.clone())
            .gap_2()
            .items_center()
            .child(env_badge::dot(profile.color, cx))
            .child(name)
            .child(env_badge::render(&profile.environment, cx))
            .when(profile.read_only, |el| {
                el.child(env_badge::lock(&tab_id, unlocked, cx))
            })
    }
}

impl Render for CollectionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edge = env_badge::color(self.connection.read(cx).profile.color, cx.theme());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(COLLECTIONS)
            .on_action(cx.listener(|this, _: &LockAgain, _, cx| {
                this.connection.update(cx, |c, cx| c.lock(cx));
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                this.list.update(cx, |l, cx| l.reload(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| {
                this.list.update(cx, |l, cx| l.toggle_favorite(cx));
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.list.update(cx, |l, cx| l.focus_search(window, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenCollectionRow, _, cx| {
                this.list.update(cx, |l, cx| l.open_selected(cx));
            }))
            .on_action(cx.listener(|this, _: &NewCollection, window, cx| {
                this.list.update(cx, |l, cx| l.new_collection(window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloneCollection, window, cx| {
                this.list.update(cx, |l, cx| l.clone_selected(window, cx));
            }))
            .on_action(cx.listener(|this, _: &EditSchema, window, cx| {
                this.list.update(cx, |l, cx| l.edit_selected(window, cx));
            }))
            .on_action(cx.listener(|this, _: &TruncateCollection, window, cx| {
                this.list
                    .update(cx, |l, cx| l.truncate_selected(window, cx));
            }))
            .on_action(cx.listener(|this, _: &DeleteCollection, window, cx| {
                this.list.update(cx, |l, cx| l.delete_selected(window, cx));
            }))
            .size_full()
            .border_t_2()
            .border_color(edge)
            .children(unlock::banner(&self.connection, COLLECTIONS, window, cx))
            .child(self.list.clone())
    }
}

/// What a collection tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollectionView {
    /// The documents table.
    #[default]
    Documents,
    /// The schema.
    Schema,
}

/// A center tab with the documents and the schema of one collection.
pub struct CollectionPanel {
    connection: Entity<ConnectionState>,
    name: String,
    view: CollectionView,
    documents: Entity<DocumentsView>,
    schema: Entity<SchemaView>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 3],
}

impl CollectionPanel {
    /// The tab of collection `name` on `connection`; loads its schema, then its documents.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let documents = cx.new(|cx| {
            DocumentsView::new(state.clone(), connection.clone(), name.clone(), window, cx)
        });
        let schema =
            cx.new(|cx| SchemaView::new(state, connection.clone(), name.clone(), window, cx));
        Self {
            _subscriptions: [
                cx.observe(&connection, |_, _, cx| cx.notify()),
                cx.subscribe_in(&schema, window, |this, _, SchemaLoaded(c), window, cx| {
                    this.documents
                        .update(cx, |d, cx| d.set_schema(c.clone(), window, cx));
                }),
                cx.subscribe(&schema, |this, _, SchemaFailed(e), cx| {
                    this.documents
                        .update(cx, |d, cx| d.schema_failed(e.clone(), cx));
                }),
            ],
            view: CollectionView::Documents,
            documents,
            schema,
            connection,
            name,
            focus_handle: cx.focus_handle(),
        }
    }

    fn show(&mut self, view: CollectionView, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        // Focus left in the view going away would leave no key context behind.
        self.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// The connection this tab shows.
    pub fn connection(&self) -> &Entity<ConnectionState> {
        &self.connection
    }

    /// The collection this tab shows.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The schema view, for tests.
    pub fn schema(&self) -> &Entity<SchemaView> {
        &self.schema
    }

    /// The handle of the tab's own root, outside either view, for tests.
    pub fn chrome_focus(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The documents view, for tests.
    pub fn documents(&self) -> &Entity<DocumentsView> {
        &self.documents
    }
}

impl EventEmitter<PanelEvent> for CollectionPanel {}

impl Focusable for CollectionPanel {
    /// The table on the documents view: its shortcuts work from the moment the tab shows.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.view {
            CollectionView::Documents => self.documents.read(cx).table().read(cx).focus_handle(cx),
            CollectionView::Schema => self.focus_handle.clone(),
        }
    }
}

impl BasePanel for CollectionPanel {
    fn panel_name(&self) -> &'static str {
        "Collection"
    }

    /// Unsaved changes veto the kit's close, which cannot ask. ⌘W asks in [`Render`].
    fn closable(&self, cx: &App) -> bool {
        !self.documents.read(cx).shows_unsaved(cx)
    }
}

impl Panel for CollectionPanel {
    /// Color dot, collection name and environment badge (UX rule 1).
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        let (profile, unlocked) = (&conn.profile, conn.writes_unlocked);
        let tab_id = format!("tab.{}.collection.{}", profile.id, self.name);
        let name = SharedString::from(self.name.clone());
        h_flex()
            .id(SharedString::from(tab_id.clone()))
            .test_support()
            .aria_label(name.clone())
            .gap_2()
            .items_center()
            .child(env_badge::dot(profile.color, cx))
            .child(name)
            .child(env_badge::render(&profile.environment, cx))
            .when(profile.read_only, |el| {
                el.child(env_badge::lock(&tab_id, unlocked, cx))
            })
    }
}

/// One entry of a tab's view switch, with its shortcut in key context `context`.
pub(super) fn view_tab(
    id: &'static str,
    label: SharedString,
    action: &dyn Action,
    context: &str,
    window: &Window,
) -> Tab {
    Tab::new()
        .aria_label(label.clone())
        .child(
            div()
                .id(id)
                .test_support()
                .aria_label(label.clone())
                .child(label),
        )
        .suffix(div().children(Kbd::binding_for_action(action, Some(context), window)))
}

impl Render for CollectionPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edge = env_badge::color(self.connection.read(cx).profile.color, cx.theme());
        let on_schema = self.view == CollectionView::Schema;
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(COLLECTION)
            .on_action(cx.listener(|this, _: &LockAgain, _, cx| {
                this.connection.update(cx, |c, cx| c.lock(cx));
            }))
            .on_action(cx.listener(|this, _: &ClosePanel, window, cx| {
                this.documents.update(cx, |d, cx| d.close_tab(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ShowDocuments, window, cx| {
                this.show(CollectionView::Documents, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSchema, window, cx| {
                this.show(CollectionView::Schema, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Refresh, window, cx| {
                // Without a schema the documents wait for it, so it is what can be retried.
                if this.view == CollectionView::Documents && this.documents.read(cx).has_schema() {
                    this.documents.update(cx, |d, cx| d.reload(cx));
                } else {
                    this.schema.update(cx, |s, cx| s.reload(true, window, cx));
                }
            }))
            // Only on the schema view: focus outside the documents view's key context, on
            // the tab bar say, must not reach a dialog the user cannot see the reason for.
            .when(on_schema, |el| {
                el.on_action(cx.listener(|this, _: &CloneCollection, window, cx| {
                    this.schema
                        .update(cx, |s, cx| s.clone_collection(window, cx));
                }))
                .on_action(cx.listener(|this, _: &EditSchema, window, cx| {
                    this.schema.update(cx, |s, cx| s.edit_schema(window, cx));
                }))
                .on_action(cx.listener(|this, _: &TruncateCollection, window, cx| {
                    this.schema.update(cx, |s, cx| s.truncate(window, cx));
                }))
                .on_action(cx.listener(
                    |this, _: &DeleteCollection, window, cx| {
                        this.schema.update(cx, |s, cx| s.delete(window, cx));
                    },
                ))
            })
            .size_full()
            .border_t_2()
            .border_color(edge)
            .children(unlock::banner(&self.connection, COLLECTION, window, cx))
            .child(
                TabBar::new("collection.views")
                    .underline()
                    .selected_index(usize::from(on_schema))
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        let view = if *ix == 0 {
                            CollectionView::Documents
                        } else {
                            CollectionView::Schema
                        };
                        this.show(view, window, cx);
                    }))
                    .child(view_tab(
                        "collection.view.documents",
                        t!("documents.view").to_string().into(),
                        &ShowDocuments,
                        COLLECTION,
                        window,
                    ))
                    .child(view_tab(
                        "collection.view.schema",
                        t!("documents.schema_view").to_string().into(),
                        &ShowSchema,
                        COLLECTION,
                        window,
                    )),
            )
            .child(div().flex_1().min_h_0().map(|el| {
                if on_schema {
                    el.child(self.schema.clone())
                } else {
                    el.child(self.documents.clone())
                }
            }))
    }
}

/// A center tab searching one connection.
pub struct SearchPanel {
    connection: Entity<ConnectionState>,
    workbench: Entity<SearchWorkbench>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl SearchPanel {
    /// The search tab of `connection`.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
            workbench: cx.new(|cx| SearchWorkbench::new(state, connection.clone(), window, cx)),
            connection,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The connection this tab shows.
    pub fn connection(&self) -> &Entity<ConnectionState> {
        &self.connection
    }

    /// The workbench.
    pub fn workbench(&self) -> &Entity<SearchWorkbench> {
        &self.workbench
    }
}

impl EventEmitter<PanelEvent> for SearchPanel {}

impl Focusable for SearchPanel {
    /// The workbench: its shortcuts work from the moment the tab shows.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.workbench.focus_handle(cx)
    }
}

impl BasePanel for SearchPanel {
    fn panel_name(&self) -> &'static str {
        "Search"
    }
}

impl Panel for SearchPanel {
    /// Color dot, "Search" and environment badge (UX rule 1).
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        let (profile, unlocked) = (&conn.profile, conn.writes_unlocked);
        let tab_id = format!("tab.{}.search", profile.id);
        let name = SharedString::from(t!("sidebar.search").to_string());
        h_flex()
            .id(SharedString::from(tab_id.clone()))
            .test_support()
            .aria_label(name.clone())
            .gap_2()
            .items_center()
            .child(env_badge::dot(profile.color, cx))
            .child(name)
            .child(env_badge::render(&profile.environment, cx))
            .when(profile.read_only, |el| {
                el.child(env_badge::lock(&tab_id, unlocked, cx))
            })
    }
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edge = env_badge::color(self.connection.read(cx).profile.color, cx.theme());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(WORKBENCH)
            .on_action(cx.listener(|this, _: &LockAgain, _, cx| {
                this.connection.update(cx, |c, cx| c.lock(cx));
            }))
            .size_full()
            .border_t_2()
            .border_color(edge)
            .children(unlock::banner(&self.connection, WORKBENCH, window, cx))
            .child(div().flex_1().min_h_0().child(self.workbench.clone()))
    }
}

/// The center while no overview tab is open.
pub struct WelcomePanel {
    state: Entity<AppState>,
    copied: bool,
    copy_reset: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl WelcomePanel {
    /// The welcome screen; shows the load error of `state`.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            _subscription: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            copied: false,
            copy_reset: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn copy_docker_command(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(
            welcome::DOCKER_COMMAND.to_string(),
        ));
        self.copied = true;
        cx.notify();
        self.copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |view, cx| {
                view.copied = false;
                cx.notify();
            })
            .ok();
        }));
    }
}

panel_basics!(WelcomePanel);

impl BasePanel for WelcomePanel {
    fn panel_name(&self) -> &'static str {
        "Welcome"
    }

    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for WelcomePanel {
    fn title_bar(&self, _: &App) -> bool {
        false
    }
}

impl Render for WelcomePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let add_state = self.state.clone();
        let import_state = self.state.clone();
        let this = cx.entity();
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(welcome::render(
                self.copied,
                self.state.read(cx).load_error.clone(),
                move |_, window, cx| ConnectDialog::open_new(add_state.clone(), None, window, cx),
                move |_, window, cx| ImportDialog::open(import_state.clone(), window, cx),
                move |_, _, cx| this.update(cx, |view, cx| view.copy_docker_command(cx)),
                window,
                cx,
            ))
    }
}
