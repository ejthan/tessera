// SPDX-License-Identifier: GPL-3.0-or-later

//! The Advanced section: timeout, retries, nearest node and TLS.

use std::path::PathBuf;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::form::Field;
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, PathPromptOptions, SharedString,
    Styled as _, Window, div, prelude::*, px,
};
use rust_i18n::t;

use super::ConnectDialog;
use super::node_rows::{self, RowRef};

/// The TLS mode as the summary names it.
pub fn tls_mode(ca_file: bool, allow_insecure: bool) -> String {
    match (allow_insecure, ca_file) {
        (true, _) => t!("connect.tls.insecure"),
        (false, true) => t!("connect.tls.ca_file"),
        (false, false) => t!("connect.tls.system"),
    }
    .to_string()
}

impl ConnectDialog {
    /// Puts a CA file into the form; `None` removes it.
    pub(super) fn set_ca_file(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.ca_file = path;
        self.errors.remove("connect.ca_file.error");
        cx.notify();
    }

    fn choose_ca_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("connect.ca_file.prompt").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = picked.await;
            this.update(cx, |view, cx| match picked {
                Ok(Ok(Some(paths))) => view.set_ca_file(paths.into_iter().next(), cx),
                Ok(Err(e)) => {
                    let text = t!("connect.error.picker", detail = e.to_string());
                    view.errors
                        .insert("connect.ca_file.error".into(), text.to_string().into());
                    cx.notify();
                }
                // Cancelled.
                Ok(Ok(None)) | Err(_) => {}
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_advanced(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let field = |label: String| Field::new().label(label);
        let text = |id: &str| SharedString::from(t!(id).to_string());
        let summary = t!(
            "connect.advanced_summary",
            timeout = self.timeout.read(cx).value().trim(),
            retries = self.retries.read(cx).value().trim(),
            tls = tls_mode(self.ca_file.is_some(), self.allow_insecure)
        )
        .to_string();
        let ca_label = match &self.ca_file {
            Some(path) => path.display().to_string(),
            None => t!("connect.ca_file.choose").to_string(),
        };
        let number = |input, id: &'static str, label: &str| {
            field(label.to_string())
                .w(px(160.))
                .child(Input::new(input).id(id).aria_label(label.to_string()))
                .children(self.error(&format!("{id}.error"), cx))
        };
        let content =
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(number(
                            &self.timeout,
                            "connect.timeout",
                            &t!("connect.timeout"),
                        ))
                        .child(number(
                            &self.retries,
                            "connect.retries",
                            &t!("connect.retries"),
                        )),
                )
                .child(
                    field(t!("connect.nearest").to_string())
                        .child(node_rows::render_row(
                            &self.nearest,
                            "connect.nearest",
                            self.errors.get("connect.nearest.error"),
                            RowRef::Nearest,
                            false,
                            cx,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(text("connect.nearest_hint")),
                        ),
                )
                .child(
                    field(t!("connect.ca_file.label").to_string())
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("connect.ca_file")
                                        .outline()
                                        .small()
                                        .label(ca_label)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.choose_ca_file(window, cx)
                                        })),
                                )
                                .when(self.ca_file.is_some(), |el| {
                                    el.child(
                                        Button::new("connect.ca_file.clear")
                                            .ghost()
                                            .small()
                                            .icon(IconName::Close)
                                            .accessibility_label(
                                                t!("connect.ca_file.clear").to_string(),
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_ca_file(None, cx)
                                            })),
                                    )
                                }),
                        )
                        .children(self.error("connect.ca_file.error", cx)),
                )
                .child(
                    Switch::new("connect.insecure")
                        .checked(self.allow_insecure)
                        .label(t!("connect.insecure").to_string())
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.allow_insecure = *checked;
                            cx.notify();
                        })),
                )
                .when(self.allow_insecure, |el| {
                    let warning = text("connect.insecure_warning");
                    el.child(
                        div()
                            .id("connect.insecure_warning")
                            .test_support()
                            .aria_label(warning.clone())
                            .p_2()
                            .rounded_md()
                            .bg(theme.danger.opacity(0.15))
                            .text_sm()
                            .child(warning),
                    )
                });
        Collapsible::new()
            .open(self.advanced)
            .gap_2()
            .child(
                h_flex().child(
                    Button::new("connect.advanced")
                        .ghost()
                        .small()
                        .icon(if self.advanced {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .label(summary)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.advanced = !this.advanced;
                            cx.notify();
                        })),
                ),
            )
            .content(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_names_the_tls_mode() {
        rust_i18n::set_locale("en");
        assert_eq!(tls_mode(false, false), "system certificates");
        assert_eq!(tls_mode(true, false), "CA file");
        assert_eq!(tls_mode(true, true), "insecure");
        assert_eq!(tls_mode(false, true), "insecure");
    }
}
