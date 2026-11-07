// SPDX-License-Identifier: GPL-3.0-or-later

//! Delete and update by query: a dialog with a filter, its live count and, for an update, the
//! fields to set.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, WeakEntity, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{Client, Collection};
use tessera_core::documents::CONFIRM_ABOVE;
use tessera_core::documents::export::{Danger, danger_level};
use tessera_core::documents::filter::names_everything;
use tessera_core::documents::validate::validate_document;
use tessera_widgets::confirm;
use tessera_widgets::confirm_danger::{self, DangerOptions};
use tessera_widgets::{FilterInput, FilterInputEvent, JsonEditor, JsonEditorEvent};

use super::super::env_badge;
use super::super::overview::grouped;
use super::inspector::marks_and_lines;
use super::{DocumentsView, facet_fetch, filter_fields};
use crate::actions::{BULK_DIALOG, BulkContinue};
use crate::connection::{ConnectionState, error_text, long_error_text};
use crate::documents::DocumentsBackendGlobal;

/// How long the count waits for the next keystroke.
const COUNT_DELAY: Duration = Duration::from_millis(300);
const WIDTH: f32 = 560.;
const EDITOR_HEIGHT: f32 = 160.;

/// What the dialog does to the matching documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkMode {
    /// Deletes them.
    Delete,
    /// Merges fields into them.
    Update,
}

/// What Continue settled on: the write the confirmation names.
#[derive(Clone)]
struct Job {
    filter: String,
    /// The fields to set; none deletes.
    partial: Option<Value>,
    count: u64,
    /// The confirmation said an unsaved draft may go with the documents.
    draft: bool,
}

struct Bulk {
    view: WeakEntity<DocumentsView>,
    connection: Entity<ConnectionState>,
    collection: String,
    mode: BulkMode,
    schema: Option<Collection>,
    filter: Entity<FilterInput>,
    editor: Option<Entity<JsonEditor>>,
    /// The answer for the filter as it stands.
    count: Option<u64>,
    error: Option<SharedString>,
    /// An update that reached fewer documents than it counted.
    mismatch: Option<SharedString>,
    /// Why the fields cannot be sent, then what does not fit the schema.
    issues: Vec<SharedString>,
    /// The count waiting or running; a newer filter replaces it, which cancels it.
    count_task: Option<Task<()>>,
    /// Continue's own count is running.
    checking: bool,
    /// The write is running.
    running: bool,
    continue_focus: FocusHandle,
    cancel_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// Opens the dialog with `filter`, focus in the filter.
pub fn open(
    view: WeakEntity<DocumentsView>,
    mode: BulkMode,
    filter: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(documents) = view.upgrade() else {
        return;
    };
    let (connection, collection, schema) = {
        let this = documents.read(cx);
        let schema = this.inspector.read(cx).schema().cloned();
        (this.connection.clone(), this.name.clone(), schema)
    };
    let fields = schema.as_ref().map(filter_fields).unwrap_or_default();
    let profile = connection.read(cx).profile.clone();
    let fetch = facet_fetch(
        connection.clone(),
        Rc::new(RefCell::new(collection.clone())),
    );
    let bulk = cx.new(|cx| {
        let input = cx.new(|cx| FilterInput::new(fields, fetch, window, cx));
        let mut subscriptions = vec![cx.subscribe(
            &input,
            |this: &mut Bulk, _, event: &FilterInputEvent, cx| {
                if matches!(event, FilterInputEvent::Changed(_)) {
                    this.filter_changed(cx);
                }
            },
        )];
        let editor = (mode == BulkMode::Update).then(|| {
            let editor = cx.new(|cx| JsonEditor::new("{}", false, window, cx));
            subscriptions.push(
                cx.subscribe(&editor, |this: &mut Bulk, _, _: &JsonEditorEvent, cx| {
                    this.recheck(cx)
                }),
            );
            editor
        });
        let mut this = Bulk {
            view,
            connection,
            collection,
            mode,
            schema,
            filter: input.clone(),
            editor,
            count: None,
            error: None,
            mismatch: None,
            issues: Vec::new(),
            count_task: None,
            checking: false,
            running: false,
            continue_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.recheck(cx);
        input.update(cx, |f, cx| f.set_value(&filter, window, cx));
        this
    });
    let title = match mode {
        BulkMode::Delete => t!("documents.bulk.delete_title"),
        BulkMode::Update => t!("documents.bulk.update_title"),
    }
    .to_string();
    let first_build = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_build.replace(false) {
            let input = bulk.read(cx).filter.clone();
            input.update(cx, |f, cx| f.focus_end(window, cx));
        }
        let (enter, cancel) = (bulk.clone(), bulk.clone());
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
            // The dialog's Enter binding wins over the focused button's own key handling.
            .on_ok(move |_, window, cx| enter.update(cx, |b, cx| b.press_focused(window, cx)))
            // Its result has nowhere else to show, and the close below would take another dialog.
            .on_cancel(move |_, _, cx| !cancel.read(cx).running)
            .child(bulk.clone())
    });
}

impl Bulk {
    /// The filter as it goes to the server; none when it may not (spec §7.8).
    fn target(&self, cx: &App) -> Option<String> {
        let text = self.filter.read(cx).value(cx);
        (!names_everything(&text)).then(|| text.trim().to_string())
    }

    /// The fields to set, when they can be sent.
    fn partial(&self, cx: &App) -> Option<Value> {
        let text = self.editor.as_ref()?.read(cx).text(cx);
        let value: Value = serde_json::from_str(&text).ok()?;
        let fields = value.as_object()?;
        (!fields.is_empty() && !fields.contains_key("id")).then_some(value)
    }

    fn can_continue(&self, cx: &App) -> bool {
        !self.checking
            && !self.running
            && self.count.is_some_and(|count| count > 0)
            && self.target(cx).is_some()
            && (self.mode == BulkMode::Delete || self.partial(cx).is_some())
    }

    /// Whether the inspector holds unsaved changes to a stored document.
    fn has_draft(&self, cx: &App) -> bool {
        self.view.upgrade().is_some_and(|view| {
            let inspector = view.read(cx).inspector.read(cx);
            inspector.current_id().is_some() && inspector.is_dirty(cx)
        })
    }

    fn inspector_busy(&self, cx: &App) -> bool {
        let view = self.view.upgrade();
        view.is_some_and(|view| view.read(cx).inspector.read(cx).is_busy())
    }

    fn client(&self, cx: &App) -> Option<Client> {
        let connection = self.connection.read(cx);
        connection.connected.as_ref().map(|c| c.client.clone())
    }

    /// Drops what was known of the old filter and counts the new one after the wait.
    fn filter_changed(&mut self, cx: &mut Context<Self>) {
        self.count_task = None;
        self.checking = false;
        (self.count, self.error, self.mismatch) = (None, None, None);
        if let Some(filter) = self.target(cx) {
            self.count_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(COUNT_DELAY).await;
                let Ok(counting) = this.update(cx, |this, cx| this.counting(filter, cx)) else {
                    return;
                };
                let answer = counting.await;
                this.update(cx, |this, cx| this.counted(answer, cx)).ok();
            }));
        }
        cx.notify();
    }

    fn counting(&self, filter: String, cx: &App) -> Task<Result<u64, SharedString>> {
        let Some(client) = self.client(cx) else {
            let text = t!("status.not_connected").to_string();
            return Task::ready(Err(text.into()));
        };
        let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
        let count = backend.count(client, self.collection.clone(), Some(filter));
        let profile = self.connection.read(cx).profile.clone();
        cx.background_spawn(async move { count.await.map_err(|e| error_text(&e, &profile).into()) })
    }

    fn counted(&mut self, answer: Result<u64, SharedString>, cx: &mut Context<Self>) {
        (self.count, self.error) = match answer {
            Ok(count) => (Some(count), None),
            Err(error) => (None, Some(error)),
        };
        cx.notify();
    }

    /// Checks the fields against the schema, limited to the keys present.
    fn recheck(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let text = editor.read(cx).text(cx);
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let found = match (&parsed, &self.schema) {
            (Some(partial), Some(schema)) => validate_document(schema, partial)
                .into_iter()
                .filter(|issue| issue.key != "document.missing_required")
                .collect(),
            _ => Vec::new(),
        };
        let (marks, mut lines) = marks_and_lines(found, &text);
        editor.update(cx, |e, cx| e.set_marks(marks, cx));
        let fields = parsed.as_ref().and_then(Value::as_object);
        if fields.is_some_and(|f| f.contains_key("id")) {
            lines.insert(0, t!("documents.bulk.partial_id").to_string().into());
        } else if parsed.is_some() && fields.is_none_or(|f| f.is_empty()) {
            lines.insert(0, t!("documents.bulk.partial_object").to_string().into());
        }
        self.issues = lines;
        cx.notify();
    }

    /// Enter: runs Continue when it holds the focus. Says whether the dialog closes.
    fn press_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.continue_focus.is_focused(window) {
            self.proceed(window, cx);
            return false;
        }
        self.cancel_focus.is_focused(window) && !self.running
    }

    /// Continue: unlocks if it must, counts once more and asks.
    fn proceed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_continue(cx) {
            return;
        }
        let this = cx.entity().downgrade();
        self.connection.clone().update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.count_again(window, cx))
                    .ok();
            });
        });
    }

    fn count_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(filter) = self.target(cx) else {
            return;
        };
        self.checking = true;
        let counting = self.counting(filter, cx);
        self.count_task = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = counting.await;
            this.update_in(cx, |this, window, cx| {
                this.checking = false;
                this.counted(answer, cx);
                this.ask(window, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Confirmation by the fresh count. Nothing is written until it is accepted.
    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (true, Some(count), Some(filter)) =
            (self.can_continue(cx), self.count, self.target(cx))
        else {
            return;
        };
        // Whether the row matches is not known here, so the draft is named when it may go.
        let draft = self.mode == BulkMode::Delete && self.has_draft(cx);
        if draft && self.inspector_busy(cx) {
            self.error = Some(t!("documents.delete.busy").to_string().into());
            return cx.notify();
        }
        let shown = grouped(count);
        let name = self.collection.clone();
        let partial = self.partial(cx);
        let fields = partial
            .iter()
            .filter_map(Value::as_object)
            .flat_map(|fields| fields.keys().cloned())
            .collect::<Vec<_>>()
            .join(", ");
        let (title, consequence, action) = match self.mode {
            BulkMode::Delete => (
                t!("documents.bulk.delete_title"),
                t!(
                    "documents.bulk.consequence_delete",
                    count = shown,
                    collection = name,
                    filter = filter
                ),
                t!("documents.delete.action"),
            ),
            BulkMode::Update => (
                t!("documents.bulk.update_title"),
                t!(
                    "documents.bulk.consequence_update",
                    count = shown,
                    collection = name,
                    filter = filter,
                    fields = fields
                ),
                t!("documents.bulk.update_action"),
            ),
        };
        let (title, consequence, action) = (
            title.to_string(),
            consequence.to_string(),
            action.to_string(),
        );
        let mut consequences = vec![consequence];
        if draft {
            consequences.push(t!("documents.bulk.draft").to_string());
        }
        let job = Job {
            filter,
            partial,
            count,
            draft,
        };
        let this = cx.entity().downgrade();
        let run = move |window: &mut Window, cx: &mut App| run(this.clone(), &job, window, cx);
        let prod = self.connection.read(cx).profile.environment.is_prod();
        match danger_level(count, CONFIRM_ABOVE, prod) {
            // The dialog is gone by then, so a failure shows here.
            Danger::Medium => confirm::open(
                title,
                consequences.join(" "),
                action,
                move |window, cx| run(window, cx).detach(),
                window,
                cx,
            ),
            Danger::High => confirm_danger::open(
                DangerOptions {
                    title: title.into(),
                    consequences: consequences.into_iter().map(Into::into).collect(),
                    expected: name.into(),
                    action_label: action.into(),
                    prod,
                },
                run,
                window,
                cx,
            ),
        }
    }
}

/// Sends `job`, outside the dialog's tasks: "Stop waiting" leaves it running.
fn run(
    bulk: WeakEntity<Bulk>,
    job: &Job,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(this) = bulk.upgrade() else {
        return Task::ready(Ok(()));
    };
    let Some(client) = this.read(cx).client(cx) else {
        let text = SharedString::from(t!("status.not_connected").to_string());
        this.update(cx, |b, cx| {
            b.error = Some(text.clone());
            cx.notify();
        });
        return Task::ready(Err(text));
    };
    let name = this.read(cx).collection.clone();
    let view = this.read(cx).view.clone();
    this.update(cx, |b, cx| {
        b.running = true;
        (b.error, b.mismatch) = (None, None);
        cx.notify();
    });
    view.update(cx, |v, cx| {
        v.deleting = true;
        cx.notify();
    })
    .ok();
    let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
    let call = match job.partial.clone() {
        Some(partial) => backend.update_by_filter(client, name, job.filter.clone(), partial),
        None => backend.delete_by_filter(client, name, job.filter.clone()),
    };
    let job = job.clone();
    window.spawn(cx, async move |cx| {
        let answer = call.await;
        cx.update(|window, cx| this.update(cx, |b, cx| b.finished(&job, answer, window, cx)))
            .unwrap_or(Ok(()))
    })
}

impl Bulk {
    /// Takes in the answer. `Err` is for `ConfirmDanger`, which keeps it in its dialog.
    fn finished(
        &mut self,
        job: &Job,
        answer: Result<u64, tessera_client::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), SharedString> {
        self.running = false;
        let profile = self.connection.read(cx).profile.clone();
        let (cleared, report, outcome) = match answer {
            Err(e) => {
                let text = SharedString::from(long_error_text(&e, &profile));
                self.error = Some(text.clone());
                (false, None, Err(text))
            }
            // A refused value answers 200 with fewer, or none (spec §4.4). A delete falls short
            // when others got there first.
            Ok(done) if done != job.count => {
                let (done, count) = (grouped(done), grouped(job.count));
                let text = match job.partial {
                    Some(_) => t!("documents.bulk.update_mismatch", done = done, count = count),
                    None => t!("documents.bulk.delete_mismatch", done = done, count = count),
                };
                self.mismatch = Some(text.to_string().into());
                (true, None, Ok(()))
            }
            Ok(done) => {
                let shown = grouped(done);
                let text = match job.partial {
                    Some(_) => t!("documents.bulk.updated", count = shown),
                    None => t!("documents.bulk.deleted", count = shown),
                };
                close_after_danger(window, cx);
                (true, Some(text.to_string().into()), Ok(()))
            }
        };
        self.view
            .update(cx, |v, cx| {
                v.bulk_finished(cleared, report, job.draft, window, cx)
            })
            .ok();
        // It reloads the rows, so `bulk_finished` does not.
        let name = self.collection.clone();
        self.connection
            .update(cx, |c, cx| c.collection_changed(name, cx));
        cx.notify();
        outcome
    }
}

/// Closes this dialog once `ConfirmDanger`, if one sits above it, has closed itself: its task
/// is polled after the one that called this, so two yields. After "Stop waiting" or the plain
/// confirmation, this dialog is the top one anyway.
fn close_after_danger(window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let mut yields = 0;
            std::future::poll_fn(|task| {
                yields += 1;
                if yields > 2 {
                    return std::task::Poll::Ready(());
                }
                task.waker().wake_by_ref();
                std::task::Poll::Pending
            })
            .await;
            cx.update(|window, cx| window.close_dialog(cx)).ok();
        })
        .detach();
}

fn line(id: &'static str, text: SharedString) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(text.clone())
        .text_sm()
        .child(text)
}

impl Render for Bulk {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = match (self.target(cx), self.count) {
            (None, _) => match self.mode {
                BulkMode::Delete => Some(t!("documents.bulk.no_filter_delete")),
                BulkMode::Update => Some(t!("documents.bulk.no_filter_update")),
            },
            (_, Some(0)) => Some(t!("documents.bulk.no_match")),
            (_, Some(count)) => Some(t!("documents.bulk.affects", count = grouped(count))),
            _ if self.error.is_some() => None,
            _ => Some(t!("documents.bulk.counting")),
        };
        let can_continue = self.can_continue(cx);
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;
        v_flex()
            .key_context(BULK_DIALOG)
            .on_action(cx.listener(|this, _: &BulkContinue, window, cx| this.proceed(window, cx)))
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .child(t!("documents.bulk.filter").to_string()),
            )
            .child(
                div()
                    .id("bulk.filter")
                    .test_support()
                    .child(self.filter.clone()),
            )
            .children(status.map(|text| line("bulk.count", text.to_string().into())))
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_color(danger).child(line("bulk.error", e))),
            )
            .when_some(self.editor.clone(), |el, editor| {
                el.child(
                    div()
                        .text_sm()
                        .child(t!("documents.bulk.changes").to_string()),
                )
                .child(
                    div()
                        .id("bulk.editor")
                        .test_support()
                        .h(px(EDITOR_HEIGHT))
                        .child(editor),
                )
            })
            .when(!self.issues.is_empty(), |el| {
                el.child(line("bulk.issues", self.issues.join("\n").into()))
            })
            .children(
                self.mismatch
                    .clone()
                    .map(|m| div().text_color(warning).child(line("bulk.partial", m))),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("bulk.cancel")
                            .label(t!("common.cancel").to_string())
                            .disabled(self.running)
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("bulk.continue")
                            .primary()
                            .label(t!("documents.bulk.continue").to_string())
                            .loading(self.checking || self.running)
                            .disabled(!can_continue)
                            .children(Kbd::binding_for_action(
                                &BulkContinue,
                                Some(BULK_DIALOG),
                                window,
                            ))
                            .track_focus(&self.continue_focus)
                            .on_click(cx.listener(|this, _, window, cx| this.proceed(window, cx))),
                    ),
            )
    }
}
