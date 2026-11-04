// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's top-level view: title bar, sidebar, main area, status bar.

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Theme, TitleBar, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;

use super::{connect_dialog::ConnectDialog, status_bar, welcome};
use crate::actions::NewConnection;
use crate::connection::Connection;

/// Width of the left sidebar.
const SIDEBAR_WIDTH: f32 = 240.;
/// How long the Copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_secs(2);

/// The main window content.
pub struct RootView {
    focus_handle: FocusHandle,
    connection: Entity<Connection>,
    copied: bool,
    copy_reset: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    /// Builds the view and keeps the theme in sync with the system appearance.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let connection = cx.new(|_| Connection::default());
        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
            cx.observe(&connection, |_, _, cx| cx.notify()),
        ];
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self {
            focus_handle,
            connection,
            copied: false,
            copy_reset: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ConnectDialog::open(self.connection.clone(), window, cx);
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let connection = SharedString::from(t!("titlebar.no_connection").to_string());
        let this = cx.entity();
        let copy_target = this.clone();
        let main = welcome::render(
            self.copied,
            move |_, window, cx| {
                this.update(cx, |view, cx| view.open_connect(window, cx));
            },
            move |_, _, cx| {
                copy_target.update(cx, |view, cx| view.copy_docker_command(cx));
            },
            cx,
        );
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
                this.open_connect(window, cx);
            }))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                TitleBar::new().child(
                    div()
                        .id("titlebar.connection")
                        .test_support()
                        .aria_label(connection.clone())
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(connection),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("sidebar")
                            .test_support()
                            .w(px(SIDEBAR_WIDTH))
                            .h_full()
                            .bg(theme.sidebar)
                            .border_r_1()
                            .border_color(theme.sidebar_border),
                    )
                    .child(div().flex_1().h_full().min_w_0().child(main)),
            )
            .child(status_bar::render())
    }
}
