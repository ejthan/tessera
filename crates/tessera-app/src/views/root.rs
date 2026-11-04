// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's top-level view: title bar, sidebar, main area, status bar.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Theme, TitleBar, h_flex, v_flex};
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Window, div, prelude::*, px,
};
use rust_i18n::t;

use super::status_bar;

/// Width of the left sidebar.
const SIDEBAR_WIDTH: f32 = 240.;

/// The main window content.
pub struct RootView {
    _appearance: Subscription,
}

impl RootView {
    /// Builds the view and keeps the theme in sync with the system appearance.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        Self {
            _appearance: appearance,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let connection = SharedString::from(t!("titlebar.no_connection").to_string());
        v_flex()
            .size_full()
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
                    .child(div().flex_1().h_full().min_w_0()),
            )
            .child(status_bar::render())
    }
}
