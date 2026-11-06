// SPDX-License-Identifier: GPL-3.0-or-later

//! The banner above a connection's tabs when its transport can be read or changed on the way.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*,
};
use rust_i18n::t;
use tessera_core::profile::TransportRisk;

use super::connect_dialog::ConnectDialog;
use crate::connection::ConnectionState;
use crate::state::AppState;

fn risk_text(risk: &TransportRisk) -> String {
    match risk {
        TransportRisk::PlainHttp { host } => t!("transport.plain_http", host = host).to_string(),
        TransportRisk::InsecureCertificates => t!("transport.insecure").to_string(),
    }
}

/// One line per risk, or nothing. There is no way to close it: the fix is in the profile, and a
/// temporary connection has none to edit.
pub fn banner(
    conn: &Entity<ConnectionState>,
    state: &Entity<AppState>,
    cx: &App,
) -> Option<impl IntoElement> {
    let profile = &conn.read(cx).profile;
    let risks = profile.transport_risks();
    if risks.is_empty() {
        return None;
    }
    let id = profile.id;
    let saved = state.read(cx).profiles.iter().any(|p| p.id == id);
    let state = state.clone();
    let text: Vec<String> = risks.iter().map(risk_text).collect();
    let theme = cx.theme();
    Some(
        h_flex()
            .id("transport.banner")
            .test_support()
            .aria_label(SharedString::from(text.join(" ")))
            .flex_none()
            .gap_2()
            .px_3()
            .py_1()
            .items_center()
            .text_sm()
            .bg(theme.warning)
            .text_color(theme.warning_foreground)
            .child(
                v_flex()
                    .flex_1()
                    .children(text.into_iter().map(|line| div().child(line))),
            )
            .when(saved, |el| {
                el.child(
                    Button::new("transport.edit")
                        .xsmall()
                        .label(t!("transport.edit").to_string())
                        .on_click(move |_, window, cx| {
                            ConnectDialog::open_edit(state.clone(), id, window, cx);
                        }),
                )
            }),
    )
}
