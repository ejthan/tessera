// SPDX-License-Identifier: GPL-3.0-or-later

//! The panels of the window's dock: sidebar (left), overview tabs and the welcome screen
//! (center). The request log (bottom) lives in [`super::request_log`].

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*,
};
use rust_i18n::t;

use super::collections_list::CollectionsList;
use super::connect_dialog::ConnectDialog;
use super::import_dialog::ImportDialog;
use super::overview::Overview;
use super::schema_view::SchemaView;
use super::sidebar::Sidebar;
use super::{env_badge, unlock, welcome};
use crate::actions::{
    COLLECTION, COLLECTIONS, CloneCollection, DeleteCollection, EditSchema, FocusSearch, LockAgain,
    NewCollection, OVERVIEW, OpenCollectionRow, Refresh, ToggleFavorite, TogglePolling,
    TruncateCollection,
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

/// A center tab with the schema of one collection.
pub struct CollectionPanel {
    connection: Entity<ConnectionState>,
    name: String,
    schema: Entity<SchemaView>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl CollectionPanel {
    /// The tab of collection `name` on `connection`; loads its schema.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
            schema: cx
                .new(|cx| SchemaView::new(state, connection.clone(), name.clone(), window, cx)),
            connection,
            name,
            focus_handle: cx.focus_handle(),
        }
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
}

panel_basics!(CollectionPanel);

impl BasePanel for CollectionPanel {
    fn panel_name(&self) -> &'static str {
        "Collection"
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

impl Render for CollectionPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edge = env_badge::color(self.connection.read(cx).profile.color, cx.theme());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(COLLECTION)
            .on_action(cx.listener(|this, _: &LockAgain, _, cx| {
                this.connection.update(cx, |c, cx| c.lock(cx));
            }))
            .on_action(cx.listener(|this, _: &Refresh, window, cx| {
                this.schema.update(cx, |s, cx| s.reload(true, window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloneCollection, window, cx| {
                this.schema
                    .update(cx, |s, cx| s.clone_collection(window, cx));
            }))
            .on_action(cx.listener(|this, _: &EditSchema, window, cx| {
                this.schema.update(cx, |s, cx| s.edit_schema(window, cx));
            }))
            .on_action(cx.listener(|this, _: &TruncateCollection, window, cx| {
                this.schema.update(cx, |s, cx| s.truncate(window, cx));
            }))
            .on_action(cx.listener(|this, _: &DeleteCollection, window, cx| {
                this.schema.update(cx, |s, cx| s.delete(window, cx));
            }))
            .size_full()
            .border_t_2()
            .border_color(edge)
            .children(unlock::banner(&self.connection, COLLECTION, window, cx))
            .child(self.schema.clone())
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
