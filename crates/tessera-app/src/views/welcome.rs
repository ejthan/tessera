// SPDX-License-Identifier: GPL-3.0-or-later

//! First-run screen shown while no server is connected.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, ClickEvent, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::*, px,
};
use rust_i18n::t;

use crate::actions::NewConnection;

/// The Docker command shown on the welcome screen. Tessera never runs it.
pub const DOCKER_COMMAND: &str = "docker run -p 8108:8108 -v \"$(pwd)/typesense-data:/data\" typesense/typesense:30.2 --data-dir /data --api-key=xyz";

/// Renders the welcome screen. `copied` swaps the Copy button's label; `load_error` explains
/// why saved connections are missing.
pub fn render(
    copied: bool,
    load_error: Option<String>,
    on_add: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_import: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_copy: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let card = || {
        v_flex()
            .flex_1()
            .gap_2()
            .p_4()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
    };
    v_flex().size_full().items_center().justify_center().child(
        v_flex()
            .w(px(680.))
            .gap_6()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(t!("welcome.title").to_string()),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(t!("welcome.body").to_string()),
                    ),
            )
            .when_some(load_error, |el, error| {
                let text = SharedString::from(t!("welcome.load_error", error = error).to_string());
                el.child(
                    div()
                        .id("welcome.load_error")
                        .test_support()
                        .aria_label(text.clone())
                        .p_3()
                        .border_1()
                        .border_color(theme.danger)
                        .rounded_md()
                        .text_sm()
                        .child(text),
                )
            })
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .child(
                        card()
                            .child(
                                Button::new("welcome.add_connection")
                                    .primary()
                                    .label(t!("welcome.add_connection").to_string())
                                    .on_click(on_add),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("welcome.add_connection_hint").to_string())
                                    .children(Kbd::global_binding_for_action(
                                        &NewConnection,
                                        window,
                                    )),
                            )
                            .child(
                                h_flex().child(
                                    Button::new("welcome.import")
                                        .outline()
                                        .label(t!("welcome.import").to_string())
                                        .on_click(on_import),
                                ),
                            ),
                    )
                    .child(
                        card()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(t!("welcome.run_locally").to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("welcome.run_locally_hint").to_string()),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .p_3()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .bg(theme.secondary)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(theme.mono_font_family.clone())
                            .text_sm()
                            .child(DOCKER_COMMAND),
                    )
                    .child(
                        Button::new("welcome.copy")
                            .label(if copied {
                                t!("welcome.copied").to_string()
                            } else {
                                t!("welcome.copy").to_string()
                            })
                            .on_click(on_copy),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("welcome.trademark").to_string()),
            ),
    )
}
