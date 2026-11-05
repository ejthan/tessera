// SPDX-License-Identifier: GPL-3.0-or-later

//! Overview of a connection (spec §7.7): polling header, node cards, key figures, charts
//! and API stats.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{Metrics, RaftState};
use tessera_core::metrics::{MetricsHistory, Series};

use super::metric_chart::{self, Charts};
use super::node_card::NodeCards;
use super::request_detail::size;
use super::stats_table::{self, Empty, Sort};
use crate::actions::{OVERVIEW, Refresh, TogglePolling};
use crate::connection::ConnectionState;

/// Intervals the header offers, in seconds.
const INTERVALS: [u64; 4] = [2, 5, 10, 30];

/// Display text for a Raft role.
pub fn role_text(state: RaftState) -> String {
    match state {
        RaftState::Leader => t!("role.leader").to_string(),
        RaftState::Follower => t!("role.follower").to_string(),
        RaftState::Other(n) => t!("role.other", state = n).to_string(),
        RaftState::Unknown => t!("role.unknown").to_string(),
    }
}

/// The overview of one connection, inside its tab.
pub struct Overview {
    connection: Entity<ConnectionState>,
    cards: Entity<NodeCards>,
    charts: Entity<Charts>,
    /// The stats headers: endpoint, rps, latency.
    headers: [FocusHandle; 3],
    sort: Sort,
    _subscription: Subscription,
}

impl Overview {
    /// The overview of `connection`, redrawn whenever it changes.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tab_stop = |cx: &mut Context<Self>| cx.focus_handle().tab_stop(true);
        Self {
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
            cards: cx.new(|cx| NodeCards::new(connection.clone(), window, cx)),
            charts: cx.new(|cx| Charts::new(connection.clone(), cx)),
            connection,
            headers: [tab_stop(cx), tab_stop(cx), tab_stop(cx)],
            sort: Sort::default(),
        }
    }

    fn header(&self, conn: &ConnectionState, window: &Window) -> AnyElement {
        let secs = conn.interval.as_secs();
        let live = SharedString::from(if conn.paused {
            t!("overview.paused").to_string()
        } else {
            t!("overview.live", s = secs).to_string()
        });
        let pause_label = if conn.paused {
            t!("overview.resume")
        } else {
            t!("overview.pause")
        };
        let pause = {
            let connection = self.connection.clone();
            Button::new("overview.pause")
                .outline()
                .small()
                .label(pause_label.to_string())
                .children(Kbd::binding_for_action(
                    &TogglePolling,
                    Some(OVERVIEW),
                    window,
                ))
                .on_click(move |_, _, cx| {
                    connection.update(cx, |c, cx| c.set_paused(!c.paused, cx));
                })
        };
        let interval = {
            let connection = self.connection.clone();
            Button::new("overview.interval")
                .outline()
                .small()
                .dropdown_caret(true)
                .label(t!("overview.interval", s = secs).to_string())
                .dropdown_menu(move |menu, _, _| {
                    INTERVALS.into_iter().fold(menu, |menu, choice| {
                        let connection = connection.clone();
                        menu.item(
                            PopupMenuItem::new(t!("overview.interval", s = choice).to_string())
                                .checked(choice == secs)
                                .on_click(move |_, _, cx| {
                                    connection.update(cx, |c, cx| {
                                        c.set_interval(Duration::from_secs(choice), cx)
                                    });
                                }),
                        )
                    })
                })
        };
        let refresh = {
            let connection = self.connection.clone();
            Button::new("overview.refresh")
                .ghost()
                .small()
                .label(t!("overview.refresh").to_string())
                .children(Kbd::binding_for_action(&Refresh, Some(OVERVIEW), window))
                .on_click(move |_, _, cx| connection.update(cx, |c, cx| c.refresh_collections(cx)))
        };
        h_flex()
            .gap_3()
            .items_center()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("overview.title").to_string()),
            )
            .child(
                div()
                    .id("overview.live")
                    .test_support()
                    .aria_label(live.clone())
                    .text_sm()
                    .child(live),
            )
            .child(div().flex_1())
            .child(pause)
            .child(interval)
            .child(refresh)
            .into_any_element()
    }
}

/// `n` with the locale's digit grouping: "1,234,567" in English, "1.234.567" in German.
pub(super) fn grouped(n: u64) -> String {
    group_digits(n, &t!("number.group_separator"))
}

fn group_digits(n: u64, separator: &str) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(separator);
        }
        out.push(digit);
    }
    out
}

/// "–" for a figure the server did not send.
fn missing() -> String {
    t!("figure.missing").to_string()
}

/// "1.0 GB of 4.0 GB", or "–" unless both are known.
fn used_of(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(used), Some(total)) => {
            t!("figure.used_of", used = size(used), total = size(total)).to_string()
        }
        _ => missing(),
    }
}

/// The newest network rates, or "–" when the newest sample has no network counters.
fn network(last: Option<&Metrics>, history: Option<&MetricsHistory>) -> String {
    let newest = |series| {
        history?
            .points(series)
            .last()
            .filter(|(age, _)| age.is_zero())
            .map(|(_, v)| v)
    };
    match (last, newest(Series::NetRx), newest(Series::NetTx)) {
        (Some(m), Some(rx), Some(tx)) if m.net_received.is_some() && m.net_sent.is_some() => {
            metric_chart::network_text(rx, tx)
        }
        _ => missing(),
    }
}

impl Render for Overview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        let Some(connected) = &conn.connected else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let info = &connected.info;
        let active = connected.client.active_node();
        let history = conn.history.get(active);
        // The active node is down: the figures are its last sample.
        let stale = conn
            .nodes
            .get(active)
            .is_some_and(|n| n.next_retry.is_some());

        let last = history.and_then(MetricsHistory::last);
        let version = conn
            .nodes
            .get(active)
            .and_then(|n| n.debug.as_ref())
            .map_or_else(|| info.version.clone(), |d| d.version.clone());
        let collections = conn.collections.as_ref();
        let figures = [
            ("figure.version", t!("figure.version"), version),
            (
                "figure.collections",
                t!("figure.collections"),
                collections.map_or_else(missing, |c| grouped(c.len() as u64)),
            ),
            (
                "figure.documents",
                t!("figure.documents"),
                collections.map_or_else(missing, |c| {
                    grouped(c.iter().map(|c| c.num_documents).sum::<u64>())
                }),
            ),
            (
                "figure.ts_memory",
                t!("figure.ts_memory"),
                last.and_then(|m| m.typesense_memory_active)
                    .map_or_else(missing, size),
            ),
            (
                "figure.cpu",
                t!("figure.cpu"),
                last.and_then(|m| m.cpu_percent)
                    .map_or_else(missing, metric_chart::percent_text),
            ),
            (
                "figure.memory",
                t!("figure.memory"),
                used_of(
                    last.and_then(|m| m.memory_used),
                    last.and_then(|m| m.memory_total),
                ),
            ),
            (
                "figure.disk",
                t!("figure.disk"),
                used_of(
                    last.and_then(|m| m.disk_used),
                    last.and_then(|m| m.disk_total),
                ),
            ),
            (
                "figure.network",
                t!("figure.network"),
                network(last, history),
            ),
        ]
        .map(|(id, label, value)| {
            let value = SharedString::from(value);
            v_flex()
                .id(id)
                .test_support()
                .aria_label(value.clone())
                .min_w(px(150.))
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
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(value),
                )
        });
        let collections_error = conn.collections_error.clone().map(|error| {
            div()
                .id("overview.collections_error")
                .test_support()
                .aria_label(error.clone())
                .text_sm()
                .text_color(theme.danger)
                .child(error)
        });

        let stats = conn.stats.as_ref().ok_or_else(|| {
            match conn.nodes.get(active).and_then(|n| n.error.as_deref()) {
                Some(error) => Empty::Error(error),
                None => Empty::Waiting,
            }
        });
        let on_sort: stats_table::OnSort = {
            let this = cx.entity().downgrade();
            Rc::new(move |column, cx| {
                this.update(cx, |view, cx| {
                    view.sort = view.sort.toggle(column);
                    cx.notify();
                })
                .ok();
            })
        };
        v_flex()
            .id("overview")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .gap_4()
            .p_5()
            .child(self.header(conn, window))
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
            .child(self.cards.clone())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .when(stale, |el| el.opacity(0.5))
                    .children(figures),
            )
            .children(collections_error)
            .child(Charts::cached_element(self.charts.clone()))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("stats.title").to_string()),
                    )
                    .child(stats_table::render(
                        stats,
                        self.sort,
                        &self.headers,
                        on_sort,
                        window,
                        cx,
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_use_the_locale_grouping() {
        let en = t!("number.group_separator", locale = "en");
        let de = t!("number.group_separator", locale = "de");
        assert_eq!(group_digits(1_234_567, &en), "1,234,567");
        assert_eq!(group_digits(1_234_567, &de), "1.234.567");
        assert_eq!(group_digits(123_456, &en), "123,456");
        assert_eq!(group_digits(999, &en), "999");
        assert_eq!(group_digits(1000, &de), "1.000");
        assert_eq!(group_digits(0, &en), "0");
    }
}
