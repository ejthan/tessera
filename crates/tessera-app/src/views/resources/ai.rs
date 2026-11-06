// SPDX-License-Identifier: GPL-3.0-or-later

//! The AI tab: conversation models and NL search models, each a list beside a form.

use gpui_kit::component::button::{Button, ButtonVariants as _};
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
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Map, Value};
use tessera_client::ModelKind;
use tessera_core::ai::{
    AI_SECRET_KEYS, ModelDraft, ModelProblem, create_body, draft_of, scrub, update_body,
};

use super::super::dock::view_tab;
use super::ai_form::ModelForm;
use super::ai_test::AiTestPanel;
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, row_button, toolbar,
};
use crate::actions::{
    DeleteResource, FocusResourceFilter, NewResource, NextControl, PreviousControl, RESOURCES,
    ReloadResource, SaveResource, ShowDocuments, ShowSchema,
};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, error_text};

/// The columns of conversation models.
const CONVERSATION_COLUMNS: &[ResourceColumn] = &[
    ("id", 200., false),
    ("model", 220., false),
    ("history", 160., false),
];
/// The columns of NL search models.
const NL_COLUMNS: &[ResourceColumn] = &[("id", 200., false), ("model", 220., false)];
/// Width of the list side.
const LIST_WIDTH: f32 = 480.;
const CONVERSATION_DOCS: &str =
    "https://typesense.org/docs/30.2/api/conversational-search-rag.html";
const NL_DOCS: &str = "https://typesense.org/docs/30.2/api/natural-language-search.html";

fn prefix(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Conversation => "ai.conversation",
        ModelKind::NlSearch => "ai.nl",
    }
}

fn text_of<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn row(kind: ModelKind, model: &Map<String, Value>) -> ListRow {
    let id = text_of(model, "id").to_string();
    let mut cells = vec![id.clone(), text_of(model, "model_name").to_string()];
    if kind == ModelKind::Conversation {
        cells.push(text_of(model, "history_collection").to_string());
    }
    ListRow {
        key: id,
        cells,
        warning: None,
    }
}

fn problem_text(problem: ModelProblem) -> String {
    match problem {
        ModelProblem::BadId => t!("resources.bad_name"),
        ModelProblem::NoModelName => t!("ai.problem.no_model"),
        ModelProblem::NoApiKey => t!("ai.problem.no_key"),
        ModelProblem::NoHistory => t!("ai.problem.no_history"),
        ModelProblem::BadMaxBytes => t!("ai.problem.bad_max_bytes"),
        ModelProblem::BadTtl => t!("ai.problem.bad_ttl"),
        ModelProblem::BadTemperature => t!("ai.problem.bad_temperature"),
        ModelProblem::HoldsMask => t!("ai.problem.holds_mask"),
    }
    .to_string()
}

/// The credentials in a body, kept apart only to scrub the answer.
fn secrets_of(body: &Value) -> Vec<SecretString> {
    AI_SECRET_KEYS
        .iter()
        .filter_map(|key| body.get(*key)?.as_str())
        .map(|secret| SecretString::from(secret.to_string()))
        .collect()
}

/// `text` without any of `secrets`, because a provider's message can echo them.
fn scrubbed(text: String, secrets: &[SecretString]) -> String {
    secrets
        .iter()
        .fold(text, |text, secret| scrub(&text, secret.expose_secret()))
}

/// What the right side shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pane {
    /// Nothing selected.
    Empty,
    /// The form of a model not sent yet.
    New,
    /// The form of the model of this id.
    Edit(String),
}

/// One kind of model.
pub struct ModelsView {
    kind: ModelKind,
    connection: Entity<ConnectionState>,
    list: Entity<ResourceList>,
    models: Vec<Map<String, Value>>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    pane: Pane,
    /// The model the form followed last, so that only a new one replaces it.
    followed: Option<String>,
    form: Entity<ModelForm>,
    test: Entity<AiTestPanel>,
    problem: Option<ModelProblem>,
    /// What a save that sent nothing has to say.
    note: Option<SharedString>,
    saving: bool,
    /// Why the last write failed, in full.
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl ModelsView {
    fn new(
        kind: ModelKind,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let columns = match kind {
            ModelKind::Conversation => CONVERSATION_COLUMNS,
            ModelKind::NlSearch => NL_COLUMNS,
        };
        let list = cx.new(|cx| ResourceList::new(prefix(kind), columns, window, cx));
        let subscription = cx.observe_in(&list, window, |this, _, window, cx| {
            this.follow_selection(window, cx);
        });
        let form = cx.new(|cx| ModelForm::new(kind, connection.clone(), window, cx));
        let test = cx.new(|cx| AiTestPanel::new(kind, connection.clone(), window, cx));
        let mut this = Self {
            kind,
            connection,
            list,
            models: Vec::new(),
            loaded: false,
            list_error: None,
            load_task: None,
            pane: Pane::Empty,
            followed: None,
            form,
            test,
            problem: None,
            note: None,
            saving: false,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    /// The form.
    pub fn form(&self) -> &Entity<ModelForm> {
        &self.form
    }

    /// The test panel.
    pub fn test_panel(&self) -> &Entity<AiTestPanel> {
        &self.test
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.connection
            .update(cx, |c, cx| c.load_collection_list(cx));
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx
            .global::<AdminBackendGlobal>()
            .0
            .ai_models(client, self.kind);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match answer {
                    Ok(models) => {
                        this.list_error = None;
                        this.take_models(models, cx);
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

    fn take_models(&mut self, mut models: Vec<Map<String, Value>>, cx: &mut Context<Self>) {
        models.sort_by(|a, b| text_of(a, "id").cmp(text_of(b, "id")));
        let kind = self.kind;
        let rows = models.iter().map(|m| row(kind, m)).collect();
        self.models = models;
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    fn selected_model(&self, cx: &App) -> Option<&Map<String, Value>> {
        let id = self.list.read(cx).selected(cx)?;
        self.models.iter().find(|m| text_of(m, "id") == id)
    }

    /// A newly selected row replaces the form and empties the key field.
    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        let Some(model) = self.selected_model(cx).cloned() else {
            if self.pane != Pane::New && self.pane != Pane::Empty {
                self.pane = Pane::Empty;
                self.followed = None;
                self.form.update(cx, |f, cx| f.clear_key(window, cx));
                self.test.update(cx, |t, cx| t.set_model(None, cx));
            }
            return;
        };
        let id = text_of(&model, "id").to_string();
        if self.followed.as_ref() == Some(&id) {
            return;
        }
        self.followed = Some(id.clone());
        self.error = None;
        self.problem = None;
        self.note = None;
        self.pane = Pane::Edit(id.clone());
        self.test.update(cx, |t, cx| t.set_model(Some(id), cx));
        let draft = draft_of(self.kind, &model);
        self.form
            .update(cx, |f, cx| f.show(&draft, false, window, cx));
    }

    fn ask_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = Pane::New;
        self.followed = None;
        self.error = None;
        self.problem = None;
        self.note = None;
        self.list.update(cx, |l, cx| l.select(None, cx));
        self.test.update(cx, |t, cx| t.set_model(None, cx));
        self.form.update(cx, |f, cx| {
            f.show(&ModelDraft::default(), true, window, cx);
        });
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editing = match &self.pane {
            Pane::New => None,
            Pane::Edit(id) => Some(id.clone()),
            Pane::Empty => return,
        };
        if self.saving {
            return;
        }
        self.error = None;
        self.problem = None;
        self.note = None;
        cx.notify();
        let form = self.form.read(cx);
        let typed = form.key_text(cx);
        let Some(draft) = form.draft(cx) else {
            self.error = Some(t!("ai.bad_more").to_string().into());
            return;
        };
        let stored = editing
            .as_ref()
            .and_then(|id| self.models.iter().find(|m| text_of(m, "id") == id));
        let body = match stored {
            Some(stored) => update_body(self.kind, stored, &draft, &typed),
            None => create_body(self.kind, &draft, &typed),
        };
        let body = match body {
            Ok(body) => body,
            Err(problem) => {
                self.problem = Some(problem);
                return;
            }
        };
        if editing.is_some() && body.as_object().is_some_and(Map::is_empty) {
            self.note = Some(t!("ai.nothing_changed").to_string().into());
            return;
        }
        let view = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                send_save(&view, editing, body, window, cx);
            });
        });
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.selected_model(cx) else {
            return;
        };
        let id = text_of(model, "id").to_string();
        let kind = self.kind;
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let mut lines = vec![t!("ai.delete_consequence").to_string()];
                if kind == ModelKind::Conversation {
                    lines.push(t!("ai.delete_history").to_string());
                }
                let danger = Danger {
                    title: t!("ai.delete_title", name = id).to_string(),
                    lines,
                    expected: id.clone(),
                    action: t!("ai.delete").to_string(),
                    typed: connection.read(cx).profile.environment.is_prod(),
                };
                let (view, conn) = (this.clone(), connection.clone());
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), conn.clone(), kind, id.clone(), cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![
            (self.selected_model(cx).is_none(), &DeleteResource),
            (self.pane == Pane::Empty, &SaveResource),
        ]
    }

    fn form_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let locked = self.connection.read(cx).writes_locked();
        let problem = self
            .problem
            .map(|p| notice("ai.problem", problem_text(p).into(), danger));
        let note = self.note.clone().map(|n| notice("ai.note", n, muted));
        v_flex()
            .child(self.form.clone())
            .children(problem)
            .children(note)
            .child(
                h_flex().p_2().justify_end().child(
                    Button::new("ai.save")
                        .primary()
                        .small()
                        .label(t!("ai.save").to_string())
                        .when(locked, |b| {
                            b.child(super::env_badge::lock("ai.save", false, cx))
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
            .when(matches!(self.pane, Pane::Edit(_)), |el| {
                el.child(self.test.clone())
            })
    }
}

impl Focusable for ModelsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ModelsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted, border) = (theme.danger, theme.muted_foreground, theme.border);
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.list.read(cx).is_empty(cx);
        let none = self.selected_model(cx).is_none();
        let (name, docs) = match self.kind {
            ModelKind::Conversation => ("ai.conversation", CONVERSATION_DOCS),
            ModelKind::NlSearch => ("ai.nl", NL_DOCS),
        };
        let right = if self.pane == Pane::Empty {
            div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("ai.pick").to_string())
                .into_any_element()
        } else {
            self.form_pane(window, cx).into_any_element()
        };
        let left = v_flex()
            .size_full()
            .child(toolbar(
                name,
                &self.list,
                &self.focus_handle,
                locked,
                window,
                cx,
            ))
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("ai.list_error", e, danger)),
            )
            .child(if empty {
                empty_state(name, docs).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            })
            .children(self.error.clone().map(|e| notice("ai.error", e, danger)))
            .child(div().flex_none().p_2().child(row_button(
                "ai.delete".into(),
                t!("ai.delete").to_string(),
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
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.save(window, cx)))
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
                h_resizable(SharedString::from(format!("{name}.split")))
                    .child(
                        resizable_panel()
                            .size(px(LIST_WIDTH))
                            .border_r_1()
                            .border_color(border)
                            .child(left),
                    )
                    .child(
                        resizable_panel().child(
                            div()
                                .id(SharedString::from(format!("{name}.right")))
                                .size_full()
                                .overflow_y_scroll()
                                .child(right),
                        ),
                    ),
            )
    }
}

impl ErrorSlot for ModelsView {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

/// Sends the model and takes what the server answers. The key leaves the field as the send
/// starts, and its copy for scrubbing goes with the answer. Runs on through a closed tab.
fn send_save(
    view: &Entity<ModelsView>,
    editing: Option<String>,
    body: Value,
    window: &mut Window,
    cx: &mut App,
) {
    let (kind, connection) = {
        let v = view.read(cx);
        (v.kind, v.connection.clone())
    };
    let Some(client) = client_of(&connection, cx) else {
        let text: SharedString = t!("status.not_connected").to_string().into();
        view.update(cx, |v, cx| {
            v.error = Some(text);
            cx.notify();
        });
        return;
    };
    let profile = connection.read(cx).profile.clone();
    let secrets = secrets_of(&body);
    let backend = cx.global::<AdminBackendGlobal>().0.clone();
    let save = match editing.clone() {
        Some(id) => backend.update_ai_model(client, kind, id, body),
        None => backend.create_ai_model(client, kind, body),
    };
    view.update(cx, |v, cx| {
        v.saving = true;
        v.form.update(cx, |f, cx| f.clear_key(window, cx));
        cx.notify();
    });
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        let saved = save.await;
        view.update(cx, |v, cx| {
            v.saving = false;
            match saved {
                Ok(answer) => {
                    v.error = None;
                    let id = editing.unwrap_or_else(|| text_of(&answer, "id").to_string());
                    let mut models = std::mem::take(&mut v.models);
                    match models.iter_mut().find(|m| text_of(m, "id") == id) {
                        // `PUT` merges, so the answer may name only part of the model.
                        Some(model) => model.extend(answer),
                        None => models.push(answer),
                    }
                    // The form reads again what the server holds now.
                    v.followed = None;
                    v.take_models(models, cx);
                    v.list.update(cx, |l, cx| l.select(Some(id), cx));
                }
                Err(e) => {
                    let text = scrubbed(error_text(&e, &profile), &secrets);
                    v.error = Some(text.into());
                }
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}

fn send_delete(
    view: WeakEntity<ModelsView>,
    connection: Entity<ConnectionState>,
    kind: ModelKind,
    id: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx
        .global::<AdminBackendGlobal>()
        .0
        .delete_ai_model(client, kind, id);
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

/// The content of the AI tab.
pub struct AiView {
    conversation: Entity<ModelsView>,
    nl: Entity<ModelsView>,
    on_nl: bool,
}

impl AiView {
    /// The models of `connection`; loads both kinds.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let conversation =
            cx.new(|cx| ModelsView::new(ModelKind::Conversation, connection.clone(), window, cx));
        let nl = cx.new(|cx| ModelsView::new(ModelKind::NlSearch, connection, window, cx));
        Self {
            conversation,
            nl,
            on_nl: false,
        }
    }

    /// The view on screen.
    pub fn current(&self) -> &Entity<ModelsView> {
        if self.on_nl {
            &self.nl
        } else {
            &self.conversation
        }
    }

    fn show(&mut self, on_nl: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.on_nl = on_nl;
        self.current().read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        self.current().read(cx).gates(cx)
    }
}

impl Focusable for AiView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.current().read(cx).focus_handle(cx)
    }
}

impl Render for AiView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .on_action(cx.listener(|this, _: &ShowDocuments, window, cx| {
                this.show(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSchema, window, cx| {
                this.show(true, window, cx);
            }))
            .child(
                TabBar::new("ai.views")
                    .underline()
                    .selected_index(usize::from(self.on_nl))
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        this.show(*ix == 1, window, cx);
                    }))
                    .child(view_tab(
                        "ai.view.conversation",
                        t!("ai.conversation").to_string().into(),
                        &ShowDocuments,
                        RESOURCES,
                        window,
                    ))
                    .child(view_tab(
                        "ai.view.nl",
                        t!("ai.nl").to_string().into(),
                        &ShowSchema,
                        RESOURCES,
                        window,
                    )),
            )
            .child(div().flex_1().min_h_0().child(self.current().clone()))
    }
}
