// SPDX-License-Identifier: GPL-3.0-or-later

//! The bottom status bar.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*, px, rgb,
};
use rust_i18n::t;

use super::env_badge::PROD_RED;
use super::overview::role_text;
use crate::connection::ConnectionState;

/// Renders the status bar for the selected connection.
pub fn render(selected: Option<&ConnectionState>, cx: &App) -> impl IntoElement {
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
        .right(h_flex().children(prod_read_only))
}
