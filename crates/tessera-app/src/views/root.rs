// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's top-level view: title bar, the dock (sidebar, tabs, request log), status bar.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::dock::{
    DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, PaneRef, PanelId, panel_handle,
};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Theme, TitleBar, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, px, rgb,
};
use rust_i18n::t;

use super::connect_dialog::ConnectDialog;
use super::dock::{OverviewPanel, RequestLogPanel, SidebarPanel, WelcomePanel};
use super::{env_badge, status_bar};
use crate::actions::{NewConnection, ToggleRequestLog};
use crate::connection::ConnectionState;
use crate::state::{AppState, OpenOverview};

/// Width of the left sidebar.
const SIDEBAR_WIDTH: f32 = 240.;
/// Height of the request log when open.
const LOG_HEIGHT: f32 = 240.;

/// The main window content.
pub struct RootView {
    focus_handle: FocusHandle,
    state: Entity<AppState>,
    dock: Entity<DockArea>,
    welcome: Entity<WelcomePanel>,
    /// The overview tab shown last in the center. Band, frame, title bar and status bar
    /// follow it.
    active: Option<PanelId>,
    /// The center overview tabs on screen at the last layout change.
    displayed: Vec<PanelId>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    /// Builds the view and keeps the theme in sync with the system appearance.
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let (dock, skin) = DockSkin::dock_area("tessera", None, window, cx);
        skin.set_close_button_visible(true, cx);
        let sidebar = cx.new(|cx| SidebarPanel::new(state.clone(), cx));
        let log = cx.new(RequestLogPanel::new);
        let welcome = cx.new(|cx| WelcomePanel::new(state.clone(), cx));
        dock.update(cx, |area, cx| {
            let tabs = |panel| DockLayout::tabs().panel_view(panel, cx);
            let (left, bottom, center) = (
                tabs(panel_handle(sidebar)),
                tabs(panel_handle(log)),
                tabs(panel_handle(welcome.clone())),
            );
            area.set_dock(DockPlacement::Left, left, window, cx);
            area.set_dock_size(DockPlacement::Left, px(SIDEBAR_WIDTH), window, cx);
            // Its collapse button would have no shortcut.
            area.set_dock_collapsible(DockPlacement::Left, false, window, cx);
            area.set_dock(DockPlacement::Bottom, bottom, window, cx);
            area.set_dock_size(DockPlacement::Bottom, px(LOG_HEIGHT), window, cx);
            area.toggle_dock(DockPlacement::Bottom, window, cx);
            area.set_center(center, window, cx);
        });
        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.close_stale_tabs(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &OpenOverview, window, cx| {
                    this.open_overview(event.0, window, cx);
                },
            ),
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.layout_changed(window, cx);
                }
            }),
        ];
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self {
            focus_handle,
            state,
            dock,
            welcome,
            active: None,
            displayed: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The connection of the active center tab.
    pub fn active_connection(&self, cx: &App) -> Option<Entity<ConnectionState>> {
        let panel = overview_panel(self.dock.read(cx), self.active?)?;
        Some(panel.read(cx).connection().clone())
    }

    /// Every overview tab in the dock, wherever the user moved it.
    fn overview_tabs(&self, cx: &App) -> Vec<Entity<OverviewPanel>> {
        let area = self.dock.read(cx);
        [
            DockPlacement::Center,
            DockPlacement::Left,
            DockPlacement::Right,
            DockPlacement::Bottom,
        ]
        .into_iter()
        .filter_map(|placement| area.layout(placement))
        .flat_map(|tree| tree.panels())
        .filter_map(|id| overview_panel(area, id))
        .collect()
    }

    /// Opens the overview tab of profile `id`, or focuses the one already open.
    fn open_overview(&mut self, id: uuid::Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conn) = self.state.read(cx).connection(id, cx) else {
            return;
        };
        if conn.read(cx).connected.is_none() {
            return;
        }
        let existing = self
            .overview_tabs(cx)
            .into_iter()
            .find(|tab| tab.read(cx).connection() == &conn);
        let welcome = self.welcome.clone();
        self.dock.update(cx, |area, cx| {
            let panel = existing.unwrap_or_else(|| {
                let panel = cx.new(|cx| OverviewPanel::new(conn, cx));
                area.add_panel_view(
                    panel_handle(panel.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
                area.remove_panel(welcome, window, cx);
                panel
            });
            area.select_panel(PanelId::from(panel.entity_id()), window, cx);
        });
    }

    /// Closes the tabs of connections that were closed. Closing a tab never disconnects.
    fn close_stale_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let stale: Vec<_> = self
            .overview_tabs(cx)
            .into_iter()
            .filter(|tab| !state.connections.contains(tab.read(cx).connection()))
            .collect();
        if stale.is_empty() {
            return;
        }
        self.dock.update(cx, |area, cx| {
            for tab in stale {
                area.remove_panel(tab, window, cx);
            }
        });
    }

    /// Finds the active tab after a layout change, brings the welcome screen back into an
    /// empty center, and lets the sidebar selection follow the active tab.
    fn layout_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.dock.read(cx);
        let mut displayed = Vec::new();
        if let Some(center) = area.layout(DockPlacement::Center) {
            center.root().walk(&mut |node| {
                if let PaneRef::Tabs { panels, active_ix } = node.kind() {
                    displayed.extend(
                        panels
                            .get(active_ix)
                            .filter(|id| overview_panel(area, **id).is_some()),
                    );
                }
            });
        }
        let center_empty = area
            .layout(DockPlacement::Center)
            .is_none_or(|tree| tree.panels().next().is_none());
        // shortcut: a tab that just came on screen wins; focus moves between split groups
        // do not change the active tab, until tabs report focus.
        let fresh = displayed.iter().find(|id| !self.displayed.contains(id));
        self.active = fresh
            .or_else(|| self.active.as_ref().filter(|id| displayed.contains(id)))
            .or(displayed.first())
            .copied();
        self.displayed = displayed;
        if center_empty {
            let welcome = panel_handle(self.welcome.clone());
            self.dock.update(cx, |area, cx| {
                area.add_panel_view(welcome, DockPlacement::Center, None, window, cx);
            });
        }
        let active = self.active_connection(cx).map(|c| c.read(cx).profile.id);
        if active.is_some() {
            self.state.update(cx, |s, cx| {
                if s.selected != active {
                    s.selected = active;
                    cx.notify();
                }
            });
        }
        cx.notify();
    }
}

/// The overview tab `id`, if the dock holds one under that id.
fn overview_panel(area: &DockArea, id: PanelId) -> Option<Entity<OverviewPanel>> {
    area.panel(id)?.view().downcast::<OverviewPanel>().ok()
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active_connection(cx);
        let active = active.as_ref().map(|c| c.read(cx));
        let profile = active.map(|c| &c.profile);
        let prod = profile.is_some_and(|p| p.environment.is_prod());
        let log_open = self.dock.read(cx).is_dock_open(DockPlacement::Bottom);
        let title = SharedString::from(
            profile
                .map(|p| p.name.clone())
                .unwrap_or_else(|| t!("titlebar.no_connection").to_string()),
        );
        let switcher = h_flex()
            .gap_2()
            .items_center()
            .text_sm()
            .when_some(profile, |el, p| el.child(env_badge::dot(p.color, cx)))
            .child(
                div()
                    .id("titlebar.connection")
                    .test_support()
                    .aria_label(title.clone())
                    .text_color(theme.muted_foreground)
                    .child(title),
            )
            .when_some(profile, |el, p| {
                el.child(
                    div()
                        .id("titlebar.env")
                        .test_support()
                        .aria_label(SharedString::from(env_badge::label(&p.environment)))
                        .child(env_badge::render(&p.environment, cx)),
                )
                .when(p.read_only, |el| {
                    el.child(
                        div()
                            .id("titlebar.readonly")
                            .test_support()
                            .aria_label(SharedString::from(t!("sidebar.read_only").to_string()))
                            .child(
                                Icon::new(gpui_kit::assets::IconName::Lock)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                })
            });
        let red = rgb(env_badge::PROD_RED);
        v_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
                ConnectDialog::open_new(this.state.clone(), None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleRequestLog, window, cx| {
                this.dock.update(cx, |area, cx| {
                    area.toggle_dock(DockPlacement::Bottom, window, cx);
                });
            }))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(TitleBar::new().child(switcher))
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
            .child(status_bar::render(active, log_open, window, cx))
            // Last, so they draw over the content without moving it; they have no listeners,
            // so clicks pass through.
            .when(prod, |el| {
                el.child(
                    div()
                        .id("window.prod_band")
                        .test_support()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(4.))
                        .bg(red),
                )
                .child(
                    div()
                        .id("window.prod_frame")
                        .test_support()
                        .absolute()
                        .inset_0()
                        .border_2()
                        .border_color(red),
                )
            })
    }
}
