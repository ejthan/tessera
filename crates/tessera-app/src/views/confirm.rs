// SPDX-License-Identifier: GPL-3.0-or-later

//! Confirmation for medium-danger actions (UX rule 3): the body states the consequences,
//! focus starts on Cancel and the confirm button is red.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{WindowExt as _, h_flex};
use gpui_kit::{App, ParentElement as _, SharedString, Styled as _, Window, div, prelude::*};
use rust_i18n::t;

/// Opens the confirmation. Enter activates the focused button; `on_confirm` runs only for
/// the confirm button. Cancel, Esc and Enter anywhere else close without running anything.
pub fn open(
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    confirm_label: impl Into<SharedString>,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let body = body.into();
    let confirm_label = confirm_label.into();
    let on_confirm = Rc::new(on_confirm);
    let cancel_focus = cx.focus_handle();
    let ok_focus = cx.focus_handle();
    let first_build = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_build.replace(false) {
            cancel_focus.focus(window, cx);
        }
        let on_confirm = on_confirm.clone();
        let on_enter = on_confirm.clone();
        let ok_focused = ok_focus.clone();
        dialog
            .title(title.clone())
            // The dialog's Enter binding wins over the focused button's own key handling.
            .on_ok(move |_, window, cx| {
                if ok_focused.is_focused(window) {
                    on_enter(window, cx);
                }
                true
            })
            .child(
                div()
                    .id("confirm.body")
                    .test_support()
                    .aria_label(body.clone())
                    .text_sm()
                    .child(body.clone()),
            )
            .footer(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("confirm.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("confirm.ok")
                            .danger()
                            .label(confirm_label.clone())
                            .track_focus(&ok_focus)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                on_confirm(window, cx);
                            }),
                    ),
            )
    });
}
