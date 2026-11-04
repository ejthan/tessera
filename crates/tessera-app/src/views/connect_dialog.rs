// SPDX-License-Identifier: GPL-3.0-or-later

//! The "Connect to Typesense" dialog.

use std::cell::Cell;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::Field;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use secrecy::SecretString;

use crate::connection::{Connection, ConnectorGlobal, error_text, host_label};

const DEFAULT_URL: &str = "http://localhost:8108";

/// Dialog state. Lives as long as the dialog is open.
pub struct ConnectDialog {
    url: Entity<InputState>,
    key: Entity<InputState>,
    error: Option<SharedString>,
    connecting: bool,
    connection: Entity<Connection>,
    /// Replacing this task cancels the previous connect attempt.
    task: Option<Task<()>>,
}

impl ConnectDialog {
    /// Opens the dialog with focus in the URL field.
    pub fn open(connection: Entity<Connection>, window: &mut Window, cx: &mut App) {
        let view = cx.new(|cx| Self::new(connection, window, cx));
        let url = view.read(cx).url.clone();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                url.focus_handle(cx).focus(window, cx);
            }
            let on_enter = view.clone();
            dialog
                .title(t!("connect.title").to_string())
                .w(px(520.))
                // Enter dispatches the dialog's Confirm action; submit and keep it open.
                // The dialog closes itself once the connection succeeds.
                .on_ok(move |_, window, cx| {
                    on_enter.update(cx, |view, cx| view.submit(window, cx));
                    false
                })
                .child(view.clone())
        });
    }

    fn new(connection: Entity<Connection>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_URL));
        let initial_key = std::env::var("TYPESENSE_API_KEY").unwrap_or_default();
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .default_value(initial_key)
        });
        Self {
            url,
            key,
            error: None,
            connecting: false,
            connection,
            task: None,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_string();
        let key = SecretString::from(self.key.read(cx).value().to_string());
        let connect = cx.global::<ConnectorGlobal>().0.clone();
        let attempt = connect(url.clone(), key);
        self.error = None;
        self.connecting = true;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = attempt.await;
            this.update_in(cx, |view, window, cx| {
                view.connecting = false;
                match result {
                    Ok(info) => {
                        view.connection.update(cx, |c, cx| c.set(info, cx));
                        window.close_dialog(cx);
                    }
                    Err(e) => view.error = Some(error_text(&e, &host_label(&url)).into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for ConnectDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url_label = t!("connect.url").to_string();
        let key_label = t!("connect.api_key").to_string();
        v_flex()
            .gap_3()
            .child(
                Field::new().label(url_label.clone()).child(
                    Input::new(&self.url)
                        .id("connect.url")
                        .aria_label(url_label),
                ),
            )
            .child(
                Field::new().label(key_label.clone()).child(
                    Input::new(&self.key)
                        .id("connect.key")
                        .aria_label(key_label)
                        .mask_toggle(),
                ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .id("connect.error")
                        .test_support()
                        .aria_label(error.clone())
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("connect.cancel")
                            .label(t!("connect.cancel").to_string())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("connect.submit")
                            .primary()
                            .label(t!("connect.submit").to_string())
                            .loading(self.connecting)
                            .disabled(self.connecting)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
