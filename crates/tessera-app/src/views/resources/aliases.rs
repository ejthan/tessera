// SPDX-License-Identifier: GPL-3.0-or-later

//! The aliases of a connection: their state, and New, Repoint and Delete.

use std::cell::Cell;
use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{Alias, Collection, Error};
use tessera_core::relevance::{AliasState, alias_state, name_ok};

use super::super::overview::grouped;
use super::{
    Danger, ListRow, ResourceColumn, ResourceList, client_of, confirm_write, empty_state, notice,
    row_button, toolbar,
};
use crate::actions::{
    ALIAS_DIALOG, DeleteResource, EditResource, FocusResourceFilter, NewResource, ReloadResource,
    SaveResource,
};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/collection-alias.html";
/// Width of the dialog.
const DIALOG_WIDTH: f32 = 420.;

/// The columns of the table.
const COLUMNS: &[ResourceColumn] = &[
    ("name", 240., false),
    ("target", 240., false),
    ("documents", 100., true),
    ("state", 340., false),
];

/// What a row's state says; empty for a working alias.
fn state_text(state: AliasState) -> String {
    match state {
        AliasState::Ok => String::new(),
        AliasState::TargetMissing => t!("aliases.state.target_missing").to_string(),
        AliasState::TargetEmpty => t!("aliases.state.target_empty").to_string(),
        AliasState::TargetIsAlias => t!("aliases.state.target_is_alias").to_string(),
        AliasState::Shadowed => t!("aliases.state.shadowed").to_string(),
    }
}

fn documents(collections: &[Collection], name: &str) -> Option<u64> {
    let found = collections.iter().find(|c| c.name == name);
    found.map(|c| c.num_documents)
}

/// The count of `name` as text, "–" for no collection.
fn count_text(collections: &[Collection], name: &str) -> String {
    documents(collections, name).map_or_else(|| t!("figure.missing").to_string(), grouped)
}

/// The content of an aliases tab.
pub struct AliasesView {
    connection: Entity<ConnectionState>,
    list: Entity<ResourceList>,
    /// Why the last write failed, in full.
    error: Option<SharedString>,
    dialog: Option<WeakEntity<AliasDialog>>,
    /// When the rows were built from the cache.
    loaded: Option<Instant>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl AliasesView {
    /// The aliases of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("aliases", COLUMNS, window, cx));
        let subscriptions = [
            cx.observe(&connection, |this, _, cx| this.sync(cx)),
            cx.observe(&list, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            connection,
            list,
            error: None,
            dialog: None,
            loaded: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this.sync(cx);
        this
    }

    /// Rebuilds the rows when the list changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let cache = &self.connection.read(cx).collection_cache;
        if self.loaded == cache.loaded_at {
            return cx.notify();
        }
        let collections = cache.collections.as_deref().unwrap_or_default();
        let mut rows: Vec<ListRow> = cache
            .aliases
            .iter()
            .map(|alias| {
                let state = alias_state(alias, collections, &cache.aliases);
                ListRow {
                    key: alias.name.clone(),
                    cells: vec![
                        alias.name.clone(),
                        alias.collection_name.clone(),
                        count_text(collections, &alias.collection_name),
                        state_text(state),
                    ],
                    warning: state != AliasState::Ok,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        self.loaded = cache.loaded_at;
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
        cx.notify();
    }

    /// The selected alias and its target, only while its row is on screen.
    fn visible_selected(&self, cx: &App) -> Option<(String, String)> {
        let name = self.list.read(cx).selected(cx)?;
        let aliases = &self.connection.read(cx).collection_cache.aliases;
        let alias = aliases.iter().find(|a| a.name == name)?;
        Some((name, alias.collection_name.clone()))
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        let none = self.visible_selected(cx).is_none();
        vec![(none, &EditResource), (none, &DeleteResource)]
    }

    /// The open dialog.
    pub fn dialog(&self) -> Option<Entity<AliasDialog>> {
        self.dialog.as_ref()?.upgrade()
    }

    /// Loads the list again, keeping the old one on screen until the new one arrives.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.connection
            .update(cx, |c, cx| c.load_collection_list(cx));
    }

    /// Unlocks if needed, then opens the dialog; `repoint` holds an alias and its target.
    fn open_dialog(
        &mut self,
        repoint: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let dialog = AliasDialog::open(this.clone(), connection, repoint, window, cx);
                this.update(cx, |view, _| view.dialog = Some(dialog)).ok();
            });
        });
    }

    fn repoint_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected) = self.visible_selected(cx) {
            self.open_dialog(Some(selected), window, cx);
        }
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, _)) = self.visible_selected(cx) else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let prod = connection.read(cx).profile.environment.is_prod();
                let danger = Danger {
                    title: t!("aliases.delete.title", name = name).to_string(),
                    lines: vec![t!("aliases.delete.line").to_string()],
                    expected: name.clone(),
                    action: t!("aliases.delete").to_string(),
                    typed: prod,
                    prod,
                };
                let view = this.clone();
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), connection.clone(), name.clone(), cx)
                };
                confirm_write(danger, run, report_to(this), window, cx);
            });
        });
    }
}

impl Focusable for AliasesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AliasesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let conn = self.connection.read(cx);
        let locked = conn.writes_locked();
        let cache = &conn.collection_cache;
        let list_error = cache.error.clone().or(cache.aliases_error.clone());
        let empty = cache.collections.is_some() && cache.aliases.is_empty() && list_error.is_none();
        let none = self.visible_selected(cx).is_none();
        let top = toolbar(
            "aliases",
            &self.list,
            &self.focus_handle,
            locked,
            window,
            cx,
        );
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| {
                this.open_dialog(None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &EditResource, window, cx| {
                this.repoint_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.delete_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(top)
            .children(list_error.map(|e| notice("aliases.list_error", e, danger)))
            .children(
                self.error
                    .clone()
                    .map(|e| notice("aliases.error", e, danger)),
            )
            .child(if empty {
                empty_state("aliases", DOCS_URL).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .p_2()
                    .child(row_button(
                        "aliases.repoint".into(),
                        t!("aliases.repoint").to_string(),
                        &EditResource,
                        (none, locked),
                        &self.focus_handle,
                        window,
                        cx,
                    ))
                    .child(row_button(
                        "aliases.delete".into(),
                        t!("aliases.delete").to_string(),
                        &DeleteResource,
                        (none, locked),
                        &self.focus_handle,
                        window,
                        cx,
                    )),
            )
    }
}

/// Shows a failed write on the view that started it.
fn report_to(view: WeakEntity<AliasesView>) -> impl Fn(SharedString, &mut App) + 'static {
    move |text, cx| {
        view.update(cx, |view, cx| {
            view.error = Some(text);
            cx.notify();
        })
        .ok();
    }
}

/// `PUT`s the alias and compares the answer, since the server stores any target. Runs on
/// through a closed tab.
fn send_save(
    connection: Entity<ConnectionState>,
    name: String,
    target: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let save =
        cx.global::<RelevanceBackendGlobal>()
            .0
            .save_alias(client, name.clone(), target.clone());
    cx.spawn(async move |cx| {
        let saved = save.await;
        connection.update(cx, |c, cx| c.invalidate_collections(cx));
        let saved = saved.map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        if saved.collection_name != target {
            let text = t!(
                "aliases.mismatch",
                name = name,
                sent = target,
                stored = saved.collection_name
            );
            return Err(text.to_string().into());
        }
        Ok(())
    })
}

/// Deletes the alias. One that is already gone is said on the view, and the list reloads.
fn send_delete(
    view: WeakEntity<AliasesView>,
    connection: Entity<ConnectionState>,
    name: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx
        .global::<RelevanceBackendGlobal>()
        .0
        .delete_alias(client, name.clone());
    cx.spawn(async move |cx| {
        let deleted = delete.await;
        connection.update(cx, |c, cx| c.invalidate_collections(cx));
        match deleted {
            // The server says `Collection not found`, which misleads.
            Err(Error::Http { status: 404, .. }) => {
                let text = t!("aliases.gone", name = name).to_string();
                view.update(cx, |view, cx| {
                    view.error = Some(text.into());
                    cx.notify();
                })
                .ok();
                Ok(())
            }
            Err(e) => Err(error_text(&e, &profile).into()),
            Ok(()) => Ok(()),
        }
    })
}

/// New and Repoint.
pub struct AliasDialog {
    view: WeakEntity<AliasesView>,
    connection: Entity<ConnectionState>,
    name: Entity<InputState>,
    target: Entity<SelectState<Vec<String>>>,
    /// Repoint of this alias; `None` is New.
    repoint: Option<String>,
    /// A New named an existing alias: its target, for the Repoint button.
    existing: Option<String>,
    error: Option<SharedString>,
    busy: bool,
    check: Option<Task<()>>,
    cancel_focus: FocusHandle,
    _subscription: Subscription,
}

impl AliasDialog {
    /// Opens it. `repoint` holds the alias and its target; otherwise it is New.
    fn open(
        view: WeakEntity<AliasesView>,
        connection: Entity<ConnectionState>,
        repoint: Option<(String, String)>,
        window: &mut Window,
        cx: &mut App,
    ) -> WeakEntity<Self> {
        let dialog = cx.new(|cx| Self::new(view, connection, repoint, window, cx));
        let weak = dialog.downgrade();
        let title = t!(if dialog.read(cx).repoint.is_some() {
            "aliases.dialog.repoint_title"
        } else {
            "aliases.dialog.new_title"
        })
        .to_string();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |d, window, cx| {
            if first_build.replace(false) {
                let (repoint, name, target) = {
                    let d = dialog.read(cx);
                    (d.repoint.is_some(), d.name.clone(), d.target.clone())
                };
                if repoint {
                    target.update(cx, |s, cx| s.focus(window, cx));
                } else {
                    name.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
            let enter = dialog.clone();
            d.title(title.clone())
                .w(px(DIALOG_WIDTH))
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
                .child(dialog.clone())
        });
        weak
    }

    fn new(
        view: WeakEntity<AliasesView>,
        connection: Entity<ConnectionState>,
        repoint: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cache = &connection.read(cx).collection_cache;
        let names: Vec<String> = cache
            .collections
            .iter()
            .flatten()
            .map(|c| c.name.clone())
            .collect();
        let (name, old) = repoint.unzip();
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(name.clone().unwrap_or_default(), window, cx);
            input
        });
        let target = cx.new(|cx| {
            let mut select = SelectState::new(names, None, window, cx).searchable(true);
            if let Some(old) = &old {
                select.set_selected_value(old, window, cx);
            }
            select
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            },
        );
        Self {
            view,
            connection,
            name: input,
            target,
            repoint: name,
            existing: None,
            error: None,
            busy: false,
            check: None,
            cancel_focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    /// Picks `target` in the select.
    pub fn pick_target(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = target.to_string();
        self.target
            .update(cx, |s, cx| s.set_selected_value(&target, window, cx));
    }

    fn refuse(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(text.into());
        self.busy = false;
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        let Some(target) = self.target.read(cx).selected_value().cloned() else {
            return self.refuse(t!("aliases.target_required").to_string(), cx);
        };
        self.existing = None;
        if self.repoint.is_none() && !name_ok(&name) {
            return self.refuse(t!("resources.bad_name").to_string(), cx);
        }
        let Some(client) = client_of(&self.connection, cx) else {
            return self.refuse(t!("status.not_connected").to_string(), cx);
        };
        self.busy = true;
        self.error = None;
        let list = cx.global::<CollectionsBackendGlobal>().0.list(client);
        self.check = Some(cx.spawn_in(window, async move |this, cx| {
            let fresh = list.await;
            this.update_in(cx, |this, window, cx| {
                let profile = this.connection.read(cx).profile.clone();
                let fresh = match fresh {
                    Ok((collections, Ok(aliases))) => (collections, aliases),
                    Ok((_, Err(e))) | Err(e) => return this.refuse(error_text(&e, &profile), cx),
                };
                this.decide(name, target, &fresh, window, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// With the fresh list: refuses, or goes on to the confirmation and the write.
    fn decide(
        &mut self,
        name: String,
        target: String,
        (collections, aliases): &(Vec<Collection>, Vec<Alias>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if documents(collections, &target).is_none() {
            return self.refuse(t!("aliases.target_gone", name = target).to_string(), cx);
        }
        let current = aliases.iter().find(|a| a.name == name);
        if self.repoint.is_some() {
            let Some(current) = current else {
                return self.refuse(t!("aliases.gone", name = name).to_string(), cx);
            };
            if current.collection_name == target {
                return self.refuse(t!("aliases.same_target").to_string(), cx);
            }
            let old = current.collection_name.clone();
            return self.confirm_repoint(name, old, target, collections, window, cx);
        }
        if documents(collections, &name).is_some() {
            return self.refuse(t!("aliases.is_collection", name = name).to_string(), cx);
        }
        if let Some(current) = current {
            self.existing = Some(current.collection_name.clone());
            let text = t!(
                "aliases.exists",
                name = name,
                target = current.collection_name
            );
            return self.refuse(text.to_string(), cx);
        }
        if self.connection.read(cx).profile.environment.is_prod() {
            return self.confirm_new(name, target, window, cx);
        }
        let save = send_save(self.connection.clone(), name, target, cx);
        cx.spawn_in(window, async move |this, cx| {
            let saved = save.await;
            this.update_in(cx, |this, window, cx| match saved {
                Ok(()) => window.close_dialog(cx),
                Err(error) => this.refuse(error, cx),
            })
            .ok();
        })
        .detach();
    }

    fn confirm_new(
        &mut self,
        name: String,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let danger = Danger {
            title: t!("aliases.dialog.new_title").to_string(),
            lines: vec![t!("aliases.new.line", name = name, target = target).to_string()],
            expected: name.clone(),
            action: t!("resources.new").to_string(),
            typed: false,
            prod: true,
        };
        self.confirm(danger, name, target, window, cx);
    }

    fn confirm_repoint(
        &mut self,
        name: String,
        old: String,
        new: String,
        collections: &[Collection],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let line = t!(
            "aliases.repoint.line",
            alias = name,
            old = old,
            old_docs = count_text(collections, &old),
            new = new,
            new_docs = count_text(collections, &new)
        );
        let mut lines = vec![line.to_string()];
        if documents(collections, &old) != documents(collections, &new) {
            lines.push(t!("aliases.repoint.differs").to_string());
        }
        let prod = self.connection.read(cx).profile.environment.is_prod();
        let danger = Danger {
            title: t!("aliases.repoint.title", name = name).to_string(),
            lines,
            expected: name.clone(),
            action: t!("aliases.repoint").to_string(),
            typed: prod,
            prod,
        };
        self.confirm(danger, name, new, window, cx);
    }

    fn confirm(
        &self,
        danger: Danger,
        name: String,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection = self.connection.clone();
        let run = move |_: &mut Window, cx: &mut App| {
            send_save(connection.clone(), name.clone(), target.clone(), cx)
        };
        confirm_write(danger, run, report_to(self.view.clone()), window, cx);
    }

    /// Turns a New that named an existing alias into its Repoint.
    fn switch_to_repoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.existing.take() else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_string();
        window.close_dialog(cx);
        let (view, connection) = (self.view.clone(), self.connection.clone());
        let dialog = Self::open(view.clone(), connection, Some((name, target)), window, cx);
        view.update(cx, |view, _| view.dialog = Some(dialog)).ok();
    }
}

impl Render for AliasDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let repoint = self.repoint.is_some();
        v_flex()
            .key_context(ALIAS_DIALOG)
            .gap_2()
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.submit(window, cx)))
            .child(t!("aliases.dialog.name").to_string())
            .child(
                Input::new(&self.name)
                    .id("aliases.dialog.name")
                    .aria_label(t!("aliases.dialog.name").to_string())
                    .disabled(repoint),
            )
            .child(t!("aliases.dialog.target").to_string())
            .child(
                Select::new(&self.target)
                    .id("aliases.dialog.target")
                    .accessibility_label(t!("aliases.dialog.target").to_string())
                    .placeholder(t!("aliases.dialog.target").to_string()),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("aliases.dialog.error", e, danger)),
            )
            .children(self.existing.is_some().then(|| {
                Button::new("aliases.dialog.repoint")
                    .small()
                    .label(t!("aliases.repoint").to_string())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_to_repoint(window, cx);
                    }))
            }))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("aliases.dialog.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("aliases.dialog.save")
                            .primary()
                            .label(t!("aliases.dialog.save").to_string())
                            .children(Kbd::binding_for_action(
                                &SaveResource,
                                Some(ALIAS_DIALOG),
                                window,
                            ))
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
