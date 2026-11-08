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
use crate::actions::{ToggleJobs, ToggleRequestLog};
use crate::connection::{ConnectionState, host_label};

/// What the status bar says about a connected connection (spec §7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The status line: the active node's last poll, or the connect result before it.
    pub text: String,
    /// The active node waits for a retry, so the line is not "Connected".
    pub down: bool,
    /// How many nodes wait for a retry, if any.
    pub unreachable: Option<String>,
}

/// The status bar's summary of `conn`; `None` while it is not connected.
pub fn summary(conn: &ConnectionState) -> Option<Summary> {
    let connected = conn.connected.as_ref()?;
    let info = &connected.info;
    let active = connected.client.active_node();
    let node = conn.nodes.get(active);
    let host = connected
        .client
        .nodes()
        .get(active)
        .map_or_else(|| info.host_label(), |url| host_label(url.as_str()));
    let down = node.is_some_and(|n| n.next_retry.is_some());
    let text = if down {
        t!("status.active_down", name = conn.profile.name, node = host).to_string()
    } else {
        let debug = node.and_then(|n| n.debug.as_ref());
        t!(
            "status.connected",
            name = conn.profile.name,
            version = debug.map_or(info.version.as_str(), |d| d.version.as_str()),
            role = role_text(debug.map_or(info.state, |d| d.state)),
            node = host,
            ms = node
                .and_then(|n| n.latency)
                .unwrap_or(info.latency)
                .as_millis()
        )
        .to_string()
    };
    // Nodes waiting for a retry: their /health or /debug failed.
    let count = conn.nodes.iter().filter(|n| n.next_retry.is_some()).count();
    let unreachable = (count > 0).then(|| match conn.nodes.len() {
        1 => t!("status.unreachable_one").to_string(),
        m => t!("status.unreachable", n = count, m = m).to_string(),
    });
    Some(Summary {
        text,
        down,
        unreachable,
    })
}

/// What the status bar's entry for the jobs shows.
pub struct JobsEntry {
    /// How many run.
    pub running: usize,
    /// The jobs tab is on screen.
    pub open: bool,
}

/// Renders the status bar for the active connection, with the request log toggle and, while
/// a job exists, the jobs toggle.
pub fn render(
    selected: Option<&ConnectionState>,
    log_open: bool,
    jobs: Option<JobsEntry>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let summary = selected.and_then(summary);
    let (text, dot) = match &summary {
        Some(s) if s.down => (s.text.clone(), theme.danger),
        Some(s) => (s.text.clone(), theme.success),
        None => (
            t!("status.not_connected").to_string(),
            theme.muted_foreground,
        ),
    };
    let text = SharedString::from(text);
    let unreachable = summary.and_then(|s| s.unreachable).map(|text| {
        let text = SharedString::from(text);
        div()
            .id("status.unreachable")
            .test_support()
            .aria_label(text.clone())
            .text_color(theme.danger)
            .child(text)
    });
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
                .gap_3()
                .items_center()
                .child(
                    h_flex()
                        .id("status.text")
                        .test_support()
                        .aria_label(text.clone())
                        .gap_2()
                        .items_center()
                        .child(div().size(px(7.)).rounded_full().bg(dot))
                        .child(text),
                )
                .children(unreachable),
        )
        .right(
            h_flex()
                .gap_2()
                .items_center()
                .children(prod_read_only)
                .children(jobs.map(|jobs| {
                    let text = match jobs.running {
                        0 => t!("status.jobs_idle").to_string(),
                        1 => t!("status.jobs_one").to_string(),
                        count => t!("status.jobs", count = count).to_string(),
                    };
                    Button::new("status.jobs")
                        .ghost()
                        .xsmall()
                        .selected(jobs.open)
                        .label(text)
                        .children(Kbd::global_binding_for_action(&ToggleJobs, window))
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleJobs), cx))
                }))
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
