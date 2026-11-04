// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar: saved profiles and open connections.

use std::collections::HashMap;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, transparent_black,
};
use rust_i18n::t;
use secrecy::ExposeSecret as _;
use tessera_core::profile::Profile;
use uuid::Uuid;

use super::connect_dialog::ConnectDialog;
use super::import_dialog::ImportDialog;
use super::{confirm, env_badge};
use crate::actions::{
    ConnectSelected, DeleteConnection, DisconnectSelected, EditConnection, ExportProfiles,
    ImportProfiles, NewConnection, SIDEBAR_ENTRY, SIDEBAR_ENTRY_BUTTON, ToggleReopenLast,
};
use crate::state::AppState;

/// File name the export save dialog suggests.
const EXPORT_NAME: &str = "tessera-profiles.json";

/// The sidebar view. Keeps one focus handle per entry so Tab and Enter work.
pub struct Sidebar {
    state: Entity<AppState>,
    focus: HashMap<Uuid, FocusHandle>,
    /// Why the last export failed, shown under the header.
    export_error: Option<SharedString>,
    export_task: Option<Task<()>>,
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
    missing_key: bool,
}

impl Sidebar {
    /// A sidebar that re-renders whenever the app state changes.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            focus: HashMap::new(),
            export_error: None,
            export_task: None,
            _subscription: subscription,
        }
    }

    /// Asks where to save and writes the profiles there, without keys.
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = std::env::home_dir().unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some(EXPORT_NAME));
        self.export_error = None;
        cx.notify();
        self.export_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match picked.await {
                Ok(Ok(Some(path))) => {
                    let Ok(written) =
                        this.update(cx, |view, cx| view.state.read(cx).export_to(path, cx))
                    else {
                        return;
                    };
                    written
                        .await
                        .map_err(|e| t!("sidebar.export_error", detail = e).to_string())
                }
                Ok(Err(e)) => Err(t!("connect.error.picker", detail = e.to_string()).to_string()),
                // Cancelled.
                Ok(Ok(None)) | Err(_) => Ok(()),
            };
            this.update(cx, |view, cx| {
                view.export_error = result.err().map(Into::into);
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle_reopen_last(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.reopen_last = !s.reopen_last;
            s.save(cx).detach();
            cx.notify();
        });
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
                    missing_key: conn.is_some_and(|c| c.missing_key),
                    profile,
                    saved,
                }
            })
            .collect()
    }

    fn render_entry(
        &mut self,
        entry: Entry,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let saved = entry.saved;
        // Edit for a saved profile, "Save as profile…" for the temporary connection.
        let edit = {
            let state = self.state.clone();
            move |window: &mut Window, cx: &mut gpui_kit::App| {
                if saved {
                    return ConnectDialog::open_edit(state.clone(), id, window, cx);
                }
                let temporary = state.read(cx).temporary.clone();
                if let Some((profile, key)) = temporary.filter(|(p, _)| p.id == id) {
                    let key = Some(key).filter(|k| !k.expose_secret().is_empty());
                    ConnectDialog::open_new(state.clone(), Some((profile, key)), window, cx);
                }
            }
        };
        let delete = {
            let state = self.state.clone();
            let name = entry.profile.name.clone();
            move |window: &mut Window, cx: &mut gpui_kit::App| {
                let state = state.clone();
                confirm::open(
                    t!("delete.title", name = name.clone()).to_string(),
                    t!("delete.body", name = name.clone()).to_string(),
                    t!("delete.confirm").to_string(),
                    move |_, cx| {
                        let deleted = state.update(cx, |s, cx| s.delete_profile(id, cx));
                        cx.spawn(async move |_| {
                            if let Err(e) = deleted.await {
                                // shortcut: the entry is gone, so there is no inline place for
                                // this; logged until M1 has a place for background errors.
                                tracing::warn!("removing the stored key failed: {e}");
                            }
                        })
                        .detach();
                    },
                    window,
                    cx,
                );
            }
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
                let label = SharedString::from(t!("sidebar.not_saved").to_string());
                el.child(
                    div()
                        .id(SharedString::from(format!("{element_id}.not_saved")))
                        .test_support()
                        .aria_label(label)
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
                let menu = if live {
                    menu.menu(
                        t!("sidebar.disconnect").to_string(),
                        Box::new(DisconnectSelected),
                    )
                } else {
                    menu.menu(t!("sidebar.connect").to_string(), Box::new(ConnectSelected))
                };
                if saved {
                    menu.menu(t!("sidebar.edit").to_string(), Box::new(EditConnection))
                        .menu(t!("sidebar.delete").to_string(), Box::new(DeleteConnection))
                } else {
                    menu.menu(
                        t!("sidebar.save_temporary").to_string(),
                        Box::new(EditConnection),
                    )
                }
            });
        let button = if !saved {
            Some((
                "sidebar.save_temporary".to_string(),
                t!("sidebar.save_temporary"),
            ))
        } else if entry.missing_key {
            Some((format!("{element_id}.add_key"), t!("sidebar.add_key")))
        } else {
            None
        };
        let add_key = button.map(|(button_id, label)| {
            // Its own key context: in SIDEBAR_ENTRY, Enter would connect instead of clicking.
            div()
                .key_context(SIDEBAR_ENTRY_BUTTON)
                .on_action({
                    let edit = edit.clone();
                    move |_: &EditConnection, window, cx| edit(window, cx)
                })
                .px_2()
                .pb_1()
                .child(
                    Button::new(SharedString::from(button_id))
                        .outline()
                        .small()
                        .label(label.to_string())
                        .children(Kbd::binding_for_action(
                            &EditConnection,
                            Some(SIDEBAR_ENTRY_BUTTON),
                            window,
                        ))
                        .on_click({
                            let edit = edit.clone();
                            move |_, window, cx| edit(window, cx)
                        }),
                )
        });
        let entry_el = v_flex()
            // On the wrapper, so the context menu (a child of the row) resolves Enter too.
            .key_context(SIDEBAR_ENTRY)
            .on_action(move |_: &ConnectSelected, _, cx| connect(cx))
            .on_action(move |_: &DisconnectSelected, _, cx| disconnect(cx))
            .on_action({
                let edit = edit.clone();
                move |_: &EditConnection, window, cx| edit(window, cx)
            })
            .when(saved, |el| {
                el.on_action(move |_: &DeleteConnection, window, cx| delete(window, cx))
            })
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
            });
        v_flex()
            .child(entry_el)
            .children(add_key)
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
            .map(|entry| self.render_entry(entry, window, cx))
            .collect();
        let reopen_last = self.state.read(cx).reopen_last;
        let theme = cx.theme();
        v_flex()
            .on_action(cx.listener(|this, _: &ImportProfiles, window, cx| {
                ImportDialog::open(this.state.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ExportProfiles, window, cx| this.export(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleReopenLast, _, cx| this.toggle_reopen_last(cx)))
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
                        h_flex()
                            .child(
                                Button::new("sidebar.add_connection")
                                    .ghost()
                                    .small()
                                    .label(t!("sidebar.add_connection").to_string())
                                    .children(Kbd::global_binding_for_action(
                                        &NewConnection,
                                        window,
                                    ))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(NewConnection), cx)
                                    }),
                            )
                            .child(
                                Button::new("sidebar.more")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Ellipsis)
                                    .accessibility_label(t!("sidebar.more").to_string())
                                    .dropdown_menu(move |menu, _, _| {
                                        menu.menu(
                                            t!("sidebar.import").to_string(),
                                            Box::new(ImportProfiles),
                                        )
                                        .menu(
                                            t!("sidebar.export").to_string(),
                                            Box::new(ExportProfiles),
                                        )
                                        .separator()
                                        .menu_with_check(
                                            t!("sidebar.reopen_last").to_string(),
                                            reopen_last,
                                            Box::new(ToggleReopenLast),
                                        )
                                    }),
                            ),
                    ),
            )
            .children(self.export_error.clone().map(|error| {
                div()
                    .id("sidebar.export_error")
                    .test_support()
                    .aria_label(error.clone())
                    .px_2()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(error)
            }))
            .children(rows)
    }
}
