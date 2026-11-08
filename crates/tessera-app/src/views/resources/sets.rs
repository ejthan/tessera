// SPDX-License-Identifier: GPL-3.0-or-later

//! The sets of a connection, of one kind: a list beside the items of the selected set.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{InputEvent, InputState};
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
use tessera_client::{Collection, ItemSet, SetKind};
use tessera_core::relevance::attachment::users;
use tessera_core::relevance::changes::{ItemIssue, changes, dropped_by_id};
use tessera_core::relevance::curations::CurationDraft;
use tessera_core::relevance::synonym_file::{
    SET_FILE_LIMIT, SetFileError, parse_edited, synonyms_to_json,
};
use tessera_core::relevance::synonyms::SynonymDraft;
use tessera_widgets::confirm;
use tessera_widgets::json_editor::JsonEditor;

use super::super::dock::view_tab;
use super::super::env_badge;
use super::curation_dialog::CurationItemDialog;
use super::curations;
use super::name_dialog::NameDialog;
use super::synonyms::{self, SynonymItemDialog};
use super::test_panel::TestPanel;
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, row_button, toolbar,
};
use crate::actions::{
    CreateMissingSet, DeleteResource, DeleteSetItem, DetachMissing, EditResource, ExportResource,
    FocusResourceFilter, ImportResource, NewResource, NewSetItem, RESOURCES, ReloadResource,
    RunTest, SET_ITEMS, SaveResource, ShowAttachment, ShowItems, ShowSetJson, ShowTest,
};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// Where the empty state points to.
const fn docs_url(kind: SetKind) -> &'static str {
    match kind {
        SetKind::Synonym => "https://typesense.org/docs/30.2/api/synonyms.html",
        SetKind::Curation => "https://typesense.org/docs/30.2/api/curation.html",
    }
}

/// The start of the locale keys and test ids of `kind`, and those of its items.
const fn prefixes(kind: SetKind) -> (&'static str, &'static str) {
    match kind {
        SetKind::Synonym => ("synonyms", "synonyms.items"),
        SetKind::Curation => ("curations", "curations.items"),
    }
}
/// The columns of the set table.
const COLUMNS: &[ResourceColumn] = &[
    ("name", 200., false),
    ("items", 70., true),
    ("collections", 200., false),
];
/// Width of the list side.
const LIST_WIDTH: f32 = 480.;

/// The tabs of the right side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetTab {
    Items,
    Json,
    Collections,
    Test,
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
        ItemIssue::DuplicatePosition(n) => t!("sets.issue.duplicate_position", n = n).to_string(),
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
        SetFileError::Read(detail) => t!("sets.file.read", detail = detail).to_string(),
        SetFileError::TooLarge(size) => {
            t!("sets.file.too_large", size = size, limit = SET_FILE_LIMIT).to_string()
        }
    }
}

/// What the server did not keep, as a note.
fn dropped_note(keys: &[String]) -> Option<SharedString> {
    (!keys.is_empty()).then(|| {
        t!("sets.dropped", keys = keys.join(", "))
            .to_string()
            .into()
    })
}

/// The content of a sets tab. `kind` decides the endpoints, the items do the rest.
pub struct SetsView {
    pub(super) kind: SetKind,
    pub(super) connection: Entity<ConnectionState>,
    pub(super) list: Entity<ResourceList>,
    items: Entity<ResourceList>,
    pub(super) sets: Vec<ItemSet>,
    /// The first answer arrived.
    pub(super) loaded: bool,
    pub(super) list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// The last collection list. An invalidated one stays until the next arrives.
    pub(super) collections: Vec<Collection>,
    collections_loaded: Option<Instant>,
    /// Why the last load of the collection list failed.
    collections_error: Option<SharedString>,
    /// The set on the right, which the selection follows.
    pub(super) followed: Option<String>,
    tab: SetTab,
    pub(super) attach_filter: Entity<InputState>,
    /// The collection a toggle is running for, or the set a delete or a detach-all.
    pub(super) pending: Option<String>,
    json: Entity<JsonEditor>,
    /// What the JSON tab held when it was filled.
    json_loaded: String,
    /// A whole-set save is running.
    saving: bool,
    review_task: Option<Task<()>>,
    /// Why the last write failed, or what it left.
    pub(super) error: Option<SharedString>,
    pub(super) note: Option<SharedString>,
    dialog: Option<Entity<SynonymItemDialog>>,
    curation_dialog: Option<Entity<CurationItemDialog>>,
    test: Entity<TestPanel>,
    items_focus: FocusHandle,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 3],
}

impl SetsView {
    /// The sets of `kind` on `connection`; loads them.
    pub fn new(
        kind: SetKind,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (prefix, items_prefix) = prefixes(kind);
        let columns = match kind {
            SetKind::Synonym => synonyms::ITEM_COLUMNS,
            SetKind::Curation => curations::ITEM_COLUMNS,
        };
        let list = cx.new(|cx| ResourceList::new(prefix, COLUMNS, window, cx));
        let items = cx.new(|cx| ResourceList::new(items_prefix, columns, window, cx));
        let attach_filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("sets.attach.filter").to_string())
        });
        let test = {
            let connection = connection.clone();
            cx.new(|cx| TestPanel::new(kind, connection, window, cx))
        };
        let subscriptions = [
            cx.subscribe(&attach_filter, |_, _, _: &InputEvent, cx| cx.notify()),
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
            collections: Vec::new(),
            collections_loaded: None,
            collections_error: None,
            followed: None,
            tab: SetTab::Items,
            attach_filter,
            pending: None,
            json: cx.new(|cx| JsonEditor::new("", false, window, cx)),
            json_loaded: String::new(),
            saving: false,
            review_task: None,
            error: None,
            note: None,
            dialog: None,
            curation_dialog: None,
            test,
            items_focus: cx.focus_handle(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this.take_collections(cx);
        this
    }

    /// Why the collection list failed to load, for the Collections tab.
    pub(super) fn collections_error(&self) -> Option<SharedString> {
        self.collections_error.clone()
    }

    /// The JSON tab's editor.
    pub fn json(&self) -> &Entity<JsonEditor> {
        &self.json
    }

    /// The curation item dialog opened last.
    pub fn curation_dialog(&self) -> Option<&Entity<CurationItemDialog>> {
        self.curation_dialog.as_ref()
    }

    /// The Test tab's panel.
    pub fn test_panel(&self) -> &Entity<TestPanel> {
        &self.test
    }

    /// The item dialog opened last.
    pub fn dialog(&self) -> Option<&Entity<SynonymItemDialog>> {
        self.dialog.as_ref()
    }

    pub(super) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let cache = &self.connection.read(cx).collection_cache;
        let (loaded, collections) = (cache.loaded_at, cache.collections.clone());
        let error = cache.error.clone();
        if error != self.collections_error {
            self.collections_error = error;
            cx.notify();
        }
        if let (true, Some(collections)) = (loaded != self.collections_loaded, collections) {
            self.collections_loaded = loaded;
            self.collections = collections;
            self.show_rows(cx);
            cx.notify();
        }
    }

    fn show_rows(&mut self, cx: &mut Context<Self>) {
        let rows: Vec<ListRow> = self
            .sets
            .iter()
            .map(|set| ListRow {
                key: set.name.clone(),
                cells: vec![
                    set.name.clone(),
                    set.items.len().to_string(),
                    users(&self.collections, self.kind, &set.name).join(", "),
                ],
                warning: None,
            })
            .collect();
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    pub(super) fn current(&self) -> Option<&ItemSet> {
        let name = self.followed.as_ref()?;
        self.sets.iter().find(|s| &s.name == name)
    }

    /// Fills the right side from the set. Unsaved JSON stays.
    fn show_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .current()
            .map(|s| s.items.as_slice())
            .unwrap_or_default();
        let rows: Vec<ListRow> = match self.kind {
            SetKind::Synonym => items.iter().map(synonyms::item_row).collect(),
            SetKind::Curation => {
                let now = jiff::Timestamp::now().as_second();
                items.iter().map(|i| curations::item_row(i, now)).collect()
            }
        };
        let text = synonyms_to_json(items);
        self.items.update(cx, |l, cx| l.set_rows(rows, cx));
        let set = self.current().map(|s| s.name.clone());
        self.test.update(cx, |t, cx| t.follow(set, window, cx));
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
            SetTab::Collections => {}
            SetTab::Test => self.test.update(cx, |t, cx| t.shown(window, cx)),
        }
        cx.notify();
    }

    /// Asks for a name, with `prefill` in the input, and creates the empty set.
    pub(super) fn ask_new(
        &mut self,
        prefill: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                        send_set(view.clone(), name.clone(), Vec::new(), None, window, cx)
                    };
                    write_after_prod_confirm(danger, &this, &conn, run, window, cx);
                };
                let title = t!("sets.new_title").to_string();
                let prefill = prefill.unwrap_or_default();
                NameDialog::prefilled(prefill, title, connection, check, on_name, window, cx);
            });
        });
    }

    fn open_dialog(&mut self, editing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(set) = self.followed.clone() else {
            return;
        };
        let item = editing.then(|| self.selected_item(cx)).flatten();
        self.error = None;
        if editing && item.is_none() {
            let id = self.items.read(cx).selected(cx);
            self.error = id.map(|id| t!("sets.item.unreadable", id = id).to_string().into());
            return cx.notify();
        }
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        let kind = self.kind;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let view = this.clone();
                match kind {
                    SetKind::Synonym => {
                        let draft = item.and_then(|i| SynonymDraft::from_value(&i).ok());
                        let dialog =
                            SynonymItemDialog::open(view, connection, set, draft, window, cx);
                        this.update(cx, |this, _| this.dialog = Some(dialog)).ok();
                    }
                    SetKind::Curation => {
                        let draft = item.and_then(|i| CurationDraft::from_value(&i).ok());
                        let dialog =
                            CurationItemDialog::open(view, connection, set, draft, window, cx);
                        this.update(cx, |this, _| this.curation_dialog = Some(dialog))
                            .ok();
                    }
                }
            });
        });
    }

    /// The stored item of the selected row, when the form can read it.
    fn selected_item(&self, cx: &App) -> Option<Value> {
        let id = self.items.read(cx).selected(cx)?;
        let items = &self.current()?.items;
        let item = items
            .iter()
            .find(|i| i.get("id").and_then(Value::as_str) == Some(&id))?;
        let readable = match self.kind {
            SetKind::Synonym => SynonymDraft::from_value(item).is_ok(),
            SetKind::Curation => CurationDraft::from_value(item).is_ok(),
        };
        readable.then(|| item.clone())
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
        self.note = dropped_note(dropped);
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

    /// Reads the set fresh, then parses the JSON tab against it: the review counts what the save
    /// really does, and a row nobody touched is no reason to refuse.
    fn save_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.followed.clone().filter(|_| !self.saving);
        let (Some(name), SetTab::Json) = (name, self.tab) else {
            return;
        };
        self.note = None;
        self.error = None;
        let text = self.json.read(cx).text(cx);
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
                    Ok(stored) => match parse_edited(this.kind, &text, &stored.items) {
                        Ok(items) => this.review(stored, items, Some(text), window, cx),
                        Err(error) => this.error = Some(file_error_text(&error).into()),
                    },
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

    /// D62: a save that removes items asks, by name on prod; on prod any save asks. `json` is the
    /// text of the JSON tab the items came from.
    pub(super) fn review(
        &mut self,
        stored: ItemSet,
        items: Vec<Value>,
        json: Option<String>,
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
                    let json = json.clone();
                    send_set(view.clone(), name.clone(), items.clone(), json, window, cx)
                };
                if removes {
                    confirm_write(danger, &connection, run, report_to(this), window, cx);
                } else {
                    write_after_prod_confirm(danger, &this, &connection, run, window, cx);
                }
            });
        });
    }

    /// `stored` is what the server holds now. The JSON tab follows unless it holds work that did
    /// not go into this save: only `json`, the text that did, may be replaced.
    fn take_stored(
        &mut self,
        stored: ItemSet,
        json: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = stored.name.clone();
        self.sets.retain(|s| s.name != name);
        self.sets.push(stored);
        self.sets.sort_by(|a, b| a.name.cmp(&b.name));
        self.show_rows(cx);
        if self.followed.as_ref() == Some(&name) {
            if json.is_some_and(|sent| sent == self.json.read(cx).text(cx)) {
                self.forget_json(cx);
            }
            self.show_items(window, cx);
        } else {
            self.list.update(cx, |l, cx| l.select(Some(name), cx));
        }
    }

    /// The server no longer holds `name`.
    pub(super) fn set_deleted(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.followed.as_deref() == Some(name) {
            self.followed = None;
            self.list.update(cx, |l, cx| l.select(None, cx));
            self.forget_json(cx);
        }
        self.sets.retain(|s| s.name != name);
        self.show_rows(cx);
        self.show_items(window, cx);
        self.reload(window, cx);
    }

    /// Detaches the first missing set, which carries the shortcut.
    fn detach_first_missing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.missing_sets().into_iter().next() {
            self.ask_detach_missing(row.set, window, cx);
        }
    }

    fn create_first_missing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.missing_sets().into_iter().next() {
            self.ask_new(Some(row.set), window, cx);
        }
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        let no_set = self.current().is_none();
        let no_item = self.items.read(cx).selected(cx).is_none();
        let no_row = self.list.read(cx).selected(cx).is_none();
        let none_missing = self.missing_sets().is_empty();
        let busy = self.pending.is_some();
        vec![
            (no_row || busy, &DeleteResource),
            (no_set, &ExportResource),
            (none_missing || busy, &DetachMissing),
            (none_missing, &CreateMissingSet),
            (no_set, &NewSetItem),
            (no_item, &EditResource),
            (no_item, &DeleteSetItem),
            (no_set || self.tab != SetTab::Json, &SaveResource),
            (self.test.read(cx).run_blocked(), &RunTest),
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
                    .child(
                        button(
                            "sets.items.delete",
                            t!("sets.delete").to_string(),
                            &DeleteSetItem,
                            none,
                        )
                        .danger(),
                    ),
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
            return v_flex()
                .children(self.error.clone().map(|e| notice("sets.error", e, danger)))
                .children(self.note.clone().map(|e| notice("sets.note", e, muted)))
                .child(
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(muted)
                        .child(t!("sets.pick").to_string()),
                )
                .into_any_element();
        };
        let tabs = TabBar::new("sets.tabs")
            .underline()
            .selected_index(self.tab as usize)
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                let tab = match ix {
                    0 => SetTab::Items,
                    1 => SetTab::Json,
                    2 => SetTab::Collections,
                    _ => SetTab::Test,
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
            ))
            .child(view_tab(
                "sets.tab.collections",
                t!("sets.tab.collections").to_string().into(),
                &ShowAttachment,
                RESOURCES,
                window,
            ))
            .child(view_tab(
                "sets.tab.test",
                t!("sets.tab.test").to_string().into(),
                &ShowTest,
                RESOURCES,
                window,
            ));
        let content = match self.tab {
            SetTab::Items => self.items_tab(window, cx).into_any_element(),
            SetTab::Json => self.json_tab(window, cx).into_any_element(),
            SetTab::Collections => self.attachment_tab(&name, cx).into_any_element(),
            SetTab::Test => self.test.clone().into_any_element(),
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
        let no_row = self.list.read(cx).selected(cx).is_none();
        let no_set = self.current().is_none();
        let synonyms = self.kind == SetKind::Synonym;
        let (prefix, _) = prefixes(self.kind);
        let missing = self.missing_rows(window, cx);
        let left = v_flex()
            .size_full()
            .child(toolbar(
                prefix,
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
            .children(missing)
            .child(if empty {
                empty_state(prefix, docs_url(self.kind)).into_any_element()
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
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .child(
                        row_button(
                            format!("{prefix}.delete"),
                            t!("sets.delete").to_string(),
                            &DeleteResource,
                            (no_row || self.pending.is_some(), locked),
                            &self.focus_handle,
                            window,
                            cx,
                        )
                        .danger(),
                    )
                    .when(synonyms, |el| {
                        el.child(row_button(
                            "synonyms.import".into(),
                            t!("sets.import").to_string(),
                            &ImportResource,
                            (false, locked),
                            &self.focus_handle,
                            window,
                            cx,
                        ))
                        .child(row_button(
                            "synonyms.export".into(),
                            t!("sets.export").to_string(),
                            &ExportResource,
                            (no_set, false),
                            &self.focus_handle,
                            window,
                            cx,
                        ))
                    }),
            );
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| {
                this.ask_new(None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.ask_delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DetachMissing, window, cx| {
                this.detach_first_missing(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CreateMissingSet, window, cx| {
                this.create_first_missing(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowAttachment, window, cx| {
                this.show_tab(SetTab::Collections, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowTest, window, cx| {
                this.show_tab(SetTab::Test, window, cx);
            }))
            .when(synonyms, |el| {
                el.on_action(cx.listener(|this, _: &ImportResource, window, cx| {
                    this.ask_import(window, cx);
                }))
                .on_action(cx.listener(
                    |this, _: &ExportResource, window, cx| {
                        this.ask_export(window, cx);
                    },
                ))
            })
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
                this.connection
                    .update(cx, |c, cx| c.load_collection_list(cx));
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
pub(super) fn write_after_prod_confirm(
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
pub(super) fn exists(
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

/// `PUT`s the whole set and compares what it reads back: the server keeps the last of equal ids
/// and drops unknown keys without a word. Runs on through a closed tab. `json`: see
/// [`SetsView::take_stored`].
pub(super) fn send_set(
    view: WeakEntity<SetsView>,
    name: String,
    items: Vec<Value>,
    json: Option<String>,
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
    let sent = items.clone();
    let save = cx
        .global::<RelevanceBackendGlobal>()
        .0
        .save_set(client, kind, name, items);
    window.spawn(cx, async move |cx| {
        let stored = save
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        let kept = stored.items.len();
        let dropped = dropped_by_id(&sent, &stored.items);
        view.update_in(cx, |view, window, cx| {
            view.take_stored(stored, json.as_deref(), window, cx);
            cx.notify();
        })
        .ok();
        // In its own update: selecting a new set clears the note as that update ends.
        view.update(cx, |view, cx| {
            view.note = dropped_note(&dropped);
            cx.notify();
        })
        .ok();
        if kept != sent.len() {
            return Err(t!("sets.count_differs", sent = sent.len(), stored = kept)
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
