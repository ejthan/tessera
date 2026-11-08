// SPDX-License-Identifier: GPL-3.0-or-later

//! Unlocking writes on a read-only connection (D10): the confirm dialog and the banner.

use std::cell::RefCell;
use std::rc::Rc;
use std::task::Poll;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::{
    App, Entity, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, Task,
    WeakEntity, Window, div, prelude::*, rgb,
};
use rust_i18n::t;
use tessera_core::profile::Profile;
use tessera_widgets::confirm_danger::{self, DangerOptions};

use super::env_badge;
use crate::actions::LockAgain;
use crate::connection::ConnectionState;

/// Opens the unlock dialog for `profile`, the profile of `conn` (which may be mid-update).
/// `then` runs after the unlock, once the dialog has closed, so a dialog that `then` opens
/// is not closed by this one. If `conn` is gone or disconnected by then, nothing happens.
pub fn open(
    conn: WeakEntity<ConnectionState>,
    profile: &Profile,
    then: impl FnOnce(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let name = SharedString::from(profile.name.clone());
    let env = env_badge::label(&profile.environment);
    let prod = profile.environment.is_prod();
    let then = Rc::new(RefCell::new(Some(then)));
    confirm_danger::open(
        DangerOptions {
            title: t!("unlock.title").to_string().into(),
            consequences: vec![
                t!("unlock.consequence", name = name, env = env)
                    .to_string()
                    .into(),
            ],
            expected: name,
            action_label: t!("unlock.action").to_string().into(),
            prod,
        },
        move |window, cx| {
            let live = conn
                .update(cx, |c, cx| {
                    !c.closed && c.connected.is_some() && {
                        c.unlock(cx);
                        true
                    }
                })
                .unwrap_or(false);
            if !live {
                return Task::ready(Ok(()));
            }
            if let Some(then) = then.borrow_mut().take() {
                // The dialog closes when its task is polled, which is queued after this one, so
                // one yield: `then` may open a dialog, and the close must not take that one.
                window
                    .spawn(cx, async move |cx| {
                        let mut yielded = false;
                        std::future::poll_fn(|task| {
                            if std::mem::replace(&mut yielded, true) {
                                return Poll::Ready(());
                            }
                            task.waker().wake_by_ref();
                            Poll::Pending
                        })
                        .await;
                        cx.update(|window, cx| then(window, cx)).ok();
                    })
                    .detach();
            }
            Task::ready(Ok(()))
        },
        window,
        cx,
    );
}

/// The "Writes unlocked" banner for the top of a tab whose key context is `context`; `None`
/// while the connection is locked.
pub fn banner(
    conn: &Entity<ConnectionState>,
    context: &'static str,
    window: &Window,
    cx: &App,
) -> Option<impl IntoElement> {
    let c = conn.read(cx);
    if !c.writes_unlocked {
        return None;
    }
    let profile = &c.profile;
    let color = if profile.environment.is_prod() {
        rgb(env_badge::PROD_RED).into()
    } else {
        env_badge::color(profile.color, cx.theme())
    };
    let name = SharedString::from(profile.name.clone());
    let conn = conn.clone();
    Some(
        h_flex()
            .id("unlocked.banner")
            .test_support()
            .aria_label(SharedString::from(t!("unlock.unlocked").to_string()))
            .flex_none()
            .gap_2()
            .px_3()
            .py_1()
            .items_center()
            .text_sm()
            .bg(color)
            .text_color(gpui_kit::white())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("unlock.unlocked").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .child(t!("unlock.banner", name = name).to_string()),
            )
            .child(
                Button::new("unlocked.lock")
                    .outline()
                    .small()
                    .label(t!("unlock.lock_again").to_string())
                    .children(Kbd::binding_for_action(&LockAgain, Some(context), window))
                    .on_click(move |_, _, cx| conn.update(cx, |c, cx| c.lock(cx))),
            ),
    )
}
