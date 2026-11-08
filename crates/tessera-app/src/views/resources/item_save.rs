// SPDX-License-Identifier: GPL-3.0-or-later

//! What the item dialogs of both set kinds share: the fresh check of the id, D62 and the write.

use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, Context, Entity, SharedString, Task, WeakEntity, Window};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::SetKind;
use tessera_core::relevance::changes::dropped_keys;
use tessera_widgets::confirm;

use super::sets::SetsView;
use super::{client_of, titled};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// The state of a save through the item endpoint: a row is one change and must not overwrite a
/// colleague's other rows (D60).
pub(super) struct ItemSave {
    view: WeakEntity<SetsView>,
    pub connection: Entity<ConnectionState>,
    kind: SetKind,
    set: String,
    /// The id of the item being edited.
    original: Option<String>,
    pub error: Option<SharedString>,
    pub busy: bool,
    check: Option<Task<()>>,
}

impl ItemSave {
    pub fn new(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        kind: SetKind,
        set: String,
        original: Option<String>,
    ) -> Self {
        Self {
            view,
            connection,
            kind,
            set,
            original,
            error: None,
            busy: false,
            check: None,
        }
    }

    /// Stops with `text` shown in the dialog.
    pub fn refuse<D: 'static>(&mut self, text: impl Into<SharedString>, cx: &mut Context<D>) {
        self.error = Some(text.into());
        self.busy = false;
        cx.notify();
    }
}

/// A dialog that saves one item.
pub(super) trait ItemDialog: Sized + 'static {
    /// Its state of the save.
    fn save_state(&mut self) -> &mut ItemSave;
}

/// Checks `id` against a fresh read, because a `PUT` overwrites silently, then saves `sent`,
/// after a confirmation on prod.
pub(super) fn submit_item<D: ItemDialog>(
    dialog: &mut D,
    id: String,
    sent: Value,
    window: &mut Window,
    cx: &mut Context<D>,
) {
    let save = dialog.save_state();
    let Some(client) = client_of(&save.connection, cx) else {
        return save.refuse(t!("status.not_connected").to_string(), cx);
    };
    save.busy = true;
    save.error = None;
    let fresh = cx
        .global::<RelevanceBackendGlobal>()
        .0
        .set(client, save.kind, save.set.clone());
    save.check = Some(cx.spawn_in(window, async move |this, cx| {
        let fresh = fresh.await;
        this.update_in(cx, |this, window, cx| {
            let save = this.save_state();
            let profile = save.connection.read(cx).profile.clone();
            let fresh = match fresh {
                Ok(fresh) => fresh,
                Err(e) => return save.refuse(error_text(&e, &profile), cx),
            };
            let moved = save.original.as_ref() != Some(&id);
            let taken = fresh
                .items
                .iter()
                .any(|i| i.get("id").and_then(Value::as_str) == Some(&id));
            if moved && taken {
                return save.refuse(t!("sets.item.exists", id = id).to_string(), cx);
            }
            confirm_save(this, id, sent, window, cx);
        })
        .ok();
    }));
    cx.notify();
}

fn confirm_save<D: ItemDialog>(
    dialog: &mut D,
    id: String,
    sent: Value,
    window: &mut Window,
    cx: &mut Context<D>,
) {
    let save = dialog.save_state();
    let profile = save.connection.read(cx).profile.clone();
    if !profile.environment.is_prod() {
        return send(dialog, id, sent, window, cx);
    }
    save.busy = false;
    let body = t!("sets.item.save_line", name = save.set).to_string();
    let this = cx.entity().downgrade();
    let title = t!("sets.item.save_title", id = id).to_string();
    confirm::open(
        titled(&title, &profile),
        body,
        t!("sets.save").to_string(),
        move |window, cx: &mut App| {
            this.update(cx, |this, cx| {
                send(this, id.clone(), sent.clone(), window, cx);
            })
            .ok();
        },
        window,
        cx,
    );
    cx.notify();
}

/// `save_item` new, then `delete_item` old for a changed id. Detached: closing the tab must not
/// abort a write, and above all not between the two.
fn send<D: ItemDialog>(
    dialog: &mut D,
    id: String,
    sent: Value,
    window: &mut Window,
    cx: &mut Context<D>,
) {
    let save = dialog.save_state();
    let Some(client) = client_of(&save.connection, cx) else {
        return save.refuse(t!("status.not_connected").to_string(), cx);
    };
    save.busy = true;
    let backend = cx.global::<RelevanceBackendGlobal>().0.clone();
    let (kind, set, view) = (save.kind, save.set.clone(), save.view.clone());
    let old = save.original.clone().filter(|old| *old != id);
    let profile = save.connection.read(cx).profile.clone();
    let write = backend.save_item(client.clone(), kind, set.clone(), id.clone(), sent.clone());
    cx.spawn_in(window, async move |this, cx| {
        let stored = match write.await {
            Ok(stored) => stored,
            Err(e) => {
                this.update(cx, |this, cx| {
                    this.save_state().refuse(error_text(&e, &profile), cx);
                })
                .ok();
                return;
            }
        };
        let mut failed = None;
        if let Some(old) = old {
            let delete = backend.delete_item(client, kind, set, old.clone());
            if let Err(e) = delete.await {
                let detail = error_text(&e, &profile);
                let text = t!("sets.item.both_exist", old = old, new = id, detail = detail);
                failed = Some(SharedString::from(text.to_string()));
            }
        }
        this.update_in(cx, |_, window, cx| window.close_dialog(cx))
            .ok();
        let dropped = dropped_keys(&sent, &stored);
        view.update_in(cx, |view, window, cx| {
            view.item_saved(&dropped, failed, window, cx);
        })
        .ok();
    })
    .detach();
    cx.notify();
}
