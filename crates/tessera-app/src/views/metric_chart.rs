// SPDX-License-Identifier: GPL-3.0-or-later

//! The overview's metric charts: CPU, memory and disk in percent, network rx · tx.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::chart::{AreaChart, LineChart};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*,
    px, transparent_black,
};
use rust_i18n::t;
use tessera_core::metrics::{MetricsHistory, Series};

use super::request_detail::size;

/// One point: x label (age) and value.
pub type Point = (SharedString, f64);
/// One network point: x label, received and sent bytes per second.
pub type NetPoint = (SharedString, f64, f64);

/// The points of one node's four charts.
#[derive(Default)]
pub struct ChartData {
    cpu: Vec<Point>,
    memory: Vec<Point>,
    disk: Vec<Point>,
    network: Vec<NetPoint>,
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
pub fn render(data: &ChartData, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let percent = |id: &'static str, title: String, points: &[Point], color| {
        let latest = points.last().map(|(_, v)| percent_text(*v));
        let chart = LineChart::new(points.iter().cloned())
            .id(SharedString::from(format!("{id}.plot")))
            .x(|(x, _)| x.clone())
            .y(|(_, y)| *y)
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
    let network = AreaChart::new(data.network.iter().cloned())
        .id("chart.network.plot")
        .x(|(x, _, _)| x.clone())
        .y(|(_, rx, _)| *rx)
        .stroke(theme.chart_1)
        .fill(transparent_black())
        .name(t!("chart.rx").to_string())
        .y(|(_, _, tx)| *tx)
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
        .min_w(px(240.))
        .flex_1()
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
