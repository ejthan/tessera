// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog's layout: fields, warnings, result and buttons.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::form::Field;
use gpui_kit::component::input::Input;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use tessera_core::profile::Environment;

use super::{ConnectDialog, Phase, node_rows, result};
use crate::actions::{CONNECT_DIALOG, SaveConnection, TestConnection, ToggleKeyMask};
use crate::views::env_badge;

impl Render for ConnectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let field = |label: String| Field::new().label(label);
        let name_label = t!("connect.name").to_string();
        let key_label = t!("connect.api_key").to_string();
        let masked = self.key.read(cx).presentation().is_masked();
        let kbd = |action: &dyn gpui_kit::Action| {
            Kbd::binding_for_action(action, Some(CONNECT_DIALOG), window)
        };
        let colors = h_flex()
            .gap_1()
            .children(env_badge::COLORS.map(|(color, name)| {
                Button::new(SharedString::from(format!("connect.color.{name}")))
                    .ghost()
                    .small()
                    .selected(self.color == color)
                    .accessibility_label(t!(format!("connect.color.{name}")).to_string())
                    .child(
                        div()
                            .size_4()
                            .rounded_full()
                            .bg(env_badge::color(color, &theme)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color = color;
                        cx.notify();
                    }))
            }));
        let envs = [
            (Environment::Local, "local"),
            (Environment::Dev, "dev"),
            (Environment::Staging, "staging"),
            (Environment::Prod, "prod"),
            (Environment::Custom(String::new()), "other"),
        ];
        let custom = matches!(self.env, Environment::Custom(_));
        let env_group = ButtonGroup::new("connect.env")
            .outline()
            .small()
            .children(envs.map(|(env, name)| {
                let selected = std::mem::discriminant(&env) == std::mem::discriminant(&self.env);
                Button::new(SharedString::from(format!("connect.env.{name}")))
                    .label(t!(format!("connect.env.{name}")).to_string())
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_env(env.clone(), window, cx);
                    }))
            }));
        let key_hint = if cfg!(target_os = "macos") {
            t!("connect.key_hint.macos")
        } else if cfg!(target_os = "windows") {
            t!("connect.key_hint.windows")
        } else {
            t!("connect.key_hint.linux")
        };
        let remote_http = self
            .nodes
            .iter()
            .chain([&self.nearest])
            .any(|r| r.is_remote_http(cx));
        let fresh = self.fresh(cx);
        let busy = self.running();
        let blocked = self.blocked(cx);
        let save_label = if matches!(self.phase, Phase::TestFailed) && fresh {
            t!("connect.save_anyway")
        } else {
            t!("connect.save")
        };
        v_flex()
            .key_context(CONNECT_DIALOG)
            .on_action(cx.listener(|this, _: &TestConnection, window, cx| this.test(window, cx)))
            .on_action(cx.listener(|this, _: &SaveConnection, window, cx| this.save(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleKeyMask, window, cx| this.toggle_key_mask(window, cx)),
            )
            .gap_3()
            .child(
                field(name_label.clone()).child(
                    Input::new(&self.name)
                        .id("connect.name")
                        .aria_label(name_label),
                ),
            )
            .child(field(t!("connect.color").to_string()).child(colors))
            .child(
                field(t!("connect.environment").to_string())
                    .child(h_flex().gap_2().child(env_group).when(custom, |el| {
                        el.child(
                            div().w(px(160.)).child(
                                Input::new(&self.env_name)
                                    .id("connect.env.name")
                                    .aria_label(t!("connect.env.name").to_string()),
                            ),
                        )
                    }))
                    .children(self.error("connect.env.error", cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("connect.env.prod_hint").to_string()),
                    ),
            )
            .child(
                field(t!("connect.nodes").to_string())
                    .child(node_rows::render(&self.nodes, &self.errors, cx))
                    .when(remote_http, |el| {
                        let text = SharedString::from(t!("connect.http_warning").to_string());
                        el.child(
                            div()
                                .id("connect.http_warning")
                                .test_support()
                                .aria_label(text.clone())
                                .p_2()
                                .rounded_md()
                                .bg(theme.warning.opacity(0.15))
                                .text_sm()
                                .child(text),
                        )
                    }),
            )
            .child(
                field(key_label.clone())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.key)
                                        .id("connect.key")
                                        .aria_label(key_label),
                                ),
                            )
                            .child(
                                Button::new("connect.key.toggle")
                                    .ghost()
                                    .icon(if masked {
                                        IconName::Eye
                                    } else {
                                        IconName::EyeOff
                                    })
                                    .accessibility_label(
                                        if masked {
                                            t!("connect.key.show")
                                        } else {
                                            t!("connect.key.hide")
                                        }
                                        .to_string(),
                                    )
                                    .children(kbd(&ToggleKeyMask))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_key_mask(window, cx)
                                    })),
                            ),
                    )
                    .children(self.error("connect.key.error", cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(key_hint.to_string()),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Switch::new("connect.read_only")
                            .checked(self.read_only)
                            .label(t!("connect.read_only").to_string())
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.read_only = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("connect.read_only_hint").to_string()),
                    ),
            )
            .child(self.render_advanced(cx))
            .children(
                self.outcome
                    .as_ref()
                    .filter(|_| !matches!(self.phase, Phase::Testing { .. }))
                    .map(|o| result::render(o, !fresh, cx)),
            )
            .children(self.render_save_error(cx))
            .children(
                blocked
                    .clone()
                    .map(|text| super::error_line("connect.blocked".into(), text, cx)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("connect.test")
                            .outline()
                            .label(t!("connect.test").to_string())
                            .children(kbd(&TestConnection))
                            .loading(matches!(self.phase, Phase::Testing { then_save: false }))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.test(window, cx))),
                    )
                    .child(
                        Button::new("connect.cancel")
                            .label(t!("connect.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .disabled(self.saving())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("connect.save")
                            .primary()
                            .label(save_label.to_string())
                            .children(kbd(&SaveConnection))
                            .loading(matches!(
                                self.phase,
                                Phase::Testing { then_save: true } | Phase::Saving
                            ))
                            .disabled(busy || blocked.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}
