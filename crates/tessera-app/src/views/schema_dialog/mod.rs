// SPDX-License-Identifier: GPL-3.0-or-later

//! The create and clone dialog (design §7.6): a Form | JSON switch over one draft, kept in
//! sync both ways ([`sync`]), validation, and Create, which sends exactly the JSON tab's
//! content once the connection allows writes.

mod form;
mod sync;

use std::cell::Cell;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::Enter;
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
use tessera_client::Collection;
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

/// The dialog's state; lives while the dialog is open. Closing it drops a running create.
pub struct SchemaDialog {
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    focus_handle: FocusHandle,
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
    /// Fields whose embedding credentials the clone blanked (design §5.4).
    blanked: Vec<String>,
    server_error: Option<SharedString>,
    busy: bool,
    task: Option<Task<()>>,
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
        Self::open(state, connection, title, draft, Vec::new(), window, cx)
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
        let view = Self::open(state, connection, title, draft, blanked, window, cx);
        let name = view.read(cx).form.opt(Opt::Name).clone();
        name.update(cx, |n, cx| n.select_all(window, cx));
        view
    }

    fn open(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        title: String,
        draft: SchemaDraft,
        blanked: Vec<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let profile = connection.read(cx).profile.clone();
        let view = cx.new(|cx| Self::new(state, connection, draft, blanked, window, cx));
        let name = view.read(cx).form.opt(Opt::Name).clone();
        let first_build = Cell::new(true);
        let dialog_view = view.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            let busy = dialog_view.clone();
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
                // Enter would close the dialog; Create is ⌘↩ / Ctrl+↩ instead.
                .on_ok(|_, _, _| false)
                // A create in flight finishes first, so its result lands in this dialog.
                .on_cancel(move |_, _, cx| !busy.read(cx).busy)
                .child(dialog_view.clone())
        });
        view
    }

    fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
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
            tab: Tab::Form,
            form: Form::new(&draft, window, cx),
            draft,
            json_error: None,
            json,
            syncing: 0,
            issues: Vec::new(),
            blanked,
            server_error: None,
            busy: false,
            task: None,
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
        !self.busy
            && !self.locked()
            && !has_errors(&self.issues)
            && self.form_problems(cx).is_empty()
    }

    fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        // The control that had focus may be gone with the other tab; keep it in the dialog.
        match tab {
            Tab::Json => self.json.update(cx, |e, cx| e.focus(window, cx)),
            Tab::Form if self.locked() => self.focus_handle.focus(window, cx),
            Tab::Form => self
                .form
                .opt(Opt::Name)
                .update(cx, |n, cx| n.focus(window, cx)),
        }
        cx.notify();
    }

    fn jump_to_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(Tab::Json, window, cx);
        self.json.update(cx, |e, cx| e.jump_to_error(window, cx));
    }

    /// Create: asks for the unlock on a read-only connection first, then sends.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
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
        let (Some(connected), profile) = (&conn.connected, conn.profile.clone()) else {
            return;
        };
        let create = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .create(connected.client.clone(), draft.to_json());
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
                            .label(t!("schema.create").to_string())
                            .children(kbd(&SubmitSchema))
                            .loading(self.busy)
                            .disabled(!self.can_submit(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
