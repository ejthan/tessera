// SPDX-License-Identifier: GPL-3.0-or-later

//! The Ops tab: operations on one picked node, and the schema changes running on it (D85).

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window, div, prelude::*, px,
};
use jiff::Zoned;
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{RaftState, SchemaChange};
use tessera_core::operations::{
    CONFIG_KEYS, ConfigKind, ConfigProblem, Confirm, Operation, PathProblem, check_snapshot_path,
    confirm_for, parse_config,
};

use super::{Danger, client_of, confirm_write, notice, row_button};
use crate::actions::{RunClearCache, RunCompact, RunSnapshot, RunVote, SaveResource};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, node_error_text};
use crate::views::overview::role_text;

/// How often the running schema changes are read.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// Rows of the tab, the size of what it keeps per operation.
const OPERATIONS: usize = 5;

/// What a confirmed operation sends.
#[derive(Clone)]
enum Job {
    Snapshot(String),
    Vote,
    Compact,
    ClearCache,
    Config(String, Value),
}

impl Job {
    fn operation(&self) -> Operation {
        match self {
            Self::Snapshot(_) => Operation::Snapshot,
            Self::Vote => Operation::Vote,
            Self::Compact => Operation::Compact,
            Self::ClearCache => Operation::ClearCache,
            Self::Config(..) => Operation::Config,
        }
    }
}

/// How an operation ended, under its row. It names its own node, also after another is picked.
#[derive(Clone)]
struct RunResult {
    node: String,
    at: String,
    text: SharedString,
    failed: bool,
}

fn path_problem(problem: PathProblem) -> String {
    let key = match problem {
        PathProblem::Empty => "ops.path.empty",
        PathProblem::NotAbsolute => "ops.path.not_absolute",
        PathProblem::Root => "ops.path.root",
        PathProblem::Climbs => "ops.path.climbs",
        PathProblem::Control => "ops.path.control",
        PathProblem::TooLong => "ops.path.too_long",
    };
    t!(key).to_string()
}

fn config_problem(problem: ConfigProblem) -> String {
    match problem {
        ConfigProblem::NotInteger => t!("ops.config.not_integer").to_string(),
        ConfigProblem::TooLow(min) => t!("ops.config.too_low", min = min).to_string(),
        ConfigProblem::TooHigh => t!("ops.config.too_high", max = i32::MAX).to_string(),
        ConfigProblem::NotBool => t!("ops.config.not_bool").to_string(),
    }
}

/// The content of an Ops tab.
pub struct OperationsView {
    connection: Entity<ConnectionState>,
    node_select: Entity<SelectState<Vec<String>>>,
    /// The text of each node in the picker, indexed like `Client::nodes`.
    node_labels: Vec<String>,
    /// The picked node. Never changes the active node.
    node: usize,
    path: Entity<InputState>,
    config_select: Entity<SelectState<Vec<String>>>,
    /// Index into [`CONFIG_KEYS`].
    config_key: usize,
    config_value: Entity<InputState>,
    config_flag: bool,
    busy: [bool; OPERATIONS],
    results: [Option<RunResult>; OPERATIONS],
    changes: Vec<SchemaChange>,
    changes_error: Option<SharedString>,
    /// Polls while the tab draws: see [`Self::keep_polling`].
    poll: Option<Task<()>>,
    /// Drawn since the last poll.
    drawn: bool,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 5],
}

impl OperationsView {
    /// The tab of `connection`, on its active node.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let node = client_of(&connection, cx).map_or(0, |c| c.active_node());
        let names = CONFIG_KEYS.iter().map(|k| k.name.to_string()).collect();
        let first = Some(gpui_kit::component::IndexPath::default());
        let config_select = cx.new(|cx| SelectState::new(names, first, window, cx));
        let node_select = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let path = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("ops.snapshot.placeholder").to_string())
        });
        let config_value = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = [
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.take_nodes(window, cx);
            }),
            cx.subscribe_in(
                &node_select,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, _, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event {
                        this.pick_node(label, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &config_select,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, _, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.pick_config(name, cx);
                    }
                },
            ),
            cx.subscribe(&path, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&config_value, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            connection,
            node_select,
            node_labels: Vec::new(),
            node,
            path,
            config_select,
            config_key: 0,
            config_value,
            config_flag: false,
            busy: [false; OPERATIONS],
            results: Default::default(),
            changes: Vec::new(),
            changes_error: None,
            poll: None,
            drawn: false,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.take_nodes(window, cx);
        this
    }

    /// The connection this tab acts on.
    pub fn connection(&self) -> &Entity<ConnectionState> {
        &self.connection
    }

    /// Picks node `index`, as the picker does.
    pub fn pick_node_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(label) = self.node_labels.get(index).cloned() {
            self.pick_node(&label, cx);
        }
    }

    /// Picks the config key `name`, as the picker does.
    pub fn pick_config(&mut self, name: &str, cx: &mut Context<Self>) {
        self.config_key = CONFIG_KEYS
            .iter()
            .position(|k| k.name == name)
            .unwrap_or_default();
        cx.notify();
    }

    /// Whether the path on screen may go to the server.
    pub fn can_snapshot(&self, cx: &App) -> bool {
        check_snapshot_path(&self.snapshot_path(cx)).is_ok()
    }

    /// Keeps the picker's roles current; the connection changes with every poll.
    fn take_nodes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let labels = node_labels(&self.connection, cx);
        if labels == self.node_labels {
            return;
        }
        let picked = labels.get(self.node).cloned();
        self.node_labels.clone_from(&labels);
        self.node_select.update(cx, |s, cx| {
            s.set_items(labels, window, cx);
            if let Some(label) = picked {
                s.set_selected_value(&label, window, cx);
            }
        });
        cx.notify();
    }

    fn pick_node(&mut self, label: &str, cx: &mut Context<Self>) {
        if let Some(node) = self.node_labels.iter().position(|l| l == label) {
            self.node = node;
            self.changes.clear();
            self.changes_error = None;
            cx.notify();
        }
    }

    /// The host of node `index`.
    fn host(&self, index: usize, cx: &App) -> String {
        let client = client_of(&self.connection, cx);
        let url = client.and_then(|c| c.nodes().get(index).cloned());
        url.map(|u| host_of(&u)).unwrap_or_default()
    }

    fn role(&self, cx: &App) -> Option<RaftState> {
        let nodes = &self.connection.read(cx).nodes;
        nodes.get(self.node)?.debug.as_ref().map(|d| d.state)
    }

    /// Why a vote cannot help now: both of these answer `success: false` (P4).
    fn vote_block(&self, cx: &App) -> Option<String> {
        let nodes = client_of(&self.connection, cx).map_or(0, |c| c.nodes().len());
        if nodes < 2 {
            return Some(t!("ops.vote.one_node").to_string());
        }
        (self.role(cx) == Some(RaftState::Leader)).then(|| t!("ops.vote.leader").to_string())
    }

    fn snapshot_path(&self, cx: &App) -> String {
        self.path.read(cx).value().to_string()
    }

    /// The value of the config key on screen.
    fn config_parsed(&self, cx: &App) -> Result<Value, ConfigProblem> {
        let key = &CONFIG_KEYS[self.config_key];
        match key.kind {
            ConfigKind::Bool => Ok(self.config_flag.into()),
            ConfigKind::Integer { .. } => {
                parse_config(key, self.config_value.read(cx).value().trim())
            }
        }
    }

    fn ask(&mut self, job: Job, lines: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let op = job.operation();
        if self.busy[op as usize] {
            return;
        }
        let (view, connection, node) =
            (cx.entity().downgrade(), self.connection.clone(), self.node);
        let host = self.host(node, cx);
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let profile = &connection.read(cx).profile;
                let confirm = confirm_for(op, profile.environment.is_prod());
                let expected = profile.name.clone();
                let (v, conn, label) = (view.clone(), connection.clone(), host.clone());
                let run = move |_: &mut Window, cx: &mut App| {
                    send(
                        v.clone(),
                        conn.clone(),
                        job.clone(),
                        node,
                        label.clone(),
                        cx,
                    )
                };
                if confirm == Confirm::None {
                    return run(window, cx).detach();
                }
                let (title, action) = titles(op, &host);
                let danger = Danger {
                    title,
                    lines,
                    expected,
                    action,
                    typed: confirm == Confirm::Typed,
                };
                // The row shows every outcome, so nothing is left for on_error.
                confirm_write(danger, &connection, run, |_, _| {}, window, cx);
            });
        });
    }

    fn ask_snapshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.snapshot_path(cx);
        if check_snapshot_path(&path).is_err() {
            return;
        }
        let host = self.host(self.node, cx);
        let lines = vec![
            t!("ops.snapshot.where", path = path, node = host).to_string(),
            t!("ops.snapshot.room").to_string(),
            t!("ops.snapshot.used").to_string(),
            t!("ops.snapshot.long").to_string(),
        ];
        self.ask(Job::Snapshot(path), lines, window, cx);
    }

    fn ask_vote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vote_block(cx).is_some() {
            return;
        }
        let host = self.host(self.node, cx);
        let lines = vec![
            t!("ops.vote.becomes", node = host).to_string(),
            t!("ops.vote.pause").to_string(),
            t!("ops.vote.clients").to_string(),
        ];
        self.ask(Job::Vote, lines, window, cx);
    }

    fn ask_compact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host(self.node, cx);
        let lines = vec![t!("ops.compact.load", node = host).to_string()];
        self.ask(Job::Compact, lines, window, cx);
    }

    fn ask_clear_cache(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host(self.node, cx);
        let lines = vec![t!("ops.cache.latency", node = host).to_string()];
        self.ask(Job::ClearCache, lines, window, cx);
    }

    fn ask_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(value) = self.config_parsed(cx) else {
            return;
        };
        let key = CONFIG_KEYS[self.config_key].name.to_string();
        let host = self.host(self.node, cx);
        let lines = vec![
            t!("ops.config.sets", key = key, value = value, node = host).to_string(),
            t!("ops.config.note").to_string(),
        ];
        self.ask(Job::Config(key, value), lines, window, cx);
    }

    /// Starts polling when the tab draws. A round in which it did not draw ends the polling, so
    /// a hidden tab sends nothing and closing it stops the task with the view.
    fn keep_polling(&mut self, cx: &mut Context<Self>) {
        self.drawn = true;
        if self.poll.is_some() {
            return;
        }
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let backend = cx.global::<AdminBackendGlobal>().0.clone();
        let client = client.polling();
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(node)) = this.update(cx, |this, cx| {
                    let go = std::mem::take(&mut this.drawn).then_some(this.node);
                    if go.is_none() {
                        this.poll = None;
                    }
                    cx.notify();
                    go
                }) else {
                    return;
                };
                let answer = backend.schema_changes(client.clone(), node).await;
                let alive = this.update(cx, |this, cx| {
                    // An answer of a node no longer picked says nothing about the one on screen.
                    if this.node == node {
                        this.take_changes(answer, cx);
                    }
                });
                if alive.is_err() {
                    return;
                }
                cx.background_executor().timer(POLL_EVERY).await;
            }
        }));
    }

    fn take_changes(
        &mut self,
        answer: Result<Vec<SchemaChange>, tessera_client::Error>,
        cx: &mut Context<Self>,
    ) {
        match answer {
            Ok(changes) => {
                self.changes = changes;
                self.changes_error = None;
            }
            Err(e) => {
                let profile = &self.connection.read(cx).profile;
                let host = self.host(self.node, cx);
                self.changes_error = Some(node_error_text(&e, &host, profile).into());
            }
        }
        cx.notify();
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![
            (
                check_snapshot_path(&self.snapshot_path(cx)).is_err(),
                &RunSnapshot,
            ),
            (self.vote_block(cx).is_some(), &RunVote),
            (self.config_parsed(cx).is_err(), &SaveResource),
        ]
    }

    fn result_row(&self, op: Operation, cx: &App) -> Option<impl IntoElement + use<>> {
        let result = self.results[op as usize].as_ref()?;
        let theme = cx.theme();
        let color = if result.failed {
            theme.danger
        } else {
            theme.muted_foreground
        };
        let text = t!(
            "ops.result",
            at = result.at,
            node = result.node,
            text = result.text
        )
        .to_string();
        Some(notice(
            format!("ops.{}.result", key(op)),
            text.into(),
            color,
        ))
    }

    /// One operation: name, explanation, its controls and its result.
    fn row(
        &self,
        op: Operation,
        controls: AnyElement,
        below: Option<(String, SharedString, Hsla)>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let name = key(op);
        v_flex()
            .gap_1()
            .p_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!(format!("ops.{name}")).to_string()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("ops.{name}.explain")))
                    .text_sm()
                    .text_color(muted)
                    .child(t!(format!("ops.{name}.explain")).to_string()),
            )
            .child(h_flex().gap_2().items_center().child(controls))
            .children(below.map(|(id, text, color)| notice(id, text, color)))
            .children(self.result_row(op, cx))
    }

    fn snapshot_row(&self, locked: bool, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let problem = check_snapshot_path(&self.snapshot_path(cx)).err();
        let busy = self.busy[Operation::Snapshot as usize];
        let danger = cx.theme().danger;
        let controls = h_flex()
            .gap_2()
            .child(
                div().w(px(320.)).child(
                    Input::new(&self.path)
                        .small()
                        .id("ops.snapshot.path")
                        .aria_label(t!("ops.snapshot.path").to_string()),
                ),
            )
            .child(
                row_button(
                    "ops.snapshot.run".into(),
                    t!("ops.snapshot.run").to_string(),
                    &RunSnapshot,
                    (problem.is_some() || busy, locked),
                    &self.focus_handle,
                    window,
                    cx,
                )
                .danger()
                .loading(busy),
            );
        let below = problem.map(|p| {
            (
                "ops.snapshot.problem".to_string(),
                path_problem(p).into(),
                danger,
            )
        });
        self.row(Operation::Snapshot, controls.into_any_element(), below, cx)
    }

    fn plain_row(
        &self,
        op: Operation,
        action: &'static dyn Action,
        block: Option<String>,
        locked: bool,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let name = key(op);
        let busy = self.busy[op as usize];
        let button = row_button(
            format!("ops.{name}.run"),
            t!(format!("ops.{name}.run")).to_string(),
            action,
            (block.is_some() || busy, locked),
            &self.focus_handle,
            window,
            cx,
        )
        .loading(busy);
        let button = if matches!(op, Operation::ClearCache) {
            button
        } else {
            button.danger()
        };
        let muted = cx.theme().muted_foreground;
        let below = block.map(|b| (format!("ops.{name}.block"), b.into(), muted));
        self.row(op, button.into_any_element(), below, cx)
    }

    fn config_row(
        &mut self,
        locked: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let is_flag = matches!(CONFIG_KEYS[self.config_key].kind, ConfigKind::Bool);
        let problem = self.config_parsed(cx).err();
        let busy = self.busy[Operation::Config as usize];
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let value = if is_flag {
            Switch::new("ops.config.flag")
                .small()
                .checked(self.config_flag)
                .label(t!("ops.config.flag").to_string())
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.config_flag = *checked;
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div()
                .w(px(140.))
                .child(
                    Input::new(&self.config_value)
                        .small()
                        .id("ops.config.value")
                        .aria_label(t!("ops.config.value").to_string()),
                )
                .into_any_element()
        };
        let controls = h_flex()
            .gap_2()
            .child(
                div().w(px(280.)).child(
                    Select::new(&self.config_select)
                        .id("ops.config.key")
                        .small()
                        .accessibility_label(t!("ops.config.key").to_string()),
                ),
            )
            .child(value)
            .child(
                row_button(
                    "ops.config.run".into(),
                    t!("ops.config.run").to_string(),
                    &SaveResource,
                    (problem.is_some() || busy, locked),
                    &self.focus_handle,
                    window,
                    cx,
                )
                .loading(busy),
            );
        let below = match problem {
            Some(p) => (
                "ops.config.problem".to_string(),
                config_problem(p).into(),
                danger,
            ),
            None => (
                "ops.config.note".to_string(),
                t!("ops.config.note").to_string().into(),
                muted,
            ),
        };
        self.row(
            Operation::Config,
            controls.into_any_element(),
            Some(below),
            cx,
        )
    }

    fn changes_section(&self, cx: &App) -> impl IntoElement + use<> {
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let empty = self.changes.is_empty() && self.changes_error.is_none();
        let cell = |text: String, width: f32| div().w(px(width)).child(text);
        v_flex()
            .gap_1()
            .p_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("ops.schema").to_string()),
            )
            .when(empty, |el| {
                el.child(notice(
                    "ops.schema.empty",
                    t!("ops.schema.empty").to_string().into(),
                    muted,
                ))
            })
            .children(
                self.changes_error
                    .clone()
                    .map(|e| notice("ops.schema.error", e, danger)),
            )
            .when(!self.changes.is_empty(), |el| {
                el.child(
                    h_flex()
                        .text_sm()
                        .text_color(muted)
                        .child(cell(t!("ops.schema.collection").to_string(), 240.))
                        .child(cell(t!("ops.schema.validated").to_string(), 120.))
                        .child(cell(t!("ops.schema.altered").to_string(), 120.)),
                )
            })
            .children(self.changes.iter().map(|change| {
                let (validated, altered) = (change.validated_docs, change.altered_docs);
                h_flex()
                    .id(SharedString::from(format!(
                        "ops.schema.row.{}",
                        change.collection
                    )))
                    .test_support()
                    .aria_label(format!("{} {validated} {altered}", change.collection))
                    .text_sm()
                    .child(cell(change.collection.clone(), 240.))
                    .child(cell(validated.to_string(), 120.))
                    .child(cell(altered.to_string(), 120.))
            }))
    }
}

impl Focusable for OperationsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for OperationsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.keep_polling(cx);
        let locked = self.connection.read(cx).writes_locked();
        let vote_block = self.vote_block(cx);
        div()
            .id("ops.view")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .on_action(cx.listener(|this, _: &RunSnapshot, window, cx| {
                this.ask_snapshot(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RunCompact, window, cx| {
                this.ask_compact(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RunClearCache, window, cx| {
                this.ask_clear_cache(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RunVote, window, cx| this.ask_vote(window, cx)))
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| {
                this.ask_config(window, cx);
            }))
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .items_center()
                    .child(t!("ops.node").to_string())
                    .child(
                        div().w(px(320.)).child(
                            Select::new(&self.node_select)
                                .id("ops.node")
                                .small()
                                .accessibility_label(t!("ops.node").to_string()),
                        ),
                    ),
            )
            .child(self.snapshot_row(locked, window, cx))
            .child(self.plain_row(Operation::Compact, &RunCompact, None, locked, window, cx))
            .child(self.plain_row(
                Operation::ClearCache,
                &RunClearCache,
                None,
                locked,
                window,
                cx,
            ))
            .child(self.plain_row(Operation::Vote, &RunVote, vote_block, locked, window, cx))
            .child(self.config_row(locked, window, cx))
            .child(self.changes_section(cx))
    }
}

/// The part of the locale keys and ids of `op`.
fn key(op: Operation) -> &'static str {
    match op {
        Operation::Snapshot => "snapshot",
        Operation::Vote => "vote",
        Operation::Compact => "compact",
        Operation::ClearCache => "cache",
        Operation::Config => "config",
    }
}

/// A confirmation's title and action.
fn titles(op: Operation, host: &str) -> (String, String) {
    let name = key(op);
    (
        t!(format!("ops.{name}.title"), node = host).to_string(),
        t!(format!("ops.{name}.run")).to_string(),
    )
}

fn host_of(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port_or_known_default() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

fn node_labels(connection: &Entity<ConnectionState>, cx: &App) -> Vec<String> {
    let Some(client) = client_of(connection, cx) else {
        return Vec::new();
    };
    let nodes = &connection.read(cx).nodes;
    let urls = client.nodes().iter().enumerate();
    urls.map(|(i, url)| {
        let state = nodes.get(i).and_then(|n| n.debug.as_ref()).map(|d| d.state);
        let role = role_text(state.unwrap_or(RaftState::Unknown));
        format!("{} · {role}", host_of(url))
    })
    .collect()
}

/// Sends `job` to `node`. It runs on the backend's runtime whatever happens to the view, and its
/// outcome shows under its row, and in a typed dialog that still waits.
fn send(
    view: WeakEntity<OperationsView>,
    connection: Entity<ConnectionState>,
    job: Job,
    node: usize,
    host: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let op = job.operation() as usize;
    let profile = connection.read(cx).profile.clone();
    let backend = cx.global::<AdminBackendGlobal>().0.clone();
    view.update(cx, |view, cx| {
        view.busy[op] = true;
        view.results[op] = None;
        cx.notify();
    })
    .ok();
    let run = match job {
        Job::Snapshot(path) => backend.snapshot(client, node, path),
        Job::Vote => backend.vote(client, node),
        Job::Compact => backend.compact(client, node),
        Job::ClearCache => backend.clear_cache(client, node),
        Job::Config(key, value) => backend.set_config(client, node, key, value),
    };
    let done = match op {
        o if o == Operation::Vote as usize => "ops.vote.started",
        o if o == Operation::Snapshot as usize => "ops.snapshot.done",
        o if o == Operation::Compact as usize => "ops.compact.done",
        o if o == Operation::ClearCache as usize => "ops.cache.done",
        _ => "ops.config.done",
    };
    cx.spawn(async move |cx| {
        let answer = run.await;
        let error = answer
            .as_ref()
            .err()
            .map(|e| SharedString::from(node_error_text(e, &host, &profile)));
        let text = match (&answer, &error) {
            (_, Some(text)) => text.clone(),
            // A refused vote is a result, not a failure (D88).
            (Ok(false), _) => t!("ops.vote.refused").to_string().into(),
            _ => t!(done).to_string().into(),
        };
        let result = RunResult {
            node: host,
            at: Zoned::now().strftime("%H:%M:%S").to_string(),
            text,
            failed: error.is_some(),
        };
        view.update(cx, |view, cx| {
            view.busy[op] = false;
            view.results[op] = Some(result);
            cx.notify();
        })
        .ok();
        error.map_or(Ok(()), Err)
    })
}
