// SPDX-License-Identifier: GPL-3.0-or-later

//! The node rows of the connection dialog: protocol, host, port and path prefix.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, prelude::*, px,
};
use rust_i18n::t;
use tessera_core::profile::{Node, Protocol};

use super::connect_dialog::{ConnectDialog, error_line};

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

    /// The trimmed host text.
    pub fn host(&self, cx: &App) -> String {
        self.host.read(cx).value().trim().to_string()
    }

    /// The node, or why it is invalid.
    pub fn read(&self, cx: &App) -> Result<Node, SharedString> {
        let port = self.port.read(cx).value();
        let path = self.path.read(cx).value();
        parse_node(self.protocol, &self.host(cx), port.trim(), path.trim())
    }

    /// Whether this row sends requests unencrypted to another machine.
    pub fn is_remote_http(&self, cx: &App) -> bool {
        let host = self.host(cx);
        let node = Node {
            protocol: self.protocol,
            host,
            port: 0,
            path: String::new(),
        };
        node.protocol == Protocol::Http && !node.host.is_empty() && !node.is_local()
    }
}

/// Builds a node from the field texts, with the dialog's error text on failure.
pub fn parse_node(
    protocol: Protocol,
    host: &str,
    port: &str,
    path: &str,
) -> Result<Node, SharedString> {
    if host.is_empty() {
        return Err(t!("connect.error.host_required").into());
    }
    let port = match port.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(t!("connect.error.port_range").into()),
    };
    let node = Node {
        protocol,
        host: host.to_string(),
        port,
        path: path.to_string(),
    };
    // Rejects a scheme, a path or whitespace in the host (Review Focus 2). The port is
    // checked above and every path is accepted, so a failure is always the host.
    node.validate()
        .map_err(|_| SharedString::from(t!("connect.error.host_format").to_string()))?;
    Ok(node)
}

/// The rows with their errors and remove buttons, and "Add node for a cluster".
pub fn render(
    rows: &[NodeRow],
    errors: &HashMap<String, SharedString>,
    cx: &mut Context<ConnectDialog>,
) -> impl IntoElement {
    let removable = rows.len() > 1;
    let host_label = SharedString::from(t!("connect.node.host").to_string());
    let port_label = SharedString::from(t!("connect.node.port").to_string());
    let path_label = SharedString::from(t!("connect.node.path").to_string());
    v_flex()
        .gap_2()
        .children(rows.iter().enumerate().map(|(i, row)| {
            let id = |part: &str| SharedString::from(format!("connect.node.{i}.{part}"));
            let protocol = match row.protocol {
                Protocol::Http => t!("connect.protocol.http"),
                Protocol::Https => t!("connect.protocol.https"),
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
                                    this.flip_protocol(i, cx);
                                })),
                        )
                        .child(
                            div_flex(
                                Input::new(&row.host)
                                    .id(id("host"))
                                    .aria_label(host_label.clone()),
                            )
                            .flex_1(),
                        )
                        .child(
                            div_flex(
                                Input::new(&row.port)
                                    .id(id("port"))
                                    .aria_label(port_label.clone()),
                            )
                            .w(px(80.)),
                        )
                        .child(
                            div_flex(
                                Input::new(&row.path)
                                    .id(id("path"))
                                    .aria_label(path_label.clone()),
                            )
                            .w(px(110.)),
                        )
                        .when(removable, |el| {
                            el.child(
                                Button::new(id("remove"))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .accessibility_label(t!("connect.node.remove").to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_node(i, cx);
                                    })),
                            )
                        }),
                )
                .when_some(
                    errors.get(&format!("connect.node.{i}.error")),
                    |el, error| el.child(error_line(id("error"), error.clone(), cx)),
                )
        }))
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
}

/// Wraps an input so the row can size it.
fn div_flex(input: Input) -> gpui_kit::Div {
    gpui_kit::div().min_w_0().child(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_with_scheme_or_path_is_rejected() {
        rust_i18n::set_locale("en");
        for host in ["https://ts.example.com/", "ts.example.com/x", "a b"] {
            let err = parse_node(Protocol::Https, host, "443", "").expect_err(host);
            assert_eq!(err, "Enter the host without protocol or path.");
        }
        let node = parse_node(Protocol::Http, "::1", "8108", "/ts").expect("IPv6 host");
        assert_eq!(node.label(), "::1:8108");
    }

    #[test]
    fn port_must_be_in_range() {
        rust_i18n::set_locale("en");
        for port in ["0", "65536", "", "x"] {
            assert!(
                parse_node(Protocol::Http, "localhost", port, "").is_err(),
                "{port:?}"
            );
        }
        assert!(parse_node(Protocol::Http, "localhost", "65535", "").is_ok());
    }
}
