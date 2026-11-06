// SPDX-License-Identifier: GPL-3.0-or-later

//! The search tab of a connection: a draft on the left, its result on the right.

pub mod form;

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
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
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::{Collection, Error};
use tessera_core::profile::Node;
use tessera_core::search::INSTANT_DEBOUNCE_MS;
use tessera_core::search::params::{
    ParamError, SearchDraft, SideEffect, params_text, parse_params, side_effects, to_pairs,
};
use tessera_core::search::results::SearchResult;
use tessera_widgets::{BodyView, JsonEditor, JsonEditorEvent};

use self::form::{FormEvent, SearchForm};
use super::dock::view_tab;
use super::overview::grouped;
use crate::actions::{
    FocusQuery, RunSearch, ShowForm, ShowHits, ShowJson, ShowRawResult, ToggleInstant, WORKBENCH,
};
use crate::connection::{ConnectionState, error_text};
use crate::search::{SearchAnswer, SearchBackendGlobal};
use crate::state::AppState;

/// Width of the draft's side of the split.
const DRAFT_WIDTH: f32 = 400.;

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

fn param_error_text(error: &ParamError) -> String {
    match error {
        ParamError::BadKey(key) => t!("search.param_error.bad_key", key = key),
        ParamError::Reserved(key) => t!("search.param_error.reserved", key = key),
        ParamError::BadValue(key) => t!("search.param_error.bad_value", key = key),
        ParamError::NotAnObject => t!("search.param_error.not_an_object"),
    }
    .to_string()
}

/// A connection's searches.
pub struct SearchWorkbench {
    _state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    drafts: Vec<SearchDraft>,
    active: usize,
    union: bool,
    mode: Mode,
    view: ResultView,
    form: Entity<SearchForm>,
    json: Entity<JsonEditor>,
    /// Why the JSON text is not used; the map stays and Run is disabled.
    json_error: Option<SharedString>,
    collection: Entity<SelectState<Vec<String>>>,
    /// The collection list this view took last.
    list_loaded: Option<std::time::Instant>,
    schema: Option<Collection>,
    raw: Entity<BodyView>,
    result: Option<SearchResult>,
    error: Option<SharedString>,
    running: bool,
    /// Identifies the newest run; an answer of an older one is dropped.
    generation: u64,
    /// The newest run. Replacing it drops the older read, which cancels its request.
    run_task: Option<Task<()>>,
    instant: bool,
    /// The wait before an instant run; a newer change replaces it.
    instant_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 4],
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
        let raw = cx.new(|cx| BodyView::new(window, cx));
        let instant = !connection.read(cx).profile.environment.is_prod();
        if connection.read(cx).collection_cache.collections.is_none() {
            connection.update(cx, |c, cx| c.load_collection_list(cx));
        }
        let mut this = Self {
            _subscriptions: [
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
                            this.sync_schema(window, cx);
                            this.changed(window, cx);
                        }
                    },
                ),
            ],
            _state: state,
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
            raw,
            result: None,
            error: None,
            running: false,
            generation: 0,
            run_task: None,
            instant,
            instant_task: None,
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
        self.drafts = drafts;
        self.active = 0;
        self.union = union;
        self.instant_task = None;
        self.sync_collection(window, cx);
        self.sync_schema(window, cx);
        self.sync_editors(window, cx);
        cx.notify();
    }

    /// The drafts.
    pub fn drafts(&self) -> &[SearchDraft] {
        &self.drafts
    }

    /// Whether the drafts run as one merged search.
    pub fn union(&self) -> bool {
        self.union
    }

    /// The last result; it stays while a newer run is going or failed.
    pub fn result(&self) -> Option<&SearchResult> {
        self.result.as_ref()
    }

    /// The form.
    pub fn form(&self) -> &Entity<SearchForm> {
        &self.form
    }

    /// The JSON editor.
    pub fn json(&self) -> &Entity<JsonEditor> {
        &self.json
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

    fn sync_schema(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.draft().collection.clone();
        let cache = &self.connection.read(cx).collection_cache;
        let alias = cache.aliases.iter().find(|a| a.name == name);
        let target = alias.map_or(name.as_str(), |a| &a.collection_name);
        self.schema = cache
            .collections
            .iter()
            .flatten()
            .find(|c| c.name == target)
            .cloned();
        let schema = self.schema.clone();
        self.form
            .update(cx, |f, cx| f.set_schema(&name, schema.as_ref(), window, cx));
    }

    /// Shows the draft's map in the editor on screen.
    fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let params = self.draft().params.clone();
        match self.mode {
            Mode::Form => self.form.update(cx, |f, cx| f.load(&params, window, cx)),
            Mode::Json => self
                .json
                .update(cx, |j, cx| j.set_text(&params_text(&params), window, cx)),
        }
    }

    fn set_param(
        &mut self,
        key: &str,
        value: Option<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let params = &mut self.drafts[self.active].params;
        if params.get(key) == value.as_ref() {
            return;
        }
        match value {
            Some(value) => {
                params.insert(key.to_string(), value);
            }
            None => {
                params.shift_remove(key);
            }
        }
        self.changed(window, cx);
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

    fn side_effects(&self) -> Vec<SideEffect> {
        side_effects(&self.draft().params, self.schema.as_ref())
    }

    /// Instant search would call a model or store something.
    fn instant_paused(&self) -> bool {
        self.side_effects()
            .iter()
            .any(|e| *e != SideEffect::Analytics)
    }

    /// There is nothing to send: no collection, or JSON that does not parse.
    fn run_blocked(&self) -> bool {
        self.draft().collection.is_empty() || self.json_error.is_some()
    }

    /// The draft changed: starts the wait for an instant run. Only the last change of a burst
    /// gets to run.
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.instant_task = None;
        cx.notify();
        if !self.instant || self.run_blocked() || self.instant_paused() {
            return;
        }
        self.instant_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(INSTANT_DEBOUNCE_MS))
                .await;
            this.update_in(cx, |this, window, cx| this.run(window, cx))
                .ok();
        }));
    }

    /// Runs the draft, replacing a run going or waiting. A conversation writes, so it first
    /// asks to unlock a read-only connection.
    pub fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.instant_task = None;
        if self.run_blocked() {
            return;
        }
        if !self.side_effects().contains(&SideEffect::Conversation) {
            return self.send(window, cx);
        }
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.send(window, cx)).ok();
            });
        });
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.draft();
        let pairs = match to_pairs(&draft.params) {
            Ok(pairs) => pairs,
            Err(error) => {
                self.error = Some(param_error_text(&error).into());
                return cx.notify();
            }
        };
        let Some(connected) = &self.connection.read(cx).connected else {
            return;
        };
        let search = cx.global::<SearchBackendGlobal>().0.search(
            connected.client.clone(),
            draft.collection.clone(),
            pairs,
            tags(&draft.params),
        );
        self.generation += 1;
        let generation = self.generation;
        self.running = true;
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

    fn take_answer(
        &mut self,
        answer: Result<SearchAnswer, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        match answer {
            Ok(answer) => {
                self.error = None;
                self.result = Some(answer.result);
                self.raw
                    .update(cx, |r, cx| r.set_body(answer.text, window, cx));
            }
            Err(error) => {
                let profile = &self.connection.read(cx).profile;
                self.error = Some(error_text(&error, profile).into());
            }
        }
        cx.notify();
    }

    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.json_error = None;
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
        h_flex()
            .flex_none()
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
                    .disabled(self.run_blocked())
                    .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
            )
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
        let (id, key) = if self.instant_paused() {
            ("search.instant.paused", "search.instant_paused")
        } else if self.side_effects().contains(&SideEffect::Analytics) {
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
        let json_error = self
            .json_error
            .clone()
            .filter(|_| self.json.read(cx).error().is_none());
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

    fn status(&self) -> Option<SharedString> {
        let result = self.result.as_ref()?;
        Some(
            t!(
                "search.status",
                found = grouped(result.found),
                out_of = grouped(result.out_of),
                ms = result.search_time_ms
            )
            .to_string()
            .into(),
        )
    }

    fn result_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let danger = cx.theme().danger;
        let on_raw = self.view == ResultView::Raw;
        let dimmed = self.error.is_some();
        v_flex()
            .size_full()
            .children(self.status().map(|status| {
                div()
                    .id("search.status")
                    .test_support()
                    .aria_label(status.clone())
                    .flex_none()
                    .p_1()
                    .text_sm()
                    .child(status)
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
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.view = if *ix == 0 {
                            ResultView::Hits
                        } else {
                            ResultView::Raw
                        };
                        cx.notify();
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
                    .when(dimmed, |el| el.opacity(0.5))
                    .when(on_raw, |el| el.child(self.raw.clone())),
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
            .on_action(cx.listener(|this, _: &ShowHits, _, cx| {
                this.view = ResultView::Hits;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowRawResult, _, cx| {
                this.view = ResultView::Raw;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleInstant, _, cx| this.toggle_instant(cx)))
            .on_action(cx.listener(|this, _: &FocusQuery, window, cx| this.focus_query(window, cx)))
            .size_full()
            .child(toolbar)
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
