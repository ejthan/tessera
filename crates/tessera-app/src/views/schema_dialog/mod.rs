// SPDX-License-Identifier: GPL-3.0-or-later

//! The schema dialog. Create and clone (design §7.6): a Form | JSON switch over one draft,
//! kept in sync both ways ([`sync`]), validation, and Create, which sends exactly the JSON
//! tab's content once the connection allows writes. Edit (§7.7, [`review`]): the same draft
//! with the fixed parts read-only, then a review of the diff, then progress while it applies.

mod form;
mod review;
mod sync;

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Enter, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{Collection, SchemaChange};
use tessera_core::schema::{
    DraftError, FieldDraft, FieldType, Issue, IssuePath, SchemaDraft, Severity, has_errors,
};
use tessera_core::transfer::infer::{InferNote, InferredSchema};
use tessera_core::transfer::source::{ReadError, ReadOptions, Source, sniff};
use tessera_widgets::{JsonEditor, JsonEditorEvent, confirm};

use self::form::{Form, line};
use self::sync::{Opt, Text, pretty};
use super::env_badge;
use super::jobs::import_dialog::{Inspect, inspect};
use super::jobs::{infer_notes, note};
use crate::actions::{AddField, InferFromFile, SCHEMA_DIALOG, ShowForm, ShowJson, SubmitSchema};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::runtime::Tokio;
use crate::state::AppState;
use crate::transfer::{TransferBackend, TransferBackendGlobal, read_error_text};

const WIDTH: f32 = 900.;
const JSON_HEIGHT: f32 = 440.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Form,
    Json,
}

/// What Pick mode calls with the schema JSON.
type OnPick = dyn Fn(Value, &mut Window, &mut App);

/// What the dialog does with the draft.
enum Mode {
    Create,
    /// Changes this collection, masked as the schema view shows it (design §5.4): secrets
    /// never show, and the diff against it never sends an unchanged `embed`.
    Edit(Box<Collection>),
    /// Hands the validated JSON back and sends nothing.
    Pick(Rc<OnPick>),
}

/// Where an edit stands; create and clone stay at `Draft`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Draft,
    Review,
    Progress,
}

/// The dialog's state; lives while the dialog is open. Closing it drops a running create.
pub struct SchemaDialog {
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    focus_handle: FocusHandle,
    mode: Mode,
    step: Step,
    tab: Tab,
    /// The last draft the JSON parsed to; the form shows it.
    draft: SchemaDraft,
    /// Why the JSON text is no draft. The form is read-only while it is set.
    json_error: Option<DraftError>,
    json: Entity<JsonEditor>,
    form: Form,
    /// How many of the JSON's next `Changed` events are the form's own rewrites, not edits to
    /// refill the form from.
    syncing: usize,
    issues: Vec<Issue>,
    /// Fields whose embedding credentials the clone blanked, or whose changed embedding
    /// still holds the mask in edit mode (design §5.4).
    blanked: Vec<String>,
    /// Edit mode: why the draft cannot be reviewed beyond validation, as summary lines.
    edit_problems: Vec<String>,
    /// Edit mode: the draft changes nothing the server would take.
    unchanged: bool,
    /// The diff and its PATCH body while reviewing.
    review: Option<review::Review>,
    /// The last counts `schema_changes` gave for this collection.
    progress: Option<SchemaChange>,
    /// Back in the review, "Stop waiting" in the progress; focus starts there.
    safe_focus: FocusHandle,
    apply_focus: FocusHandle,
    server_error: Option<SharedString>,
    /// Notes of the last inference from a file, above the tabs.
    infer_notes: Vec<InferNote>,
    /// Why the last inference failed.
    infer_error: Option<SharedString>,
    /// The file picker or the inference; dropped with the dialog.
    infer_task: Option<Task<()>>,
    busy: bool,
    task: Option<Task<()>>,
    /// Polls `schema_changes` while a PATCH runs; dropped with the dialog.
    poll: Option<Task<()>>,
    _subscription: Subscription,
}

impl SchemaDialog {
    /// Opens the dialog for a new collection on `connection`.
    pub fn open_create(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let draft = SchemaDraft {
            fields: vec![FieldDraft::new("", FieldType::String)],
            ..Default::default()
        };
        let title = t!("schema.title_create").to_string();
        Self::open(
            state,
            connection,
            title,
            Mode::Create,
            draft,
            Vec::new(),
            window,
            cx,
        )
    }

    /// Opens the dialog filled from `source`, named "{name}_copy" (selected), with the
    /// embedding credentials blanked.
    pub fn open_clone(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        source: Collection,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let mut draft = SchemaDraft::from_collection(&source);
        let before = draft.fields.clone();
        draft.blank_secrets();
        let blanked = draft
            .fields
            .iter()
            .zip(&before)
            .filter(|(now, was)| now.embed != was.embed)
            .map(|(f, _)| f.name.clone())
            .collect();
        draft.name = format!("{}_copy", source.name);
        let title = t!("schema.title_clone", name = source.name).to_string();
        let view = Self::open(
            state,
            connection,
            title,
            Mode::Create,
            draft,
            blanked,
            window,
            cx,
        );
        let name = view.read(cx).form.opt(Opt::Name).clone();
        name.update(cx, |n, cx| n.select_all(window, cx));
        view
    }

    /// Opens the dialog to change `collection`'s schema. It masks the collection first, so
    /// it may come unmasked from the list's cache.
    pub fn open_edit(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        collection: Collection,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let old = review::masked(collection);
        let draft = SchemaDraft::from_collection(&old);
        let title = t!("schema.title_edit", name = old.name).to_string();
        Self::open(
            state,
            connection,
            title,
            Mode::Edit(Box::new(old)),
            draft,
            Vec::new(),
            window,
            cx,
        )
    }

    /// Opens the dialog on `draft` for another dialog: Use schema validates as Create does,
    /// closes this dialog and calls `on_pick` with the JSON. Nothing reaches the server, so
    /// nothing needs unlocking.
    pub fn open_pick(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        draft: SchemaDraft,
        on_pick: impl Fn(Value, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let title = t!("schema.title_create").to_string();
        let mode = Mode::Pick(Rc::new(on_pick));
        Self::open(
            state,
            connection,
            title,
            mode,
            draft,
            Vec::new(),
            window,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        title: String,
        mode: Mode,
        draft: SchemaDraft,
        blanked: Vec<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let profile = connection.read(cx).profile.clone();
        let view = cx.new(|cx| Self::new(state, connection, mode, draft, blanked, window, cx));
        let d = view.read(cx);
        let first = d
            .first_input()
            .map(|i| i.focus_handle(cx))
            .unwrap_or_else(|| d.focus_handle.clone());
        let first_build = Cell::new(true);
        let dialog_view = view.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                first.focus(window, cx);
            }
            let cancel = dialog_view.clone();
            let enter = dialog_view.clone();
            dialog
                .title(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(title.clone())
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(profile.name.clone()),
                        )
                        .child(env_badge::render(&profile.environment, cx)),
                )
                .w(px(WIDTH))
                // Enter would close the dialog; Create is ⌘↩ / Ctrl+↩ instead. In the review,
                // Enter activates the focused button.
                .on_ok(move |_, window, cx| {
                    enter.update(cx, |d, cx| d.press_focused(window, cx));
                    false
                })
                .on_cancel(move |_, window, cx| cancel.update(cx, |d, cx| d.cancel(window, cx)))
                .child(dialog_view.clone())
        });
        view
    }

    fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        mode: Mode,
        draft: SchemaDraft,
        blanked: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let json = cx.new(|cx| JsonEditor::new(&pretty(&draft), false, window, cx));
        let subscription =
            cx.subscribe_in(&json, window, |this, _, _: &JsonEditorEvent, window, cx| {
                this.json_changed(window, cx)
            });
        let mut this = Self {
            state,
            connection,
            focus_handle: cx.focus_handle(),
            mode,
            step: Step::Draft,
            tab: Tab::Form,
            form: Form::new(&draft, window, cx),
            draft,
            json_error: None,
            json,
            syncing: 0,
            issues: Vec::new(),
            blanked,
            edit_problems: Vec::new(),
            unchanged: false,
            review: None,
            progress: None,
            safe_focus: cx.focus_handle(),
            apply_focus: cx.focus_handle(),
            server_error: None,
            infer_notes: Vec::new(),
            infer_error: None,
            infer_task: None,
            busy: false,
            task: None,
            poll: None,
            _subscription: subscription,
        };
        this.refresh(window, cx);
        this
    }

    /// The JSON tab's editor.
    pub fn json_editor(&self) -> &Entity<JsonEditor> {
        &self.json
    }

    /// The JSON tab's text, the body Create sends.
    pub fn json_text(&self, cx: &App) -> String {
        self.json.read(cx).text(cx)
    }

    /// The JSON does not parse into a draft, so the form is read-only.
    fn locked(&self) -> bool {
        self.json_error.is_some()
    }

    /// The dialog changes an existing collection.
    fn editing(&self) -> bool {
        matches!(self.mode, Mode::Edit(_))
    }

    /// Where the cursor goes in the form: the name, or in edit mode, where the name is fixed,
    /// the first field's name (`None` when there is no field).
    fn first_input(&self) -> Option<Entity<InputState>> {
        match (&self.mode, self.form.rows.first()) {
            (Mode::Edit(_), row) => row.map(|r| r.input(Text::Name).clone()),
            (Mode::Create | Mode::Pick(_), _) => Some(self.form.opt(Opt::Name).clone()),
        }
    }

    /// Puts the cursor in [`Self::first_input`], or keeps focus in the dialog.
    fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.first_input() {
            Some(input) => input.update(cx, |i, cx| i.focus(window, cx)),
            None => self.focus_handle.focus(window, cx),
        }
    }

    /// Whether the draft names any field, which an inference would replace.
    fn has_named_field(&self) -> bool {
        self.draft.fields.iter().any(|f| !f.name.is_empty())
    }

    /// Picks a file and proposes the fields of its first rows. Nothing but the draft changes.
    fn infer_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing() || self.infer_task.is_some() {
            return;
        }
        if self.locked() {
            self.infer_error = Some(t!("schema.infer_locked").to_string().into());
            cx.notify();
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("schema.infer").to_string().into()),
        });
        let tokio = cx.global::<Tokio>().0.clone();
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        self.infer_error = None;
        self.infer_task = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = match picked.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => infer_file(tokio, backend, path).await,
                    None => Err(String::new()),
                },
                Ok(Err(e)) => Err(t!("connect.error.picker", detail = e.to_string()).to_string()),
                // Cancelled.
                Ok(Ok(None)) | Err(_) => Err(String::new()),
            };
            this.update_in(cx, |this, window, cx| {
                this.infer_task = None;
                match answer {
                    Ok(inferred) => this.inferred(inferred, window, cx),
                    Err(text) if text.is_empty() => {}
                    Err(text) => this.infer_error = Some(text.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// A draft with fields asks first; focus starts on Cancel.
    fn inferred(&mut self, inferred: InferredSchema, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_named_field() {
            return self.apply_inferred(&inferred, window, cx);
        }
        let this = cx.entity().downgrade();
        confirm::open(
            t!("schema.infer_replace_title").to_string(),
            t!("schema.infer_replace_body").to_string(),
            t!("schema.infer_replace_action").to_string(),
            move |window, cx| {
                this.update(cx, |d, cx| d.apply_inferred(&inferred, window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    /// Fills the form and the JSON tab. The fields come from the file; everything else of the
    /// draft stays, and so does the sorting field while it can still sort.
    fn apply_inferred(
        &mut self,
        inferred: &InferredSchema,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.infer_notes = inferred.notes.clone();
        let proposed = inferred.to_draft(&self.draft.name);
        let mut draft = self.draft.clone();
        draft.fields = proposed.fields;
        draft.enable_nested_fields = proposed.enable_nested_fields;
        if !sync::sortable(&draft).contains(&draft.default_sorting_field) {
            draft.default_sorting_field.clear();
        }
        self.replace_draft(draft, window, cx);
    }

    fn use_geopoint(&mut self, field: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut draft = self.draft.clone();
        for f in draft.fields.iter_mut().filter(|f| f.name == field) {
            sync::retarget(f, |f| f.r#type = FieldType::Geopoint);
        }
        self.replace_draft(draft, window, cx);
    }

    /// Esc, the close button or the backdrop: in the review they go back to the draft; a
    /// create or change in flight finishes first, so its result lands in this dialog.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.step == Step::Review && !self.busy {
            self.back(window, cx);
            return false;
        }
        !self.busy
    }

    /// Form inputs whose text the draft could not take, as summary lines.
    fn form_problems(&self, cx: &App) -> Vec<String> {
        if self.locked() {
            return Vec::new();
        }
        let mut out: Vec<String> = self
            .form
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| sync::num_dim(&r.input(Text::NumDim).read(cx).value()).is_none())
            .map(|(i, _)| self.issue_line(i, t!("schema.field.num_dim_invalid").to_string()))
            .collect();
        if sync::metadata(&self.form.opt(Opt::Metadata).read(cx).value()).is_none() {
            out.push(t!("schema.metadata_invalid").to_string());
        }
        out
    }

    fn issue_line(&self, i: usize, message: String) -> String {
        let name = self
            .draft
            .fields
            .get(i)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let field = if name.is_empty() {
            t!("schema.field_n", n = i + 1).to_string()
        } else {
            name
        };
        t!("schema.issue_line", field = field, message = message).to_string()
    }

    fn can_submit(&self, cx: &App) -> bool {
        let edit_ok = !self.editing()
            || (self.edit_problems.is_empty() && self.blanked.is_empty() && !self.unchanged);
        !self.busy
            && self.step == Step::Draft
            && !self.locked()
            && !has_errors(&self.issues)
            && self.form_problems(cx).is_empty()
            && edit_ok
    }

    fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.step != Step::Draft {
            return;
        }
        self.tab = tab;
        // The control that had focus may be gone with the other tab; keep it in the dialog.
        match tab {
            Tab::Json => self.json.update(cx, |e, cx| e.focus(window, cx)),
            Tab::Form if self.locked() => self.focus_handle.focus(window, cx),
            Tab::Form => self.focus_first(window, cx),
        }
        cx.notify();
    }

    fn jump_to_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(Tab::Json, window, cx);
        self.json.update(cx, |e, cx| e.jump_to_error(window, cx));
    }

    /// Create: asks for the unlock on a read-only connection first, then sends. Edit: shows
    /// the review.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        if self.editing() {
            return self.show_review(window, cx);
        }
        if let Mode::Pick(on_pick) = &self.mode {
            let on_pick = on_pick.clone();
            if let Ok(draft) = SchemaDraft::from_json(&self.json_text(cx)) {
                window.close_dialog(cx);
                on_pick(draft.to_json(), window, cx);
            }
            return;
        }
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |d, cx| d.send(window, cx)).ok();
            });
        });
    }

    /// Sends the JSON tab's content as it is now.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let Ok(draft) = SchemaDraft::from_json(&self.json_text(cx)) else {
            return;
        };
        let conn = self.connection.read(cx);
        let profile = conn.profile.clone();
        let Some(client) = conn.connected.as_ref().map(|c| c.client.clone()) else {
            self.server_error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        let create = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .create(client, draft.to_json());
        self.busy = true;
        self.server_error = None;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = create.await;
            this.update_in(cx, |d, window, cx| {
                d.busy = false;
                match result {
                    Ok(created) => {
                        d.connection
                            .update(cx, |c, cx| c.invalidate_collections(cx));
                        window.close_dialog(cx);
                        d.state
                            .update(cx, |s, cx| s.open_collection(profile.id, created.name, cx));
                    }
                    Err(e) => d.server_error = Some(error_text(&e, &profile).into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_json(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        v_flex()
            .gap_2()
            .child(
                div()
                    .h(px(JSON_HEIGHT))
                    // The editor takes ⌘↩ as a line break; here it means Create.
                    .capture_action(cx.listener(|this, e: &Enter, window, cx| {
                        if e.secondary {
                            cx.stop_propagation();
                            this.submit(window, cx);
                        }
                    }))
                    .child(self.json.clone()),
            )
            .children(
                self.json_error
                    .as_ref()
                    .filter(|e| e.line.is_none())
                    .map(|e| {
                        line(
                            "schema.json_invalid".into(),
                            t!("schema.json_invalid", message = e.message.clone())
                                .to_string()
                                .into(),
                            danger,
                        )
                    }),
            )
    }

    fn render_summary(&self, cx: &App) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let mut lines: Vec<(String, gpui_kit::Hsla)> = if self.locked() {
            Vec::new()
        } else {
            self.issues
                .iter()
                .map(|x| {
                    let message = t!(x.key).to_string();
                    let text = match x.path {
                        IssuePath::Field(i) => self.issue_line(i, message),
                        IssuePath::Collection => message,
                    };
                    let color = match x.severity {
                        Severity::Error => theme.danger,
                        Severity::Warning => theme.warning,
                    };
                    (text, color)
                })
                .collect()
        };
        lines.extend(
            self.form_problems(cx)
                .into_iter()
                .chain(
                    self.edit_problems
                        .iter()
                        .filter(|_| !self.locked())
                        .cloned(),
                )
                // The row says it too, but embeddings are edited in the JSON tab.
                .chain(self.blanked.iter().filter(|_| self.editing()).map(|name| {
                    let i = self.draft.fields.iter().position(|f| &f.name == name);
                    self.issue_line(
                        i.unwrap_or_default(),
                        t!("schema.field.secret_blanked").to_string(),
                    )
                }))
                .map(|l| (l, theme.danger)),
        );
        if lines.is_empty() {
            return None;
        }
        let label: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        Some(
            v_flex()
                .id("schema.issues")
                .test_support()
                .aria_label(SharedString::from(label.join("\n")))
                .gap_1()
                .text_sm()
                .children(lines.into_iter().map(|(l, c)| div().text_color(c).child(l))),
        )
    }
}

impl Render for SchemaDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.step {
            Step::Draft => self.render_draft(window, cx).into_any_element(),
            Step::Review => self.render_review(window, cx).into_any_element(),
            Step::Progress => self.render_progress(cx).into_any_element(),
        };
        v_flex()
            .key_context(SCHEMA_DIALOG)
            .track_focus(&self.focus_handle)
            .on_action(
                cx.listener(|this, _: &ShowForm, window, cx| this.show_tab(Tab::Form, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowJson, window, cx| this.show_tab(Tab::Json, window, cx)),
            )
            .on_action(cx.listener(|this, _: &AddField, window, cx| this.add_field(window, cx)))
            .on_action(cx.listener(|this, _: &SubmitSchema, window, cx| this.submit(window, cx)))
            .on_action(
                cx.listener(|this, _: &InferFromFile, window, cx| this.infer_from_file(window, cx)),
            )
            .child(content)
    }
}

impl SchemaDialog {
    fn render_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kbd = |action: &dyn gpui_kit::Action| {
            Kbd::binding_for_action(action, Some(SCHEMA_DIALOG), window)
        };
        let tabs = ButtonGroup::new("schema.mode")
            .outline()
            .small()
            .child(
                Button::new("schema.mode.form")
                    .label(t!("schema.mode.form").to_string())
                    .selected(self.tab == Tab::Form)
                    .children(kbd(&ShowForm))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_tab(Tab::Form, window, cx)),
                    ),
            )
            .child(
                Button::new("schema.mode.json")
                    .label(t!("schema.mode.json").to_string())
                    .selected(self.tab == Tab::Json)
                    .children(kbd(&ShowJson))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_tab(Tab::Json, window, cx)),
                    ),
            );
        let body = match self.tab {
            Tab::Form => self.render_form(window, cx),
            Tab::Json => self.render_json(cx).into_any_element(),
        };
        let danger = cx.theme().danger;
        let submit_label = match self.mode {
            Mode::Edit(_) => t!("schema.review_changes"),
            Mode::Pick(_) => t!("schema.use"),
            Mode::Create => t!("schema.create"),
        };
        let infer = (!self.editing()).then(|| {
            Button::new("schema.infer")
                .outline()
                .small()
                .label(t!("schema.infer").to_string())
                .children(kbd(&InferFromFile))
                .loading(self.infer_task.is_some())
                .on_click(cx.listener(|this, _, window, cx| this.infer_from_file(window, cx)))
        });
        let notes = infer_notes(&self.infer_notes, cx, Self::use_geopoint);
        v_flex()
            .gap_3()
            .child(h_flex().justify_between().child(tabs).children(infer))
            .children(
                self.infer_error
                    .clone()
                    .map(|e| note("schema.infer_error", e.to_string(), danger)),
            )
            .children(notes)
            .child(body)
            .children(self.render_summary(cx))
            .children(
                self.server_error
                    .clone()
                    .map(|e| line("schema.server_error".into(), e, danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("schema.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .disabled(self.busy)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("schema.submit")
                            .primary()
                            .label(submit_label.to_string())
                            .children(kbd(&SubmitSchema))
                            .loading(self.busy)
                            .disabled(!self.can_submit(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

/// Reads the head of `path` for its format, then proposes a schema from its first rows.
async fn infer_file(
    tokio: tokio::runtime::Handle,
    backend: std::sync::Arc<dyn TransferBackend>,
    path: std::path::PathBuf,
) -> Result<InferredSchema, String> {
    let shown = path.clone();
    let head = match tokio.spawn_blocking(move || inspect(&shown)).await {
        Ok(Ok((_, head))) => head,
        Ok(Err(Inspect::NotAFile)) => return Err(t!("transfer.import.not_a_file").to_string()),
        Ok(Err(Inspect::Io(detail))) => return Err(read_error_text(&ReadError::Io(detail))),
        Err(e) => return Err(read_error_text(&ReadError::Io(e.to_string()))),
    };
    let (format, csv) = sniff(Some(&path), &head);
    let options = ReadOptions {
        format,
        csv,
        fields: Vec::new(),
    };
    let answer = backend.infer(Source::File(path), options).await;
    answer.map_err(|e| read_error_text(&e))
}
