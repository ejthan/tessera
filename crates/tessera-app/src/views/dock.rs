// SPDX-License-Identifier: GPL-3.0-or-later

//! The panels of the window's dock: sidebar (left), overview tabs and the welcome screen
//! (center), request log (bottom).

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

use super::connect_dialog::ConnectDialog;
use super::import_dialog::ImportDialog;
use super::sidebar::Sidebar;
use super::{env_badge, overview, welcome};
use crate::connection::ConnectionState;
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
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl OverviewPanel {
    /// The overview of `connection`, redrawn whenever it changes.
    pub fn new(connection: Entity<ConnectionState>, cx: &mut Context<Self>) -> Self {
        Self {
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
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
        let profile = &self.connection.read(cx).profile;
        let name = SharedString::from(profile.name.clone());
        h_flex()
            .id(SharedString::from(format!("tab.{}.overview", profile.id)))
            .test_support()
            .aria_label(name.clone())
            .gap_2()
            .items_center()
            .child(env_badge::dot(profile.color, cx))
            .child(name)
            .child(env_badge::render(&profile.environment, cx))
    }
}

impl Render for OverviewPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        // The tab chrome cannot be styled per panel, so the profile color edges the content.
        let edge = env_badge::color(conn.profile.color, cx.theme());
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .border_t_2()
            .border_color(edge)
            .children(
                conn.connected
                    .as_ref()
                    .map(|c| overview::render(&c.info, cx)),
            )
    }
}

/// The bottom dock. A placeholder until the request log view lands.
pub struct RequestLogPanel {
    focus_handle: FocusHandle,
}

impl RequestLogPanel {
    /// An empty log panel.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

panel_basics!(RequestLogPanel);

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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("dock.bottom")
            .test_support()
            .track_focus(&self.focus_handle)
            .size_full()
            .p_3()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(t!("log.placeholder").to_string())
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
