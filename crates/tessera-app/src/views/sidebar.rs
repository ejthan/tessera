// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar: saved profiles and open connections.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, transparent_black,
};
use rust_i18n::t;
use secrecy::ExposeSecret as _;
use tessera_core::profile::Profile;
use uuid::Uuid;

use super::connect_dialog::ConnectDialog;
use super::env_badge;
use super::import_dialog::ImportDialog;
use crate::actions::{
    ConnectSelected, DeleteConnection, DisconnectSelected, EditConnection, ExportProfiles,
    ImportProfiles, NewConnection, OpenAliases, OpenAnalytics, OpenConsoleTab, OpenCurations,
    OpenKeys, OpenLink, OpenPresetsTab, OpenSearchTab, OpenStemming, OpenStopwords, OpenSynonyms,
    SIDEBAR_ENTRY, SIDEBAR_ENTRY_BUTTON, SIDEBAR_LINK, ToggleReopenLast,
};
use crate::state::{AppState, ResourceKind};
use tessera_widgets::confirm;

/// File name the export save dialog suggests.
const EXPORT_NAME: &str = "tessera-profiles.json";

/// The sidebar view. Keeps one focus handle per entry so Tab and Enter work.
pub struct Sidebar {
    state: Entity<AppState>,
    focus: HashMap<Uuid, FocusHandle>,
    /// Focus handles of the "Overview" rows under connected entries.
    overview_focus: HashMap<Uuid, FocusHandle>,
    /// Focus handles of the "Collections" rows and of the favorites under them.
    collections_focus: HashMap<Uuid, FocusHandle>,
    favorite_focus: HashMap<(Uuid, String), FocusHandle>,
    /// Focus handles of the "Search" rows.
    search_focus: HashMap<Uuid, FocusHandle>,
    /// Focus handles of the "Console" rows.
    console_focus: HashMap<Uuid, FocusHandle>,
    /// Focus handles of the rows of the relevance resources.
    resource_focus: HashMap<(Uuid, ResourceKind), FocusHandle>,
    /// Why the last header action (export, reopen toggle, delete) failed, shown under the header.
    header_error: Option<SharedString>,
    export_task: Option<Task<()>>,
    _subscription: Subscription,
}

/// What one entry shows.
struct Entry {
    profile: Profile,
    saved: bool,
    /// The connection from CLI flags or `TYPESENSE_*`.
    temporary: bool,
    selected: bool,
    connecting: bool,
    connected: bool,
    /// The server has the resources of its version.
    supported: bool,
    unlocked: bool,
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
            overview_focus: HashMap::new(),
            collections_focus: HashMap::new(),
            favorite_focus: HashMap::new(),
            search_focus: HashMap::new(),
            console_focus: HashMap::new(),
            resource_focus: HashMap::new(),
            header_error: None,
            export_task: None,
            _subscription: subscription,
        }
    }

    /// Asks where to save and writes the profiles there, without keys.
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if state.profiles.is_empty() || state.load_error.is_some() {
            return;
        }
        let dir = std::env::home_dir().unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some(EXPORT_NAME));
        self.header_error = None;
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
                view.header_error = result.err().map(Into::into);
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle_reopen_last(&mut self, cx: &mut Context<Self>) {
        let saved = self.state.update(cx, |s, cx| {
            s.reopen_last = !s.reopen_last;
            cx.notify();
            s.save(cx)
        });
        cx.spawn(async move |this, cx| {
            let error = saved
                .await
                .err()
                .map(|e| t!("connect.error.save_file", detail = e).to_string());
            this.update(cx, |view, cx| {
                view.header_error = error.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                    temporary: state
                        .temporary
                        .as_ref()
                        .is_some_and(|(p, _)| p.id == profile.id),
                    selected: state.selected == Some(profile.id),
                    connecting: conn.is_some_and(|c| c.connecting),
                    connected: conn.is_some_and(|c| c.connected.is_some()),
                    supported: conn
                        .and_then(|c| c.connected.as_ref())
                        .is_some_and(|c| c.info.capabilities.supported),
                    unlocked: conn.is_some_and(|c| c.writes_unlocked),
                    error: conn.and_then(|c| c.error.clone()),
                    missing_key: conn.is_some_and(|c| c.missing_key),
                    profile,
                    saved,
                }
            })
            .collect()
    }

    /// A row under a connected entry that opens a tab. Enter or a click calls `open`.
    fn link_row(
        id: String,
        (label, shortcut): (SharedString, Option<Kbd>),
        focus: &FocusHandle,
        indent: bool,
        selected: bool,
        open: Rc<dyn Fn(&mut gpui_kit::App)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let on_click = open.clone();
        div()
            // Its own context: in SIDEBAR_ENTRY, Enter would open the overview instead.
            .key_context(SIDEBAR_LINK)
            .on_action(move |_: &OpenLink, _, cx| open(cx))
            .child(
                div()
                    .id(SharedString::from(id))
                    .test_support()
                    .track_focus(focus)
                    .aria_label(label.clone())
                    .when(indent, |el| el.ml_8())
                    .when(!indent, |el| el.ml_4())
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(transparent_black())
                    .text_sm()
                    .truncate()
                    .when(selected, |el| el.bg(theme.secondary))
                    .hover(|s| s.bg(theme.secondary))
                    .focus(|s| s.border_color(theme.ring))
                    .on_click(move |_, _, cx| on_click(cx))
                    .flex()
                    .justify_between()
                    .child(label)
                    .children(shortcut),
            )
            .into_any_element()
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
        let open_overview = connect.clone();
        let disconnect = {
            let state = self.state.clone();
            move |cx: &mut gpui_kit::App| state.update(cx, |s, cx| s.disconnect(id, cx))
        };
        let saved = entry.saved;
        let temporary = entry.temporary;
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
            let sidebar = cx.entity().downgrade();
            let name = entry.profile.name.clone();
            move |window: &mut Window, cx: &mut gpui_kit::App| {
                let state = state.clone();
                let sidebar = sidebar.clone();
                let key_name = name.clone();
                confirm::open(
                    t!("delete.title", name = name.clone()).to_string(),
                    t!("delete.body", name = name.clone()).to_string(),
                    t!("delete.confirm").to_string(),
                    move |_, cx| {
                        let deleted = state.update(cx, |s, cx| s.delete_profile(id, cx));
                        let sidebar = sidebar.clone();
                        let name = key_name.clone();
                        cx.spawn(async move |cx| {
                            // The entry is gone, so the header line reports a key left behind.
                            let Err(e) = deleted.await else {
                                return;
                            };
                            let text = t!(
                                "sidebar.delete_key_error",
                                name = name,
                                detail = e.to_string()
                            );
                            sidebar
                                .update(cx, |view, cx| {
                                    view.header_error = Some(text.to_string().into());
                                    cx.notify();
                                })
                                .ok();
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
                el.child(env_badge::lock(&element_id, entry.unlocked, cx))
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
                } else if temporary {
                    menu.menu(
                        t!("sidebar.save_temporary").to_string(),
                        Box::new(EditConnection),
                    )
                } else {
                    menu
                }
            });
        let button = if temporary {
            Some((
                "sidebar.save_temporary".to_string(),
                t!("sidebar.save_temporary"),
            ))
        } else if saved && entry.missing_key {
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
            .when(live, |el| {
                el.on_action(move |_: &DisconnectSelected, _, cx| disconnect(cx))
            })
            .when(saved || temporary, |el| {
                let edit = edit.clone();
                el.on_action(move |_: &EditConnection, window, cx| edit(window, cx))
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
            // The connection's tree: Enter (ConnectSelected above) or a click opens the tab.
            .when(entry.connected, |el| {
                let overview = SharedString::from(t!("sidebar.overview").to_string());
                let focus = self
                    .overview_focus
                    .entry(id)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
                el.child(
                    div()
                        .id(SharedString::from(format!("{element_id}.overview")))
                        .test_support()
                        .track_focus(focus)
                        .aria_label(overview.clone())
                        .ml_4()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(transparent_black())
                        .text_sm()
                        .when(entry.selected, |el| el.bg(theme.secondary))
                        .hover(|s| s.bg(theme.secondary))
                        .focus(|s| s.border_color(theme.ring))
                        .on_click(move |_, _, cx| open_overview(cx))
                        .child(overview),
                )
            })
            .when(entry.connected, |el| {
                let state = self.state.clone();
                let focus = self
                    .collections_focus
                    .entry(id)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone();
                let open = {
                    let state = state.clone();
                    Rc::new(move |cx: &mut gpui_kit::App| {
                        state.update(cx, |s, cx| s.open_collections(id, cx))
                    })
                };
                let label = SharedString::from(t!("sidebar.collections").to_string());
                let el = el.child(Self::link_row(
                    format!("sidebar.{id}.collections"),
                    (label, None),
                    &focus,
                    false,
                    entry.selected,
                    open,
                    cx,
                ));
                entry
                    .profile
                    .favorite_collections
                    .iter()
                    .fold(el, |el, name| {
                        let focus = self
                            .favorite_focus
                            .entry((id, name.clone()))
                            .or_insert_with(|| cx.focus_handle().tab_stop(true))
                            .clone();
                        let open = {
                            let state = state.clone();
                            let name = name.clone();
                            Rc::new(move |cx: &mut gpui_kit::App| {
                                state.update(cx, |s, cx| s.open_collection(id, name.clone(), cx))
                            })
                        };
                        el.child(Self::link_row(
                            format!("sidebar.{id}.collection.{name}"),
                            (SharedString::from(name.clone()), None),
                            &focus,
                            true,
                            false,
                            open,
                            cx,
                        ))
                    })
            })
            .when(entry.connected, |el| {
                let state = self.state.clone();
                let focus = self
                    .search_focus
                    .entry(id)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone();
                let open = Rc::new(move |cx: &mut gpui_kit::App| {
                    state.update(cx, |s, cx| s.open_search(id, None, None, cx))
                });
                el.child(Self::link_row(
                    format!("sidebar.{id}.search"),
                    (
                        SharedString::from(t!("sidebar.search").to_string()),
                        Kbd::global_binding_for_action(&OpenSearchTab, window),
                    ),
                    &focus,
                    false,
                    entry.selected,
                    open,
                    cx,
                ))
            })
            .when(entry.connected, |el| {
                let state = self.state.clone();
                let focus = self
                    .console_focus
                    .entry(id)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone();
                let open = Rc::new(move |cx: &mut gpui_kit::App| {
                    state.update(cx, |s, cx| s.open_console(id, cx))
                });
                el.child(Self::link_row(
                    format!("sidebar.{id}.console"),
                    (
                        SharedString::from(t!("sidebar.console").to_string()),
                        Kbd::global_binding_for_action(&OpenConsoleTab, window),
                    ),
                    &focus,
                    false,
                    entry.selected,
                    open,
                    cx,
                ))
            });
        let resources = ResourceKind::ALL
            .into_iter()
            .filter(|kind| entry.connected && kind.shown(entry.supported));
        let entry_el = resources.fold(entry_el, |el, kind| {
            let state = self.state.clone();
            let focus = self
                .resource_focus
                .entry((id, kind))
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let open = Rc::new(move |cx: &mut gpui_kit::App| {
                state.update(cx, |s, cx| s.open_resource(id, kind, cx))
            });
            el.child(Self::link_row(
                format!("sidebar.{id}.{}", kind.key()),
                (
                    SharedString::from(t!(kind.title_key()).to_string()),
                    resource_shortcut(kind, window),
                ),
                &focus,
                false,
                entry.selected,
                open,
                cx,
            ))
        });
        v_flex()
            .child(entry_el)
            .children(add_key)
            .into_any_element()
    }
}

/// The shortcut of a resource's row.
fn resource_shortcut(kind: ResourceKind, window: &Window) -> Option<Kbd> {
    match kind {
        ResourceKind::Aliases => Kbd::global_binding_for_action(&OpenAliases, window),
        ResourceKind::Synonyms => Kbd::global_binding_for_action(&OpenSynonyms, window),
        ResourceKind::Curations => Kbd::global_binding_for_action(&OpenCurations, window),
        ResourceKind::Stopwords => Kbd::global_binding_for_action(&OpenStopwords, window),
        ResourceKind::Presets => Kbd::global_binding_for_action(&OpenPresetsTab, window),
        ResourceKind::Keys => Kbd::global_binding_for_action(&OpenKeys, window),
        ResourceKind::Analytics => Kbd::global_binding_for_action(&OpenAnalytics, window),
        ResourceKind::Stemming => Kbd::global_binding_for_action(&OpenStemming, window),
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.entries(cx);
        self.focus
            .retain(|id, _| entries.iter().any(|e| e.profile.id == *id));
        self.overview_focus
            .retain(|id, _| entries.iter().any(|e| e.profile.id == *id && e.connected));
        self.collections_focus
            .retain(|id, _| entries.iter().any(|e| e.profile.id == *id && e.connected));
        self.search_focus
            .retain(|id, _| entries.iter().any(|e| e.profile.id == *id && e.connected));
        self.console_focus
            .retain(|id, _| entries.iter().any(|e| e.profile.id == *id && e.connected));
        self.resource_focus
            .retain(|(id, _), _| entries.iter().any(|e| e.profile.id == *id && e.connected));
        self.favorite_focus.retain(|(id, name), _| {
            entries.iter().any(|e| {
                e.profile.id == *id && e.connected && e.profile.favorite_collections.contains(name)
            })
        });
        let rows: Vec<_> = entries
            .into_iter()
            .map(|entry| self.render_entry(entry, window, cx))
            .collect();
        let state = self.state.read(cx);
        let reopen_last = state.reopen_last;
        let can_export = !state.profiles.is_empty() && state.load_error.is_none();
        let theme = cx.theme();
        v_flex()
            .on_action(cx.listener(|this, _: &ImportProfiles, window, cx| {
                ImportDialog::open(this.state.clone(), window, cx);
            }))
            .when(can_export, |el| {
                el.on_action(
                    cx.listener(|this, _: &ExportProfiles, window, cx| this.export(window, cx)),
                )
            })
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
                                        .menu_with_disabled(
                                            t!("sidebar.export").to_string(),
                                            Box::new(ExportProfiles),
                                            !can_export,
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
            .children(self.header_error.clone().map(|error| {
                div()
                    .id("sidebar.header_error")
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
