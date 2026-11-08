// SPDX-License-Identifier: GPL-3.0-or-later

//! Asks for the name of a new resource.

use std::cell::Cell;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use tessera_core::relevance::name_ok;

use super::dialog_title;
use crate::connection::ConnectionState;

/// Width of the dialog.
const WIDTH: f32 = 420.;

type Check = Box<dyn Fn(String, &mut App) -> Task<Result<(), SharedString>>>;
type OnName = Box<dyn Fn(String, &mut Window, &mut App)>;

/// Refuses a name [`name_ok`] does not take, then one that `check` refuses after a fresh list.
pub struct NameDialog {
    name: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
    check: Check,
    on_name: OnName,
    checking: Option<Task<()>>,
    cancel_focus: FocusHandle,
    _subscription: Subscription,
}

impl NameDialog {
    /// Opens it. `check` answers why the name is taken, inline, and `on_name` runs once it
    /// passed, with the dialog closed.
    pub fn open(
        title: String,
        connection: Entity<ConnectionState>,
        check: impl Fn(String, &mut App) -> Task<Result<(), SharedString>> + 'static,
        on_name: impl Fn(String, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = cx.new(|cx| {
            let name = cx.new(|cx| InputState::new(window, cx));
            Self {
                _subscription: cx.subscribe_in(
                    &name,
                    window,
                    |this: &mut Self, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.submit(window, cx);
                        }
                    },
                ),
                name,
                error: None,
                busy: false,
                check: Box::new(check),
                on_name: Box::new(on_name),
                checking: None,
                cancel_focus: cx.focus_handle(),
            }
        });
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                let name = view.read(cx).name.read(cx);
                name.focus_handle(cx).focus(window, cx);
            }
            let enter = view.clone();
            dialog
                .title(dialog_title(title.clone(), &connection, cx))
                .w(px(WIDTH))
                // The dialog's Enter binding wins over the focused button's own key handling.
                .on_ok(move |_, window, cx| {
                    enter.update(cx, |this, cx| {
                        let cancel = this.cancel_focus.is_focused(window);
                        if !cancel {
                            this.submit(window, cx);
                        }
                        cancel
                    })
                })
                .child(view.clone())
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        if !name_ok(&name) {
            self.error = Some(t!("resources.bad_name").to_string().into());
            return cx.notify();
        }
        self.busy = true;
        self.error = None;
        let check = (self.check)(name.clone(), cx);
        self.checking = Some(cx.spawn_in(window, async move |this, cx| {
            let checked = check.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match checked {
                    Ok(()) => {
                        window.close_dialog(cx);
                        (this.on_name)(name, window, cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for NameDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        v_flex()
            .gap_2()
            .child(t!("resources.name").to_string())
            .child(
                Input::new(&self.name)
                    .id("resources.name")
                    .aria_label(t!("resources.name").to_string()),
            )
            .children(self.error.clone().map(|error| {
                div()
                    .id("resources.name.error")
                    .test_support()
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("resources.name.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("resources.name.ok")
                            .primary()
                            .label(t!("resources.name.ok").to_string())
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
