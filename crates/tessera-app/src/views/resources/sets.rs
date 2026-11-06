// SPDX-License-Identifier: GPL-3.0-or-later

//! The sets of a connection, of one kind: a list beside the items of the selected set.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{ItemSet, SetKind};
use tessera_core::relevance::attachment::users;
use tessera_core::relevance::changes::{ItemIssue, changes};
use tessera_core::relevance::synonym_file::{
    SET_FILE_LIMIT, SetFileError, SetFormat, parse_synonyms, synonyms_to_json,
};
use tessera_core::relevance::synonyms::SynonymDraft;
use tessera_widgets::confirm;
use tessera_widgets::json_editor::JsonEditor;

use super::super::dock::view_tab;
use super::super::env_badge;
use super::name_dialog::NameDialog;
use super::synonyms::{ITEM_COLUMNS, SynonymItemDialog, item_row};
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, toolbar,
};
use crate::actions::{
    DeleteSetItem, EditResource, FocusResourceFilter, NewResource, NewSetItem, RESOURCES,
    ReloadResource, SET_ITEMS, SaveResource, ShowItems, ShowSetJson,
};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/synonyms.html";
/// The columns of the set table.
const COLUMNS: &[ResourceColumn] = &[
    ("name", 200., false),
    ("items", 70., true),
    ("collections", 200., false),
];
/// Width of the list side.
const LIST_WIDTH: f32 = 480.;

/// What a whole-set save asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetTab {
    Items,
    Json,
}

/// What `issue` means, in words.
pub(super) fn issue_text(issue: &ItemIssue) -> String {
    match issue {
        ItemIssue::NoId => t!("sets.issue.no_id").to_string(),
        ItemIssue::DuplicateId { id, rows } => t!(
            "sets.issue.duplicate",
            id = id,
            first = rows.0,
            later = rows.1
        )
        .to_string(),
        ItemIssue::TooFewSynonyms => t!("sets.issue.too_few").to_string(),
        ItemIssue::NoSynonyms => t!("sets.issue.no_synonyms").to_string(),
        ItemIssue::BadMatch(value) => t!("sets.issue.bad_match", value = value).to_string(),
        ItemIssue::NoTrigger => t!("sets.issue.no_trigger").to_string(),
        ItemIssue::NoAction => t!("sets.issue.no_action").to_string(),
        ItemIssue::BadInclude(id) => t!("sets.issue.bad_include", id = id).to_string(),
        ItemIssue::BadPosition(n) => t!("sets.issue.bad_position", n = n).to_string(),
        ItemIssue::IncludedAndExcluded(id) => t!("sets.issue.both", id = id).to_string(),
        ItemIssue::DatesReversed => t!("sets.issue.dates").to_string(),
        ItemIssue::UnknownSortField { field, collection } => t!(
            "sets.issue.sort_field",
            field = field,
            collection = collection
        )
        .to_string(),
        ItemIssue::AtRow(row, inner) => {
            t!("sets.issue.at_row", row = row, issue = issue_text(inner)).to_string()
        }
    }
}

/// Why a file or the JSON tab is not a set.
pub(super) fn file_error_text(error: &SetFileError) -> String {
    match error {
        SetFileError::Json(detail) => t!("sets.file.json", detail = detail).to_string(),
        SetFileError::Csv { row, message } => {
            t!("sets.file.csv", row = row, detail = message).to_string()
        }
        SetFileError::Items(issues) => {
            let lines: Vec<String> = issues.iter().map(issue_text).collect();
            lines.join(" ")
        }
        SetFileError::TooLarge(size) => {
            t!("sets.file.too_large", size = size, limit = SET_FILE_LIMIT).to_string()
        }
    }
}

/// The content of a sets tab. `kind` decides the endpoints, the items do the rest.
pub struct SetsView {
    kind: SetKind,
    connection: Entity<ConnectionState>,
    list: Entity<ResourceList>,
    items: Entity<ResourceList>,
    sets: Vec<ItemSet>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// The collection list the Collections column shows.
    collections_loaded: Option<Instant>,
    /// The set on the right, which the selection follows.
    followed: Option<String>,
    tab: SetTab,
    json: Entity<JsonEditor>,
    /// What the JSON tab held when it was filled.
    json_loaded: String,
    /// A whole-set save is running.
    saving: bool,
    review_task: Option<Task<()>>,
    /// Why the last write failed, or what it left.
    error: Option<SharedString>,
    note: Option<SharedString>,
    dialog: Option<Entity<SynonymItemDialog>>,
    items_focus: FocusHandle,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl SetsView {
    /// The sets of `kind` on `connection`; loads them.
    pub fn new(
        kind: SetKind,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("synonyms", COLUMNS, window, cx));
        let items = cx.new(|cx| ResourceList::new("synonyms.items", ITEM_COLUMNS, window, cx));
        let subscriptions = [
            cx.observe_in(&list, window, |this, _, window, cx| {
                this.follow_selection(window, cx);
            }),
            cx.observe(&connection, |this, _, cx| this.take_collections(cx)),
        ];
        if connection.read(cx).collection_cache.collections.is_none() {
            connection.update(cx, |c, cx| c.load_collection_list(cx));
        }
        let mut this = Self {
            kind,
            connection,
            list,
            items,
            sets: Vec::new(),
            loaded: false,
            list_error: None,
            load_task: None,
            collections_loaded: None,
            followed: None,
            tab: SetTab::Items,
            json: cx.new(|cx| JsonEditor::new("", false, window, cx)),
            json_loaded: String::new(),
            saving: false,
            review_task: None,
            error: None,
            note: None,
            dialog: None,
            items_focus: cx.focus_handle(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this
    }

    /// The JSON tab's editor.
    pub fn json(&self) -> &Entity<JsonEditor> {
        &self.json
    }

    /// The item dialog opened last.
    pub fn dialog(&self) -> Option<&Entity<SynonymItemDialog>> {
        self.dialog.as_ref()
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx
            .global::<RelevanceBackendGlobal>()
            .0
            .sets(client, self.kind);
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = load.await;
            this.update_in(cx, |this, window, cx| {
                this.loaded = true;
                match answer {
                    Ok(mut sets) => {
                        sets.sort_by(|a, b| a.name.cmp(&b.name));
                        this.list_error = None;
                        this.sets = sets;
                        this.show_rows(cx);
                        this.show_items(window, cx);
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

    /// The Collections column follows a new collection list; the rest of a poll changes nothing.
    fn take_collections(&mut self, cx: &mut Context<Self>) {
        let loaded = self.connection.read(cx).collection_cache.loaded_at;
        if loaded != self.collections_loaded {
            self.collections_loaded = loaded;
            self.show_rows(cx);
        }
    }

    fn show_rows(&mut self, cx: &mut Context<Self>) {
        let cache = &self.connection.read(cx).collection_cache;
        let collections = cache.collections.as_deref().unwrap_or_default();
        let rows: Vec<ListRow> = self
            .sets
            .iter()
            .map(|set| ListRow {
                key: set.name.clone(),
                cells: vec![
                    set.name.clone(),
                    set.items.len().to_string(),
                    users(collections, self.kind, &set.name).join(", "),
                ],
                warning: None,
            })
            .collect();
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    fn current(&self) -> Option<&ItemSet> {
        let name = self.followed.as_ref()?;
        self.sets.iter().find(|s| &s.name == name)
    }

    /// Fills the right side from the set. Unsaved JSON stays.
    fn show_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .current()
            .map(|s| s.items.as_slice())
            .unwrap_or_default();
        let rows: Vec<ListRow> = items.iter().map(item_row).collect();
        let text = synonyms_to_json(items);
        self.items.update(cx, |l, cx| l.set_rows(rows, cx));
        if !self.json_dirty(cx) {
            self.json_loaded.clone_from(&text);
            self.json.update(cx, |e, cx| e.set_text(&text, window, cx));
        }
    }

    fn json_dirty(&self, cx: &App) -> bool {
        self.json.read(cx).text(cx) != self.json_loaded
    }

    /// The JSON tab counts as filled, so that [`Self::show_items`] replaces it.
    fn forget_json(&mut self, cx: &App) {
        self.json_loaded = self.json.read(cx).text(cx);
    }

    /// A newly selected row replaces the right side.
    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.list.read(cx).selected(cx);
        cx.notify();
        if selected.is_none() || selected == self.followed {
            return;
        }
        let follow = move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.followed.clone_from(&selected);
            this.list.update(cx, |l, cx| l.select(selected.clone(), cx));
            this.error = None;
            this.note = None;
            this.forget_json(cx);
            this.show_items(window, cx);
        };
        if !self.json_dirty(cx) {
            return follow(self, window, cx);
        }
        // Until Discard, the row stays the one on the right.
        let shown = self.followed.clone();
        self.list.update(cx, |l, cx| l.select(shown, cx));
        let view = cx.entity().downgrade();
        confirm::open(
            t!("sets.discard.title").to_string(),
            t!("sets.discard.body").to_string(),
            t!("sets.discard.confirm").to_string(),
            move |window, cx| {
                view.update(cx, |this, cx| follow(this, window, cx)).ok();
            },
            window,
            cx,
        );
    }

    fn show_tab(&mut self, tab: SetTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        match tab {
            SetTab::Items => self.items_focus.focus(window, cx),
            SetTab::Json => self.json.update(cx, |e, cx| e.focus(window, cx)),
        }
        cx.notify();
    }

    fn ask_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (this, kind) = (cx.entity().downgrade(), self.kind);
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let check = {
                    let connection = connection.clone();
                    move |name: String, cx: &mut App| exists(&connection, kind, name, cx)
                };
                let conn = connection.clone();
                let on_name = move |name: String, window: &mut Window, cx: &mut App| {
                    let view = this.clone();
                    let danger = Danger {
                        title: t!("sets.new_title").to_string(),
                        lines: vec![t!("sets.new_line", name = name).to_string()],
                        expected: name.clone(),
                        action: t!("resources.new").to_string(),
                        typed: false,
                    };
                    let run = move |window: &mut Window, cx: &mut App| {
                        send_set(view.clone(), name.clone(), Vec::new(), window, cx)
                    };
                    write_after_prod_confirm(danger, &this, &conn, run, window, cx);
                };
                let title = t!("sets.new_title").to_string();
                NameDialog::open(title, connection, check, on_name, window, cx);
            });
        });
    }

    fn open_dialog(&mut self, editing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(set) = self.followed.clone() else {
            return;
        };
        let draft = editing.then(|| self.selected_item(cx)).flatten();
        if editing && draft.is_none() {
            return;
        }
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let view = this.clone();
                let dialog = SynonymItemDialog::open(view, connection, set, draft, window, cx);
                this.update(cx, |this, _| this.dialog = Some(dialog)).ok();
            });
        });
    }

    fn selected_item(&self, cx: &App) -> Option<SynonymDraft> {
        let id = self.items.read(cx).selected(cx)?;
        let items = &self.current()?.items;
        let item = items
            .iter()
            .find(|i| i.get("id").and_then(Value::as_str) == Some(&id))?;
        SynonymDraft::from_value(item).ok()
    }

    /// After the dialog saved. `failed`: the old item of a changed id still exists.
    pub(super) fn item_saved(
        &mut self,
        dropped: &[String],
        failed: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = failed;
        self.note = (!dropped.is_empty()).then(|| {
            t!("sets.dropped", keys = dropped.join(", "))
                .to_string()
                .into()
        });
        self.reload(window, cx);
        cx.notify();
    }

    fn ask_delete_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.items.read(cx).selected(cx);
        let (Some(id), Some(set)) = (id, self.followed.clone()) else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let danger = Danger {
                    title: t!("sets.item.delete_title", id = id).to_string(),
                    lines: vec![t!("sets.item.delete_line", name = set).to_string()],
                    expected: id.clone(),
                    action: t!("sets.delete").to_string(),
                    typed: false,
                };
                let view = this.clone();
                let run = move |window: &mut Window, cx: &mut App| {
                    send_delete_item(view.clone(), set.clone(), id.clone(), window, cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }

    /// Parses the JSON tab and reads the set fresh: the review counts what the save really does.
    fn save_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.followed.clone().filter(|_| !self.saving);
        let (Some(name), SetTab::Json) = (name, self.tab) else {
            return;
        };
        self.note = None;
        let items = match parse_synonyms(&self.json.read(cx).text(cx), SetFormat::Json) {
            Ok(items) => items,
            Err(error) => {
                self.error = Some(file_error_text(&error).into());
                return cx.notify();
            }
        };
        self.error = None;
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        self.saving = true;
        let fresh = cx
            .global::<RelevanceBackendGlobal>()
            .0
            .set(client, self.kind, name);
        self.review_task = Some(cx.spawn_in(window, async move |this, cx| {
            let fresh = fresh.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match fresh {
                    Ok(stored) => this.review(stored, items, window, cx),
                    Err(e) => {
                        let profile = &this.connection.read(cx).profile;
                        this.error = Some(error_text(&e, profile).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// D62: a save that removes items asks, by name on prod; on prod any save asks.
    fn review(
        &mut self,
        stored: ItemSet,
        items: Vec<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let diff = changes(self.kind, &stored.items, &items);
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let name = stored.name;
                let removes = !diff.removed.is_empty();
                let prod = connection.read(cx).profile.environment.is_prod();
                let danger = Danger {
                    title: t!("sets.json.save_title", name = name).to_string(),
                    lines: vec![
                        t!(
                            "sets.json.review",
                            added = diff.added.len(),
                            changed = diff.changed.len(),
                            removed = diff.removed.len()
                        )
                        .to_string(),
                    ],
                    expected: name.clone(),
                    action: t!("sets.save").to_string(),
                    typed: removes && prod,
                };
                let view = this.clone();
                let run = move |window: &mut Window, cx: &mut App| {
                    send_set(view.clone(), name.clone(), items.clone(), window, cx)
                };
                if removes {
                    confirm_write(danger, &connection, run, report_to(this), window, cx);
                } else {
                    write_after_prod_confirm(danger, &this, &connection, run, window, cx);
                }
            });
        });
    }

    /// `stored` is what the server holds now.
    fn take_stored(&mut self, stored: ItemSet, window: &mut Window, cx: &mut Context<Self>) {
        let name = stored.name.clone();
        self.sets.retain(|s| s.name != name);
        self.sets.push(stored);
        self.sets.sort_by(|a, b| a.name.cmp(&b.name));
        self.show_rows(cx);
        if self.followed.as_ref() == Some(&name) {
            self.forget_json(cx);
            self.show_items(window, cx);
        } else {
            self.list.update(cx, |l, cx| l.select(Some(name), cx));
        }
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        let no_set = self.current().is_none();
        let no_item = self.items.read(cx).selected(cx).is_none();
        vec![
            (no_set, &NewSetItem),
            (no_item, &EditResource),
            (no_item, &DeleteSetItem),
            (no_set || self.tab != SetTab::Json, &SaveResource),
        ]
    }

    fn items_tab(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locked = self.connection.read(cx).writes_locked();
        let none = self.items.read(cx).selected(cx).is_none();
        let button = |id: &'static str, label: String, action: &'static dyn Action, off: bool| {
            let focus = self.items_focus.clone();
            Button::new(id)
                .small()
                .label(label)
                .when(locked, |b| b.child(env_badge::lock(id, false, cx)))
                .children(Kbd::binding_for_action(action, Some(SET_ITEMS), window))
                .disabled(off)
                .on_click(move |_, window, cx| focus.dispatch_action(action, window, cx))
        };
        v_flex()
            .key_context(SET_ITEMS)
            .track_focus(&self.items_focus)
            .size_full()
            .child(div().flex_1().min_h_0().child(self.items.clone()))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .p_2()
                    .child(button(
                        "sets.items.add",
                        t!("sets.items.add").to_string(),
                        &NewSetItem,
                        false,
                    ))
                    .child(button(
                        "sets.items.edit",
                        t!("sets.items.edit").to_string(),
                        &EditResource,
                        none,
                    ))
                    .child(button(
                        "sets.items.delete",
                        t!("sets.delete").to_string(),
                        &DeleteSetItem,
                        none,
                    )),
            )
    }

    fn json_tab(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locked = self.connection.read(cx).writes_locked();
        v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().child(self.json.clone()))
            .child(
                h_flex().flex_none().p_2().justify_end().child(
                    Button::new("sets.json.save")
                        .small()
                        .label(t!("sets.save").to_string())
                        .loading(self.saving)
                        .disabled(self.saving)
                        .when(locked, |b| {
                            b.child(env_badge::lock("sets.json.save", false, cx))
                        })
                        .children(Kbd::binding_for_action(
                            &SaveResource,
                            Some(RESOURCES),
                            window,
                        ))
                        .on_click(cx.listener(|this, _, window, cx| this.save_json(window, cx))),
                ),
            )
    }

    fn right_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let Some(name) = self.current().map(|s| s.name.clone()) else {
            return div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("sets.pick").to_string())
                .into_any_element();
        };
        let tabs = TabBar::new("sets.tabs")
            .underline()
            .selected_index(usize::from(self.tab == SetTab::Json))
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                let tab = if *ix == 0 {
                    SetTab::Items
                } else {
                    SetTab::Json
                };
                this.show_tab(tab, window, cx);
            }))
            .child(view_tab(
                "sets.tab.items",
                t!("sets.tab.items").to_string().into(),
                &ShowItems,
                RESOURCES,
                window,
            ))
            .child(view_tab(
                "sets.tab.json",
                t!("sets.tab.json").to_string().into(),
                &ShowSetJson,
                RESOURCES,
                window,
            ));
        let content = match self.tab {
            SetTab::Items => self.items_tab(window, cx).into_any_element(),
            SetTab::Json => self.json_tab(window, cx).into_any_element(),
        };
        v_flex()
            .size_full()
            .child(
                div()
                    .id("sets.current")
                    .test_support()
                    .aria_label(SharedString::from(name.clone()))
                    .flex_none()
                    .p_2()
                    .child(name),
            )
            .child(tabs)
            .children(self.error.clone().map(|e| notice("sets.error", e, danger)))
            .children(self.note.clone().map(|e| notice("sets.note", e, muted)))
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}

impl Focusable for SetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ErrorSlot for SetsView {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

impl Render for SetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, border) = (theme.danger, theme.border);
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.sets.is_empty();
        let right = self.right_pane(window, cx);
        let left = v_flex()
            .size_full()
            .child(toolbar(
                "synonyms",
                &self.list,
                &self.focus_handle,
                locked,
                window,
                cx,
            ))
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("sets.list_error", e, danger)),
            )
            .child(if empty {
                empty_state("synonyms", DOCS_URL).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            });
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| this.ask_new(window, cx)))
            .on_action(cx.listener(|this, _: &NewSetItem, window, cx| {
                this.open_dialog(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &EditResource, window, cx| {
                this.open_dialog(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteSetItem, window, cx| {
                this.ask_delete_item(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| {
                this.save_json(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowItems, window, cx| {
                this.show_tab(SetTab::Items, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSetJson, window, cx| {
                this.show_tab(SetTab::Json, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, window, cx| {
                this.reload(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(
                h_resizable("sets.split")
                    .child(
                        resizable_panel()
                            .size(px(LIST_WIDTH))
                            .border_r_1()
                            .border_color(border)
                            .child(left),
                    )
                    .child(resizable_panel().child(right)),
            )
    }
}

/// Runs `run` at once, or on prod after a plain confirmation (D62).
fn write_after_prod_confirm(
    danger: Danger,
    view: &WeakEntity<SetsView>,
    connection: &Entity<ConnectionState>,
    run: impl Fn(&mut Window, &mut App) -> Task<Result<(), SharedString>> + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let report = report_to(view.clone());
    if connection.read(cx).profile.environment.is_prod() {
        return confirm_write(danger, connection, run, report, window, cx);
    }
    let task = run(window, cx);
    cx.spawn(async move |cx| {
        if let Err(text) = task.await {
            cx.update(|cx| report(text, cx));
        }
    })
    .detach();
}

/// Refuses a name that exists, after a fresh list.
fn exists(
    connection: &Entity<ConnectionState>,
    kind: SetKind,
    name: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let list = cx.global::<RelevanceBackendGlobal>().0.sets(client, kind);
    cx.spawn(async move |_| {
        let sets = list.await.map_err(|e| list_error_text(&e, &profile))?;
        match sets.iter().find(|s| s.name == name) {
            Some(set) => Err(t!("sets.exists", name = name, n = set.items.len())
                .to_string()
                .into()),
            None => Ok(()),
        }
    })
}

/// `PUT`s the whole set and compares the count read back: the server keeps the last of equal
/// ids without a word. Runs on through a closed tab.
fn send_set(
    view: WeakEntity<SetsView>,
    name: String,
    items: Vec<Value>,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some((connection, kind)) = view
        .read_with(cx, |v, _| (v.connection.clone(), v.kind))
        .ok()
    else {
        return Task::ready(Ok(()));
    };
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let sent = items.len();
    let save = cx
        .global::<RelevanceBackendGlobal>()
        .0
        .save_set(client, kind, name, items);
    window.spawn(cx, async move |cx| {
        let stored = save
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        let kept = stored.items.len();
        view.update_in(cx, |view, window, cx| {
            view.take_stored(stored, window, cx);
            cx.notify();
        })
        .ok();
        if kept != sent {
            return Err(t!("sets.count_differs", sent = sent, stored = kept)
                .to_string()
                .into());
        }
        Ok(())
    })
}

fn send_delete_item(
    view: WeakEntity<SetsView>,
    set: String,
    id: String,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some((connection, kind)) = view
        .read_with(cx, |v, _| (v.connection.clone(), v.kind))
        .ok()
    else {
        return Task::ready(Ok(()));
    };
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx
        .global::<RelevanceBackendGlobal>()
        .0
        .delete_item(client, kind, set, id);
    window.spawn(cx, async move |cx| {
        delete
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        view.update_in(cx, |view, window, cx| view.reload(window, cx))
            .ok();
        Ok(())
    })
}
