// SPDX-License-Identifier: GPL-3.0-or-later

//! Which collections use a set: the Collections tab, missing sets and deleting a set.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Task,
    WeakEntity, Window, div, prelude::*,
};
use rust_i18n::t;
use tessera_client::{Collection, Error, SetKind};
use tessera_core::profile::Profile;
use tessera_core::relevance::attachment::{Missing, attached, missing, users};

use super::super::env_badge;
use super::sets::{SetsView, write_after_prod_confirm};
use super::{Danger, client_of, confirm_write, notice, report_to};
use crate::actions::{CreateMissingSet, DetachMissing, RESOURCES};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::relevance::{AttachError, AttachOutcome, RelevanceBackendGlobal};

fn attach_error_text(error: &AttachError, profile: &Profile) -> String {
    match error {
        AttachError::Server(e) => error_text(e, profile),
        AttachError::NoKnownSets(_) => t!("sets.attach.no_known").to_string(),
    }
}

/// What an answer has to say beyond the box: names that went, and a list unlike the one sent.
fn outcome_text(outcome: &AttachOutcome) -> (Option<SharedString>, Option<SharedString>) {
    let note = (!outcome.dropped.is_empty()).then(|| {
        t!("sets.attach.dropped", names = outcome.dropped.join(", "))
            .to_string()
            .into()
    });
    let error = outcome.mismatch.then(|| {
        t!("sets.attach.mismatch", list = outcome.list.join(", "))
            .to_string()
            .into()
    });
    (note, error)
}

fn missing_text(kind: SetKind, row: &Missing) -> String {
    let collections = row.collections.join(", ");
    match kind {
        SetKind::Synonym => t!(
            "sets.missing.synonym",
            set = row.set,
            collections = collections
        )
        .to_string(),
        SetKind::Curation => t!(
            "sets.missing.curation",
            set = row.set,
            collections = collections
        )
        .to_string(),
    }
}

fn none_if_empty(done: &[String]) -> String {
    if done.is_empty() {
        t!("sets.delete.none").to_string()
    } else {
        done.join(", ")
    }
}

impl SetsView {
    /// The sets collections name and the server lacks. Empty until both lists are in: a list
    /// that failed says nothing about what exists.
    pub(super) fn missing_sets(&self) -> Vec<Missing> {
        if !self.loaded || self.list_error.is_some() {
            return Vec::new();
        }
        let known: Vec<String> = self.sets.iter().map(|s| s.name.clone()).collect();
        missing(&self.collections, self.kind, &known)
    }

    /// Whether the box of `collection` is ticked for the open set.
    pub fn box_ticked(&self, collection: &str) -> bool {
        let found = self.collections.iter().find(|c| c.name == collection);
        let set = self.followed.as_ref();
        found.is_some_and(|c| set.is_some_and(|s| attached(c, self.kind).contains(s)))
    }

    /// The Collections tab of set `name`. The boxes show the server's list, not the click.
    pub(super) fn attachment_tab(
        &self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let locked = self.connection.read(cx).writes_locked();
        let needle = self.attach_filter.read(cx).value().to_lowercase();
        let holds = |c: &Collection| attached(c, self.kind).iter().any(|n| n == name);
        let shown = self.collections.iter();
        let mut shown: Vec<&Collection> = shown
            .filter(|c| c.name.to_lowercase().contains(&needle))
            .collect();
        shown.sort_by_key(|c| !holds(c));
        // shortcut: every collection is one element, use a virtual list if thousands get slow
        let boxes = shown.into_iter().map(|collection| {
            let checked = holds(collection);
            let target = collection.name.clone();
            Checkbox::new(SharedString::from(format!("sets.attach.{target}")))
                .checked(checked)
                .label(target.clone())
                .disabled(self.pending.is_some())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.ask_attach(target.clone(), !checked, window, cx);
                }))
        });
        v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.attach_filter)
                                .id("sets.attach.filter")
                                .aria_label(t!("sets.attach.filter").to_string()),
                        ),
                    )
                    .when(locked, |el| {
                        el.child(env_badge::lock("sets.attach", false, cx))
                    }),
            )
            .child(
                v_flex()
                    .id("sets.attach.list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .children(boxes),
            )
    }

    fn ask_attach(
        &mut self,
        collection: String,
        attach: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(set), None) = (self.followed.clone(), &self.pending) else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let key = if attach { "sets.attach" } else { "sets.detach" };
                let danger = Danger {
                    title: t!(format!("{key}.title"), name = set).to_string(),
                    lines: vec![t!(format!("{key}.line"), collection = collection).to_string()],
                    expected: set.clone(),
                    action: t!(key).to_string(),
                    typed: false,
                };
                let view = this.clone();
                let run = move |window: &mut Window, cx: &mut App| {
                    let (collection, set) = (collection.clone(), set.clone());
                    send_attach(view.clone(), collection, set, attach, window, cx)
                };
                write_after_prod_confirm(danger, &this, &connection, run, window, cx);
            });
        });
    }

    /// The rows above the set table. The first one carries the shortcuts.
    pub(super) fn missing_rows(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let danger = cx.theme().danger;
        let rows = self.missing_sets().into_iter().enumerate().map(|(i, row)| {
            let set = row.set.clone();
            let text = SharedString::from(missing_text(self.kind, &row));
            let (detach, create) = (set.clone(), set.clone());
            let shortcut = |action: &dyn gpui_kit::Action| {
                (i == 0)
                    .then(|| Kbd::binding_for_action(action, Some(RESOURCES), window))
                    .flatten()
            };
            v_flex()
                .flex_none()
                .child(notice(format!("sets.missing.{set}"), text, danger))
                .child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .child(
                            Button::new(SharedString::from(format!("sets.missing.detach.{set}")))
                                .small()
                                .danger()
                                .label(t!("sets.detach").to_string())
                                .children(shortcut(&DetachMissing))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.ask_detach_missing(detach.clone(), window, cx);
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("sets.missing.new.{set}")))
                                .small()
                                .label(t!("resources.new").to_string())
                                .children(shortcut(&CreateMissingSet))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.ask_new(Some(create.clone()), window, cx);
                                })),
                        ),
                )
        });
        rows.collect()
    }

    /// Detaches missing `set` from every collection that names it.
    pub(super) fn ask_detach_missing(
        &mut self,
        set: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = self.missing_sets().into_iter().find(|m| m.set == set);
        let Some(row) = row else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let danger = Danger {
                    title: t!("sets.detach.title", name = set).to_string(),
                    lines: vec![
                        t!("sets.detach.line", collection = row.collections.join(", ")).to_string(),
                    ],
                    expected: set.clone(),
                    action: t!("sets.detach").to_string(),
                    typed: false,
                };
                let view = this.clone();
                let run = move |window: &mut Window, cx: &mut App| {
                    send_detach_all(view.clone(), row.clone(), window, cx)
                };
                write_after_prod_confirm(danger, &this, &connection, run, window, cx);
            });
        });
    }

    /// Reads collections and the set fresh, then asks (D58, D62).
    pub(super) fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.list.read(cx).selected(cx) else {
            return;
        };
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        self.error = None;
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        let kind = self.kind;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let list = cx
                    .global::<CollectionsBackendGlobal>()
                    .0
                    .list(client.clone());
                let set = cx
                    .global::<RelevanceBackendGlobal>()
                    .0
                    .set(client, kind, name);
                let profile = connection.read(cx).profile.clone();
                let view = this.clone();
                window
                    .spawn(cx, async move |cx| {
                        let (list, set) = (list.await, set.await);
                        view.update_in(cx, |view, window, cx| match (list, set) {
                            (Ok((collections, _)), Ok(set)) => {
                                view.confirm_delete(set.name, &collections, window, cx);
                            }
                            (Err(e), _) | (_, Err(e)) => {
                                view.error = Some(error_text(&e, &profile).into());
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .detach();
            });
        });
    }

    fn confirm_delete(
        &mut self,
        name: String,
        collections: &[Collection],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let using = users(collections, self.kind, &name);
        let prod = self.connection.read(cx).profile.environment.is_prod();
        let line = if using.is_empty() {
            t!("sets.delete.line", name = name)
        } else {
            let collections = using.join(", ");
            match self.kind {
                SetKind::Synonym => t!("sets.delete.users.synonym", collections = collections),
                SetKind::Curation => t!("sets.delete.users.curation", collections = collections),
            }
        };
        let danger = Danger {
            title: t!("sets.delete.title", name = name).to_string(),
            lines: vec![line.to_string()],
            expected: name.clone(),
            action: t!("sets.delete").to_string(),
            typed: prod || !using.is_empty(),
        };
        let this = cx.entity().downgrade();
        let run = {
            let view = this.clone();
            move |window: &mut Window, cx: &mut App| {
                send_delete_set(view.clone(), name.clone(), window, cx)
            }
        };
        confirm_write(danger, &self.connection, run, report_to(this), window, cx);
    }
}

/// The connection, kind and client of `view`, or what to say instead.
fn target(
    view: &WeakEntity<SetsView>,
    cx: &App,
) -> Result<Option<(Entity<ConnectionState>, SetKind, tessera_client::Client)>, SharedString> {
    let Ok((connection, kind)) = view.read_with(cx, |v, _| (v.connection.clone(), v.kind)) else {
        return Ok(None);
    };
    match client_of(&connection, cx) {
        Some(client) => Ok(Some((connection, kind, client))),
        None => Err(t!("status.not_connected").to_string().into()),
    }
}

/// One toggle. Runs on through a closed tab.
fn send_attach(
    view: WeakEntity<SetsView>,
    collection: String,
    set: String,
    attach: bool,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let (connection, kind, client) = match target(&view, cx) {
        Ok(Some(target)) => target,
        Ok(None) => return Task::ready(Ok(())),
        Err(text) => return Task::ready(Err(text)),
    };
    let profile = connection.read(cx).profile.clone();
    let call = cx.global::<RelevanceBackendGlobal>().0.attach(
        client,
        kind,
        collection.clone(),
        set,
        attach,
    );
    view.update(cx, |v, cx| {
        v.pending = Some(collection);
        v.error = None;
        v.note = None;
        cx.notify();
    })
    .ok();
    window.spawn(cx, async move |cx| {
        let answer = call.await;
        connection.update(cx, |c, cx| c.invalidate_collections(cx));
        view.update(cx, |v, cx| {
            v.pending = None;
            match answer {
                Ok(outcome) => (v.note, v.error) = outcome_text(&outcome),
                Err(e) => v.error = Some(attach_error_text(&e, &profile).into()),
            }
            cx.notify();
        })
        .ok();
        Ok(())
    })
}

/// Detaches a missing set, in order, stopping at the first error.
fn send_detach_all(
    view: WeakEntity<SetsView>,
    row: Missing,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let (connection, kind, client) = match target(&view, cx) {
        Ok(Some(target)) => target,
        Ok(None) => return Task::ready(Ok(())),
        Err(text) => return Task::ready(Err(text)),
    };
    let profile = connection.read(cx).profile.clone();
    let backend = cx.global::<RelevanceBackendGlobal>().0.clone();
    window.spawn(cx, async move |cx| {
        let mut done = Vec::new();
        let mut stopped = None;
        for collection in row.collections {
            let detach = backend.attach(
                client.clone(),
                kind,
                collection.clone(),
                row.set.clone(),
                false,
            );
            match detach.await {
                Ok(_) => done.push(collection),
                Err(e) => {
                    stopped = Some((collection, attach_error_text(&e, &profile)));
                    break;
                }
            }
        }
        connection.update(cx, |c, cx| c.invalidate_collections(cx));
        match stopped {
            Some((collection, error)) => Err(t!(
                "sets.detach.stopped",
                collection = collection,
                error = error,
                done = none_if_empty(&done)
            )
            .to_string()
            .into()),
            None => Ok(()),
        }
    })
}

/// Reads who uses the set, detaches it from each, deletes it, and checks that nobody names it
/// any more. A set that stays attached fails every search, so a failed detach stops everything.
fn send_delete_set(
    view: WeakEntity<SetsView>,
    name: String,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let (connection, kind, client) = match target(&view, cx) {
        Ok(Some(target)) => target,
        Ok(None) => return Task::ready(Ok(())),
        Err(text) => return Task::ready(Err(text)),
    };
    let profile = connection.read(cx).profile.clone();
    let relevance = cx.global::<RelevanceBackendGlobal>().0.clone();
    let collections = cx.global::<CollectionsBackendGlobal>().0.clone();
    window.spawn(cx, async move |cx| {
        let text = |e: &Error| error_text(e, &profile);
        let fresh = collections.list(client.clone()).await;
        let (fresh, _) = fresh.map_err(|e| SharedString::from(text(&e)))?;
        let using: Vec<String> = users(&fresh, kind, &name)
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut done = Vec::new();
        for collection in using {
            let detach = relevance.attach(
                client.clone(),
                kind,
                collection.clone(),
                name.clone(),
                false,
            );
            if let Err(e) = detach.await {
                connection.update(cx, |c, cx| c.invalidate_collections(cx));
                return Err(t!(
                    "sets.delete.stopped",
                    collection = collection,
                    error = attach_error_text(&e, &profile),
                    done = none_if_empty(&done)
                )
                .to_string()
                .into());
            }
            done.push(collection);
        }
        let deleted = relevance
            .delete_set(client.clone(), kind, name.clone())
            .await;
        connection.update(cx, |c, cx| c.invalidate_collections(cx));
        if let Err(e) = deleted {
            return Err(t!(
                "sets.delete.failed",
                error = text(&e),
                done = none_if_empty(&done)
            )
            .to_string()
            .into());
        }
        view.update_in(cx, |view, window, cx| view.set_deleted(&name, window, cx))
            .ok();
        let after = collections.list(client).await;
        let (after, _) = after.map_err(|e| SharedString::from(text(&e)))?;
        let left = users(&after, kind, &name);
        if !left.is_empty() {
            return Err(
                t!("sets.delete.still_listed", collections = left.join(", "))
                    .to_string()
                    .into(),
            );
        }
        Ok(())
    })
}
