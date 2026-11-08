// SPDX-License-Identifier: GPL-3.0-or-later

//! The inspector beside the table: one whole document in a JSON editor, checked against the
//! schema as it is edited, and saved through a review. A new document is created directly.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{Client, Collection, Error};
use tessera_core::documents::edit::{diff_entries, merge_patch};
use tessera_core::documents::validate::{DocIssue, IssueSeverity, key_position, validate_document};
use tessera_widgets::{JsonEditor, JsonEditorEvent, JsonMark, MarkSeverity};

use super::copy::pretty;
use super::review::{self, OnApply, ReviewData};
use crate::actions::{INSPECTOR, RevertDocument, ReviewDocument, ToggleSaveMode};
use crate::connection::{ConnectionState, error_text};
use crate::documents::DocumentsBackendGlobal;
use crate::views::env_badge;

/// How long typing rests before the document is checked again.
const CHECK_DELAY: Duration = Duration::from_millis(150);
const ISSUES_HEIGHT: f32 = 96.;

/// How the review saves an existing document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SaveMode {
    /// A merge patch of what changed.
    #[default]
    Patch,
    /// The whole document.
    Upsert,
}

/// What the inspector holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorMode {
    /// A document from the server.
    #[default]
    Existing,
    /// A draft the server has not seen.
    New,
}

/// A document was written, so the rows may be out of date.
pub struct DocumentSaved;

impl EventEmitter<DocumentSaved> for Inspector {}

/// The editor marks of `issues` in `text`, and their lines for the list underneath.
pub(super) fn marks_and_lines(
    issues: Vec<DocIssue>,
    text: &str,
) -> (Vec<JsonMark>, Vec<SharedString>) {
    let (mut marks, mut lines) = (Vec::new(), Vec::new());
    for issue in issues {
        let message = t!(issue.key, detail = issue.detail).to_string();
        lines.push(format!("{}: {message}", issue.path).into());
        let key = issue.path.split('[').next().unwrap_or_default();
        if let Some((line, start, end)) = key_position(text, key) {
            marks.push(JsonMark {
                line,
                start,
                end,
                message,
                severity: match issue.severity {
                    IssueSeverity::Error => MarkSeverity::Error,
                    IssueSeverity::Warning => MarkSeverity::Warning,
                    IssueSeverity::Info => MarkSeverity::Info,
                },
            });
        }
    }
    (marks, lines)
}

/// The last check of the editor's text.
#[derive(Default)]
struct Check {
    dirty: bool,
    /// Why the review cannot open.
    blocked: Option<SharedString>,
    issues: Vec<SharedString>,
}

/// How a go to ID ended. A failure leaves the inspector as it was.
pub enum GoToResult {
    /// The inspector shows the document.
    Found,
    /// The document exists, but the editor holds unsaved changes it would replace.
    Unsaved {
        /// The id looked up.
        id: String,
        /// The document, for [`Inspector::show_found`].
        document: Value,
    },
    /// Why the document could not be loaded.
    Failed(SharedString),
}

impl EventEmitter<GoToResult> for Inspector {}

/// One document of a collection, loaded on request.
pub struct Inspector {
    connection: Entity<ConnectionState>,
    name: String,
    schema: Option<Collection>,
    editor: Entity<JsonEditor>,
    id: Option<String>,
    base: Option<Value>,
    /// The text `base` was shown as.
    loaded_text: String,
    mode: InspectorMode,
    save_mode: SaveMode,
    error: Option<SharedString>,
    /// Why the last create or fetch before a review failed.
    save_error: Option<SharedString>,
    check: Check,
    /// The wait before the next check; a newer change replaces it.
    check_task: Option<Task<()>>,
    /// The load, fetch or create running; the editor is read-only meanwhile.
    task: Option<Task<()>>,
    /// That task is a create, which nothing may replace.
    creating: bool,
    focus_handle: FocusHandle,
    _editor_changed: Subscription,
}

impl Inspector {
    /// An empty inspector over collection `name` of `connection`.
    pub fn new(
        connection: Entity<ConnectionState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| JsonEditor::new("", false, window, cx));
        Self {
            _editor_changed: cx.subscribe(&editor, |this, _, _: &JsonEditorEvent, cx| {
                this.check_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(CHECK_DELAY).await;
                    this.update(cx, |this, cx| this.recheck(cx)).ok();
                }));
            }),
            connection,
            name,
            schema: None,
            editor,
            id: None,
            base: None,
            loaded_text: String::new(),
            mode: InspectorMode::Existing,
            save_mode: SaveMode::Patch,
            error: None,
            save_error: None,
            check: Check::default(),
            check_task: None,
            task: None,
            creating: false,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Takes the schema the document is checked against.
    pub fn set_schema(&mut self, schema: Collection, cx: &mut Context<Self>) {
        self.schema = Some(schema);
        self.recheck(cx);
    }

    /// The schema, once known.
    pub fn schema(&self) -> Option<&Collection> {
        self.schema.as_ref()
    }

    /// Loads document `id` and shows it; the old one stays until the answer.
    pub fn show(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.id = Some(id.clone());
        self.error = None;
        self.load(id, false, window, cx);
    }

    /// Like [`Self::show`], for an id typed by hand: reports through [`GoToResult`].
    pub fn go_to(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.load(id, true, window, cx);
    }

    /// Starts a draft from `template`.
    pub fn new_document(&mut self, template: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = InspectorMode::New;
        self.id = None;
        self.error = None;
        self.set_document(template, window, cx);
        self.editor.update(cx, |e, cx| e.focus(window, cx));
    }

    /// Back to the document as it was loaded. Waits out a running task: dropping a create
    /// would leave its outcome unknown, and a load is about to replace the document.
    pub fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.task.is_none()
            && let Some(base) = self.base.clone()
        {
            self.set_document(base, window, cx);
        }
    }

    /// Shows a document Go to ID found.
    pub fn show_found(
        &mut self,
        id: String,
        document: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_loaded(id, document, window, cx);
        cx.emit(GoToResult::Found);
    }

    fn show_loaded(
        &mut self,
        id: String,
        document: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode = InspectorMode::Existing;
        self.set_document(document, window, cx);
        self.id = Some(id);
        self.error = None;
    }

    /// Shows `document` as the unchanged state of the editor.
    fn set_document(&mut self, document: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.idle(cx);
        self.save_error = None;
        self.loaded_text = pretty(&document);
        self.base = Some(document);
        let text = self.loaded_text.clone();
        self.editor
            .update(cx, |e, cx| e.set_text(&text, window, cx));
        self.recheck(cx);
    }

    /// Nothing runs any more; typing is allowed again.
    fn idle(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.creating = false;
        self.editor.update(cx, |e, cx| e.set_read_only(false, cx));
        cx.notify();
    }

    fn busy(&mut self, task: Task<()>, cx: &mut Context<Self>) {
        self.task = Some(task);
        self.editor.update(cx, |e, cx| e.set_read_only(true, cx));
        cx.notify();
    }

    fn load(&mut self, id: String, go_to: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let load =
            cx.global::<DocumentsBackendGlobal>()
                .0
                .document(client, self.name.clone(), id.clone());
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            this.update_in(cx, |this, window, cx| {
                this.finish(id, go_to, result, window, cx)
            })
            .ok();
        });
        self.busy(task, cx);
    }

    fn finish(
        &mut self,
        id: String,
        go_to: bool,
        result: Result<Value, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.idle(cx);
        match result {
            Ok(document) if go_to && self.is_dirty(cx) => {
                cx.emit(GoToResult::Unsaved { id, document });
            }
            Ok(document) if go_to => self.show_found(id, document, window, cx),
            Ok(document) => self.show_loaded(id, document, window, cx),
            Err(e) => {
                let text = self.error_text(&e, cx);
                if go_to {
                    cx.emit(GoToResult::Failed(text));
                } else {
                    self.mode = InspectorMode::Existing;
                    self.base = None;
                    self.error = Some(text);
                }
            }
        }
    }

    fn client(&self, cx: &App) -> Option<Client> {
        let connected = self.connection.read(cx).connected.as_ref();
        connected.map(|c| c.client.clone())
    }

    fn error_text(&self, error: &Error, cx: &App) -> SharedString {
        error_text(error, &self.connection.read(cx).profile).into()
    }

    /// Whether `text`, which parsed to `parsed`, differs from the document as loaded.
    fn differs(&self, text: &str, parsed: Option<&Value>) -> bool {
        self.base.as_ref().is_some_and(|base| match parsed {
            Some(document) => document != base,
            None => text != self.loaded_text,
        })
    }

    /// Whether the editor holds changes that are not saved.
    pub fn is_dirty(&self, cx: &App) -> bool {
        let text = self.editor.read(cx).text(cx);
        let parsed = serde_json::from_str::<Value>(&text).ok();
        self.creating || self.differs(&text, parsed.as_ref())
    }

    /// Whether a load, a fetch or a create is running.
    pub fn is_busy(&self) -> bool {
        self.task.is_some()
    }

    /// Whether the header says there are unsaved changes, as of the last check. Cheap enough
    /// for the tab bar, which asks on every frame.
    pub fn shows_unsaved(&self) -> bool {
        self.creating || self.check.dirty
    }

    /// The key of the reason `parsed` cannot go to review.
    fn blocked(&self, parsed: Option<&Value>) -> Option<&'static str> {
        let Some(document) = parsed else {
            return Some("documents.inspector.reason.parse");
        };
        let Some(new) = document.as_object() else {
            return Some("documents.inspector.reason.not_object");
        };
        if self.mode == InspectorMode::New {
            return None;
        }
        let base = self.base.as_ref()?;
        if document.get("id") != base.get("id") {
            return Some("documents.inspector.reason.id_changed");
        }
        diff_entries(base.as_object()?, new)
            .is_empty()
            .then_some("documents.inspector.reason.unchanged")
    }

    /// Checks the text now: marks and lists what does not fit the schema, and decides
    /// whether the review can open.
    fn recheck(&mut self, cx: &mut Context<Self>) {
        self.check_task = None;
        let text = self.editor.read(cx).text(cx);
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let found = match (&parsed, &self.schema) {
            (Some(document), Some(schema)) => validate_document(schema, document),
            _ => Vec::new(),
        };
        let (marks, issues) = marks_and_lines(found, &text);
        self.editor.update(cx, |e, cx| e.set_marks(marks, cx));
        self.check = Check {
            dirty: self.differs(&text, parsed.as_ref()),
            blocked: self
                .blocked(parsed.as_ref())
                .map(|key| t!(key).to_string().into()),
            issues,
        };
        cx.notify();
    }

    /// Why the review cannot open, as of the last check.
    pub fn review_disabled_reason(&self) -> Option<SharedString> {
        self.check.blocked.clone()
    }

    /// ⌘↩ / Ctrl+↩: the review of an existing document, the create of a new one, after the
    /// unlock on a read-only connection.
    fn review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recheck(cx);
        if self.check.blocked.is_some() || self.task.is_some() || self.base.is_none() {
            return;
        }
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| match this.mode {
                    InspectorMode::New => this.create(window, cx),
                    InspectorMode::Existing => this.fetch_for_review(window, cx),
                })
                .ok();
            });
        });
    }

    /// The editor's document and the client to write it with, or the error says why not.
    fn draft(&mut self, cx: &mut Context<Self>) -> Option<(Value, Client)> {
        self.save_error = None;
        let text = self.editor.read(cx).text(cx);
        let document = serde_json::from_str(&text).ok()?;
        let client = self.client(cx);
        if client.is_none() {
            self.save_error = Some(t!("status.not_connected").to_string().into());
            cx.notify();
        }
        Some((document, client?))
    }

    /// Asks the server for the document, so the review diffs against what is stored now.
    fn fetch_for_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.base_id() else {
            return;
        };
        let Some((document, client)) = self.draft(cx) else {
            return;
        };
        let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
        let fetch = backend.document(client, self.name.clone(), id);
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = fetch.await;
            this.update_in(cx, |this, window, cx| {
                this.open_review(document, result, window, cx)
            })
            .ok();
        });
        self.busy(task, cx);
    }

    fn base_id(&self) -> Option<String> {
        let id = self.base.as_ref()?.get("id")?.as_str()?;
        Some(id.to_string())
    }

    fn open_review(
        &mut self,
        document: Value,
        fresh: Result<Value, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.idle(cx);
        let fresh = match fresh {
            Ok(fresh) => fresh,
            Err(e) => {
                self.save_error = Some(self.error_text(&e, cx));
                return;
            }
        };
        let stale = self.base.as_ref() != Some(&fresh);
        if stale {
            // Back from the review, Revert and the diff must mean the server's version.
            self.loaded_text = pretty(&fresh);
            self.base = Some(fresh.clone());
            self.recheck(cx);
        }
        let (Some(old), Some(new), Some(id)) =
            (fresh.as_object(), document.as_object(), self.base_id())
        else {
            return;
        };
        let entries = diff_entries(old, new);
        if entries.is_empty() {
            self.save_error = Some(t!("documents.review.same").to_string().into());
            return;
        }
        let mode = self.save_mode;
        let body = match mode {
            SaveMode::Patch => Value::Object(merge_patch(old, new)),
            SaveMode::Upsert => document.clone(),
        };
        let this = cx.entity().downgrade();
        let (sent, target) = (body.clone(), id.clone());
        let on_apply: OnApply = Rc::new(move |window, cx| {
            let (body, id) = (sent.clone(), target.clone());
            this.update(cx, |this, cx| this.send(mode, id, body, window, cx))
                .unwrap_or_else(|_| Task::ready(Ok(())))
        });
        let data = ReviewData {
            profile: self.connection.read(cx).profile.clone(),
            collection: self.name.clone(),
            id,
            mode,
            entries,
            body,
            stale,
            on_apply,
        };
        review::open(data, window, cx);
    }

    /// Writes the reviewed `body`. On success the response is the new base.
    fn send(
        &mut self,
        mode: SaveMode,
        id: String,
        body: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), SharedString>> {
        let Some(client) = self.client(cx) else {
            return Task::ready(Err(t!("status.not_connected").to_string().into()));
        };
        let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
        let write = match mode {
            SaveMode::Patch => backend.patch(client, self.name.clone(), id, body),
            SaveMode::Upsert => backend.upsert(client, self.name.clone(), body),
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(document) => {
                    this.set_document(document, window, cx);
                    cx.emit(DocumentSaved);
                    Ok(())
                }
                Err(e) => Err(this.error_text(&e, cx)),
            })
            .unwrap_or(Ok(()))
        })
    }

    /// Creates the draft: there is nothing to overwrite, so no review. A refusal stays
    /// beside the draft.
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((document, client)) = self.draft(cx) else {
            return;
        };
        let name = self.name.clone();
        let write = cx
            .global::<DocumentsBackendGlobal>()
            .0
            .create(client, name.clone(), document);
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(document) => {
                    this.mode = InspectorMode::Existing;
                    this.id = document.get("id").and_then(Value::as_str).map(Into::into);
                    this.set_document(document, window, cx);
                    // The count changed; this reloads the rows too.
                    this.connection
                        .update(cx, |c, cx| c.collection_changed(name, cx));
                }
                Err(e) => {
                    this.idle(cx);
                    this.save_error = Some(this.error_text(&e, cx));
                }
            })
            .ok();
        });
        self.creating = true;
        self.busy(task, cx);
    }

    fn toggle_save_mode(&mut self, cx: &mut Context<Self>) {
        if self.mode == InspectorMode::New {
            return;
        }
        self.save_mode = match self.save_mode {
            SaveMode::Patch => SaveMode::Upsert,
            SaveMode::Upsert => SaveMode::Patch,
        };
        cx.notify();
    }

    /// The id of the document shown or loading.
    pub fn current_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The editor.
    pub fn editor(&self) -> &Entity<JsonEditor> {
        &self.editor
    }

    /// Whether the keyboard is inside the inspector.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }

    /// The heading with the unsaved state and, once a document is there, how it is saved.
    fn header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let new = self.mode == InspectorMode::New;
        let heading = SharedString::from(
            if new {
                t!("documents.inspector.new")
            } else {
                t!("documents.inspector.title")
            }
            .to_string(),
        );
        let existing = self.base.is_some() && !new;
        let mode_button =
            |id: &'static str, label: &str, mode: SaveMode, cx: &mut Context<Self>| {
                Button::new(id)
                    .ghost()
                    .small()
                    .selected(self.save_mode == mode)
                    .label(t!(label).to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_mode = mode;
                        cx.notify();
                    }))
            };
        let patch = mode_button(
            "inspector.save_mode.patch",
            "documents.inspector.patch",
            SaveMode::Patch,
            cx,
        );
        let upsert = mode_button(
            "inspector.save_mode.upsert",
            "documents.inspector.upsert",
            SaveMode::Upsert,
            cx,
        );
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("documents.inspector.heading")
                            .test_support()
                            .aria_label(heading.clone())
                            .text_sm()
                            .text_color(muted)
                            .child(heading),
                    )
                    .when(self.check.dirty, |el| {
                        el.child(
                            div()
                                .id("inspector.dirty")
                                .test_support()
                                .text_sm()
                                .text_color(warning)
                                .child(t!("documents.inspector.dirty").to_string()),
                        )
                        .child(
                            Button::new("inspector.revert")
                                .ghost()
                                .small()
                                .label(t!("documents.inspector.revert").to_string())
                                .children(Kbd::binding_for_action(
                                    &RevertDocument,
                                    Some(INSPECTOR),
                                    window,
                                ))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.revert(window, cx)),
                                ),
                        )
                    }),
            )
            .when(existing, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_sm()
                        .text_color(muted)
                        .child(t!("documents.inspector.save_as").to_string())
                        .child(patch)
                        .child(upsert)
                        .children(Kbd::binding_for_action(
                            &ToggleSaveMode,
                            Some(INSPECTOR),
                            window,
                        )),
                )
            })
            .when(self.base.is_some(), |el| {
                el.child(self.save_row(window, cx))
            })
    }

    /// The Review or Create button, with the lock and why it is disabled.
    fn save_row(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let label = match self.mode {
            InspectorMode::Existing => t!("documents.inspector.review"),
            InspectorMode::New => t!("documents.inspector.create"),
        };
        let reason = self.check.blocked.clone();
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("inspector.review")
                    .primary()
                    .small()
                    .label(label.to_string())
                    .children(Kbd::binding_for_action(
                        &ReviewDocument,
                        Some(INSPECTOR),
                        window,
                    ))
                    .disabled(reason.is_some() || self.task.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.review(window, cx))),
            )
            .when(self.connection.read(cx).writes_locked(), |el| {
                el.child(env_badge::lock("inspector.review", false, cx))
            })
            .children(reason.map(|reason| {
                div()
                    .id("inspector.review.reason")
                    .test_support()
                    .aria_label(reason.clone())
                    .min_w_0()
                    .text_sm()
                    .text_color(muted)
                    .child(reason)
            }))
    }
}

impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let body = if let Some(error) = self.error.clone() {
            div()
                .id("documents.inspector.error")
                .test_support()
                .aria_label(error.clone())
                .text_sm()
                .text_color(danger)
                .child(error)
                .into_any_element()
        } else if self.base.is_some() {
            div()
                .flex_1()
                .min_h_0()
                .child(self.editor.clone())
                .into_any_element()
        } else {
            let key = if self.task.is_some() {
                "documents.inspector.loading"
            } else {
                "documents.inspector.empty"
            };
            let text = SharedString::from(t!(key).to_string());
            div()
                .id("documents.inspector.body")
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        let issues = (!self.check.issues.is_empty() && self.base.is_some()).then(|| {
            let lines = self.check.issues.clone();
            v_flex()
                .id("inspector.issues")
                .test_support()
                .aria_label(SharedString::from(lines.join("\n")))
                .max_h(px(ISSUES_HEIGHT))
                .overflow_y_scroll()
                .text_sm()
                .text_color(muted)
                .children(lines)
        });
        let header = self.header(window, cx);
        v_flex()
            .id("documents.inspector")
            .test_support()
            .key_context(INSPECTOR)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ReviewDocument, window, cx| this.review(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSaveMode, _, cx| this.toggle_save_mode(cx)))
            .on_action(cx.listener(|this, _: &RevertDocument, window, cx| this.revert(window, cx)))
            .size_full()
            .p_2()
            .gap_2()
            .child(header)
            .children(self.save_error.clone().map(|e| {
                div()
                    .id("inspector.save_error")
                    .test_support()
                    .aria_label(e.clone())
                    .text_sm()
                    .text_color(danger)
                    .child(e)
            }))
            .child(body)
            .children(issues)
    }
}
