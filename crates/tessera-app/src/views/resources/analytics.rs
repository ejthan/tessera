// SPDX-License-Identifier: GPL-3.0-or-later

//! The analytics rules of a connection: a form per rule, what reached its destination, and a
//! test event.

use std::time::{Duration, Instant};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_core::analytics::{
    RuleDraft, RuleProblem, RuleType, check_rule, event_types, test_event,
};
use tessera_core::relevance::param_safe;
use tessera_core::search::params::SearchDraft;
use tessera_core::search::results::parse_result;
use tessera_widgets::json_editor::JsonEditor;

use super::stopwords::words_of;
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, row_button, toolbar,
};
use crate::actions::{
    ANALYTICS, DeleteResource, FlushAnalytics, FocusResourceFilter, NewResource, NextControl,
    OpenTopInSearch, PreviousControl, RESOURCES, ReloadResource, SaveResource, SendTestEvent,
};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::state::AppState;

/// Where the notice and the empty state point to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/analytics-query-suggestions.html";
/// The columns of the rules.
const COLUMNS: &[ResourceColumn] = &[
    ("name", 180., false),
    ("type", 120., false),
    ("source", 140., false),
    ("event", 80., false),
    ("destination", 140., false),
    ("tag", 80., false),
];
/// The columns of the destination: a query or a document id, and what counts it.
const TOP_COLUMNS: &[ResourceColumn] = &[("q", 240., false), ("count", 100., true)];
/// Width of the list side.
const LIST_WIDTH: f32 = 560.;
/// Height of the destination rows.
const TOP_HEIGHT: f32 = 220.;
/// How long a flush gets before the destination is read again.
const FLUSH_SETTLES: Duration = Duration::from_secs(2);

fn text_of<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn row(rule: &Map<String, Value>) -> ListRow {
    let params = rule.get("params").and_then(Value::as_object);
    let name = text_of(rule, "name").to_string();
    ListRow {
        key: name.clone(),
        cells: vec![
            name,
            text_of(rule, "type").to_string(),
            text_of(rule, "collection").to_string(),
            text_of(rule, "event_type").to_string(),
            params.map_or_else(String::new, |p| {
                text_of(p, "destination_collection").to_string()
            }),
            text_of(rule, "rule_tag").to_string(),
        ],
        warning: None,
    }
}

fn problem_text(problem: RuleProblem) -> String {
    match problem {
        RuleProblem::BadName => t!("resources.bad_name"),
        RuleProblem::NoCollection => t!("analytics.problem.no_collection"),
        RuleProblem::NoDestination => t!("analytics.problem.no_destination"),
        RuleProblem::BadLimit => t!("analytics.problem.bad_limit"),
        RuleProblem::NoCounterField => t!("analytics.problem.no_counter_field"),
        RuleProblem::BadWeight => t!("analytics.problem.bad_weight"),
    }
    .to_string()
}

/// A rule's destination rows sort by this field, `None` for a rule without one.
fn sort_field(draft: &RuleDraft) -> Option<&str> {
    match draft.rule_type {
        RuleType::PopularQueries | RuleType::NohitsQueries => Some("count"),
        RuleType::Counter => Some(draft.counter_field.as_str()),
        RuleType::Log => None,
    }
}

/// `text` with the hint that the server limits events, when that is what it says.
fn with_rate_hint(text: String) -> String {
    if text.contains("rate limit") {
        format!("{text} {}", t!("analytics.rate_hint"))
    } else {
        text
    }
}

/// The rows of a destination search: the key is what the row stands for.
fn top_rows(text: &str, field: &str, query_rule: bool) -> Result<Vec<ListRow>, String> {
    let result = parse_result(text, ("", "")).map_err(|e| e.to_string())?;
    let cell = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    let rows = result.hits.iter().map(|hit| {
        let key = if query_rule {
            cell(hit.doc.get("q"))
        } else {
            hit.id.to_string()
        };
        ListRow {
            key: key.clone(),
            cells: vec![key, cell(hit.doc.get(field))],
            warning: None,
        }
    });
    Ok(rows.collect())
}

/// What the right side shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pane {
    /// Nothing selected.
    Empty,
    /// The form of a rule not sent yet.
    New,
    /// The form of the rule of this name.
    Edit(String),
    /// A rule of a type this version does not know: its JSON.
    Unknown,
}

/// The content of an analytics tab.
pub struct AnalyticsView {
    connection: Entity<ConnectionState>,
    state: Entity<AppState>,
    list: Entity<ResourceList>,
    rules: Vec<Map<String, Value>>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    pane: Pane,
    /// The rule the form followed last, so that only a new one replaces it.
    followed: Option<String>,
    /// What the form does not show, which goes back unchanged.
    draft: RuleDraft,
    name: Entity<InputState>,
    rule_type: Entity<SelectState<Vec<String>>>,
    source: Entity<SelectState<Vec<String>>>,
    event: Entity<SelectState<Vec<String>>>,
    destination: Entity<SelectState<Vec<String>>>,
    limit: Entity<InputState>,
    counter_field: Entity<InputState>,
    weight: Entity<InputState>,
    tag: Entity<InputState>,
    meta_fields: Entity<TextareaState>,
    capture: bool,
    expand: bool,
    json: Entity<JsonEditor>,
    problems: Vec<RuleProblem>,
    saving: bool,
    /// Why the last write failed, in full.
    error: Option<SharedString>,
    /// When the collections in the selects were read.
    names_from: Option<Instant>,
    top: Entity<ResourceList>,
    top_error: Option<SharedString>,
    top_task: Option<Task<()>>,
    /// What the last flush did.
    flush_note: Option<SharedString>,
    test_user: Entity<InputState>,
    test_target: Entity<InputState>,
    /// What the last test event did, and whether it failed.
    test_note: Option<(SharedString, bool)>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 3],
}

impl AnalyticsView {
    /// The rules of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("analytics", COLUMNS, window, cx));
        let select = |window: &mut Window, cx: &mut Context<Self>, items: Vec<String>| {
            cx.new(|cx| SelectState::new(items, None, window, cx).searchable(true))
        };
        let types = [
            RuleType::PopularQueries,
            RuleType::NohitsQueries,
            RuleType::Counter,
            RuleType::Log,
        ];
        let types = types.iter().map(|t| t.as_str().to_string()).collect();
        let rule_type = select(window, cx, types);
        let subscriptions = [
            cx.observe_in(&list, window, |this, _, window, cx| {
                this.follow_selection(window, cx);
            }),
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.sync_names(window, cx);
            }),
            cx.subscribe_in(
                &rule_type,
                window,
                |this, _, _: &SelectEvent<Vec<String>>, window, cx| {
                    this.type_changed(window, cx);
                },
            ),
        ];
        let source = select(window, cx, Vec::new());
        let event = select(window, cx, Vec::new());
        let destination = select(window, cx, Vec::new());
        let mut this = Self {
            connection,
            state,
            list,
            rules: Vec::new(),
            loaded: false,
            list_error: None,
            load_task: None,
            pane: Pane::Empty,
            followed: None,
            draft: RuleDraft::default(),
            name: cx.new(|cx| InputState::new(window, cx)),
            rule_type,
            source,
            event,
            destination,
            limit: cx.new(|cx| InputState::new(window, cx)),
            counter_field: cx.new(|cx| InputState::new(window, cx)),
            weight: cx.new(|cx| InputState::new(window, cx)),
            tag: cx.new(|cx| InputState::new(window, cx)),
            meta_fields: cx.new(|cx| TextareaState::new(window, cx).rows(3)),
            capture: false,
            expand: false,
            json: cx.new(|cx| JsonEditor::new("", true, window, cx)),
            problems: Vec::new(),
            saving: false,
            error: None,
            names_from: None,
            top: cx.new(|cx| ResourceList::new("analytics.top", TOP_COLUMNS, window, cx)),
            top_error: None,
            top_task: None,
            flush_note: None,
            test_user: cx.new(|cx| InputState::new(window, cx)),
            test_target: cx.new(|cx| InputState::new(window, cx)),
            test_note: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// Picks a type in the form of a new rule.
    pub fn pick_type(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.rule_type
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
        self.type_changed(window, cx);
    }

    /// Picks the source collection.
    pub fn pick_source(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.source
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
    }

    /// Picks the destination collection.
    pub fn pick_destination(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.destination
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
    }

    fn collection_names(&self, cx: &App) -> Vec<String> {
        let cache = &self.connection.read(cx).collection_cache;
        let collections = cache.collections.iter().flatten();
        collections.map(|c| c.name.clone()).collect()
    }

    /// New collections reach the selects, which keep what they hold.
    fn sync_names(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.connection.read(cx).collection_cache.loaded_at;
        if from == self.names_from {
            return;
        }
        self.names_from = from;
        let names = self.collection_names(cx);
        for select in [self.source.clone(), self.destination.clone()] {
            let held = select.read(cx).selected_value().cloned();
            let mut items = names.clone();
            items.extend(held.filter(|h| !names.contains(h)));
            select.update(cx, |s, cx| s.set_items(items, window, cx));
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.connection
            .update(cx, |c, cx| c.load_collection_list(cx));
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx.global::<AdminBackendGlobal>().0.rules(client);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match answer {
                    Ok(rules) => {
                        this.list_error = None;
                        this.take_rules(rules, cx);
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

    fn take_rules(&mut self, mut rules: Vec<Map<String, Value>>, cx: &mut Context<Self>) {
        rules.sort_by(|a, b| text_of(a, "name").cmp(text_of(b, "name")));
        let rows = rules.iter().map(row).collect();
        self.rules = rules;
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    fn selected_rule(&self, cx: &App) -> Option<&Map<String, Value>> {
        let name = self.list.read(cx).selected(cx)?;
        self.rules.iter().find(|r| text_of(r, "name") == name)
    }

    /// A newly selected row replaces the form.
    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        let Some(rule) = self.selected_rule(cx).cloned() else {
            if self.pane != Pane::New {
                self.pane = Pane::Empty;
                self.followed = None;
            }
            return;
        };
        let name = text_of(&rule, "name").to_string();
        if self.followed.as_ref() == Some(&name) {
            return;
        }
        self.followed = Some(name.clone());
        self.error = None;
        self.problems.clear();
        self.test_note = None;
        self.flush_note = None;
        match RuleDraft::from_rule(&rule) {
            Some(draft) => {
                self.pane = Pane::Edit(name);
                self.show(draft, window, cx);
                self.load_top(cx);
            }
            None => {
                self.pane = Pane::Unknown;
                let text = serde_json::to_string_pretty(&rule).unwrap_or_default();
                self.json.update(cx, |j, cx| j.set_text(&text, window, cx));
            }
        }
    }

    fn fill(
        &self,
        select: &Entity<SelectState<Vec<String>>>,
        mut items: Vec<String>,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A value that is gone from the server still shows what the rule holds.
        if !value.is_empty() && !items.iter().any(|i| i == value) {
            items.push(value.to_string());
        }
        let value = value.to_string();
        select.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_value(&value, window, cx);
        });
    }

    fn show(&mut self, draft: RuleDraft, window: &mut Window, cx: &mut Context<Self>) {
        let names = self.collection_names(cx);
        let set =
            |input: &Entity<InputState>, text: String, w: &mut Window, cx: &mut Context<_>| {
                input.update(cx, |i, cx| i.set_value(text, w, cx));
            };
        set(&self.name, draft.name.clone(), window, cx);
        let limit = draft.limit.map(|l| l.to_string()).unwrap_or_default();
        set(&self.limit, limit, window, cx);
        let weight = draft.weight.map(|w| w.to_string()).unwrap_or_default();
        set(&self.weight, weight, window, cx);
        set(&self.counter_field, draft.counter_field.clone(), window, cx);
        set(&self.tag, draft.rule_tag.clone(), window, cx);
        let meta = draft.meta_fields.join("\n");
        self.meta_fields
            .update(cx, |m, cx| m.set_value(meta, window, cx));
        self.capture = draft.capture_search_requests;
        self.expand = draft.expand_query;
        let type_name = draft.rule_type.as_str().to_string();
        self.rule_type
            .update(cx, |s, cx| s.set_selected_value(&type_name, window, cx));
        let events = event_types(draft.rule_type).iter();
        let events: Vec<String> = events.map(ToString::to_string).collect();
        self.fill(&self.event, events, &draft.event_type, window, cx);
        self.fill(&self.source, names.clone(), &draft.collection, window, cx);
        self.fill(&self.destination, names, &draft.destination, window, cx);
        self.draft = draft;
    }

    fn form_type(&self, cx: &App) -> RuleType {
        if self.pane != Pane::New {
            return self.draft.rule_type;
        }
        let name = self.rule_type.read(cx).selected_value();
        name.and_then(|n| RuleType::from_str(n)).unwrap_or_default()
    }

    /// The events follow the type of a new rule.
    fn type_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let events: Vec<String> = event_types(self.form_type(cx))
            .iter()
            .map(ToString::to_string)
            .collect();
        let first = events.first().cloned().unwrap_or_default();
        self.fill(&self.event, events, &first, window, cx);
        cx.notify();
    }

    fn ask_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = Pane::New;
        self.followed = None;
        self.error = None;
        self.problems.clear();
        self.list.update(cx, |l, cx| l.select(None, cx));
        let draft = RuleDraft {
            event_type: "search".into(),
            ..RuleDraft::default()
        };
        self.show(draft, window, cx);
        self.name.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// The form, over what it does not show.
    fn form(&self, cx: &App) -> RuleDraft {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let picked = |select: &Entity<SelectState<Vec<String>>>| {
            select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default()
        };
        let mut draft = self.draft.clone();
        if self.pane == Pane::New {
            draft.name = text(&self.name);
            draft.rule_type = self.form_type(cx);
        }
        draft.collection = picked(&self.source);
        draft.event_type = picked(&self.event);
        draft.destination = picked(&self.destination);
        draft.limit = text(&self.limit).parse().ok();
        draft.weight = text(&self.weight).parse().ok();
        draft.counter_field = text(&self.counter_field);
        draft.rule_tag = text(&self.tag);
        draft.capture_search_requests = self.capture;
        draft.expand_query = self.expand;
        draft.meta_fields = words_of(&self.meta_fields.read(cx).value());
        draft
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editing = match &self.pane {
            Pane::New => None,
            Pane::Edit(name) => Some(name.clone()),
            Pane::Empty | Pane::Unknown => return,
        };
        if self.saving {
            return;
        }
        let draft = self.form(cx);
        // An existing name is the server's, and stays one path segment on the way out.
        self.problems = check_rule(&draft);
        self.problems
            .retain(|p| editing.is_none() || *p != RuleProblem::BadName);
        self.error = None;
        cx.notify();
        if !self.problems.is_empty() {
            return;
        }
        let body = if editing.is_some() {
            draft.update_body()
        } else {
            draft.to_body()
        };
        let view = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |_, cx| {
                send_save(&view, editing.clone(), draft.name.clone(), body.clone(), cx);
            });
        });
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self.selected_rule(cx) else {
            return;
        };
        let name = text_of(rule, "name").to_string();
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let prod = connection.read(cx).profile.environment.is_prod();
                let danger = Danger {
                    title: t!("analytics.delete_title", name = name).to_string(),
                    lines: vec![t!("analytics.delete_consequence").to_string()],
                    expected: name.clone(),
                    action: t!("analytics.delete").to_string(),
                    typed: prod,
                };
                let (view, conn) = (this.clone(), connection.clone());
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), conn.clone(), name.clone(), cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }

    /// Reads the destination of the rule in the form.
    fn load_top(&mut self, cx: &mut Context<Self>) {
        self.top.update(cx, |l, cx| l.set_rows(Vec::new(), cx));
        self.top_error = None;
        let draft = self.draft.clone();
        let Some(field) = sort_field(&draft).map(str::to_string) else {
            return;
        };
        if !param_safe(&field) {
            self.top_error = Some(t!("analytics.bad_counter_field").to_string().into());
            return;
        }
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let sort_by = format!("{field}:desc");
        let query_rule = draft.rule_type != RuleType::Counter;
        let load =
            cx.global::<AdminBackendGlobal>()
                .0
                .top_documents(client, draft.destination, sort_by);
        self.top_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                let rows = answer.map_err(|e| error_text(&e, &this.connection.read(cx).profile));
                match rows.and_then(|text| top_rows(&text, &field, query_rule)) {
                    Ok(rows) => this.top.update(cx, |l, cx| l.set_rows(rows, cx)),
                    Err(text) => this.top_error = Some(text.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_in_search(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.draft.rule_type,
            RuleType::PopularQueries | RuleType::NohitsQueries
        ) {
            return;
        }
        let Some(q) = self.top.read(cx).selected(cx) else {
            return;
        };
        let mut draft = SearchDraft::new(&self.draft.collection);
        draft.params.insert("q".into(), q.into());
        let id = self.connection.read(cx).profile.id;
        self.state.update(cx, |s, cx| {
            s.open_search(id, Some((vec![draft], false)), None, cx);
        });
    }

    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |_, cx| send_flush(&view, cx));
        });
    }

    fn send_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.pane, Pane::Edit(_)) {
            return;
        }
        let user = self.test_user.read(cx).value().trim().to_string();
        let target = self.test_target.read(cx).value().trim().to_string();
        if user.is_empty() || target.is_empty() {
            self.test_note = Some((t!("analytics.test.required").to_string().into(), true));
            return cx.notify();
        }
        self.test_note = None;
        let event = test_event(&self.draft, &user, &target);
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let (view, conn, event) = (this.clone(), connection.clone(), event.clone());
                let run = move |_: &mut Window, cx: &mut App| {
                    send_event(view.clone(), conn.clone(), event.clone(), cx)
                };
                if connection.read(cx).profile.environment.is_prod() {
                    let danger = Danger {
                        title: t!("analytics.test.title").to_string(),
                        lines: vec![t!("analytics.test.consequence").to_string()],
                        expected: String::new(),
                        action: t!("analytics.test.send").to_string(),
                        typed: false,
                    };
                    confirm_write(
                        danger,
                        &connection,
                        run,
                        report_to(this.clone()),
                        window,
                        cx,
                    );
                } else {
                    run(window, cx).detach();
                }
            });
        });
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![(self.selected_rule(cx).is_none(), &DeleteResource)]
    }

    fn analytics_button(
        &self,
        id: &'static str,
        label: String,
        action: &'static dyn Action,
        disabled: bool,
        window: &Window,
    ) -> Button {
        let focus = self.focus_handle.clone();
        Button::new(id)
            .small()
            .label(label)
            .children(Kbd::binding_for_action(action, Some(ANALYTICS), window))
            .disabled(disabled)
            .on_click(move |_, window, cx| focus.dispatch_action(action, window, cx))
    }

    fn input(
        id: &'static str,
        label: String,
        state: &Entity<InputState>,
        disabled: bool,
    ) -> impl IntoElement + use<> {
        v_flex().gap_1().child(label.clone()).child(
            Input::new(state)
                .id(id)
                .aria_label(label)
                .disabled(disabled),
        )
    }

    fn select(
        id: &'static str,
        label: String,
        state: &Entity<SelectState<Vec<String>>>,
        disabled: bool,
    ) -> impl IntoElement + use<> {
        v_flex().gap_1().child(label.clone()).child(
            Select::new(state)
                .id(id)
                .accessibility_label(label.clone())
                .placeholder(label)
                .disabled(disabled),
        )
    }

    fn form_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let danger = theme.danger;
        let locked = self.connection.read(cx).writes_locked();
        let new = self.pane == Pane::New;
        let rule_type = self.form_type(cx);
        let query = matches!(
            rule_type,
            RuleType::PopularQueries | RuleType::NohitsQueries
        );
        let counter = rule_type == RuleType::Counter;
        let problems = self.problems.iter().enumerate().map(|(i, problem)| {
            notice(
                format!("analytics.problem.{i}"),
                problem_text(*problem).into(),
                danger,
            )
        });
        let problems: Vec<_> = problems.collect();
        let name = Self::input(
            "analytics.form.name",
            t!("analytics.form.name").to_string(),
            &self.name,
            !new,
        );
        let kind = Self::select(
            "analytics.form.type",
            t!("analytics.form.type").to_string(),
            &self.rule_type,
            !new,
        );
        let source = Self::select(
            "analytics.form.source",
            t!("analytics.form.source").to_string(),
            &self.source,
            false,
        );
        let event = Self::select(
            "analytics.form.event",
            t!("analytics.form.event").to_string(),
            &self.event,
            false,
        );
        let destination = (rule_type != RuleType::Log).then(|| {
            Self::select(
                "analytics.form.destination",
                t!("analytics.form.destination").to_string(),
                &self.destination,
                false,
            )
        });
        let limit = query.then(|| {
            Self::input(
                "analytics.form.limit",
                t!("analytics.form.limit").to_string(),
                &self.limit,
                false,
            )
        });
        let counter_field = counter.then(|| {
            Self::input(
                "analytics.form.counter_field",
                t!("analytics.form.counter_field").to_string(),
                &self.counter_field,
                false,
            )
        });
        let weight = counter.then(|| {
            Self::input(
                "analytics.form.weight",
                t!("analytics.form.weight").to_string(),
                &self.weight,
                false,
            )
        });
        let tag = Self::input(
            "analytics.form.tag",
            t!("analytics.form.tag").to_string(),
            &self.tag,
            false,
        );
        v_flex()
            .gap_2()
            .p_2()
            .child(name)
            .child(kind)
            .child(source)
            .child(event)
            .children(destination)
            .children(limit)
            .children(counter_field)
            .children(weight)
            .child(tag)
            .when(query, |el| {
                el.child(
                    Checkbox::new("analytics.form.capture")
                        .checked(self.capture)
                        .label(t!("analytics.form.capture").to_string())
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.capture = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    Checkbox::new("analytics.form.expand")
                        .checked(self.expand)
                        .label(t!("analytics.form.expand").to_string())
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.expand = *checked;
                            cx.notify();
                        })),
                )
                .child(t!("analytics.form.meta_fields").to_string())
                .child(
                    div().id("analytics.form.meta_fields").test_support().child(
                        Textarea::new(&self.meta_fields)
                            .aria_label(t!("analytics.form.meta_fields").to_string())
                            .h(px(64.)),
                    ),
                )
            })
            .children(problems)
            .child(
                h_flex().justify_end().child(
                    Button::new("analytics.save")
                        .primary()
                        .small()
                        .label(t!("analytics.save").to_string())
                        .when(locked, |b| {
                            b.child(super::env_badge::lock("analytics.save", false, cx))
                        })
                        .children(Kbd::binding_for_action(
                            &SaveResource,
                            Some(RESOURCES),
                            window,
                        ))
                        .loading(self.saving)
                        .disabled(self.saving)
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                ),
            )
    }

    fn destination_section(&self, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let locked = self.connection.read(cx).writes_locked();
        let query_rule = self.draft.rule_type != RuleType::Counter;
        let picked = self.top.read(cx).selected(cx).is_some();
        let title = t!(
            "analytics.top_title",
            destination = self.draft.destination,
            field = sort_field(&self.draft).unwrap_or_default()
        );
        v_flex()
            .gap_2()
            .p_2()
            .child(div().text_sm().child(title.to_string()))
            .children(
                self.top_error
                    .clone()
                    .map(|e| notice("analytics.top_error", e, danger)),
            )
            .child(div().h(px(TOP_HEIGHT)).child(self.top.clone()))
            .children(
                self.flush_note
                    .clone()
                    .map(|n| notice("analytics.flush_note", n, muted)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .when(query_rule, |el| {
                        el.child(self.analytics_button(
                            "analytics.open_search",
                            t!("analytics.open_search").to_string(),
                            &OpenTopInSearch,
                            !picked,
                            window,
                        ))
                    })
                    .child(
                        self.analytics_button(
                            "analytics.flush",
                            t!("analytics.flush").to_string(),
                            &FlushAnalytics,
                            false,
                            window,
                        )
                        .when(locked, |b| {
                            b.child(super::env_badge::lock("analytics.flush", false, cx))
                        }),
                    ),
            )
    }

    fn test_section(&self, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let locked = self.connection.read(cx).writes_locked();
        let target = if self.draft.event_type == "search" {
            t!("analytics.test.q")
        } else {
            t!("analytics.test.doc_id")
        };
        v_flex()
            .gap_2()
            .p_2()
            .child(
                div()
                    .text_sm()
                    .child(t!("analytics.test.title").to_string()),
            )
            .child(Self::input(
                "analytics.test.user",
                t!("analytics.test.user").to_string(),
                &self.test_user,
                false,
            ))
            .child(Self::input(
                "analytics.test.target",
                target.to_string(),
                &self.test_target,
                false,
            ))
            .children(self.test_note.clone().map(|(text, failed)| {
                let color = if failed { danger } else { muted };
                notice("analytics.test.note", text, color)
            }))
            .child(
                h_flex().justify_end().child(
                    self.analytics_button(
                        "analytics.test.send",
                        t!("analytics.test.send").to_string(),
                        &SendTestEvent,
                        false,
                        window,
                    )
                    .when(locked, |b| {
                        b.child(super::env_badge::lock("analytics.test.send", false, cx))
                    }),
                ),
            )
    }

    fn right_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let pane = match self.pane {
            Pane::Empty => div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("analytics.pick").to_string())
                .into_any_element(),
            Pane::Unknown => v_flex()
                .size_full()
                .gap_2()
                .p_2()
                .child(notice(
                    "analytics.unknown",
                    t!("analytics.unknown").to_string().into(),
                    muted,
                ))
                .child(div().flex_1().min_h_0().child(self.json.clone()))
                .into_any_element(),
            Pane::New => self.form_pane(window, cx).into_any_element(),
            Pane::Edit(_) => {
                let form = self.form_pane(window, cx);
                let has_rows = sort_field(&self.draft).is_some();
                v_flex()
                    .child(form)
                    .when(has_rows, |el| {
                        el.child(self.destination_section(window, cx))
                    })
                    .child(self.test_section(window, cx))
                    .into_any_element()
            }
        };
        div()
            .id("analytics.right")
            .size_full()
            .overflow_y_scroll()
            .child(pane)
    }
}

impl Focusable for AnalyticsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AnalyticsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, warning, border) = (theme.danger, theme.warning, theme.border);
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.list.read(cx).is_empty(cx);
        let none = self.selected_rule(cx).is_none();
        let right = self.right_pane(window, cx);
        let left = v_flex()
            .size_full()
            .child(notice(
                "analytics.flags",
                t!("analytics.flags_notice").to_string().into(),
                warning,
            ))
            .child(
                div().px_2().child(
                    Button::new("analytics.flags_docs")
                        .link()
                        .label(t!("resources.docs").to_string())
                        .on_click(|_, _, cx| cx.open_url(DOCS_URL)),
                ),
            )
            .child(toolbar(
                "analytics",
                &self.list,
                &self.focus_handle,
                locked,
                window,
                cx,
            ))
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("analytics.list_error", e, danger)),
            )
            .child(if empty {
                empty_state("analytics", DOCS_URL).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            })
            .children(
                self.error
                    .clone()
                    .map(|e| notice("analytics.error", e, danger)),
            )
            .child(div().flex_none().p_2().child(row_button(
                "analytics.delete".into(),
                t!("analytics.delete").to_string(),
                &DeleteResource,
                (none, locked),
                &self.focus_handle,
                window,
                cx,
            )));
        div()
            .key_context(ANALYTICS)
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| this.ask_new(window, cx)))
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.ask_delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &FlushAnalytics, window, cx| this.flush(window, cx)))
            .on_action(cx.listener(|this, _: &SendTestEvent, window, cx| {
                this.send_test(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenTopInSearch, _, cx| this.open_in_search(cx)))
            .on_action(|_: &NextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousControl, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(
                h_resizable("analytics.split")
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

impl ErrorSlot for AnalyticsView {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

/// Sends the rule and selects what came back. Runs on through a closed tab.
fn send_save(
    view: &Entity<AnalyticsView>,
    editing: Option<String>,
    name: String,
    body: Value,
    cx: &mut App,
) {
    let connection = view.read(cx).connection.clone();
    let Some(client) = client_of(&connection, cx) else {
        let text: SharedString = t!("status.not_connected").to_string().into();
        view.update(cx, |v, cx| {
            v.error = Some(text);
            cx.notify();
        });
        return;
    };
    let profile = connection.read(cx).profile.clone();
    let save = cx
        .global::<AdminBackendGlobal>()
        .0
        .save_rule(client, editing, body);
    view.update(cx, |v, cx| {
        v.saving = true;
        cx.notify();
    });
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        let saved = save.await;
        view.update(cx, |v, cx| {
            v.saving = false;
            match saved {
                Ok(_) => {
                    v.error = None;
                    v.pane = Pane::Empty;
                    // The form reads again what the server holds now.
                    v.followed = None;
                    v.list.update(cx, |l, cx| l.select(Some(name), cx));
                    v.reload(cx);
                }
                Err(e) => v.error = Some(error_text(&e, &profile).into()),
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}

fn send_delete(
    view: WeakEntity<AnalyticsView>,
    connection: Entity<ConnectionState>,
    name: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx
        .global::<AdminBackendGlobal>()
        .0
        .delete_rule(client, name);
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

fn send_event(
    view: WeakEntity<AnalyticsView>,
    connection: Entity<ConnectionState>,
    event: Value,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let send = cx
        .global::<AdminBackendGlobal>()
        .0
        .send_event(client, event);
    cx.spawn(async move |cx| {
        let note = match send.await {
            Ok(()) => (t!("analytics.test.sent").to_string().into(), false),
            Err(e) => (with_rate_hint(error_text(&e, &profile)).into(), true),
        };
        view.update(cx, |view, cx| {
            view.test_note = Some(note);
            cx.notify();
        })
        .ok();
        Ok(())
    })
}

/// Flushes, gives the server a moment, and reads the destination again.
fn send_flush(view: &Entity<AnalyticsView>, cx: &mut App) {
    let connection = view.read(cx).connection.clone();
    let Some(client) = client_of(&connection, cx) else {
        return;
    };
    let profile = connection.read(cx).profile.clone();
    let flush = cx.global::<AdminBackendGlobal>().0.flush(client);
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        let flushed = flush.await;
        let failed = view.update(cx, |view, cx| {
            let failed = flushed.is_err();
            match flushed {
                Ok(()) => {
                    view.error = None;
                    view.flush_note = Some(t!("analytics.flushed").to_string().into());
                }
                Err(e) => view.error = Some(error_text(&e, &profile).into()),
            }
            cx.notify();
            failed
        });
        if failed.unwrap_or(true) {
            return;
        }
        cx.background_executor().timer(FLUSH_SETTLES).await;
        view.update(cx, |view, cx| view.load_top(cx)).ok();
    })
    .detach();
}
