// SPDX-License-Identifier: GPL-3.0-or-later

//! The overview's metric charts: CPU, memory and disk in percent, network rx · tx.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::chart::{AreaChart, LineChart};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Window, div, prelude::*, px, transparent_black,
};
use rust_i18n::t;
use tessera_core::metrics::{MetricsHistory, Series};

use super::request_detail::size;
use crate::connection::ConnectionState;

/// Height of one chart box.
const FRAME_HEIGHT: f32 = 170.;
/// Space between the chart boxes.
const GAP: f32 = 12.;

thread_local! {
    static BUILDS: Cell<usize> = const { Cell::new(0) };
}

/// How often chart points were built on this thread. Tests use it to check that charts
/// change only with new samples (spec §14).
#[doc(hidden)]
pub fn builds() -> usize {
    BUILDS.with(Cell::get)
}

/// What the charts show: active node, its history revision, and whether it is down.
type Key = (usize, u64, bool);

/// The four charts of the active node, as their own view. Its points are rebuilt, and it is
/// notified, only when the active node, its history or its reachability changes. It is
/// embedded with [`Charts::cached_element`], but GPUI 0.3.11 re-renders a cached view whose
/// cached ancestor re-renders (`view.rs` `prepaint_view` sets `refreshing`), and the dock
/// caches the whole tab, so a redraw of the tab (countdown, hover) still renders the charts.
/// That render only shares the built points (`Rc`), and the plots reuse their tessellated
/// paths while the points are unchanged.
pub struct Charts {
    connection: Entity<ConnectionState>,
    key: Option<Key>,
    data: ChartData,
    _subscription: Subscription,
}

impl Charts {
    /// The charts of `connection`'s active node.
    pub fn new(connection: Entity<ConnectionState>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            _subscription: cx.observe(&connection, |this, _, cx| {
                if this.update_data(cx) {
                    cx.notify();
                }
            }),
            connection,
            key: None,
            data: ChartData::default(),
        };
        this.update_data(cx);
        this
    }

    /// Rebuilds the points when the key changed; returns whether it did.
    fn update_data(&mut self, cx: &App) -> bool {
        let conn = self.connection.read(cx);
        let Some(connected) = &conn.connected else {
            return false;
        };
        let active = connected.client.active_node();
        let history = conn.history.get(active);
        let down = conn
            .nodes
            .get(active)
            .is_some_and(|n| n.next_retry.is_some());
        let key = (active, history.map_or(0, MetricsHistory::revision), down);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        BUILDS.with(|b| b.set(b.get() + 1));
        self.data = history.map(ChartData::new).unwrap_or_default();
        true
    }

    /// `charts` as a cached element of a fixed size: two rows of two boxes.
    pub fn cached_element(charts: Entity<Self>) -> impl IntoElement {
        charts.cached(
            StyleRefinement::default()
                .w_full()
                .h(px(2. * FRAME_HEIGHT + GAP)),
        )
    }
}

impl Render for Charts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let down = self.key.is_some_and(|(_, _, down)| down);
        let mut frames = frames(&self.data, cx).into_iter();
        let mut row = || {
            h_flex()
                .gap(px(GAP))
                .children(frames.next())
                .children(frames.next())
        };
        v_flex()
            .size_full()
            .gap(px(GAP))
            // The active node is down: these are its last samples.
            .when(down, |el| el.opacity(0.5))
            .child(row())
            .child(row())
    }
}

/// One point: x label (age) and value.
pub type Point = (SharedString, f64);
/// One network point: x label, received and sent bytes per second.
pub type NetPoint = (SharedString, f64, f64);

/// The points of one node's four charts.
#[derive(Default)]
pub struct ChartData {
    cpu: Rc<[Point]>,
    memory: Rc<[Point]>,
    disk: Rc<[Point]>,
    network: Rc<[NetPoint]>,
}

impl ChartData {
    /// The points of `history`, oldest first.
    pub fn new(history: &MetricsHistory) -> Self {
        let percent = |series| {
            history
                .points(series)
                .map(|(age, v)| (age_label(age), v))
                .collect()
        };
        let tx: HashMap<Duration, f64> = history.points(Series::NetTx).collect();
        let network = history
            .points(Series::NetRx)
            .filter_map(|(age, rx)| Some((age_label(age), rx, *tx.get(&age)?)))
            .collect();
        Self {
            cpu: percent(Series::Cpu),
            memory: percent(Series::Memory),
            disk: percent(Series::Disk),
            network,
        }
    }
}

/// `-2:05` for a sample taken 125 s before the newest.
fn age_label(age: Duration) -> SharedString {
    let secs = age.as_secs();
    format!("-{}:{:02}", secs / 60, secs % 60).into()
}

/// `12.5 %`.
pub fn percent_text(value: f64) -> String {
    t!("units.percent", n = format!("{value:.1}")).to_string()
}

/// `1.2 KB/s`.
pub fn rate_text(bytes_per_sec: f64) -> String {
    t!(
        "units.per_second",
        size = size(bytes_per_sec.round() as u64)
    )
    .to_string()
}

/// `rx 1.2 KB/s · tx 300 B/s`.
pub fn network_text(rx: f64, tx: f64) -> String {
    t!(
        "figure.network_value",
        rx = rate_text(rx),
        tx = rate_text(tx)
    )
    .to_string()
}

/// The four charts, labeled with their title and newest value for assistive technology.
fn frames(data: &ChartData, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    // Each render hands the plots indexes into the shared points, not copies of them.
    let percent = |id: &'static str, title: String, points: &Rc<[Point]>, color| {
        let latest = points.last().map(|(_, v)| percent_text(*v));
        let (xs, ys) = (points.clone(), points.clone());
        let chart = LineChart::new(0..points.len())
            .id(SharedString::from(format!("{id}.plot")))
            .x(move |i| xs[*i].0.clone())
            .y(move |i| ys[*i].1)
            .y_domain(0., 100.)
            .stroke(color)
            .linear()
            .appear(false)
            .x_tick_count(4)
            .name(title.clone())
            .tooltip_value(|_, v| percent_text(v).into());
        frame(id, title, latest, chart.into_any_element(), cx)
    };
    let latest = data
        .network
        .last()
        .map(|(_, rx, tx)| network_text(*rx, *tx));
    // LineChart draws one series; AreaChart draws several on shared axes, here without fill.
    let (xs, rx, tx) = (
        data.network.clone(),
        data.network.clone(),
        data.network.clone(),
    );
    let network = AreaChart::new(0..data.network.len())
        .id("chart.network.plot")
        .x(move |i| xs[*i].0.clone())
        .y(move |i| rx[*i].1)
        .stroke(theme.chart_1)
        .fill(transparent_black())
        .name(t!("chart.rx").to_string())
        .y(move |i| tx[*i].2)
        .stroke(theme.chart_3)
        .fill(transparent_black())
        .name(t!("chart.tx").to_string())
        .linear()
        .appear(false)
        .x_tick_count(4)
        .tooltip_value(|_, _, v| rate_text(v).into());
    vec![
        percent(
            "chart.cpu",
            t!("figure.cpu").to_string(),
            &data.cpu,
            theme.chart_1,
        ),
        percent(
            "chart.memory",
            t!("figure.memory").to_string(),
            &data.memory,
            theme.chart_2,
        ),
        percent(
            "chart.disk",
            t!("figure.disk").to_string(),
            &data.disk,
            theme.chart_3,
        ),
        frame(
            "chart.network",
            t!("figure.network").to_string(),
            latest,
            network.into_any_element(),
            cx,
        ),
    ]
}

/// A titled box around `chart`; "No data yet" while there are no points.
fn frame(
    id: &'static str,
    title: String,
    latest: Option<String>,
    chart: AnyElement,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let label = match &latest {
        Some(value) => format!("{title} · {value}"),
        None => format!("{title} · {}", t!("chart.no_data")),
    };
    v_flex()
        .flex_1()
        .h(px(FRAME_HEIGHT))
        .gap_1()
        .p_3()
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
        .child(
            div()
                .id(id)
                .test_support()
                .aria_label(SharedString::from(label.clone()))
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(match latest {
            Some(_) => div().h(px(120.)).child(chart),
            None => div().h(px(120.)),
        })
        .into_any_element()
}
