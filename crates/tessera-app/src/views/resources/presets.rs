// SPDX-License-Identifier: GPL-3.0-or-later

//! The presets of a connection. Editing one happens in the search workbench.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*,
};
use rust_i18n::t;
use tessera_client::Preset;
use tessera_core::search::params::SearchDraft;
use tessera_core::search::presets::{PresetKind, preset_drafts, preset_kind, preset_summary};

use super::super::search::preset_error_text;
use super::name_dialog::NameDialog;
use super::{
    Danger, ListRow, ResourceColumn, ResourceList, client_of, confirm_write, empty_state,
    list_error_text, notice, row_button, toolbar,
};
use crate::actions::{
    DeleteResource, EditResource, FocusResourceFilter, NewResource, ReloadResource,
};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;
use crate::search::SearchBackendGlobal;
use crate::state::AppState;

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/search.html#presets";
/// The columns of the table.
const COLUMNS: &[ResourceColumn] = &[
    ("name", 240., false),
    ("kind", 280., false),
    ("summary", 340., false),
];

fn kind_text(kind: &PresetKind) -> String {
    match kind {
        PresetKind::Search => t!("presets.kind.search").to_string(),
        PresetKind::Multi(n) => t!("presets.kind.multi", n = n).to_string(),
        PresetKind::Union(n) => t!("presets.kind.union", n = n).to_string(),
        PresetKind::Invalid(error) => preset_error_text(error),
    }
}

/// The content of a presets tab.
pub struct PresetsView {
    connection: Entity<ConnectionState>,
    state: Entity<AppState>,
    list: Entity<ResourceList>,
    presets: Vec<Preset>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// Why the last write failed, or what it found.
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl PresetsView {
    /// The presets of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("presets", COLUMNS, window, cx));
        let mut this = Self {
            _subscription: cx.observe(&list, |_, _, cx| cx.notify()),
            connection,
            state,
            list,
            presets: Vec::new(),
            loaded: false,
            list_error: None,
            load_task: None,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx.global::<SearchBackendGlobal>().0.presets(client);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match answer {
                    Ok(presets) => {
                        this.list_error = None;
                        this.take_presets(presets, cx);
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

    fn take_presets(&mut self, mut presets: Vec<Preset>, cx: &mut Context<Self>) {
        presets.sort_by(|a, b| a.name.cmp(&b.name));
        let rows = presets.iter().map(|preset| {
            let kind = preset_kind(&preset.value);
            ListRow {
                key: preset.name.clone(),
                cells: vec![
                    preset.name.clone(),
                    kind_text(&kind),
                    preset_summary(&preset.value),
                ],
                warning: matches!(kind, PresetKind::Invalid(_)).then_some(1),
            }
        });
        let rows: Vec<ListRow> = rows.collect();
        self.presets = presets;
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    fn selected(&self, cx: &App) -> Option<&Preset> {
        let name = self.list.read(cx).selected(cx)?;
        self.presets.iter().find(|p| p.name == name)
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![
            (self.open_blocked(cx), &EditResource),
            (self.selected(cx).is_none(), &DeleteResource),
        ]
    }

    /// Nothing is selected, or it is not something a search can use.
    fn open_blocked(&self, cx: &App) -> bool {
        let kind = self.selected(cx).map(|p| preset_kind(&p.value));
        matches!(kind, None | Some(PresetKind::Invalid(_)))
    }

    /// The collection of a draft with none: the first one the connection has.
    fn first_collection(&self, cx: &App) -> String {
        let cache = &self.connection.read(cx).collection_cache;
        let first = cache.collections.iter().flatten().next();
        first.map(|c| c.name.clone()).unwrap_or_default()
    }

    /// Opens the workbench on the selected preset. Nothing is written.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if self.open_blocked(cx) {
            return;
        }
        let Some(preset) = self.selected(cx).cloned() else {
            return;
        };
        let current = self.first_collection(cx);
        if let Ok(loaded) = preset_drafts(&preset.value, &current) {
            let id = self.connection.read(cx).profile.id;
            self.state.update(cx, |s, cx| {
                s.open_search(id, Some(loaded), Some(preset.name), cx);
            });
        }
    }

    fn ask_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (view, state) = (cx.entity().downgrade(), self.state.clone());
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let check = {
                    let connection = connection.clone();
                    move |name: String, cx: &mut App| exists(&connection, name, cx)
                };
                let conn = connection.clone();
                let on_name = move |name: String, _: &mut Window, cx: &mut App| {
                    let current = view
                        .read_with(cx, |view, cx| view.first_collection(cx))
                        .unwrap_or_default();
                    let id = conn.read(cx).profile.id;
                    let draft = SearchDraft::new(&current);
                    state.update(cx, |s, cx| {
                        s.open_search(id, Some((vec![draft], false)), Some(name), cx);
                    });
                };
                let title = t!("presets.new_title").to_string();
                NameDialog::open(title, connection, check, on_name, window, cx);
            });
        });
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.selected(cx).map(|p| p.name.clone()) else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let prod = connection.read(cx).profile.environment.is_prod();
                let danger = Danger {
                    title: t!("presets.delete.title", name = name).to_string(),
                    lines: vec![t!("presets.delete.line").to_string()],
                    expected: name.clone(),
                    action: t!("presets.delete").to_string(),
                    typed: prod,
                };
                let view = this.clone();
                let conn = connection.clone();
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), conn.clone(), name.clone(), cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }
}

impl Focusable for PresetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PresetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.presets.is_empty();
        let no_row = self.selected(cx).is_none();
        let blocked = self.open_blocked(cx);
        let top = toolbar(
            "presets",
            &self.list,
            &self.focus_handle,
            locked,
            window,
            cx,
        );
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| this.ask_new(window, cx)))
            .on_action(cx.listener(|this, _: &EditResource, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.ask_delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(top)
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("presets.list_error", e, danger)),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("presets.error", e, danger)),
            )
            .child(if empty {
                empty_state("presets", DOCS_URL).into_any_element()
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
                        "presets.open".into(),
                        t!("presets.open").to_string(),
                        &EditResource,
                        (blocked, false),
                        &self.focus_handle,
                        window,
                        cx,
                    ))
                    .child(row_button(
                        "presets.delete".into(),
                        t!("presets.delete").to_string(),
                        &DeleteResource,
                        (no_row, locked),
                        &self.focus_handle,
                        window,
                        cx,
                    )),
            )
    }
}

/// Refuses a name that exists, after a fresh list.
fn exists(
    connection: &Entity<ConnectionState>,
    name: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let list = cx.global::<SearchBackendGlobal>().0.presets(client);
    cx.spawn(async move |_| {
        let presets = list.await.map_err(|e| list_error_text(&e, &profile))?;
        if presets.iter().any(|p| p.name == name) {
            return Err(t!("presets.exists", name = name).to_string().into());
        }
        Ok(())
    })
}

/// Shows a failed write on the view that started it.
fn report_to(view: WeakEntity<PresetsView>) -> impl Fn(SharedString, &mut App) + 'static {
    move |text, cx| {
        view.update(cx, |view, cx| {
            view.error = Some(text);
            cx.notify();
        })
        .ok();
    }
}

fn send_delete(
    view: WeakEntity<PresetsView>,
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
        .delete_preset(client, name);
    cx.spawn(async move |cx| {
        delete
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        view.update(cx, |view, cx| view.reload(cx)).ok();
        Ok(())
    })
}
