// SPDX-License-Identifier: GPL-3.0-or-later

//! A node card on the overview: name, `host:port`, role, latency and the active marker. An
//! unreachable node's card is red, with the full error and the retry countdown.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, FocusHandle, FontWeight, IntoElement, Keystroke, ParentElement as _,
    SharedString, Styled as _, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::RaftState;

use super::overview::role_text;
use crate::poller::NodeStatus;

/// What one card shows.
pub struct Card<'a> {
    /// Index into the client's nodes.
    pub index: usize,
    /// `host:port`.
    pub host: String,
    /// Polled status.
    pub status: &'a NodeStatus,
    /// Requests go to this node.
    pub active: bool,
    /// The executor's clock, for the countdown.
    pub now: Instant,
}

/// Renders `card`; a click or Enter calls `on_activate`.
pub fn render(
    card: Card,
    focus: &FocusHandle,
    on_activate: impl Fn(&mut App) + 'static,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let Card {
        index,
        host,
        status,
        active,
        now,
    } = card;
    let unreachable = status.next_retry.is_some();
    let role = status.debug.as_ref().map(|d| d.state);
    let role_label = role
        .map(role_text)
        .unwrap_or_else(|| t!("figure.missing").to_string());
    let latency = status
        .latency
        .map(|l| t!("units.ms", n = l.as_millis()).to_string());
    let title = t!("node.title", n = index + 1).to_string();
    let mut parts = vec![title.clone(), host.clone(), role_label.clone()];
    parts.extend(latency.clone());
    if active {
        parts.push(t!("node.active").to_string());
    }
    let id = format!("node.{index}");
    let retry = status.next_retry.map(|at| {
        let secs = at.saturating_duration_since(now).as_secs_f64().ceil() as u64;
        t!("node.retry", s = secs).to_string()
    });
    let error = status.error.as_ref().map(|error| {
        let label = match &retry {
            Some(retry) => format!("{error}\n{retry}"),
            None => error.to_string(),
        };
        v_flex()
            .id(SharedString::from(format!("{id}.error")))
            .test_support()
            .aria_label(SharedString::from(label))
            .gap_1()
            .text_xs()
            .text_color(if unreachable {
                theme.danger
            } else {
                theme.warning
            })
            .child(error.clone())
            .children(retry.clone().map(|retry| {
                let retry = SharedString::from(retry);
                div()
                    .id(SharedString::from(format!("{id}.retry")))
                    .test_support()
                    .aria_label(retry.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(retry)
            }))
    });
    let enter = focus
        .is_focused(window)
        .then(|| Keystroke::parse("enter").ok().map(Kbd::new))
        .flatten();
    v_flex()
        .id(SharedString::from(id))
        .test_support()
        .track_focus(focus)
        .aria_label(SharedString::from(parts.join(" · ")))
        .min_w(px(200.))
        .flex_1()
        .gap_1()
        .p_3()
        .border_1()
        .rounded_lg()
        .border_color(if unreachable {
            theme.danger
        } else if active {
            theme.primary
        } else {
            theme.border
        })
        .when(unreachable, |el| el.bg(theme.danger.opacity(0.08)))
        .hover(|s| s.bg(theme.secondary))
        .focus(|s| s.border_color(theme.ring))
        .on_click(move |_, _, cx| on_activate(cx))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .when(active, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(theme.primary)
                            .child(t!("node.active").to_string()),
                    )
                })
                .child(div().flex_1())
                .children(enter),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(host),
        )
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .when(role == Some(RaftState::Leader), |el| {
                            el.font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.success)
                        })
                        .child(role_label),
                )
                .children(latency),
        )
        .children(error)
        .into_any_element()
}
