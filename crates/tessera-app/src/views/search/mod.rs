// SPDX-License-Identifier: GPL-3.0-or-later

//! The search tab of a connection: a draft on the left, its result on the right.

mod export;
pub mod form;
pub mod hits;
mod multi;
mod queries;
mod results;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::actions::{SelectDown, SelectUp};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::{Client, Collection, Error, MULTI_SEARCH_LIMIT};
use tessera_core::profile::Node;
use tessera_core::search::INSTANT_DEBOUNCE_MS;
use tessera_core::search::export::ExportLang;
use tessera_core::search::params::{
    ParamError, SearchDraft, SideEffect, params_text, parse_params, side_effects, to_pairs,
};
use tessera_core::search::results::{ItemError, MultiResult, SearchResult};
use tessera_widgets::{BodyView, JsonEditor, JsonEditorEvent, NextChunk, PreviousChunk};

use self::export::ExportState;
use self::form::{FormEvent, SearchForm};
use self::hits::HitsView;
use self::multi::multi_request;
use self::queries::{PendingHistory, Queries, QueryList};
use super::dock::view_tab;
use super::env_badge;
use crate::actions::{
    AddSearch, CopyCurl, DeleteSavedQuery, FocusQuery, LoadQuery, NextResultPage, NextSearch,
    OpenExport, OpenHistory, OpenPresets, OpenSaved, PreviousResultPage, PreviousSearch,
    RemoveSearch, RunSearch, SaveQuery, ShowForm, ShowHits, ShowJson, ShowRawResult, ToggleInstant,
    ToggleUnion, WORKBENCH,
};
use crate::collections::BoxFuture;
use crate::connection::{ConnectionState, error_text};
use crate::search::SearchBackendGlobal;
use crate::state::AppState;

/// Width of the draft's side of the split.
const DRAFT_WIDTH: f32 = 400.;

/// A run's answer: the response text and what it means.
type Answer = Result<(Arc<str>, MultiResult), Error>;

/// How the draft is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One control per parameter.
    Form,
    /// The parameters as JSON.
    Json,
}

/// How the result is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultView {
    /// The hits.
    Hits,
    /// The response text.
    Raw,
}

/// The highlight tags of `params`, Typesense's by default.
fn tags(params: &Map<String, Value>) -> (String, String) {
    let tag = |key: &str, default: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    (
        tag("highlight_start_tag", "<mark>"),
        tag("highlight_end_tag", "</mark>"),
    )
}

/// A parameter as text; a number or boolean reads as in JSON.
fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn param_error_text(error: &ParamError) -> String {
    match error {
        ParamError::BadKey(key) => t!("search.param_error.bad_key", key = key),
        ParamError::Reserved(key) => t!("search.param_error.reserved", key = key),
        ParamError::BadValue(key) => t!("search.param_error.bad_value", key = key),
        ParamError::BadNumber(key) => t!("search.param_error.bad_number", key = key),
        ParamError::NotAnObject => t!("search.param_error.not_an_object"),
    }
    .to_string()
}

/// A connection's searches.
pub struct SearchWorkbench {
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    drafts: Vec<SearchDraft>,
    active: usize,
    union: bool,
    mode: Mode,
    view: ResultView,
    form: Entity<SearchForm>,
    json: Entity<JsonEditor>,
    /// Why the JSON text is not used; the map stays, Run is disabled and the text is kept in
    /// the editor, in either mode, until it parses.
    json_error: Option<SharedString>,
    collection: Entity<SelectState<Vec<String>>>,
    /// The collection list this view took last.
    list_loaded: Option<std::time::Instant>,
    schema: Option<Collection>,
    hits: Entity<HitsView>,
    raw: Entity<BodyView>,
    /// The last response, until the Raw tab shows it.
    raw_pending: Option<Arc<str>>,
    /// The last answer, one entry per search or one for a union, in the order of `drafts`.
    results: Vec<Result<SearchResult, ItemError>>,
    /// The result tab asked for; see `shown`.
    result_tab: usize,
    /// `results` came from a union.
    union_answer: bool,
    error: Option<SharedString>,
    running: bool,
    /// Identifies the newest run; an answer of an older one is dropped.
    generation: u64,
    /// The newest run. Replacing it drops the older read, which cancels its request.
    run_task: Option<Task<()>>,
    instant: bool,
    /// The wait before an instant run; a newer change replaces it.
    instant_task: Option<Task<()>>,
    /// The run on its way into the history.
    pending_history: Option<PendingHistory>,
    /// The wait until an instant run counts as settled.
    history_task: Option<Task<()>>,
    queries: Queries,
    export: ExportState,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 5],
}

impl SearchWorkbench {
    /// The workbench of `connection`, on its first collection once the list is known.
    pub fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let form = cx.new(|cx| SearchForm::new(connection.clone(), window, cx));
        let draft = SearchDraft::new("");
        let json = cx.new(|cx| JsonEditor::new(&params_text(&draft.params), false, window, cx));
        let collection = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let hits = cx.new(HitsView::new);
        let raw = cx.new(|cx| BodyView::new(window, cx));
        let instant = !connection.read(cx).profile.environment.is_prod();
        if connection.read(cx).collection_cache.collections.is_none() {
            connection.update(cx, |c, cx| c.load_collection_list(cx));
        }
        let (queries, queries_changed) = Queries::new(window, cx);
        let mut this = Self {
            _subscriptions: [
                queries_changed,
                cx.observe_in(&connection, window, |this, _, window, cx| {
                    this.take_collection_list(window, cx);
                    cx.notify();
                }),
                cx.subscribe_in(&form, window, |this, _, event, window, cx| match event {
                    FormEvent::Set(key, value) => this.set_param(key, value.clone(), window, cx),
                    FormEvent::Submit => this.run(window, cx),
                }),
                cx.subscribe_in(
                    &json,
                    window,
                    |this, _, JsonEditorEvent::Changed, window, cx| {
                        this.json_changed(window, cx);
                    },
                ),
                cx.subscribe_in(
                    &collection,
                    window,
                    |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                        if let SelectEvent::Confirm(Some(name)) = event {
                            this.drafts[this.active].collection = name.clone();
                            this.drop_page(window, cx);
                            this.sync_schema(window, cx);
                            this.changed(window, cx);
                        }
                    },
                ),
            ],
            state,
            connection,
            drafts: vec![draft],
            active: 0,
            union: false,
            mode: Mode::Form,
            view: ResultView::Hits,
            form,
            json,
            json_error: None,
            collection,
            list_loaded: None,
            schema: None,
            hits,
            raw,
            raw_pending: None,
            results: Vec::new(),
            result_tab: 0,
            union_answer: false,
            error: None,
            running: false,
            generation: 0,
            run_task: None,
            instant,
            instant_task: None,
            pending_history: None,
            history_task: None,
            queries,
            export: ExportState::default(),
            focus_handle: cx.focus_handle(),
        };
        this.sync_editors(window, cx);
        this.take_collection_list(window, cx);
        this
    }

    /// Replaces the drafts, without running them.
    pub fn load(
        &mut self,
        drafts: Vec<SearchDraft>,
        union: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drafts.is_empty() {
            return;
        }
        self.union = union && drafts.len() > 1;
        self.drafts = drafts;
        self.active = 0;
        self.instant_task = None;
        // Its answer would show under drafts that did not ask for it.
        self.drop_run();
        self.results.clear();
        self.result_tab = 0;
        self.error = None;
        self.json_error = None;
        self.sync_draft(window, cx);
    }

    fn drop_run(&mut self) {
        self.forget_run();
        self.run_task = None;
        self.generation += 1;
        self.running = false;
    }

    /// Shows the active draft in everything that follows it.
    fn sync_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_collection(window, cx);
        self.sync_schema(window, cx);
        self.sync_editors(window, cx);
        self.sync_paging_lock(cx);
        cx.notify();
    }

    fn sync_paging_lock(&self, cx: &mut Context<Self>) {
        let locked = self.union && self.active > 0;
        self.form.update(cx, |f, cx| f.lock_paging(locked, cx));
    }

    /// The drafts.
    pub fn drafts(&self) -> &[SearchDraft] {
        &self.drafts
    }

    /// Whether the drafts run as one merged search.
    pub fn union(&self) -> bool {
        self.union
    }

    /// The draft being edited.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The result on screen; it stays while a newer run is going or failed.
    pub fn result(&self) -> Option<&SearchResult> {
        self.results.get(self.shown())?.as_ref().ok()
    }

    /// The form.
    pub fn form(&self) -> &Entity<SearchForm> {
        &self.form
    }

    /// The JSON editor.
    pub fn json(&self) -> &Entity<JsonEditor> {
        &self.json
    }

    /// The hit list.
    pub fn hits(&self) -> &Entity<HitsView> {
        &self.hits
    }

    /// The Raw tab's view.
    pub fn raw(&self) -> &Entity<BodyView> {
        &self.raw
    }

    fn draft(&self) -> &SearchDraft {
        &self.drafts[self.active]
    }

    /// Takes a newly loaded collection list: the select's entries, the schema, and the first
    /// collection for a draft without one.
    fn take_collection_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cache = &self.connection.read(cx).collection_cache;
        // The connection also changes with every poll, and an invalidated list comes back.
        if cache.loaded_at.is_none() || cache.loaded_at == self.list_loaded {
            return;
        }
        self.list_loaded = cache.loaded_at;
        let collections = cache.collections.iter().flatten().map(|c| c.name.clone());
        let aliases = cache.aliases.iter().map(|a| a.name.clone());
        let names: Vec<String> = collections.chain(aliases).collect();
        let first = names.first().cloned();
        self.collection
            .update(cx, |s, cx| s.set_items(names, window, cx));
        if let Some(first) = first
            && self.draft().collection.is_empty()
        {
            for draft in &mut self.drafts {
                draft.collection.clone_from(&first);
            }
        }
        self.sync_collection(window, cx);
        self.sync_schema(window, cx);
    }

    fn sync_collection(&self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.draft().collection.clone();
        self.collection
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
    }

    /// The schema behind collection or alias `name`, once the list is known.
    fn schema_of<'a>(&self, name: &str, cx: &'a App) -> Option<&'a Collection> {
        let cache = &self.connection.read(cx).collection_cache;
        let alias = cache.aliases.iter().find(|a| a.name == name);
        let target = alias.map_or(name, |a| &a.collection_name);
        cache
            .collections
            .iter()
            .flatten()
            .find(|c| c.name == target)
    }

    fn sync_schema(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.draft().collection.clone();
        self.schema = self.schema_of(&name, cx).cloned();
        let schema = self.schema.clone();
        self.form
            .update(cx, |f, cx| f.set_schema(&name, schema.as_ref(), window, cx));
    }

    /// Shows the draft's map in the editor on screen.
    fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let params = self.draft().params.clone();
        match self.mode {
            Mode::Form => self.form.update(cx, |f, cx| f.load(&params, window, cx)),
            // The user's text outranks the map until it parses.
            Mode::Json if self.json_error.is_some() => {}
            Mode::Json => self
                .json
                .update(cx, |j, cx| j.set_text(&params_text(&params), window, cx)),
        }
    }

    fn put_param(&mut self, draft: usize, key: &str, value: Option<Value>) {
        let params = &mut self.drafts[draft].params;
        match value {
            Some(value) => {
                params.insert(key.to_string(), value);
            }
            None => {
                params.shift_remove(key);
            }
        }
    }

    fn set_param(
        &mut self,
        key: &str,
        value: Option<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.draft().params.get(key) != value.as_ref() {
            self.put_param(self.active, key, value);
            if key != "page" {
                self.drop_page(window, cx);
            }
            self.changed(window, cx);
        }
    }

    /// Back to the first page: a page says nothing about another query's result. A union pages
    /// on its first draft, whichever is being edited.
    fn drop_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.union { 0 } else { self.active };
        if self.drafts[target].params.contains_key("page") {
            self.put_param(target, "page", None);
            self.sync_editors(window, cx);
        }
    }

    /// Writes `edits` to the draft of the result on screen, as a click outside the editors does,
    /// and runs. Not over JSON text that does not parse: showing the map would replace it.
    fn write_and_run(
        &mut self,
        edits: &[(&str, Option<Value>)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.json_error.is_some() {
            return;
        }
        let draft = self.shown_draft();
        for (key, value) in edits {
            self.put_param(draft, key, value.clone());
        }
        self.sync_editors(window, cx);
        self.run(window, cx);
    }

    fn json_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match parse_params(&self.json.read(cx).text(cx)) {
            Ok(params) => {
                self.json_error = None;
                if params != self.draft().params {
                    self.drafts[self.active].params = params;
                    self.changed(window, cx);
                }
            }
            Err(error) => {
                self.json_error = Some(param_error_text(&error).into());
                self.instant_task = None;
            }
        }
        cx.notify();
    }

    /// What a run of all drafts would do besides searching.
    fn side_effects(&self, cx: &App) -> Vec<SideEffect> {
        let of = |d: &SearchDraft| side_effects(&d.params, self.schema_of(&d.collection, cx));
        self.drafts.iter().flat_map(of).collect()
    }

    /// A schema is unknown, so a `query_by` may name a field that embeds remotely.
    fn schema_unknown(&self, cx: &App) -> bool {
        let unknown = |d: &SearchDraft| {
            d.params.contains_key("query_by") && self.schema_of(&d.collection, cx).is_none()
        };
        self.drafts.iter().any(unknown)
    }

    /// Instant search would, or might, call a model or store something.
    fn instant_paused(&self, cx: &App) -> bool {
        self.schema_unknown(cx)
            || self
                .side_effects(cx)
                .iter()
                .any(|e| *e != SideEffect::Analytics)
    }

    fn instant_allowed(&self, cx: &App) -> bool {
        self.instant && !self.run_blocked() && !self.instant_paused(cx)
    }

    /// There is nothing to send: a draft without collection, or JSON that does not parse.
    fn run_blocked(&self) -> bool {
        self.drafts.iter().any(|d| d.collection.is_empty()) || self.json_error.is_some()
    }

    /// What the toolbar, the tabs and the pager disable now, for
    /// [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        let run = self.run_blocked();
        let edit = self.json_error.is_some();
        let single = self.drafts.len() < 2;
        let (page, pages) = self
            .result()
            .map_or((0, 0), |result| (self.page(), self.last_page(result)));
        vec![
            (run, &RunSearch),
            (self.save_blocked(cx), &SaveQuery),
            (run, &OpenExport),
            (run, &CopyCurl),
            (page <= 1 || edit, &PreviousResultPage),
            (page >= pages || edit, &NextResultPage),
            (self.drafts.len() >= MULTI_SEARCH_LIMIT || edit, &AddSearch),
            (single || edit, &RemoveSearch),
            (single || edit, &NextSearch),
            (single || edit, &PreviousSearch),
            (single, &ToggleUnion),
        ]
    }

    /// The draft changed: starts the wait for an instant run. Only the last change of a burst
    /// gets to run.
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.instant_task = None;
        cx.notify();
        if !self.instant_allowed(cx) {
            return;
        }
        self.instant_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(INSTANT_DEBOUNCE_MS))
                .await;
            // The schema may have arrived during the wait.
            this.update_in(cx, |this, window, cx| {
                if this.instant_allowed(cx) {
                    this.start(false, window, cx);
                }
            })
            .ok();
        }));
    }

    /// Runs the draft, replacing a run going or waiting. A conversation writes, so it first
    /// asks to unlock a read-only connection.
    pub fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start(true, window, cx);
    }

    /// `explicit`: the user asked for this run, so its success enters the history at once.
    fn start(&mut self, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.instant_task = None;
        if self.run_blocked() {
            return;
        }
        if !self.side_effects(cx).contains(&SideEffect::Conversation) {
            return self.send(explicit, window, cx);
        }
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.send(explicit, window, cx))
                    .ok();
            });
        });
    }

    /// The read for all drafts: `search` for one, `multi` for several.
    fn ask(&self, client: Client, cx: &App) -> Result<BoxFuture<Answer>, ParamError> {
        let backend = cx.global::<SearchBackendGlobal>().0.clone();
        if let [draft] = &self.drafts[..] {
            let search = backend.search(
                client,
                draft.collection.clone(),
                to_pairs(&draft.params)?,
                tags(&draft.params),
            );
            return Ok(Box::pin(async move {
                let answer = search.await?;
                let result = MultiResult::Federated(vec![Ok(answer.result)]);
                Ok((answer.text, result))
            }));
        }
        let request = multi_request(&self.drafts, self.union)?;
        let tags = self.drafts.iter().map(|d| tags(&d.params)).collect();
        let multi = backend.multi(client, request, tags);
        Ok(Box::pin(async move {
            let answer = multi.await?;
            Ok((answer.text, answer.result))
        }))
    }

    fn send(&mut self, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connected) = &self.connection.read(cx).connected else {
            return;
        };
        let search = match self.ask(connected.client.clone(), cx) {
            Ok(search) => search,
            Err(error) => {
                self.error = Some(param_error_text(&error).into());
                return cx.notify();
            }
        };
        self.generation += 1;
        let generation = self.generation;
        self.running = true;
        self.follow_for_history(explicit, cx);
        self.run_task = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = search.await;
            this.update_in(cx, |this, window, cx| {
                if this.generation == generation {
                    this.take_answer(answer, window, cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn take_answer(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        self.running = false;
        match answer {
            Ok((text, result)) => {
                self.error = None;
                self.union_answer = matches!(result, MultiResult::Union(_));
                self.results = match result {
                    MultiResult::Federated(results) => results,
                    MultiResult::Union(result) => vec![Ok(result)],
                };
                self.show_result_tab(self.result_tab, cx);
                self.raw_pending = Some(text);
                self.fill_raw(window, cx);
                self.history_answer(self.results.iter().any(Result::is_ok), cx);
            }
            Err(error) => {
                let profile = &self.connection.read(cx).profile;
                self.error = Some(error_text(&error, profile).into());
                self.history_answer(false, cx);
            }
        }
        cx.notify();
    }

    /// Hands the last response to the Raw tab, once it shows: an editor per answer would be
    /// heavy at instant rate.
    fn fill_raw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == ResultView::Raw
            && let Some(text) = self.raw_pending.take()
        {
            self.raw.update(cx, |r, cx| r.set_body(text, window, cx));
        }
    }

    fn show_view(&mut self, view: ResultView, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        self.fill_raw(window, cx);
        cx.notify();
    }

    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        // Focus left in the editor going away would leave no key context behind.
        self.focus_handle.focus(window, cx);
        self.sync_editors(window, cx);
        cx.notify();
    }

    fn focus_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Mode::Form, window, cx);
        self.form.update(cx, |f, cx| f.focus_query(window, cx));
    }

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let needs_unlock = self.side_effects(cx).contains(&SideEffect::Conversation)
            && self.connection.read(cx).writes_locked();
        h_flex()
            .flex_none()
            .flex_wrap()
            .gap_3()
            .p_2()
            .items_center()
            .child(
                div().w(px(220.)).child(
                    Select::new(&self.collection)
                        .id("search.collection")
                        .small()
                        .accessibility_label(t!("search.collection").to_string())
                        .placeholder(t!("search.collection").to_string()),
                ),
            )
            .child(
                Button::new("search.run")
                    .primary()
                    .small()
                    .label(t!("search.run").to_string())
                    .children(Kbd::binding_for_action(&RunSearch, Some(WORKBENCH), window))
                    .children(needs_unlock.then(|| env_badge::lock("search.run", false, cx)))
                    .disabled(self.run_blocked())
                    .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
            )
            .child(self.list_button(
                QueryList::History,
                t!("search.history").to_string(),
                &OpenHistory,
                window,
                cx,
            ))
            .child(self.list_button(
                QueryList::Saved,
                t!("search.saved").to_string(),
                &OpenSaved,
                window,
                cx,
            ))
            .child(
                Button::new("search.save")
                    .ghost()
                    .small()
                    .label(t!("search.save").to_string())
                    .children(Kbd::binding_for_action(&SaveQuery, Some(WORKBENCH), window))
                    .disabled(self.save_blocked(cx))
                    .on_click(cx.listener(|this, _, window, cx| this.ask_save(window, cx))),
            )
            .child(self.list_button(
                QueryList::Presets,
                t!("search.presets").to_string(),
                &OpenPresets,
                window,
                cx,
            ))
            .child(self.export_button(window, cx))
            .children(self.export_note(cx))
            .child(
                Checkbox::new("search.instant")
                    .checked(self.instant)
                    .label(t!("search.instant").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_instant(cx))),
            )
            .children(Kbd::binding_for_action(
                &ToggleInstant,
                Some(WORKBENCH),
                window,
            ))
            .when(self.running, |el| el.child(Spinner::new().small()))
    }

    fn toggle_instant(&mut self, cx: &mut Context<Self>) {
        self.instant = !self.instant;
        self.instant_task = None;
        cx.notify();
    }

    /// What Instant does with this draft, under the checkbox.
    fn instant_note(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        if !self.instant {
            return None;
        }
        let (id, key) = if self.schema_unknown(cx) {
            ("search.instant.paused", "search.instant_paused_schema")
        } else if self.instant_paused(cx) {
            ("search.instant.paused", "search.instant_paused")
        } else if self.side_effects(cx).contains(&SideEffect::Analytics) {
            ("search.instant.analytics", "search.instant_analytics")
        } else {
            return None;
        };
        let note = SharedString::from(t!(key).to_string());
        Some(
            div()
                .id(id)
                .test_support()
                .aria_label(note.clone())
                .flex_none()
                .px_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(note),
        )
    }

    /// What the list of collections has to say, when there is nothing to search.
    fn collections_note(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let conn = self.connection.read(cx);
        let cache = &conn.collection_cache;
        let note = match (&cache.error, &cache.collections) {
            (Some(error), _) => error.clone(),
            (None, Some(list)) if list.is_empty() => {
                let local = conn.profile.nodes.iter().any(Node::is_local);
                let hint = if local {
                    t!("collections.seed_hint").to_string()
                } else {
                    String::new()
                };
                t!("search.no_collections", hint = hint).to_string().into()
            }
            _ => return None,
        };
        Some(
            div()
                .id("search.collections_note")
                .test_support()
                .aria_label(note.clone())
                .flex_none()
                .px_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(note),
        )
    }

    fn draft_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let json_error = self.json_error.clone().and_then(|error| match self.mode {
            // The form hides the editor, whose error line says the rest.
            Mode::Form => Some(t!("search.json_pending").to_string().into()),
            Mode::Json => self.json.read(cx).error().is_none().then_some(error),
        });
        let danger = cx.theme().danger;
        let on_json = self.mode == Mode::Json;
        v_flex()
            .size_full()
            .child(
                TabBar::new("search.modes")
                    .underline()
                    .selected_index(usize::from(on_json))
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        let mode = if *ix == 0 { Mode::Form } else { Mode::Json };
                        this.show(mode, window, cx);
                    }))
                    .child(view_tab(
                        "search.mode.form",
                        t!("search.mode.form").to_string().into(),
                        &ShowForm,
                        WORKBENCH,
                        window,
                    ))
                    .child(view_tab(
                        "search.mode.json",
                        t!("search.mode.json").to_string().into(),
                        &ShowJson,
                        WORKBENCH,
                        window,
                    )),
            )
            .child(div().flex_1().min_h_0().map(|el| {
                if on_json {
                    el.child(self.json.clone())
                } else {
                    el.child(self.form.clone())
                }
            }))
            .children(json_error.map(|error| {
                div()
                    .id("search.json.error")
                    .test_support()
                    .aria_label(error.clone())
                    .flex_none()
                    .p_1()
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
    }

    fn result_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let danger = cx.theme().danger;
        let on_raw = self.view == ResultView::Raw;
        let dimmed = self.error.is_some();
        let facets = self.facet_panel(cx);
        let pager = self.pager(window, cx);
        let no_hits = self.no_hits(cx);
        let result_tabs = self.result_tabs(cx);
        let status = self.result().map(|r| results::status(r, self.union_answer));
        let has_result = self.result().is_some();
        v_flex()
            .size_full()
            .children(result_tabs)
            .children(status.map(|status| {
                div()
                    .id("search.status")
                    .test_support()
                    .aria_label(status.clone())
                    .flex_none()
                    .p_1()
                    .text_sm()
                    .child(status)
            }))
            .children(self.item_error().map(|error| {
                div()
                    .id("search.item_error")
                    .test_support()
                    .aria_label(error.clone())
                    .flex_none()
                    .p_1()
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
            .children(self.error.clone().map(|error| {
                div()
                    .id("search.error")
                    .test_support()
                    .aria_label(error.clone())
                    .flex_none()
                    .p_1()
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
            .child(
                TabBar::new("search.views")
                    .underline()
                    .selected_index(usize::from(on_raw))
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        let view = if *ix == 0 {
                            ResultView::Hits
                        } else {
                            ResultView::Raw
                        };
                        this.show_view(view, window, cx);
                    }))
                    .child(view_tab(
                        "search.view.hits",
                        t!("search.view.hits").to_string().into(),
                        &ShowHits,
                        WORKBENCH,
                        window,
                    ))
                    .child(view_tab(
                        "search.view.raw",
                        t!("search.view.raw").to_string().into(),
                        &ShowRawResult,
                        WORKBENCH,
                        window,
                    )),
            )
            .child(
                div()
                    .id(if dimmed {
                        "search.result.dimmed"
                    } else {
                        "search.result"
                    })
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(dimmed, |el| el.opacity(0.5))
                    .child(v_flex().flex_1().min_w_0().h_full().map(|el| {
                        if on_raw {
                            el.child(div().flex_1().min_h_0().child(self.raw.clone()))
                        } else {
                            el.children(no_hits)
                                .when(has_result, |el| {
                                    el.child(div().flex_1().min_h_0().child(self.hits.clone()))
                                })
                                .children(pager)
                        }
                    }))
                    .children(facets),
            )
    }
}

impl Focusable for SearchWorkbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SearchWorkbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.search_tabs(window, cx);
        let toolbar = self.toolbar(window, cx);
        let draft = self.draft_pane(window, cx);
        let result = self.result_pane(window, cx);
        v_flex()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &RunSearch, window, cx| this.run(window, cx)))
            .on_action(
                cx.listener(|this, _: &ShowForm, window, cx| this.show(Mode::Form, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowJson, window, cx| this.show(Mode::Json, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowHits, window, cx| {
                this.show_view(ResultView::Hits, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowRawResult, window, cx| {
                this.show_view(ResultView::Raw, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousChunk, window, cx| {
                this.raw.update(cx, |r, cx| r.previous_chunk(window, cx));
            }))
            .on_action(cx.listener(|this, _: &NextChunk, window, cx| {
                this.raw.update(cx, |r, cx| r.next_chunk(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleInstant, _, cx| this.toggle_instant(cx)))
            .on_action(cx.listener(|this, _: &FocusQuery, window, cx| this.focus_query(window, cx)))
            .on_action(cx.listener(|this, _: &NextResultPage, window, cx| {
                this.turn_page(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousResultPage, window, cx| {
                this.turn_page(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &AddSearch, window, cx| this.add_search(window, cx)))
            .on_action(cx.listener(|this, _: &RemoveSearch, window, cx| {
                this.remove_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextSearch, window, cx| {
                this.step_search(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousSearch, window, cx| {
                this.step_search(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleUnion, window, cx| {
                this.toggle_union(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenHistory, window, cx| {
                this.toggle_list(QueryList::History, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSaved, window, cx| {
                this.toggle_list(QueryList::Saved, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPresets, window, cx| {
                this.toggle_list(QueryList::Presets, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SaveQuery, window, cx| this.ask_save(window, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.step_list(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.step_list(1, cx)))
            .on_action(cx.listener(|this, _: &LoadQuery, window, cx| {
                this.load_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteSavedQuery, window, cx| {
                this.ask_delete_saved(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenExport, window, cx| {
                this.toggle_export(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyCurl, _, cx| {
                this.copy_export(ExportLang::Curl, cx);
            }))
            .size_full()
            .child(tabs)
            .children(self.limit_note(cx))
            .child(toolbar)
            .children(self.write_note(cx))
            .children(self.instant_note(cx))
            .children(self.collections_note(cx))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("search.split")
                        .child(
                            resizable_panel()
                                .size(px(DRAFT_WIDTH))
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .child(draft),
                        )
                        .child(resizable_panel().child(result)),
                ),
            )
    }
}
