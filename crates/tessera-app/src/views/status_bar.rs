// SPDX-License-Identifier: GPL-3.0-or-later

//! The bottom status bar.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*, px,
};
use rust_i18n::t;

use super::overview::role_text;
use crate::connection::ServerInfo;

/// Renders the status bar for the current connection state.
pub fn render(info: Option<&ServerInfo>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (text, dot) = match info {
        Some(info) => (
            [
                t!("status.connected").to_string(),
                info.host_label(),
                format!("v{}", info.version),
                role_text(info.state),
                format!("{} ms", info.latency.as_millis()),
            ]
            .join(" · "),
            theme.success,
        ),
        None => (
            t!("status.not_connected").to_string(),
            theme.muted_foreground,
        ),
    };
    let text = SharedString::from(text);
    StatusBar::new().left(
        h_flex()
            .id("status.text")
            .test_support()
            .aria_label(text.clone())
            .gap_2()
            .items_center()
            .child(div().size(px(7.)).rounded_full().bg(dot))
            .child(text),
    )
}
