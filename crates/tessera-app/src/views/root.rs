// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's top-level view: title bar, the dock (sidebar, tabs, request log), status bar.

use gpui_kit::base::{Root, TestSupportExt as _};
use gpui_kit::component::dock::{
    DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, PaneRef, PanelId, panel_handle,
};
use gpui_kit::component::{ActiveTheme as _, Theme, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, prelude::*,
    px, rgb,
};
use rust_i18n::t;

use super::connect_dialog::ConnectDialog;
use super::dock::{
    CollectionPanel, CollectionsPanel, ConsolePanel, OverviewPanel, ResourcePanel, SearchPanel,
    SidebarPanel, WelcomePanel,
};
use super::jobs::panel::JobsPanel;
use super::palette::{self, PaletteMode};
use super::request_log::RequestLogPanel;
use super::{env_badge, status_bar};
use crate::actions::{
    CopyAsCurl, NewConnection, OpenAliases, OpenConsoleTab, OpenCurations, OpenPalette,
    OpenPresetsTab, OpenSearchTab, OpenStopwords, OpenSynonyms, QuickSwitch, ToggleJobs,
    ToggleRequestLog,
};
use crate::connection::ConnectionState;
use crate::jobs::{JobFinished, JobId, JobKind, Jobs, JobsGlobal, RunningChanged};
use crate::state::{
    AppState, CloseCollection, OpenCollection, OpenCollections, OpenConsole, OpenOverview,
    OpenResource, OpenSearch, ResourceKind,
};

/// Width of the left sidebar.
const SIDEBAR_WIDTH: f32 = 240.;
/// Tallest the list of `keymap.toml` errors gets before it scrolls.
const KEYMAP_ERRORS_HEIGHT: f32 = 96.;
/// Height of the request log when open.
const LOG_HEIGHT: f32 = 240.;

/// The main window content.
pub struct RootView {
    focus_handle: FocusHandle,
    state: Entity<AppState>,
    dock: Entity<DockArea>,
    welcome: Entity<WelcomePanel>,
    log: Entity<RequestLogPanel>,
    jobs: Entity<JobsPanel>,
    /// How many jobs run; `None` without a job. `render` must not read `Jobs`: that would
    /// redraw the window on every progress update.
    running_jobs: Option<usize>,
    /// The connection tab selected last, in the center or an open dock. Band, frame, title bar
    /// and status bar follow it.
    active: Option<PanelId>,
    /// The connection tabs on screen at the last layout change.
    displayed: Vec<PanelId>,
    /// What was wrong with `keymap.toml`, in full. It stays until the file is fixed.
    keymap_errors: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

/// What the status bar counts.
fn count_running(jobs: &Jobs) -> Option<usize> {
    (!jobs.jobs().is_empty()).then(|| jobs.running())
}

impl RootView {
    /// Builds the view and keeps the theme in sync with the system appearance.
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let (dock, skin) = DockSkin::dock_area("tessera", None, window, cx);
        skin.set_close_button_visible(true, cx);
        let sidebar = cx.new(|cx| SidebarPanel::new(state.clone(), cx));
        let log = cx.new(|cx| RequestLogPanel::new(state.clone(), window, cx));
        let jobs = cx.new(|cx| JobsPanel::new(state.clone(), window, cx));
        let welcome = cx.new(|cx| WelcomePanel::new(state.clone(), cx));
        dock.update(cx, |area, cx| {
            let tabs = |panel| DockLayout::tabs().panel_view(panel, cx);
            let (left, bottom, center) = (
                tabs(panel_handle(sidebar)),
                tabs(panel_handle(log.clone()))
                    .panel_view(panel_handle(jobs.clone()), cx)
                    .active_index(0),
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
        let job_list = cx.global::<JobsGlobal>().0.clone();
        let running_jobs = count_running(job_list.read(cx));
        let subscriptions = vec![
            cx.subscribe(&job_list, |this, jobs, _: &RunningChanged, cx| {
                this.running_jobs = count_running(jobs.read(cx));
                cx.notify();
            }),
            cx.subscribe_in(
                &job_list,
                window,
                |this, _, event: &JobFinished, window, cx| this.job_finished(event.0, window, cx),
            ),
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
                    this.open_tab(event.0, Kind::Overview, window, cx);
                },
            ),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &OpenCollections, window, cx| {
                    this.open_tab(event.0, Kind::Collections, window, cx);
                },
            ),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &OpenCollection, window, cx| {
                    this.open_tab(event.0, Kind::Collection(event.1.clone()), window, cx);
                },
            ),
            cx.subscribe_in(&state, window, |this, _, event: &OpenSearch, window, cx| {
                this.open_tab(event.0, Kind::Search, window, cx);
                let Some((drafts, union)) = event.1.clone() else {
                    return;
                };
                let tab = this
                    .active
                    .and_then(|id| connection_tab(this.dock.read(cx), id));
                if let Some(ConnectionTab::Search(panel)) = tab {
                    let workbench = panel.read(cx).workbench().clone();
                    workbench.update(cx, |w, cx| {
                        w.load(drafts, union, window, cx);
                        w.set_preset(event.2.clone(), cx);
                    });
                }
            }),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &OpenConsole, window, cx| {
                    this.open_tab(event.0, Kind::Console, window, cx);
                },
            ),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &OpenResource, window, cx| {
                    this.open_tab(event.0, Kind::Resource(event.1), window, cx);
                },
            ),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &CloseCollection, window, cx| {
                    this.close_collection_tab(event.0, &event.1, window, cx);
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
            log,
            jobs,
            running_jobs,
            active: None,
            displayed: Vec::new(),
            keymap_errors: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The connection of the active tab.
    pub fn active_connection(&self, cx: &App) -> Option<Entity<ConnectionState>> {
        Some(connection_tab(self.dock.read(cx), self.active?)?.connection(cx))
    }

    /// The request log panel.
    pub fn log_panel(&self) -> &Entity<RequestLogPanel> {
        &self.log
    }

    /// Whether the dock is open with `panel` as its displayed tab.
    fn bottom_shows(&self, panel: PanelId, cx: &App) -> bool {
        let area = self.dock.read(cx);
        let mut shown = false;
        if let Some(tree) = area
            .layout(DockPlacement::Bottom)
            .filter(|_| area.is_dock_open(DockPlacement::Bottom))
        {
            tree.root().walk(&mut |node| {
                if let PaneRef::Tabs { panels, active_ix } = node.kind() {
                    shown |= panels.get(active_ix) == Some(&panel);
                }
            });
        }
        shown
    }

    fn log_id(&self) -> PanelId {
        PanelId::from(self.log.entity_id())
    }

    fn jobs_id(&self) -> PanelId {
        PanelId::from(self.jobs.entity_id())
    }

    /// Opens the dock on `panel` or selects it there; closes the dock if it shows `panel`.
    fn toggle_bottom(&mut self, panel: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        // Focus inside a panel that goes away would leave no element to receive the chord again.
        let had_focus = self.log.focus_handle(cx).contains_focused(window, cx)
            || self.jobs.focus_handle(cx).contains_focused(window, cx);
        if self.bottom_shows(panel, cx) {
            self.dock.update(cx, |area, cx| {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            });
        } else {
            self.reveal(panel, window, cx);
        }
        if had_focus {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn reveal(&mut self, panel: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        self.dock.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
            area.select_panel(panel, window, cx);
        });
    }

    /// Shows the jobs tab. Focus stays where it is.
    pub fn show_jobs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal(self.jobs_id(), window, cx);
        cx.notify();
    }

    /// Shows the jobs tab of the window's root view, from a dialog.
    pub fn reveal_jobs(window: &mut Window, cx: &mut App) {
        let root = Root::read(window, cx).view().clone().downcast::<Self>();
        if let Ok(root) = root {
            root.update(cx, |root, cx| root.show_jobs(window, cx));
        }
    }

    /// Reloads what an import wrote, also after a stop or a failure: what arrived stays on the
    /// server. By profile, as the connection may be a new one by now.
    fn reload_after(&self, id: JobId, cx: &mut Context<Self>) {
        let jobs: &Jobs = cx.global::<JobsGlobal>().0.read(cx);
        let job = jobs.jobs().iter().find(|job| job.id == id);
        let Some(job) = job.filter(|job| job.kind == JobKind::Import && job.outcome.is_some())
        else {
            return;
        };
        let name = job.collection.clone();
        if let Some(connection) = self.state.read(cx).connection(job.connection, cx) {
            connection.update(cx, |c, cx| c.collection_changed(name, cx));
        }
    }

    /// A job ending is a background event, so it is the one toast, unless its tab is on screen.
    fn job_finished(&mut self, id: JobId, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_after(id, cx);
        if self.bottom_shows(self.jobs_id(), cx) {
            return;
        }
        let jobs: &Jobs = cx.global::<JobsGlobal>().0.read(cx);
        let Some(job) = jobs.jobs().iter().find(|job| job.id == id) else {
            return;
        };
        let text = job.toast_text();
        window.push_notification(text, cx);
    }

    fn open_palette(&self, mode: PaletteMode, window: &mut Window, cx: &mut Context<Self>) {
        if !window.has_active_dialog(cx) {
            palette::open(mode, self.state.clone(), window, cx);
        }
    }

    /// Shows what was wrong with `keymap.toml` above the tabs.
    pub fn set_keymap_errors(&mut self, errors: Vec<String>, cx: &mut Context<Self>) {
        self.keymap_errors = errors;
        cx.notify();
    }

    /// The dock area, for tests that rearrange panels as a user would by dragging.
    pub fn dock(&self) -> &Entity<DockArea> {
        &self.dock
    }

    /// Every connection tab in the dock with the region it sits in.
    fn connection_tabs(&self, cx: &App) -> Vec<(DockPlacement, ConnectionTab)> {
        let area = self.dock.read(cx);
        REGIONS
            .into_iter()
            .filter_map(|placement| Some((placement, area.layout(placement)?)))
            .flat_map(|(placement, tree)| tree.panels().map(move |id| (placement, id)))
            .filter_map(|(placement, id)| Some((placement, connection_tab(area, id)?)))
            .collect()
    }

    /// The connection tabs on screen: the displayed tab of each group in the center and in
    /// the open docks.
    fn displayed_tabs(&self, cx: &App) -> Vec<PanelId> {
        let area = self.dock.read(cx);
        let mut displayed = Vec::new();
        for placement in REGIONS {
            if placement != DockPlacement::Center && !area.is_dock_open(placement) {
                continue;
            }
            let Some(tree) = area.layout(placement) else {
                continue;
            };
            tree.root().walk(&mut |node| {
                if let PaneRef::Tabs { panels, active_ix } = node.kind() {
                    displayed.extend(
                        panels
                            .get(active_ix)
                            .filter(|id| connection_tab(area, **id).is_some()),
                    );
                }
            });
        }
        displayed
    }

    /// Makes `active` the active tab; the sidebar selection follows when it changes.
    fn set_active(&mut self, active: Option<PanelId>, cx: &mut Context<Self>) {
        cx.notify();
        if self.active == active {
            return;
        }
        self.active = active;
        let Some(id) = self.active_connection(cx).map(|c| c.read(cx).profile.id) else {
            return;
        };
        self.state.update(cx, |s, cx| {
            if s.selected != Some(id) {
                s.selected = Some(id);
                cx.notify();
            }
        });
    }

    /// Opens the `kind` tab of profile `id`, or shows and activates the one already open,
    /// opening the dock it was moved into.
    fn open_tab(
        &mut self,
        id: uuid::Uuid,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.state.read(cx).connection(id, cx) else {
            return;
        };
        if conn.read(cx).connected.is_none() {
            return;
        }
        let existing = self
            .connection_tabs(cx)
            .into_iter()
            .find(|(_, tab)| tab.kind(cx) == kind && tab.connection(cx) == conn);
        let welcome = self.welcome.clone();
        let state = self.state.clone();
        let panel = self.dock.update(cx, |area, cx| {
            let (id, new) = match existing {
                Some((placement, tab)) => {
                    if placement != DockPlacement::Center && !area.is_dock_open(placement) {
                        area.toggle_dock(placement, window, cx);
                    }
                    (tab.id(), None)
                }
                None => {
                    let (id, handle) = match kind {
                        Kind::Overview => {
                            let panel = cx.new(|cx| OverviewPanel::new(conn, window, cx));
                            (PanelId::from(panel.entity_id()), panel_handle(panel))
                        }
                        Kind::Collections => {
                            let panel = cx.new(|cx| CollectionsPanel::new(conn, state, window, cx));
                            (PanelId::from(panel.entity_id()), panel_handle(panel))
                        }
                        Kind::Collection(name) => {
                            let panel =
                                cx.new(|cx| CollectionPanel::new(conn, state, name, window, cx));
                            (PanelId::from(panel.entity_id()), panel_handle(panel))
                        }
                        Kind::Search => {
                            let panel = cx.new(|cx| SearchPanel::new(conn, state, window, cx));
                            (PanelId::from(panel.entity_id()), panel_handle(panel))
                        }
                        Kind::Console => {
                            let panel = cx.new(|cx| ConsolePanel::new(conn, state, window, cx));
                            (PanelId::from(panel.entity_id()), panel_handle(panel))
                        }
                        Kind::Resource(kind) => {
                            let panel =
                                cx.new(|cx| ResourcePanel::new(conn, state, kind, window, cx));
                            (PanelId::from(panel.entity_id()), panel_handle(panel))
                        }
                    };
                    (id, Some(handle))
                }
            };
            if let Some(handle) = new {
                area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
                area.remove_panel(welcome, window, cx);
            }
            // A no-op for a tab already displayed, e.g. in another split group.
            area.select_panel(id, window, cx);
            id
        });
        // Set here, not from the layout event: selecting a displayed tab changes no layout.
        self.displayed = self.displayed_tabs(cx);
        self.set_active(Some(panel), cx);
        // Its shortcuts (search, favorite) work from the moment the tab is open.
        match connection_tab(self.dock.read(cx), panel) {
            Some(ConnectionTab::Collections(tab)) => tab.focus_handle(cx).focus(window, cx),
            Some(ConnectionTab::Collection(tab)) => tab.focus_handle(cx).focus(window, cx),
            Some(ConnectionTab::Search(tab)) => tab.focus_handle(cx).focus(window, cx),
            Some(ConnectionTab::Console(tab)) => tab.focus_handle(cx).focus(window, cx),
            Some(ConnectionTab::Resource(tab)) => tab.focus_handle(cx).focus(window, cx),
            _ => {}
        }
    }

    /// Closes the schema tab of collection `name` of profile `id`, if open.
    fn close_collection_tab(
        &mut self,
        id: uuid::Uuid,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self
            .connection_tabs(cx)
            .into_iter()
            .find_map(|(_, tab)| match tab {
                ConnectionTab::Collection(panel) => {
                    let p = panel.read(cx);
                    (p.name() == name && p.connection().read(cx).profile.id == id).then_some(panel)
                }
                _ => None,
            });
        if let Some(panel) = tab {
            self.dock
                .update(cx, |area, cx| area.remove_panel(panel, window, cx));
        }
    }

    /// Closes the tabs of connections that were closed. Closing a tab never disconnects.
    fn close_stale_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let stale: Vec<_> = self
            .connection_tabs(cx)
            .into_iter()
            .map(|(_, tab)| tab)
            .filter(|tab| !state.connections.contains(&tab.connection(cx)))
            .collect();
        if stale.is_empty() {
            return;
        }
        self.dock.update(cx, |area, cx| {
            for tab in stale {
                match tab {
                    ConnectionTab::Overview(panel) => area.remove_panel(panel, window, cx),
                    ConnectionTab::Collections(panel) => area.remove_panel(panel, window, cx),
                    ConnectionTab::Collection(panel) => area.remove_panel(panel, window, cx),
                    ConnectionTab::Search(panel) => area.remove_panel(panel, window, cx),
                    ConnectionTab::Console(panel) => area.remove_panel(panel, window, cx),
                    ConnectionTab::Resource(panel) => area.remove_panel(panel, window, cx),
                }
            }
        });
    }

    /// Finds the active tab after a layout change and brings the welcome screen back into
    /// an empty center.
    fn layout_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let displayed = self.displayed_tabs(cx);
        // shortcut: a tab that just came on screen wins; a click into a group whose tab is
        // already displayed changes nothing, until tabs report focus.
        let fresh = displayed.iter().find(|id| !self.displayed.contains(id));
        let active = fresh
            .or_else(|| self.active.as_ref().filter(|id| displayed.contains(id)))
            .or(displayed.first())
            .copied();
        self.displayed = displayed;
        let center_empty = self
            .dock
            .read(cx)
            .layout(DockPlacement::Center)
            .is_none_or(|tree| tree.panels().next().is_none());
        if center_empty {
            let welcome = panel_handle(self.welcome.clone());
            self.dock.update(cx, |area, cx| {
                area.add_panel_view(welcome, DockPlacement::Center, None, window, cx);
            });
        }
        self.set_active(active, cx);
    }
}

/// The regions of the dock area.
const REGIONS: [DockPlacement; 4] = [
    DockPlacement::Center,
    DockPlacement::Left,
    DockPlacement::Right,
    DockPlacement::Bottom,
];

/// The kinds of tab a connection has.
#[derive(Clone, PartialEq)]
enum Kind {
    Overview,
    Collections,
    /// The schema tab of the named collection.
    Collection(String),
    Search,
    Console,
    Resource(ResourceKind),
}

/// A tab of one connection.
enum ConnectionTab {
    Overview(Entity<OverviewPanel>),
    Collections(Entity<CollectionsPanel>),
    Collection(Entity<CollectionPanel>),
    Search(Entity<SearchPanel>),
    Console(Entity<ConsolePanel>),
    Resource(Entity<ResourcePanel>),
}

impl ConnectionTab {
    fn kind(&self, cx: &App) -> Kind {
        match self {
            Self::Overview(_) => Kind::Overview,
            Self::Collections(_) => Kind::Collections,
            Self::Collection(panel) => Kind::Collection(panel.read(cx).name().to_owned()),
            Self::Search(_) => Kind::Search,
            Self::Console(_) => Kind::Console,
            Self::Resource(panel) => Kind::Resource(panel.read(cx).kind()),
        }
    }

    fn id(&self) -> PanelId {
        PanelId::from(match self {
            Self::Overview(panel) => panel.entity_id(),
            Self::Collections(panel) => panel.entity_id(),
            Self::Collection(panel) => panel.entity_id(),
            Self::Search(panel) => panel.entity_id(),
            Self::Console(panel) => panel.entity_id(),
            Self::Resource(panel) => panel.entity_id(),
        })
    }

    fn connection(&self, cx: &App) -> Entity<ConnectionState> {
        match self {
            Self::Overview(panel) => panel.read(cx).connection().clone(),
            Self::Collections(panel) => panel.read(cx).connection().clone(),
            Self::Collection(panel) => panel.read(cx).connection().clone(),
            Self::Search(panel) => panel.read(cx).connection().clone(),
            Self::Console(panel) => panel.read(cx).connection().clone(),
            Self::Resource(panel) => panel.read(cx).connection().clone(),
        }
    }
}

/// The connection tab `id`, if the dock holds one under that id.
fn connection_tab(area: &DockArea, id: PanelId) -> Option<ConnectionTab> {
    let view = area.panel(id)?.view();
    let view = match view.clone().downcast::<OverviewPanel>() {
        Ok(panel) => return Some(ConnectionTab::Overview(panel)),
        Err(view) => view,
    };
    let view = match view.downcast::<CollectionsPanel>() {
        Ok(panel) => return Some(ConnectionTab::Collections(panel)),
        Err(view) => view,
    };
    let view = match view.downcast::<CollectionPanel>() {
        Ok(panel) => return Some(ConnectionTab::Collection(panel)),
        Err(view) => view,
    };
    let view = match view.downcast::<SearchPanel>() {
        Ok(panel) => return Some(ConnectionTab::Search(panel)),
        Err(view) => view,
    };
    match view.downcast::<ConsolePanel>() {
        Ok(panel) => Some(ConnectionTab::Console(panel)),
        Err(view) => view
            .downcast::<ResourcePanel>()
            .ok()
            .map(ConnectionTab::Resource),
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active_connection(cx);
        let active = active.as_ref().map(|c| c.read(cx));
        let profile = active.map(|c| &c.profile);
        let unlocked = active.is_some_and(|c| c.writes_unlocked);
        let prod = profile.is_some_and(|p| p.environment.is_prod());
        let log_open = self.bottom_shows(self.log_id(), cx);
        let jobs = self.running_jobs.map(|running| status_bar::JobsEntry {
            running,
            open: self.bottom_shows(self.jobs_id(), cx),
        });
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
                    el.child(env_badge::lock("titlebar", unlocked, cx))
                })
            });
        let red = rgb(env_badge::PROD_RED);
        let state = self.state.read(cx);
        let can_search = state
            .selected
            .and_then(|id| state.connection(id, cx))
            .filter(|c| c.read(cx).connected.is_some())
            .map(|c| c.read(cx).profile.id);
        let supported = can_search
            .and_then(|id| state.connection(id, cx))
            .and_then(|c| c.read(cx).connected.as_ref())
            .is_some_and(|c| c.info.capabilities.supported);
        let shown = |kind: ResourceKind| can_search.filter(|_| kind.shown(supported));
        let open = |this: &mut Self, id, kind, cx: &mut Context<Self>| {
            this.state.update(cx, |s, cx| s.open_resource(id, kind, cx));
        };
        let root = v_flex();
        #[cfg(feature = "frame-overlay")]
        let root = root.on_action(|_: &crate::actions::ToggleFrameOverlay, window, _| {
            window.cycle_debug_frame_overlay_mode();
        });
        root.relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
                ConnectDialog::open_new(this.state.clone(), None, window, cx);
            }))
            // A hidden log has no visible selection to copy.
            .when(log_open && self.log.read(cx).has_selection(cx), |el| {
                el.on_action(cx.listener(|this, _: &CopyAsCurl, _, cx| {
                    this.log.update(cx, |log, cx| log.copy_as_curl(cx));
                }))
            })
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.open_palette(PaletteMode::All, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickSwitch, window, cx| {
                this.open_palette(PaletteMode::Collections, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleRequestLog, window, cx| {
                this.toggle_bottom(this.log_id(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleJobs, window, cx| {
                this.toggle_bottom(this.jobs_id(), window, cx);
            }))
            .when_some(can_search, |el, id| {
                el.on_action(cx.listener(move |this, _: &OpenSearchTab, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.open_search(id, None, None, cx));
                }))
                .on_action(cx.listener(
                    move |this, _: &OpenConsoleTab, _, cx| {
                        this.state.update(cx, |s, cx| s.open_console(id, cx));
                    },
                ))
            })
            .when_some(shown(ResourceKind::Aliases), |el, id| {
                el.on_action(cx.listener(move |this, _: &OpenAliases, _, cx| {
                    open(this, id, ResourceKind::Aliases, cx);
                }))
            })
            .when_some(shown(ResourceKind::Synonyms), |el, id| {
                el.on_action(cx.listener(move |this, _: &OpenSynonyms, _, cx| {
                    open(this, id, ResourceKind::Synonyms, cx);
                }))
            })
            .when_some(shown(ResourceKind::Curations), |el, id| {
                el.on_action(cx.listener(move |this, _: &OpenCurations, _, cx| {
                    open(this, id, ResourceKind::Curations, cx);
                }))
            })
            .when_some(shown(ResourceKind::Stopwords), |el, id| {
                el.on_action(cx.listener(move |this, _: &OpenStopwords, _, cx| {
                    open(this, id, ResourceKind::Stopwords, cx);
                }))
            })
            .when_some(shown(ResourceKind::Presets), |el, id| {
                el.on_action(cx.listener(move |this, _: &OpenPresetsTab, _, cx| {
                    open(this, id, ResourceKind::Presets, cx);
                }))
            })
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(TitleBar::new().child(switcher))
            .when(!self.keymap_errors.is_empty(), |el| {
                let text = SharedString::from(self.keymap_errors.join("\n"));
                el.child(
                    div()
                        .id("keymap.errors")
                        .test_support()
                        .aria_label(text.clone())
                        .flex_none()
                        .max_h(px(KEYMAP_ERRORS_HEIGHT))
                        .overflow_y_scroll()
                        .p_2()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(text),
                )
            })
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
            .child(status_bar::render(active, log_open, jobs, window, cx))
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
