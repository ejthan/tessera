// SPDX-License-Identifier: GPL-3.0-or-later

//! Node rows: protocol, host, port and path prefix. Used for the cluster nodes and the
//! nearest node.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_core::profile::{Node, Protocol};

use super::form::{FormErrors, NodeValues};
use super::{ConnectDialog, error_line};

/// Which row a control belongs to.
#[derive(Clone, Copy)]
pub enum RowRef {
    Node(usize),
    Nearest,
}

/// The inputs of one node.
pub struct NodeRow {
    pub protocol: Protocol,
    pub host: Entity<InputState>,
    pub port: Entity<InputState>,
    pub path: Entity<InputState>,
}

impl NodeRow {
    /// A row showing `node`.
    pub fn new(node: &Node, window: &mut Window, cx: &mut Context<ConnectDialog>) -> Self {
        let mut input =
            |value: String| cx.new(|cx| InputState::new(window, cx).default_value(value));
        Self {
            protocol: node.protocol,
            host: input(node.host.clone()),
            port: input(node.port.to_string()),
            path: input(node.path.clone()),
        }
    }

    /// The three inputs, to re-render the dialog when one changes.
    pub fn inputs(&self) -> [Entity<InputState>; 3] {
        [self.host.clone(), self.port.clone(), self.path.clone()]
    }

    /// The texts as typed.
    pub fn values(&self, cx: &App) -> NodeValues {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        NodeValues {
            protocol: self.protocol,
            host: text(&self.host),
            port: text(&self.port),
            path: text(&self.path),
        }
    }

    /// Whether this row sends requests unencrypted to another machine.
    pub fn is_remote_http(&self, cx: &App) -> bool {
        let node = Node {
            protocol: self.protocol,
            host: self.host.read(cx).value().trim().to_string(),
            port: 0,
            path: String::new(),
        };
        node.protocol == Protocol::Http && !node.host.is_empty() && !node.is_local()
    }
}

/// The cluster rows with their errors and remove buttons, and "Add node for a cluster".
pub fn render(
    rows: &[NodeRow],
    errors: &FormErrors,
    cx: &mut Context<ConnectDialog>,
) -> AnyElement {
    let removable = rows.len() > 1;
    let rendered: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let prefix = format!("connect.node.{i}");
            let error = errors.get(&format!("{prefix}.error"));
            render_row(row, &prefix, error, RowRef::Node(i), removable, cx)
        })
        .collect();
    v_flex()
        .gap_2()
        .children(rendered)
        .child(
            h_flex().child(
                Button::new("connect.add_node")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(t!("connect.add_node").to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.add_node(window, cx))),
            ),
        )
        .into_any_element()
}

/// One row; its controls get the test ids `{prefix}.protocol|host|port|path|remove|error`.
pub fn render_row(
    row: &NodeRow,
    prefix: &str,
    error: Option<&SharedString>,
    which: RowRef,
    removable: bool,
    cx: &mut Context<ConnectDialog>,
) -> AnyElement {
    let id = |part: &str| SharedString::from(format!("{prefix}.{part}"));
    let protocol = match row.protocol {
        Protocol::Http => t!("connect.protocol.http"),
        Protocol::Https => t!("connect.protocol.https"),
    };
    let input = |state: &Entity<InputState>, part: &str, label: String| {
        div()
            .min_w_0()
            .child(Input::new(state).id(id(part)).aria_label(label))
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(id("protocol"))
                        .outline()
                        .w(px(64.))
                        .label(protocol.to_string())
                        .accessibility_label(t!("connect.node.protocol").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.flip_protocol(which, cx);
                        })),
                )
                .child(input(&row.host, "host", t!("connect.node.host").to_string()).flex_1())
                .child(input(&row.port, "port", t!("connect.node.port").to_string()).w(px(80.)))
                .child(input(&row.path, "path", t!("connect.node.path").to_string()).w(px(110.)))
                .when(removable, |el| {
                    el.child(
                        Button::new(id("remove"))
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .accessibility_label(t!("connect.node.remove").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let RowRef::Node(i) = which {
                                    this.remove_node(i, cx);
                                }
                            })),
                    )
                }),
        )
        .when_some(error, |el, error| {
            el.child(error_line(id("error"), error.clone(), cx))
        })
        .into_any_element()
}
