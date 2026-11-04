// SPDX-License-Identifier: GPL-3.0-or-later

//! The bottom status bar.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, prelude::*, px,
    rgb,
};
use rust_i18n::t;

use super::env_badge::PROD_RED;
use super::overview::role_text;
use crate::actions::ToggleRequestLog;
use crate::connection::ConnectionState;

/// Renders the status bar for the active connection, with the request log toggle.
pub fn render(
    selected: Option<&ConnectionState>,
    log_open: bool,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let info = selected.and_then(|c| Some((&c.profile, &c.connected.as_ref()?.info)));
    let (text, dot) = match info {
        Some((profile, info)) => (
            t!(
                "status.connected",
                name = profile.name,
                version = info.version,
                role = role_text(info.state),
                node = info.host_label(),
                ms = info.latency.as_millis()
            )
            .to_string(),
            theme.success,
        ),
        None => (
            t!("status.not_connected").to_string(),
            theme.muted_foreground,
        ),
    };
    let text = SharedString::from(text);
    let prod_read_only = selected
        .map(|c| &c.profile)
        .filter(|p| p.environment.is_prod() && p.read_only)
        .map(|_| {
            div()
                .text_color(rgb(PROD_RED))
                .child(t!("status.prod_read_only").to_string())
        });
    StatusBar::new()
        .left(
            h_flex()
                .id("status.text")
                .test_support()
                .aria_label(text.clone())
                .gap_2()
                .items_center()
                .child(div().size(px(7.)).rounded_full().bg(dot))
                .child(text),
        )
        .right(
            h_flex()
                .gap_2()
                .items_center()
                .children(prod_read_only)
                .child(
                    Button::new("status.toggle_log")
                        .ghost()
                        .xsmall()
                        .selected(log_open)
                        .label(t!("status.toggle_log").to_string())
                        .children(Kbd::global_binding_for_action(&ToggleRequestLog, window))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleRequestLog), cx)
                        }),
                ),
        )
}
