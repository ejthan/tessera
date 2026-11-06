// SPDX-License-Identifier: GPL-3.0-or-later

//! The API keys of a connection: a list and what the selected key may do.

use gpui_kit::component::{ActiveTheme as _, h_resizable, resizable_panel, v_flex};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rust_i18n::t;
use tessera_client::ApiKey;
use tessera_core::keys::{Expiry, expiry};
use tessera_core::redact;
use tessera_widgets::json_editor::JsonEditor;

use super::key_dialog::{KeyDialog, RevealDialog};
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, row_button, toolbar,
};
use crate::actions::{
    DeleteResource, FocusResourceFilter, NewResource, NextControl, PreviousControl, ReloadResource,
};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::state::AppState;

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/api-keys.html";
/// The columns of the table.
const COLUMNS: &[ResourceColumn] = &[
    ("id", 60., true),
    ("description", 200., false),
    ("prefix", 90., false),
    ("actions", 200., false),
    ("collections", 140., false),
    ("expires", 120., false),
];
/// The cell that says when a key ends.
const EXPIRES_COLUMN: usize = 5;
/// Width of the list side.
const LIST_WIDTH: f32 = 640.;

/// When a key ends, in words or as a local date.
fn expires_text(key: &ApiKey, now: Timestamp) -> String {
    let date = key
        .expires_at
        .filter(|_| expiry(key.expires_at, now) != Expiry::Never)
        .and_then(|at| Timestamp::from_second(at).ok());
    match date {
        Some(at) => at
            .to_zoned(TimeZone::system())
            .strftime("%Y-%m-%d %H:%M")
            .to_string(),
        None => t!("keys.never").to_string(),
    }
}

fn row(key: &ApiKey, now: Timestamp) -> ListRow {
    let ending = matches!(expiry(key.expires_at, now), Expiry::Expired | Expiry::Soon);
    ListRow {
        key: key.id.to_string(),
        cells: vec![
            key.id.to_string(),
            key.description.clone(),
            redact::key(key.value_prefix.as_deref().unwrap_or_default()),
            key.actions.join(", "),
            key.collections.join(", "),
            expires_text(key, now),
        ],
        warning: ending.then_some(EXPIRES_COLUMN),
    }
}

/// The content of a keys tab.
pub struct KeysView {
    connection: Entity<ConnectionState>,
    state: Entity<AppState>,
    list: Entity<ResourceList>,
    keys: Vec<ApiKey>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// The key the detail shows.
    followed: Option<u64>,
    detail: Entity<JsonEditor>,
    /// Why the last write failed.
    error: Option<SharedString>,
    /// The form opened last.
    dialog: Option<Entity<KeyDialog>>,
    /// The reveal opened last, weak: the dialog alone holds the value.
    reveal: Option<WeakEntity<RevealDialog>>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl KeysView {
    /// The keys of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("keys", COLUMNS, window, cx));
        let subscription = cx.observe_in(&list, window, |this, _, window, cx| {
            this.follow_selection(window, cx);
        });
        let mut this = Self {
            connection,
            state,
            list,
            keys: Vec::new(),
            loaded: false,
            list_error: None,
            load_task: None,
            followed: None,
            detail: cx.new(|cx| JsonEditor::new("", true, window, cx)),
            error: None,
            dialog: None,
            reveal: None,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    /// The form opened last.
    pub fn dialog(&self) -> Option<&Entity<KeyDialog>> {
        self.dialog.as_ref()
    }

    /// The reveal opened last.
    pub fn reveal(&self) -> Option<&WeakEntity<RevealDialog>> {
        self.reveal.as_ref()
    }

    pub(super) fn set_reveal(&mut self, reveal: WeakEntity<RevealDialog>) {
        self.reveal = Some(reveal);
    }

    /// The reveal of key `id` closed: shows it in the list.
    pub(super) fn created(&mut self, id: u64, cx: &mut Context<Self>) {
        self.list
            .update(cx, |l, cx| l.select(Some(id.to_string()), cx));
        self.reload(cx);
    }

    fn ask_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (connection, state) = (self.connection.clone(), self.state.clone());
        let this = cx.entity().downgrade();
        self.dialog = Some(KeyDialog::open(connection, state, this, window, cx));
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx.global::<AdminBackendGlobal>().0.keys(client);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match answer {
                    Ok(keys) => {
                        this.list_error = None;
                        this.take_keys(keys, cx);
                    }
                    Err(e) => {
                        let profile = &this.connection.read(cx).profile;
                        this.list_error = Some(list_error_text(&e, profile));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn take_keys(&mut self, mut keys: Vec<ApiKey>, cx: &mut Context<Self>) {
        keys.sort_by_key(|k| k.id);
        let now = Timestamp::now();
        let rows: Vec<ListRow> = keys.iter().map(|k| row(k, now)).collect();
        self.keys = keys;
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    fn selected(&self, cx: &App) -> Option<&ApiKey> {
        let id = self.list.read(cx).selected(cx)?;
        self.keys.iter().find(|k| k.id.to_string() == id)
    }

    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        let key = self.selected(cx).cloned();
        let id = key.as_ref().map(|k| k.id);
        if id == self.followed {
            return;
        }
        self.followed = id;
        // An `ApiKey` holds no secret.
        let text = key.and_then(|k| serde_json::to_string_pretty(&k).ok());
        self.detail.update(cx, |d, cx| {
            d.set_text(&text.unwrap_or_default(), window, cx)
        });
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected(cx).cloned() else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let prod = connection.read(cx).profile.environment.is_prod();
                let name = if key.description.is_empty() {
                    key.id.to_string()
                } else {
                    key.description.clone()
                };
                let danger = Danger {
                    title: t!("keys.delete_title", name = name).to_string(),
                    lines: vec![t!("keys.delete_consequence").to_string()],
                    expected: name,
                    action: t!("keys.delete").to_string(),
                    typed: prod,
                };
                let (view, conn, id) = (this.clone(), connection.clone(), key.id);
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), conn.clone(), id, cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![(self.selected(cx).is_none(), &DeleteResource)]
    }

    fn detail_pane(&self, cx: &App) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let Some(key) = self.selected(cx) else {
            return div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("keys.pick").to_string())
                .into_any_element();
        };
        let now = Timestamp::now();
        let state = match expiry(key.expires_at, now) {
            Expiry::Never => t!("keys.state.never"),
            Expiry::Expired => t!("keys.state.expired"),
            Expiry::Soon => t!("keys.state.soon"),
            Expiry::Later => t!("keys.state.later"),
        };
        let expires = format!("{} · {state}", expires_text(key, now));
        v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .child(notice(
                "keys.detail.actions",
                t!("keys.detail.actions", list = key.actions.join(", "))
                    .to_string()
                    .into(),
                muted,
            ))
            .child(notice(
                "keys.detail.collections",
                t!("keys.detail.collections", list = key.collections.join(", "))
                    .to_string()
                    .into(),
                muted,
            ))
            .child(notice(
                "keys.detail.expires",
                t!("keys.detail.expires", state = expires)
                    .to_string()
                    .into(),
                muted,
            ))
            .child(div().flex_1().min_h_0().child(self.detail.clone()))
            .into_any_element()
    }
}

impl Focusable for KeysView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KeysView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, border) = (theme.danger, theme.border);
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.list.read(cx).is_empty(cx);
        let none = self.selected(cx).is_none();
        let left = v_flex()
            .size_full()
            .child(toolbar(
                "keys",
                &self.list,
                &self.focus_handle,
                locked,
                window,
                cx,
            ))
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("keys.list_error", e, danger)),
            )
            .child(if empty {
                empty_state("keys", DOCS_URL).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            })
            .children(self.error.clone().map(|e| notice("keys.error", e, danger)))
            .child(div().flex_none().p_2().child(row_button(
                "keys.delete".into(),
                t!("keys.delete").to_string(),
                &DeleteResource,
                (none, locked),
                &self.focus_handle,
                window,
                cx,
            )));
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| this.ask_new(window, cx)))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.ask_delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(|_: &NextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousControl, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(
                h_resizable("keys.split")
                    .child(
                        resizable_panel()
                            .size(px(LIST_WIDTH))
                            .border_r_1()
                            .border_color(border)
                            .child(left),
                    )
                    .child(resizable_panel().child(self.detail_pane(cx))),
            )
    }
}

impl ErrorSlot for KeysView {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

fn send_delete(
    view: WeakEntity<KeysView>,
    connection: Entity<ConnectionState>,
    id: u64,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx.global::<AdminBackendGlobal>().0.delete_key(client, id);
    cx.spawn(async move |cx| {
        delete
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        view.update(cx, |view, cx| {
            view.error = None;
            view.reload(cx);
        })
        .ok();
        Ok(())
    })
}
