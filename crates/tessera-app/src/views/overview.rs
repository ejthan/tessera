// SPDX-License-Identifier: GPL-3.0-or-later

//! Overview of the connected server.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, FontWeight, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, div,
    prelude::*,
};
use rust_i18n::t;
use tessera_client::RaftState;

use crate::connection::ServerInfo;

/// Display text for a Raft role.
pub fn role_text(state: RaftState) -> String {
    match state {
        RaftState::Leader => t!("role.leader").to_string(),
        RaftState::Follower => t!("role.follower").to_string(),
        RaftState::Other(n) => t!("role.other", state = n).to_string(),
        RaftState::Unknown => t!("role.unknown").to_string(),
    }
}

/// Renders the overview for a connected server.
pub fn render(info: &ServerInfo, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (health, health_color) = if info.healthy {
        (t!("overview.healthy").to_string(), theme.success)
    } else {
        (t!("overview.unhealthy").to_string(), theme.danger)
    };
    let card = |id: &'static str, label: String, value: String, color: Hsla| {
        let value = SharedString::from(value);
        v_flex()
            .id(id)
            .test_support()
            .aria_label(value.clone())
            .flex_1()
            .gap_1()
            .p_3()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(value),
            )
    };
    v_flex()
        .size_full()
        .gap_4()
        .p_5()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("overview.title").to_string()),
        )
        .when(!info.capabilities.supported, |el| {
            let text = SharedString::from(
                t!("overview.unsupported", version = info.version.as_str()).to_string(),
            );
            el.child(
                div()
                    .id("overview.unsupported")
                    .test_support()
                    .aria_label(text.clone())
                    .p_3()
                    .border_1()
                    .border_color(theme.warning)
                    .rounded_md()
                    .text_sm()
                    .child(text),
            )
        })
        .child(
            h_flex()
                .gap_3()
                .child(card(
                    "overview.version",
                    t!("overview.version").to_string(),
                    info.version.clone(),
                    theme.foreground,
                ))
                .child(card(
                    "overview.role",
                    t!("overview.role").to_string(),
                    role_text(info.state),
                    theme.foreground,
                ))
                .child(card(
                    "overview.health",
                    t!("overview.health").to_string(),
                    health,
                    health_color,
                )),
        )
}
