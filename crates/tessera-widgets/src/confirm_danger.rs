// SPDX-License-Identifier: GPL-3.0-or-later

//! Confirmation for high-danger actions (UX rule 3): the dialog lists the concrete
//! consequences and the action stays disabled until the user types the exact name.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::Focusable as _;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, rgb,
};
use rust_i18n::t;

/// Red of the PROD badge, as in the app's environment badge (widgets cannot depend on the app).
const PROD_RED: u32 = 0xDC2626;

/// What the dialog says and what the user has to type.
pub struct DangerOptions {
    /// Dialog title.
    pub title: SharedString,
    /// One line per concrete consequence.
    pub consequences: Vec<SharedString>,
    /// The text the user has to type, compared exactly (case, spaces).
    pub expected: SharedString,
    /// Label of the red action button.
    pub action_label: SharedString,
    /// Shows the PROD badge next to the title.
    pub prod: bool,
}

/// Opens the dialog. Focus starts in the text field. `on_confirm` runs only when the text
/// matches and the action button is activated (click, or Enter on the focused button);
/// Enter anywhere else does nothing. The dialog stays open with the action busy until the
/// task ends: `Ok` closes it, `Err` shows the text inline (`danger.error`) and re-enables it.
pub fn open(
    opts: DangerOptions,
    on_confirm: impl Fn(&mut Window, &mut App) -> Task<Result<(), SharedString>> + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let DangerOptions {
        title,
        consequences,
        expected,
        action_label,
        prod,
    } = opts;
    let input = cx.new(|cx| InputState::new(window, cx));
    let ok_focus = cx.focus_handle();
    let on_confirm: Rc<OnConfirm> = Rc::new(on_confirm);
    let footer = cx.new(|cx| Footer {
        input: input.clone(),
        expected: expected.clone(),
        action_label,
        ok_focus: ok_focus.clone(),
        on_confirm: on_confirm.clone(),
        busy: false,
        error: None,
        _watch: cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()),
    });
    let first_build = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_build.replace(false) {
            input.read(cx).focus_handle(cx).focus(window, cx);
        }
        let (ok_focus_, footer_, footer_cancel) =
            (ok_focus.clone(), footer.clone(), footer.clone());
        let label = t!("danger.input_label", name = expected).to_string();
        dialog
            .title(
                h_flex()
                    .gap_2()
                    .child(title.clone())
                    .when(prod, |row| row.child(prod_badge())),
            )
            // While the task runs Esc, the backdrop and the close button do nothing, so the
            // completion cannot close a dialog that opened in the meantime.
            .on_cancel(move |_, _, cx| !footer_cancel.read(cx).busy)
            // Enter dispatches the dialog's Confirm: it only acts on the focused action button.
            .on_ok(move |_, window, cx| {
                if ok_focus_.is_focused(window) {
                    footer_.update(cx, |f, cx| f.confirm(window, cx));
                }
                // Only the task's result closes the dialog.
                false
            })
            .child(
                v_flex()
                    .gap_3()
                    .child(v_flex().gap_1().text_sm().children(
                        consequences.iter().enumerate().map(|(i, line)| {
                            div()
                                .id(SharedString::from(format!("danger.consequence.{i}")))
                                .test_support()
                                .aria_label(line.clone())
                                .child(format!("• {line}"))
                        }),
                    ))
                    .child(div().text_sm().child(label.clone()))
                    .child(Input::new(&input).id("danger.input").aria_label(label)),
            )
            .footer(footer.clone())
    });
}

fn prod_badge() -> impl IntoElement {
    div()
        .id("danger.prod")
        .test_support()
        .aria_label(t!("env.prod").to_string())
        .flex_none()
        .px_1()
        .rounded_sm()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .bg(rgb(PROD_RED))
        .text_color(gpui_kit::white())
        .child(t!("env.prod").to_string())
}

type OnConfirm = dyn Fn(&mut Window, &mut App) -> Task<Result<(), SharedString>>;

/// The button row; its own view so it re-renders when the text changes.
struct Footer {
    input: Entity<InputState>,
    expected: SharedString,
    action_label: SharedString,
    ok_focus: FocusHandle,
    on_confirm: Rc<OnConfirm>,
    busy: bool,
    error: Option<SharedString>,
    _watch: Subscription,
}

impl Footer {
    /// The one confirm path for click and Enter: needs an exact match and no run in flight.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.input.read(cx).value() != self.expected {
            return;
        }
        self.busy = true;
        self.error = None;
        let task = (self.on_confirm)(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(text) => this.error = Some(text),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for Footer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.input.read(cx).value() == self.expected;
        v_flex()
            .gap_2()
            .children(self.error.clone().map(|text| {
                div()
                    .id("danger.error")
                    .test_support()
                    .aria_label(text.clone())
                    .text_sm()
                    .text_color(rgb(PROD_RED))
                    .child(text)
            }))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("danger.cancel")
                            .label(t!("common.cancel").to_string())
                            .disabled(self.busy)
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("danger.ok")
                            .danger()
                            .label(self.action_label.clone())
                            .loading(self.busy)
                            .disabled(!matches || self.busy)
                            .track_focus(&self.ok_focus)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            )
    }
}
