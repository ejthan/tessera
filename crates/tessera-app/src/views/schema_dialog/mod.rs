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
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{Collection, SchemaChange};
use tessera_core::schema::{
    DraftError, FieldDraft, FieldType, Issue, IssuePath, SchemaDraft, Severity, has_errors,
};
use tessera_widgets::{JsonEditor, JsonEditorEvent};

use self::form::{Form, line};
use self::sync::{Opt, Text, pretty};
use super::env_badge;
use crate::actions::{AddField, SCHEMA_DIALOG, ShowForm, ShowJson, SubmitSchema};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::state::AppState;

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
        v_flex()
            .gap_3()
            .child(tabs)
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
