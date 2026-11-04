// SPDX-License-Identifier: GPL-3.0-or-later

//! The node cards on the overview: name, `host:port`, role, latency and the active marker. An
//! unreachable node's card is red, with the full error and the retry countdown.

use std::time::{Duration, Instant};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, FontWeight, IntoElement, Keystroke,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, prelude::*,
    px,
};
use rust_i18n::t;
use tessera_client::RaftState;

use super::overview::role_text;
use crate::connection::{ConnectionState, host_label};
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
    /// "retry in N s", "retrying…" or "Paused" while the node waits for its retry.
    pub retry: Option<String>,
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
        retry,
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

/// The retry line of a node that waits for its retry at `at`: a countdown, "retrying…" once
/// the request is out, or "Paused" while polling is paused.
fn retry_text(at: Instant, now: Instant, paused: bool) -> String {
    if paused {
        return t!("overview.paused").to_string();
    }
    match at.checked_duration_since(now).filter(|d| !d.is_zero()) {
        Some(left) => t!("node.retry", s = left.as_secs_f64().ceil() as u64).to_string(),
        None => t!("node.retrying").to_string(),
    }
}

/// The row of node cards, with the age of the figures below while the active node is down.
/// It is its own view so the once-a-second countdown redraws only what changes; the charts
/// beside it are cached.
pub struct NodeCards {
    connection: Entity<ConnectionState>,
    /// One per card, so Tab and Enter reach them.
    focus: Vec<FocusHandle>,
    visible: bool,
    /// A task redraws the countdowns once a second.
    counting_down: bool,
    _subscriptions: [Subscription; 2],
}

impl NodeCards {
    /// The cards of `connection`'s nodes.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _subscriptions: [
                cx.observe(&connection, |this, _, cx| {
                    this.count_down(cx);
                    cx.notify();
                }),
                cx.observe_window_visibility(window, |this, visibility, _, cx| {
                    this.visible = visibility.is_visible();
                    this.count_down(cx);
                    cx.notify();
                }),
            ],
            visible: window.is_visible(),
            connection,
            focus: Vec::new(),
            counting_down: false,
        }
    }

    /// Whether a countdown is on screen and moving: a node waits for its retry, polling
    /// runs and the window is visible.
    fn counting(&self, cx: &App) -> bool {
        let conn = self.connection.read(cx);
        self.visible && !conn.paused && conn.nodes.iter().any(|n| n.next_retry.is_some())
    }

    /// Redraws once a second while [`Self::counting`].
    fn count_down(&mut self, cx: &mut Context<Self>) {
        if self.counting_down || !self.counting(cx) {
            return;
        }
        self.counting_down = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let more = this.update(cx, |this, cx| {
                    let more = this.counting(cx);
                    this.counting_down = more;
                    cx.notify();
                    more
                });
                if !matches!(more, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }
}

impl Render for NodeCards {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.connection.read(cx).nodes.len();
        self.focus
            .resize_with(count, || cx.focus_handle().tab_stop(true));
        let conn = self.connection.read(cx);
        let Some(connected) = &conn.connected else {
            return div().into_any_element();
        };
        let now = cx.background_executor().now();
        let active = connected.client.active_node();
        let visible = self.visible;
        let cards = connected
            .client
            .nodes()
            .iter()
            .zip(&conn.nodes)
            .zip(&self.focus)
            .enumerate()
            .map(|(index, ((url, status), focus))| {
                let connection = self.connection.clone();
                let retry = status
                    .next_retry
                    .filter(|_| visible)
                    .map(|at| retry_text(at, now, conn.paused));
                render(
                    Card {
                        index,
                        host: host_label(url.as_str()),
                        status,
                        active: index == active,
                        retry,
                    },
                    focus,
                    move |cx| connection.update(cx, |c, cx| c.set_active_node(index, cx)),
                    window,
                    cx,
                )
            });
        // The figures and charts below keep the down node's last sample; say how old it is.
        let stale = conn
            .nodes
            .get(active)
            .filter(|n| n.next_retry.is_some())
            .and_then(|_| conn.history.get(active)?.newest_at())
            .map(|at| {
                let text = SharedString::from(
                    t!(
                        "overview.last_sample",
                        s = now.saturating_duration_since(at).as_secs()
                    )
                    .to_string(),
                );
                div()
                    .id("overview.last_sample")
                    .test_support()
                    .aria_label(text.clone())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(text)
            });
        v_flex()
            .gap_3()
            .child(h_flex().flex_wrap().gap_3().children(cards))
            .children(stale)
            .into_any_element()
    }
}
