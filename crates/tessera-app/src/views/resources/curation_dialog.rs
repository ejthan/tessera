// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog of one curation item: a form and a JSON tab over one draft.

use std::cell::Cell;
use std::time::Duration;

use chrono::DateTime;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::date_picker::{
    DatePicker, DatePickerEvent, DatePickerState, DateTime as PickedTime,
};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState, MaskPattern};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::time_field::TimePrecision;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::{Collection, SetKind};
use tessera_core::documents::columns::exclude_fields;
use tessera_core::relevance::attachment::attached;
use tessera_core::relevance::changes::{ItemIssue, Level, sort_warnings};
use tessera_core::relevance::curations::{
    CurationDraft, Exclude, Include, RuleDraft, check_curation,
};
use tessera_core::relevance::synonyms::ModelError;
use tessera_core::search::params::text_query_fields;
use tessera_widgets::json_editor::{JsonEditor, JsonEditorEvent};

use super::item_save::{ItemDialog, ItemSave, submit_item};
use super::sets::{SetsView, issue_text};
use super::{client_of, dialog_title};
use crate::actions::{
    FocusQuery, INCLUDE_ROW, MoveRowDown, MoveRowUp, NextControl, PreviousControl, RAW_BLOCK,
    SET_ITEM_DIALOG, SaveResource, ShowForm, ShowJson,
};
use crate::connection::{ConnectionState, error_text};
use crate::search::SearchBackendGlobal;

/// Width of the dialog.
const WIDTH: f32 = 760.;
/// Height of the tabs' content; the buttons stay below it.
const BODY_HEIGHT: f32 = 500.;
/// Height of a raw block.
const RAW_HEIGHT: f32 = 70.;
/// Width of a label in front of a row.
const LABEL_WIDTH: f32 = 60.;
/// The pause after typing before the picker searches.
const PICKER_DELAY: Duration = Duration::from_millis(150);

/// The tabs of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Form,
    Json,
}

/// An include row being dragged.
#[derive(Clone)]
struct DragInclude {
    index: usize,
    id: SharedString,
}

impl Render for DragInclude {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .rounded_md()
            .text_sm()
            .bg(cx.theme().secondary)
            .child(self.id.clone())
    }
}

/// One pinned document.
struct IncludeRow {
    id: String,
    position: Entity<InputState>,
    extra: Map<String, Value>,
    focus: FocusHandle,
    _subscription: Subscription,
}

fn picked(time: Option<i64>) -> PickedTime {
    let at = time.and_then(|t| DateTime::from_timestamp(t, 0));
    PickedTime::Single(at.map(|at| at.naive_utc()))
}

/// The stamp of a picker. Minute precision must not move a stored stamp that has seconds.
fn stamp(value: PickedTime, loaded: Option<i64>) -> Option<i64> {
    let PickedTime::Single(Some(at)) = value else {
        return None;
    };
    let time = at.and_utc().timestamp();
    let same_minute = |l: &i64| l.div_euclid(60) == time.div_euclid(60);
    loaded.filter(same_minute).or(Some(time))
}

/// A form over a [`CurationDraft`]. Whatever it has no control for stays in the raw blocks.
pub struct CurationItemDialog {
    save: ItemSave,
    tab: Tab,
    id: Entity<InputState>,
    query: Entity<InputState>,
    match_kind: String,
    tags: Entity<InputState>,
    rule_filter: Entity<InputState>,
    synonyms: bool,
    stem: bool,
    /// Whether the server takes `rule.synonyms` and `rule.stem`.
    rule_flags: bool,
    includes: Vec<IncludeRow>,
    excludes: Vec<Exclude>,
    filter_by: Entity<InputState>,
    sort_by: Entity<InputState>,
    replace_query: Entity<InputState>,
    remove_tokens: bool,
    /// `remove_tokens` follows the server's rule until this is set.
    remove_touched: bool,
    filter_curated: bool,
    stop: bool,
    from: Entity<DatePickerState>,
    to: Entity<DatePickerState>,
    /// The stamps as loaded, see [`stamp`].
    loaded_times: (Option<i64>, Option<i64>),
    metadata: Entity<JsonEditor>,
    extra: Entity<JsonEditor>,
    json: Entity<JsonEditor>,
    /// Every collection, attached ones first.
    collections: Vec<Collection>,
    /// How many of `collections` are attached.
    attached_count: usize,
    picker_collection: Entity<SelectState<Vec<String>>>,
    picker_schema: Option<Collection>,
    picker_q: Entity<InputState>,
    manual_id: Entity<InputState>,
    /// Id and first string field of each result.
    hits: Vec<(String, String)>,
    picker_error: Option<SharedString>,
    search: Option<Task<()>>,
    cancel_focus: FocusHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ItemDialog for CurationItemDialog {
    fn save_state(&mut self) -> &mut ItemSave {
        &mut self.save
    }
}

impl CurationItemDialog {
    /// Opens it on `item`, or on a new one.
    pub(super) fn open(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        set: String,
        item: Option<CurationDraft>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let title = if item.is_some() {
            t!("sets.item.edit_title")
        } else {
            t!("sets.item.add_title")
        }
        .to_string();
        let conn = connection.clone();
        let dialog = cx.new(|cx| Self::new(view, connection, set, item, window, cx));
        let opened = dialog.clone();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |d, window, cx| {
            if first_build.replace(false) {
                let id = opened.read(cx).id.read(cx).focus_handle(cx);
                id.focus(window, cx);
            }
            d.title(dialog_title(title.clone(), &conn, cx))
                .w(px(WIDTH))
                .child(opened.clone())
        });
        dialog
    }

    fn new(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        set: String,
        item: Option<CurationDraft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let original = item.as_ref().map(|d| d.id.clone());
        let remove_touched = item.is_some();
        let draft = item.unwrap_or_else(CurationDraft::new_default);
        let mut input = |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let [
            id,
            query,
            tags,
            rule_filter,
            filter_by,
            sort_by,
            replace_query,
        ] = [
            input(cx),
            input(cx),
            input(cx),
            input(cx),
            input(cx),
            input(cx),
            input(cx),
        ];
        let picker_q = input(cx);
        let manual_id = input(cx);
        let date = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                DatePickerState::new(window, cx)
                    .time_precision(TimePrecision::Minute)
                    .date_format("%Y-%m-%d %H:%M")
            })
        };
        let (from, to) = (date(window, cx), date(window, cx));
        let metadata = cx.new(|cx| JsonEditor::new("", false, window, cx));
        let extra = cx.new(|cx| JsonEditor::new("", false, window, cx));
        let json = cx.new(|cx| JsonEditor::new("", false, window, cx));
        let holds = |c: &Collection| attached(c, SetKind::Curation).contains(&set);
        let mut collections = connection
            .read(cx)
            .collection_cache
            .collections
            .clone()
            .unwrap_or_default();
        collections.sort_by_key(|c| !holds(c));
        let attached_count = collections.iter().filter(|c| holds(c)).count();
        let names: Vec<String> = collections.iter().map(|c| c.name.clone()).collect();
        let picker_schema = collections.first().cloned();
        let picker_collection = cx.new(|cx| {
            let first = names.first().cloned();
            let mut select = SelectState::new(names, None, window, cx).searchable(true);
            if let Some(first) = first {
                select.set_selected_value(&first, window, cx);
            }
            select
        });
        let rule_flags = connection
            .read(cx)
            .connected
            .as_ref()
            .is_none_or(|c| c.info.capabilities.rule_flags());
        let mut subscriptions: Vec<Subscription> = [
            &id,
            &query,
            &tags,
            &rule_filter,
            &filter_by,
            &sort_by,
            &replace_query,
            &manual_id,
        ]
        .into_iter()
        .map(|input| cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify()))
        .collect();
        subscriptions.push(cx.subscribe(&picker_q, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_search(cx);
            }
        }));
        subscriptions.push(cx.subscribe(
            &picker_collection,
            |this, _, event: &SelectEvent<Vec<String>>, cx| {
                if let SelectEvent::Confirm(Some(name)) = event {
                    this.pick_collection(name, cx);
                }
            },
        ));
        for state in [&from, &to] {
            subscriptions.push(cx.subscribe(state, |_, _, _: &DatePickerEvent, cx| cx.notify()));
        }
        for editor in [&metadata, &extra, &json] {
            subscriptions.push(cx.subscribe(editor, |_, _, _: &JsonEditorEvent, cx| cx.notify()));
        }
        let mut this = Self {
            save: ItemSave::new(view, connection, SetKind::Curation, set, original),
            tab: Tab::Form,
            id,
            query,
            match_kind: String::new(),
            tags,
            rule_filter,
            synonyms: false,
            stem: false,
            rule_flags,
            includes: Vec::new(),
            excludes: Vec::new(),
            filter_by,
            sort_by,
            replace_query,
            remove_tokens: false,
            remove_touched,
            filter_curated: false,
            stop: false,
            from,
            to,
            loaded_times: (None, None),
            metadata,
            extra,
            json,
            collections,
            attached_count,
            picker_collection,
            picker_schema,
            picker_q,
            manual_id,
            hits: Vec::new(),
            picker_error: None,
            search: None,
            cancel_focus: cx.focus_handle(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load(&draft, window, cx);
        this
    }

    /// Fills the form from `draft`.
    fn load(&mut self, draft: &CurationDraft, window: &mut Window, cx: &mut Context<Self>) {
        let rule = &draft.rule;
        for (input, text) in [
            (&self.id, draft.id.clone()),
            (&self.query, rule.query.clone()),
            (&self.tags, rule.tags.join(", ")),
            (&self.rule_filter, rule.filter_by.clone()),
            (&self.filter_by, draft.filter_by.clone()),
            (&self.sort_by, draft.sort_by.clone()),
            (&self.replace_query, draft.replace_query.clone()),
        ] {
            input.update(cx, |i, cx| i.set_value(text, window, cx));
        }
        self.match_kind.clone_from(&rule.r#match);
        (self.synonyms, self.stem) = (rule.synonyms, rule.stem);
        self.includes = Vec::new();
        for include in &draft.includes {
            self.push_include(include.clone(), window, cx);
        }
        self.excludes.clone_from(&draft.excludes);
        self.remove_tokens = draft.remove_matched_tokens;
        self.filter_curated = draft.filter_curated_hits;
        self.stop = draft.stop_processing;
        self.loaded_times = (draft.effective_from_ts, draft.effective_to_ts);
        for (state, time) in [
            (&self.from, draft.effective_from_ts),
            (&self.to, draft.effective_to_ts),
        ] {
            state.update(cx, |s, cx| s.set_date_time(picked(time), window, cx));
        }
        let metadata = draft.metadata.as_ref().map_or(String::new(), pretty);
        self.metadata
            .update(cx, |e, cx| e.set_text(&metadata, window, cx));
        let mut merged = draft.extra.clone();
        for (key, value) in &rule.extra {
            merged.insert(format!("rule.{key}"), value.clone());
        }
        let extra = if merged.is_empty() {
            String::new()
        } else {
            pretty(&merged)
        };
        self.extra
            .update(cx, |e, cx| e.set_text(&extra, window, cx));
    }

    fn push_include(&mut self, include: Include, window: &mut Window, cx: &mut Context<Self>) {
        let text = include.position.to_string();
        let position = cx.new(|cx| {
            InputState::new(window, cx)
                .mask_pattern(MaskPattern::Number {
                    separator: None,
                    fraction: Some(0),
                })
                .default_value(text)
        });
        let subscription =
            cx.subscribe_in(&position, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur) {
                    let position = |r: &IncludeRow| r.position.read(cx).value().parse::<i64>();
                    let keys: Vec<i64> = this
                        .includes
                        .iter()
                        .map(|r| position(r).unwrap_or(0))
                        .collect();
                    let mut order: Vec<usize> = (0..keys.len()).collect();
                    order.sort_by_key(|i| keys[*i]);
                    if order.windows(2).any(|w| w[0] > w[1]) {
                        let mut old: Vec<Option<IncludeRow>> = std::mem::take(&mut this.includes)
                            .into_iter()
                            .map(Some)
                            .collect();
                        this.includes = order.iter().filter_map(|i| old[*i].take()).collect();
                    }
                }
                cx.notify();
            });
        self.includes.push(IncludeRow {
            id: include.id,
            position,
            extra: include.extra,
            focus: cx.focus_handle().tab_stop(true),
            _subscription: subscription,
        });
    }

    /// The JSON tab's editor.
    pub fn json(&self) -> &Entity<JsonEditor> {
        &self.json
    }

    /// The Metadata block.
    pub fn metadata(&self) -> &Entity<JsonEditor> {
        &self.metadata
    }

    /// The block of keys without a control.
    pub fn extra(&self) -> &Entity<JsonEditor> {
        &self.extra
    }

    /// The two date pickers, From first.
    pub fn dates(&self) -> [&Entity<DatePickerState>; 2] {
        [&self.from, &self.to]
    }

    /// The focus handle of the row at `index`.
    pub fn include_focus(&self, index: usize) -> Option<&FocusHandle> {
        self.includes.get(index).map(|r| &r.focus)
    }

    /// The item as the open tab says now, or why it cannot be read.
    pub fn draft(&self, cx: &App) -> Result<CurationDraft, SharedString> {
        match self.tab {
            Tab::Form => self.form_draft(cx),
            Tab::Json => self.json_draft(cx),
        }
    }

    fn form_draft(&self, cx: &App) -> Result<CurationDraft, SharedString> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let raw =
            |editor: &Entity<JsonEditor>| -> Result<Option<Map<String, Value>>, SharedString> {
                let text = editor.read(cx).text(cx);
                if text.trim().is_empty() {
                    return Ok(None);
                }
                serde_json::from_str(&text).map(Some).map_err(|e| {
                    t!("sets.item.bad_raw", detail = e.to_string())
                        .to_string()
                        .into()
                })
            };
        let metadata = raw(&self.metadata)?;
        let (mut rule_extra, mut extra) = (Map::new(), Map::new());
        for (key, value) in raw(&self.extra)?.unwrap_or_default() {
            match key.strip_prefix("rule.") {
                Some(key) => rule_extra.insert(key.to_string(), value),
                None => extra.insert(key, value),
            };
        }
        let query = text(&self.query);
        // Without a query there is no match to set; a stored one stays.
        let r#match = if !query.is_empty() && self.match_kind.is_empty() {
            "exact".to_string()
        } else {
            self.match_kind.clone()
        };
        let tags = text(&self.tags);
        let mut includes: Vec<Include> = self
            .includes
            .iter()
            .map(|row| Include {
                id: row.id.clone(),
                position: text(&row.position).parse().unwrap_or(0),
                extra: row.extra.clone(),
            })
            .collect();
        includes.sort_by_key(|i| i.position);
        let picked = |state: &Entity<DatePickerState>| state.read(cx).date_time();
        Ok(CurationDraft {
            id: text(&self.id),
            rule: RuleDraft {
                query,
                r#match,
                filter_by: text(&self.rule_filter),
                tags: tags
                    .split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect(),
                synonyms: self.synonyms,
                stem: self.stem,
                extra: rule_extra,
            },
            includes,
            excludes: self.excludes.clone(),
            filter_by: text(&self.filter_by),
            sort_by: text(&self.sort_by),
            replace_query: text(&self.replace_query),
            remove_matched_tokens: self.remove_tokens,
            filter_curated_hits: self.filter_curated,
            stop_processing: self.stop,
            effective_from_ts: stamp(picked(&self.from), self.loaded_times.0),
            effective_to_ts: stamp(picked(&self.to), self.loaded_times.1),
            metadata,
            extra,
        })
    }

    fn json_draft(&self, cx: &App) -> Result<CurationDraft, SharedString> {
        let editor = self.json.read(cx);
        if let Some(error) = editor.error() {
            let text = t!(
                "curations.bad_json",
                line = error.line,
                column = error.column,
                detail = error.message
            );
            return Err(text.to_string().into());
        }
        let bad_json = |detail: String| -> SharedString {
            t!("curations.bad_json_shape", detail = detail)
                .to_string()
                .into()
        };
        let value: Value =
            serde_json::from_str(&editor.text(cx)).map_err(|e| bad_json(e.to_string()))?;
        CurationDraft::from_value(&value).map_err(|error| {
            bad_json(match error {
                ModelError::NotObject => t!("curations.not_object").to_string(),
                ModelError::BadField(key) => t!("curations.bad_field", key = key).to_string(),
            })
        })
    }

    fn issues(&self, cx: &App) -> Vec<ItemIssue> {
        self.draft(cx).map_or(Vec::new(), |d| {
            let mut issues = check_curation(&d);
            let users = &self.collections[..self.attached_count];
            issues.extend(sort_warnings(&d.sort_by, users));
            issues
        })
    }

    /// Whether Save does nothing now.
    pub fn save_blocked(&self, cx: &App) -> bool {
        self.save.busy
            || self.draft(cx).is_err()
            || self.issues(cx).iter().any(|i| i.level() == Level::Error)
    }

    /// Switches tabs through the draft. A tab that cannot be read stays, with its text.
    fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        let Ok(draft) = self.draft(cx) else {
            return;
        };
        self.tab = tab;
        match tab {
            Tab::Json => {
                let text = serde_json::to_string_pretty(&draft.to_value()).unwrap_or_default();
                self.json.update(cx, |e, cx| {
                    e.set_text(&text, window, cx);
                    e.focus(window, cx);
                });
            }
            Tab::Form => {
                self.load(&draft, window, cx);
                let id = self.id.read(cx).focus_handle(cx);
                id.focus(window, cx);
            }
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_blocked(cx) {
            return;
        }
        match self.draft(cx) {
            Ok(draft) => submit_item(self, draft.id.clone(), draft.to_value(), window, cx),
            Err(error) => self.save.refuse(error, cx),
        }
    }

    /// While untouched, the flag goes where the server would put it.
    fn follow_remove_default(&mut self) {
        if !self.remove_touched {
            self.remove_tokens = self.includes.is_empty() && self.excludes.is_empty();
        }
    }

    fn add_include(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if id.is_empty() || self.includes.iter().any(|r| r.id == id) {
            return;
        }
        let position = self.includes.len() as i64 + 1;
        let include = Include {
            id,
            position,
            extra: Map::new(),
        };
        self.push_include(include, window, cx);
        self.follow_remove_default();
        cx.notify();
    }

    fn add_exclude(&mut self, id: String, cx: &mut Context<Self>) {
        if id.is_empty() || self.excludes.iter().any(|e| e.id == id) {
            return;
        }
        self.excludes.push(Exclude {
            id,
            extra: Map::new(),
        });
        self.follow_remove_default();
        cx.notify();
    }

    /// Adds what the manual input holds, then clears it.
    fn add_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.manual_id.read(cx).value().trim().to_string();
        self.add_include(id, window, cx);
        self.manual_id
            .update(cx, |i, cx| i.set_value("", window, cx));
    }

    fn remove_include(&mut self, index: usize, cx: &mut Context<Self>) {
        self.includes.remove(index);
        self.follow_remove_default();
        cx.notify();
    }

    fn remove_exclude(&mut self, index: usize, cx: &mut Context<Self>) {
        self.excludes.remove(index);
        self.follow_remove_default();
        cx.notify();
    }

    /// Moves a row and numbers the rows 1 to n.
    fn move_include(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from >= self.includes.len() || to >= self.includes.len() || from == to {
            return;
        }
        let row = self.includes.remove(from);
        self.includes.insert(to, row);
        for (index, row) in self.includes.iter().enumerate() {
            let text = (index + 1).to_string();
            row.position
                .update(cx, |i, cx| i.set_value(text, window, cx));
        }
        cx.notify();
    }

    /// Takes `name` as the collection to search.
    pub fn pick_collection(&mut self, name: &str, cx: &mut Context<Self>) {
        self.picker_schema = self.collections.iter().find(|c| c.name == name).cloned();
        self.schedule_search(cx);
    }

    /// Searches once the input stood still for a moment. Curations stay off: the list shows
    /// stored documents, not what today's rules make of them.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search = None;
        self.picker_error = None;
        let (Some(schema), Some(client)) =
            (&self.picker_schema, client_of(&self.save.connection, cx))
        else {
            return;
        };
        let q = self.picker_q.read(cx).value().trim().to_string();
        let q = if q.is_empty() { "*".to_string() } else { q };
        let query_by = text_query_fields(schema).join(",");
        let mut params = vec![("q".to_string(), q)];
        if !query_by.is_empty() {
            params.push(("query_by".to_string(), query_by));
        }
        for (key, value) in [
            ("per_page", "20"),
            ("enable_curations", "false"),
            ("enable_analytics", "false"),
        ] {
            params.push((key.to_string(), value.to_string()));
        }
        if let Some(fields) = exclude_fields(schema) {
            params.push(("exclude_fields".to_string(), fields));
        }
        let titles: Vec<String> = schema
            .fields
            .iter()
            .filter(|f| f.r#type == "string")
            .map(|f| f.name.clone())
            .collect();
        let name = schema.name.clone();
        let backend = cx.global::<SearchBackendGlobal>().0.clone();
        self.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PICKER_DELAY).await;
            let tags = ("<mark>".to_string(), "</mark>".to_string());
            let answer = backend.search(client, name, params, tags).await;
            this.update(cx, |this, cx| {
                match answer {
                    Ok(answer) => {
                        let line = |hit: &tessera_core::search::results::Hit| {
                            let title = titles
                                .iter()
                                .find_map(|t| hit.doc.get(t).and_then(Value::as_str));
                            (hit.id.to_string(), title.unwrap_or_default().to_string())
                        };
                        this.hits = answer.result.hits.iter().map(line).collect();
                    }
                    Err(e) => {
                        let profile = &this.save.connection.read(cx).profile;
                        this.picker_error = Some(error_text(&e, profile).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn include_rows(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let (secondary, ring) = (cx.theme().secondary, cx.theme().ring);
        let last = self.includes.len().saturating_sub(1);
        self.includes
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let id = format!("curations.include.{index}");
                let name = SharedString::from(row.id.clone());
                let drag = DragInclude {
                    index,
                    id: name.clone(),
                };
                let up = Button::new(SharedString::from(format!("{id}.up")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowUp)
                    .accessibility_label(t!("curations.move_up").to_string())
                    .children(Kbd::binding_for_action(
                        &MoveRowUp,
                        Some(INCLUDE_ROW),
                        window,
                    ))
                    .disabled(index == 0)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.move_include(index, index.saturating_sub(1), window, cx);
                    }));
                let down = Button::new(SharedString::from(format!("{id}.down")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowDown)
                    .accessibility_label(t!("curations.move_down").to_string())
                    .children(Kbd::binding_for_action(
                        &MoveRowDown,
                        Some(INCLUDE_ROW),
                        window,
                    ))
                    .disabled(index == last)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.move_include(index, index + 1, window, cx);
                    }));
                let remove = Button::new(SharedString::from(format!("{id}.remove")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .accessibility_label(t!("curations.remove").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_include(index, cx)));
                h_flex()
                    .id(SharedString::from(id.clone()))
                    .test_support()
                    .aria_label(name.clone())
                    .key_context(INCLUDE_ROW)
                    .track_focus(&row.focus)
                    .on_action(cx.listener(move |this, _: &MoveRowUp, window, cx| {
                        this.move_include(index, index.saturating_sub(1), window, cx);
                    }))
                    .on_action(cx.listener(move |this, _: &MoveRowDown, window, cx| {
                        this.move_include(index, index + 1, window, cx);
                    }))
                    .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_drop(cx.listener(move |this, drag: &DragInclude, window, cx| {
                        this.move_include(drag.index, index, window, cx);
                    }))
                    .gap_2()
                    .px_1()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .bg(secondary)
                    .text_sm()
                    .focus(|s| s.border_color(ring))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(t!("curations.position").to_string())
                    .child(
                        div().w(px(60.)).child(
                            Input::new(&row.position)
                                .xsmall()
                                .id(SharedString::from(format!("{id}.position")))
                                .aria_label(t!("curations.position").to_string()),
                        ),
                    )
                    .child(up)
                    .child(down)
                    .child(remove)
            })
            .collect()
    }

    fn exclude_rows(&self, cx: &mut Context<Self>) -> Vec<impl IntoElement + use<>> {
        let secondary = cx.theme().secondary;
        self.excludes
            .iter()
            .enumerate()
            .map(|(index, exclude)| {
                let id = format!("curations.exclude.{index}");
                let name = SharedString::from(exclude.id.clone());
                h_flex()
                    .id(SharedString::from(id.clone()))
                    .test_support()
                    .aria_label(name.clone())
                    .gap_2()
                    .px_1()
                    .items_center()
                    .rounded_md()
                    .bg(secondary)
                    .text_sm()
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        Button::new(SharedString::from(format!("{id}.remove")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .accessibility_label(t!("curations.remove").to_string())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove_exclude(index, cx)),
                            ),
                    )
            })
            .collect()
    }

    fn picker(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let danger = cx.theme().danger;
        let rows = self.hits.iter().enumerate().map(|(index, (id, title))| {
            let (pin, hide) = (id.clone(), id.clone());
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().child(id.clone()))
                .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                .child(
                    Button::new(SharedString::from(format!("curations.hit.{index}.pin")))
                        .xsmall()
                        .label(t!("curations.pin").to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.add_include(pin.clone(), window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("curations.hit.{index}.hide")))
                        .xsmall()
                        .label(t!("curations.hide").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add_exclude(hide.clone(), cx);
                        })),
                )
        });
        let rows: Vec<_> = rows.collect();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(160.)).child(
                            Select::new(&self.picker_collection)
                                .id("curations.picker.collection")
                                .small()
                                .accessibility_label(t!("curations.picker.collection").to_string())
                                .placeholder(t!("curations.picker.collection").to_string()),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(&self.picker_q)
                                .small()
                                .suffix(div().children(Kbd::binding_for_action(
                                    &FocusQuery,
                                    Some(SET_ITEM_DIALOG),
                                    window,
                                )))
                                .id("curations.picker.q")
                                .aria_label(t!("curations.picker.q").to_string()),
                        ),
                    )
                    .child(
                        div().w(px(140.)).child(
                            Input::new(&self.manual_id)
                                .small()
                                .id("curations.picker.manual")
                                .aria_label(t!("curations.picker.manual").to_string()),
                        ),
                    )
                    .child(
                        Button::new("curations.picker.add")
                            .small()
                            .label(t!("curations.picker.add").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_manual(window, cx);
                            })),
                    ),
            )
            .children(self.picker_error.clone().map(|error| {
                div()
                    .id("curations.picker.error")
                    .test_support()
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
            .children(rows)
    }

    fn form(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let field = |label: String, id: &'static str, state: &Entity<InputState>| {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .text_sm()
                .child(label.clone())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(state).small().id(id).aria_label(label)),
                )
        };
        let line = |label: String| {
            h_flex()
                .gap_2()
                .items_start()
                .child(div().w(px(LABEL_WIDTH)).flex_none().text_sm().child(label))
        };
        let exact = self.match_kind != "contains";
        let matching = ButtonGroup::new("curations.match")
            .outline()
            .xsmall()
            .child(
                Button::new("curations.match.exact")
                    .label(t!("curations.match.exact").to_string())
                    .selected(exact)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.match_kind = "exact".to_string();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("curations.match.contains")
                    .label(t!("curations.match.contains").to_string())
                    .selected(!exact)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.match_kind = "contains".to_string();
                        cx.notify();
                    })),
            );
        let flags = h_flex()
            .gap_3()
            .child(
                Checkbox::new("curations.rule_synonyms")
                    .checked(self.synonyms)
                    .label(t!("curations.rule_synonyms").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.synonyms = !this.synonyms;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("curations.rule_stem")
                    .checked(self.stem)
                    .label(t!("curations.rule_stem").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stem = !this.stem;
                        cx.notify();
                    })),
            );
        let includes = self.include_rows(window, cx);
        let excludes = self.exclude_rows(cx);
        let picker = self.picker(window, cx);
        let date = |label: String, state: &Entity<DatePickerState>| {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .text_sm()
                .child(label)
                .child(DatePicker::new(state).small().cleanable(true))
        };
        let raw = |label: String, editor: &Entity<JsonEditor>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .text_sm()
                .child(label)
                .child(
                    div()
                        .key_context(RAW_BLOCK)
                        .h(px(RAW_HEIGHT))
                        .child(editor.clone()),
                )
        };
        v_flex()
            .gap_2()
            .child(field(
                t!("sets.item.id").to_string(),
                "curations.id",
                &self.id,
            ))
            .child(
                line(t!("curations.when").to_string()).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(field(
                                    t!("curations.query").to_string(),
                                    "curations.query",
                                    &self.query,
                                ))
                                .child(matching)
                                .child(field(
                                    t!("curations.tags").to_string(),
                                    "curations.tags",
                                    &self.tags,
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(field(
                                    t!("curations.rule_filter").to_string(),
                                    "curations.rule_filter",
                                    &self.rule_filter,
                                ))
                                .when(self.rule_flags, |el| el.child(flags)),
                        ),
                ),
            )
            .child(
                line(t!("curations.pin_label").to_string())
                    .child(v_flex().flex_1().min_w_0().gap_1().children(includes)),
            )
            .child(
                line(t!("curations.hide_label").to_string())
                    .child(v_flex().flex_1().min_w_0().gap_1().children(excludes)),
            )
            .child(
                line(t!("curations.find").to_string())
                    .child(div().flex_1().min_w_0().child(picker)),
            )
            .child(
                line(t!("curations.then").to_string()).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(field(
                                    t!("curations.filter_by").to_string(),
                                    "curations.filter_by",
                                    &self.filter_by,
                                ))
                                .child(field(
                                    t!("curations.sort_by").to_string(),
                                    "curations.sort_by",
                                    &self.sort_by,
                                ))
                                .child(field(
                                    t!("curations.replace_query").to_string(),
                                    "curations.replace_query",
                                    &self.replace_query,
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    Checkbox::new("curations.remove_tokens")
                                        .checked(self.remove_tokens)
                                        .label(t!("curations.remove_tokens").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.remove_tokens = !this.remove_tokens;
                                            this.remove_touched = true;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Checkbox::new("curations.filter_curated")
                                        .checked(self.filter_curated)
                                        .label(t!("curations.filter_curated").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.filter_curated = !this.filter_curated;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Checkbox::new("curations.stop")
                                        .checked(self.stop)
                                        .label(t!("curations.stop").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.stop = !this.stop;
                                            cx.notify();
                                        })),
                                ),
                        ),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(date(t!("curations.from_utc").to_string(), &self.from))
                    .child(date(t!("curations.to_utc").to_string(), &self.to)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(raw(t!("curations.metadata").to_string(), &self.metadata))
                    .child(raw(t!("curations.extra").to_string(), &self.extra)),
            )
    }
}

/// `map` as the raw blocks show it.
fn pretty(map: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(map).unwrap_or_default()
}

impl Render for CurationItemDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, warning) = (theme.danger, theme.warning);
        let issues = self.issues(cx);
        let blocked = self.save_blocked(cx);
        let unreadable = self.draft(cx).err();
        let drops_flags =
            !self.rule_flags && self.draft(cx).is_ok_and(|d| d.rule.synonyms || d.rule.stem);
        let line = |id: String, text: SharedString, color| {
            div()
                .id(SharedString::from(id))
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(color)
                .child(text)
        };
        let tab_kbd = |action: &dyn gpui_kit::Action| {
            Kbd::binding_for_action(action, Some(SET_ITEM_DIALOG), window)
        };
        let tabs = ButtonGroup::new("curations.mode")
            .outline()
            .small()
            .child(
                Button::new("curations.mode.form")
                    .label(t!("schema.mode.form").to_string())
                    .selected(self.tab == Tab::Form)
                    .children(tab_kbd(&ShowForm))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_tab(Tab::Form, window, cx)),
                    ),
            )
            .child(
                Button::new("curations.mode.json")
                    .label(t!("schema.mode.json").to_string())
                    .selected(self.tab == Tab::Json)
                    .children(tab_kbd(&ShowJson))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_tab(Tab::Json, window, cx)),
                    ),
            );
        let body = match self.tab {
            Tab::Form => self.form(window, cx).into_any_element(),
            Tab::Json => div()
                .size_full()
                .child(self.json.clone())
                .into_any_element(),
        };
        v_flex()
            .key_context(SET_ITEM_DIALOG)
            .track_focus(&self.focus_handle)
            .gap_2()
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.submit(window, cx)))
            .on_action(
                cx.listener(|this, _: &ShowForm, window, cx| this.show_tab(Tab::Form, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowJson, window, cx| this.show_tab(Tab::Json, window, cx)),
            )
            .on_action(|_: &NextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousControl, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(|this, _: &FocusQuery, window, cx| {
                this.picker_q.update(cx, |i, cx| i.focus(window, cx));
            }))
            .child(tabs)
            .child(
                div()
                    .id("curations.body")
                    .h(px(BODY_HEIGHT))
                    .overflow_y_scroll()
                    .child(body),
            )
            .children(unreadable.map(|e| line("curations.unreadable".to_string(), e, danger)))
            .when(drops_flags, |el| {
                el.child(line(
                    "curations.flags_dropped".to_string(),
                    t!("curations.flags_dropped").to_string().into(),
                    warning,
                ))
            })
            .children(issues.iter().enumerate().map(|(i, issue)| {
                let color = if issue.level() == Level::Error {
                    danger
                } else {
                    warning
                };
                line(
                    format!("sets.item.issue.{i}"),
                    issue_text(issue).into(),
                    color,
                )
            }))
            .children(
                self.save
                    .error
                    .clone()
                    .map(|error| line("sets.item.error".to_string(), error, danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("sets.item.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sets.item.save")
                            .primary()
                            .label(t!("sets.save").to_string())
                            .loading(self.save.busy)
                            .disabled(blocked)
                            .children(Kbd::binding_for_action(
                                &SaveResource,
                                Some(SET_ITEM_DIALOG),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
