// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's top-level view: title bar, sidebar, main area, status bar.

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Theme, TitleBar, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px, rgb,
};
use rust_i18n::t;

use super::sidebar::Sidebar;
use super::{connect_dialog::ConnectDialog, env_badge, overview, status_bar, welcome};
use crate::actions::NewConnection;
use crate::state::AppState;

/// Width of the left sidebar.
const SIDEBAR_WIDTH: f32 = 240.;
/// How long the Copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_secs(2);

/// The main window content.
pub struct RootView {
    focus_handle: FocusHandle,
    state: Entity<AppState>,
    sidebar: Entity<Sidebar>,
    copied: bool,
    copy_reset: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    /// Builds the view and keeps the theme in sync with the system appearance.
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let sidebar = cx.new(|cx| Sidebar::new(state.clone(), cx));
        Self {
            focus_handle,
            sidebar,
            state,
            copied: false,
            copy_reset: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ConnectDialog::open(self.state.clone(), window, cx);
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

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let state = self.state.read(cx);
        let load_error = state.load_error.clone();
        let selected = state
            .selected
            .and_then(|id| state.connection(id, cx))
            .map(|c| c.read(cx));
        let profile = selected.map(|c| &c.profile);
        let info = selected.and_then(|c| c.connected.as_ref().map(|c| c.info.clone()));
        let prod = profile.is_some_and(|p| p.environment.is_prod());
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
        let main = match &info {
            Some(info) => overview::render(info, cx).into_any_element(),
            None => {
                let this = cx.entity();
                let copy_target = this.clone();
                welcome::render(
                    self.copied,
                    load_error,
                    move |_, window, cx| {
                        this.update(cx, |view, cx| view.open_connect(window, cx));
                    },
                    move |_, _, cx| {
                        copy_target.update(cx, |view, cx| view.copy_docker_command(cx));
                    },
                    window,
                    cx,
                )
                .into_any_element()
            }
        };
        let red = rgb(env_badge::PROD_RED);
        v_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
                this.open_connect(window, cx);
            }))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(TitleBar::new().child(switcher))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("sidebar")
                            .test_support()
                            .overflow_y_scroll()
                            .w(px(SIDEBAR_WIDTH))
                            .h_full()
                            .bg(theme.sidebar)
                            .border_r_1()
                            .border_color(theme.sidebar_border)
                            .child(self.sidebar.clone()),
                    )
                    .child(div().flex_1().h_full().min_w_0().child(main)),
            )
            .child(status_bar::render(selected, cx))
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
