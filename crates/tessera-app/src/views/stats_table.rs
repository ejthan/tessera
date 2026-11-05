// SPDX-License-Identifier: GPL-3.0-or-later

//! The overview's API stats: endpoint, requests per second and average latency from the
//! active node's `/stats.json`.

use std::cmp::Ordering;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, FocusHandle, FontWeight, IntoElement, Keystroke, ParentElement as _,
    SharedString, Styled as _, Window, div, prelude::*, px, transparent_black,
};
use rust_i18n::t;
use tessera_client::Stats;

/// A sortable column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The endpoint, like `GET /collections`.
    Endpoint,
    /// Requests per second.
    Rps,
    /// Average latency.
    Latency,
}

/// The table's order. Starts at requests per second, descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    /// The column sorted by.
    pub column: Column,
    /// Largest (or last) first.
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            column: Column::Rps,
            descending: true,
        }
    }
}

impl Sort {
    /// The order after activating `column`'s header: the same column flips, another one
    /// starts with the endpoint A to Z and the figures largest first.
    pub fn toggle(self, column: Column) -> Self {
        if column == self.column {
            Self {
                descending: !self.descending,
                ..self
            }
        } else {
            Self {
                column,
                descending: column != Column::Endpoint,
            }
        }
    }
}

/// One row of the table.
struct Row {
    endpoint: String,
    rps: Option<f64>,
    latency: Option<f64>,
}

/// The rows of `stats` in `sort` order. An endpoint missing from one map shows "–" there.
fn rows(stats: &Stats, sort: Sort) -> Vec<Row> {
    let mut endpoints: Vec<&String> = stats
        .latency_ms
        .keys()
        .chain(stats.requests_per_second.keys())
        .collect();
    endpoints.sort();
    endpoints.dedup();
    let mut rows: Vec<Row> = endpoints
        .into_iter()
        .map(|e| Row {
            endpoint: e.clone(),
            rps: stats.requests_per_second.get(e).copied(),
            latency: stats.latency_ms.get(e).copied(),
        })
        .collect();
    let figure = |a: Option<f64>, b: Option<f64>| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    rows.sort_by(|a, b| {
        let order = match sort.column {
            Column::Endpoint => a.endpoint.cmp(&b.endpoint),
            Column::Rps => figure(a.rps, b.rps),
            Column::Latency => figure(a.latency, b.latency),
        };
        if sort.descending {
            order.reverse()
        } else {
            order
        }
    });
    rows
}

/// Called with the column whose header was activated.
pub type OnSort = Rc<dyn Fn(Column, &mut App)>;

/// What the stats area shows instead of the table, if anything.
pub enum Empty<'a> {
    /// The active node's stats failed or it is down: its full error (UX rule 2).
    Error(&'a str),
    /// No answer from the active node yet.
    Waiting,
}

/// Width of the two figure columns.
const FIGURE_WIDTH: f32 = 140.;

/// The table, or the reason there is none. `headers` are the focus handles of the endpoint,
/// rps and latency headers; Enter or a click on one calls `on_sort`.
pub fn render(
    stats: Result<&Stats, Empty>,
    sort: Sort,
    headers: &[FocusHandle; 3],
    on_sort: OnSort,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let stats = match stats {
        Ok(stats) if !stats.latency_ms.is_empty() => stats,
        other => {
            let (id, text, color) = match other {
                Err(Empty::Error(error)) => ("stats.error", error.to_string(), theme.danger),
                Err(Empty::Waiting) => (
                    "stats.waiting",
                    t!("stats.waiting").to_string(),
                    theme.muted_foreground,
                ),
                Ok(_) => (
                    "stats.empty",
                    t!("stats.empty").to_string(),
                    theme.muted_foreground,
                ),
            };
            let text = SharedString::from(text);
            return div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element();
        }
    };
    let columns = [
        (
            Column::Endpoint,
            "stats.sort.endpoint",
            t!("stats.endpoint"),
        ),
        (Column::Rps, "stats.sort.rps", t!("stats.rps")),
        (Column::Latency, "stats.sort.latency", t!("stats.latency")),
    ];
    let header = h_flex()
        .gap_2()
        .children(
            columns
                .into_iter()
                .zip(headers)
                .map(|((column, id, title), focus)| {
                    let title = SharedString::from(title.to_string());
                    let on_sort = on_sort.clone();
                    let arrow = (sort.column == column).then(|| {
                        Icon::new(if sort.descending {
                            IconName::ArrowDown
                        } else {
                            IconName::ArrowUp
                        })
                        .xsmall()
                    });
                    let enter = focus
                        .is_focused(window)
                        .then(|| Keystroke::parse("enter").ok().map(Kbd::new))
                        .flatten();
                    h_flex()
                        .id(id)
                        .test_support()
                        .track_focus(focus)
                        .aria_label(title.clone())
                        .when(column == Column::Endpoint, |el| el.flex_1())
                        .when(column != Column::Endpoint, |el| {
                            el.w(px(FIGURE_WIDTH)).justify_end()
                        })
                        .gap_1()
                        .px_1()
                        .items_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(transparent_black())
                        .font_weight(FontWeight::SEMIBOLD)
                        .hover(|s| s.bg(theme.secondary))
                        .focus(|s| s.border_color(theme.ring))
                        .on_click(move |_, _, cx| on_sort(column, cx))
                        .child(title)
                        .children(arrow)
                        .children(enter)
                }),
        );
    let dash = || t!("figure.missing").to_string();
    let body = rows(stats, sort).into_iter().map(|row| {
        let rps = row
            .rps
            .map(|v| t!("stats.rps_value", n = format!("{v:.1}")).to_string())
            .unwrap_or_else(dash);
        let latency = row
            .latency
            .map(|v| t!("units.ms", n = format!("{v:.1}")).to_string())
            .unwrap_or_else(dash);
        let label = SharedString::from(format!("{} · {rps} · {latency}", row.endpoint));
        h_flex()
            .id(SharedString::from(format!("stats.row.{}", row.endpoint)))
            .test_support()
            .aria_label(label)
            .gap_2()
            .px_1()
            .py_0p5()
            .border_t_1()
            .border_color(theme.border)
            .child(div().flex_1().min_w_0().truncate().child(row.endpoint))
            .child(div().w(px(FIGURE_WIDTH)).text_right().child(rps))
            .child(div().w(px(FIGURE_WIDTH)).text_right().child(latency))
    });
    v_flex()
        .text_sm()
        .child(header)
        .children(body)
        .into_any_element()
}
