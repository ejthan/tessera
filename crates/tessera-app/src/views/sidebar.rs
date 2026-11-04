// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar: saved profiles and open connections.

use std::collections::HashMap;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, transparent_black,
};
use rust_i18n::t;
use tessera_core::profile::Profile;
use uuid::Uuid;

use super::env_badge;
use crate::actions::{ConnectSelected, DisconnectSelected, NewConnection, SIDEBAR_ENTRY};
use crate::state::AppState;

/// The sidebar view. Keeps one focus handle per entry so Tab and Enter work.
pub struct Sidebar {
    state: Entity<AppState>,
    focus: HashMap<Uuid, FocusHandle>,
    _subscription: Subscription,
}

/// What one entry shows.
struct Entry {
    profile: Profile,
    saved: bool,
    selected: bool,
    connecting: bool,
    connected: bool,
    error: Option<SharedString>,
}

impl Sidebar {
    /// A sidebar that re-renders whenever the app state changes.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            focus: HashMap::new(),
            _subscription: subscription,
        }
    }

    /// Saved profiles in order, then the temporary and other unsaved connections.
    fn entries(&self, cx: &Context<Self>) -> Vec<Entry> {
        let state = self.state.read(cx);
        let mut profiles: Vec<(Profile, bool)> =
            state.profiles.iter().map(|p| (p.clone(), true)).collect();
        let unsaved = state
            .temporary
            .iter()
            .map(|(p, _)| p.clone())
            .chain(state.connections.iter().map(|c| c.read(cx).profile.clone()));
        for p in unsaved {
            if !profiles.iter().any(|(q, _)| q.id == p.id) {
                profiles.push((p, false));
            }
        }
        profiles
            .into_iter()
            .map(|(profile, saved)| {
                let conn = state.connection(profile.id, cx);
                let conn = conn.as_ref().map(|c| c.read(cx));
                Entry {
                    selected: state.selected == Some(profile.id),
                    connecting: conn.is_some_and(|c| c.connecting),
                    connected: conn.is_some_and(|c| c.connected.is_some()),
                    error: conn.and_then(|c| c.error.clone()),
                    profile,
                    saved,
                }
            })
            .collect()
    }

    fn render_entry(&mut self, entry: Entry, cx: &mut Context<Self>) -> AnyElement {
        let id = entry.profile.id;
        let handle = self
            .focus
            .entry(id)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let theme = cx.theme();
        let element_id = format!("sidebar.profile.{id}");
        let name = SharedString::from(entry.profile.name.clone());
        let live = entry.connected || entry.connecting;
        let connect = {
            let state = self.state.clone();
            move |cx: &mut gpui_kit::App| state.update(cx, |s, cx| s.connect(id, None, cx))
        };
        let on_click = connect.clone();
        let disconnect = {
            let state = self.state.clone();
            move |cx: &mut gpui_kit::App| state.update(cx, |s, cx| s.disconnect(id, cx))
        };
        let row = h_flex()
            .id(SharedString::from(element_id.clone()))
            .test_support()
            .track_focus(&handle)
            .aria_label(name.clone())
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(transparent_black())
            .text_sm()
            .when(entry.selected, |el| el.bg(theme.secondary))
            .hover(|s| s.bg(theme.secondary))
            .focus(|s| s.border_color(theme.ring))
            .on_click(move |_, _, cx| on_click(cx))
            .child(env_badge::dot(entry.profile.color, cx))
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when(!entry.saved, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("sidebar.not_saved").to_string()),
                )
            })
            .when(entry.connecting, |el| el.child(Spinner::new().small()))
            .when(entry.profile.read_only, |el| {
                let label = SharedString::from(t!("sidebar.read_only").to_string());
                el.child(
                    div()
                        .id(SharedString::from(format!("{element_id}.readonly")))
                        .test_support()
                        .aria_label(label)
                        .child(
                            Icon::new(gpui_kit::assets::IconName::Lock)
                                .small()
                                .text_color(theme.muted_foreground),
                        ),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("{element_id}.env")))
                    .test_support()
                    .aria_label(SharedString::from(env_badge::label(
                        &entry.profile.environment,
                    )))
                    .child(env_badge::render(&entry.profile.environment, cx)),
            )
            .context_menu(move |menu, _, _| {
                if live {
                    menu.menu(
                        t!("sidebar.disconnect").to_string(),
                        Box::new(DisconnectSelected),
                    )
                } else {
                    menu.menu(t!("sidebar.connect").to_string(), Box::new(ConnectSelected))
                }
            });
        v_flex()
            // On the wrapper, so the context menu (a child of the row) resolves Enter too.
            .key_context(SIDEBAR_ENTRY)
            .on_action(move |_: &ConnectSelected, _, cx| connect(cx))
            .on_action(move |_: &DisconnectSelected, _, cx| disconnect(cx))
            .child(row)
            .when_some(entry.error, |el, error| {
                el.child(
                    div()
                        .id(SharedString::from(format!("{element_id}.error")))
                        .test_support()
                        .aria_label(error.clone())
                        .px_2()
                        .pb_1()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .when(entry.selected && entry.connected, |el| {
                let overview = SharedString::from(t!("sidebar.overview").to_string());
                el.child(
                    div()
                        .id("sidebar.overview")
                        .test_support()
                        .aria_label(overview.clone())
                        .ml_4()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.secondary)
                        .text_sm()
                        .child(overview),
                )
            })
            .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.entries(cx);
        self.focus
            .retain(|id, _| entries.iter().any(|e| e.profile.id == *id));
        let rows: Vec<_> = entries
            .into_iter()
            .map(|entry| self.render_entry(entry, cx))
            .collect();
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .p_2()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .px_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(t!("sidebar.connections").to_string()),
                    )
                    .child(
                        Button::new("sidebar.add_connection")
                            .ghost()
                            .small()
                            .label(t!("sidebar.add_connection").to_string())
                            .children(Kbd::global_binding_for_action(&NewConnection, window))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(NewConnection), cx)
                            }),
                    ),
            )
            .children(rows)
    }
}
