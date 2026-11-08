// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog that exports documents of a collection to a file. It is a read: nothing here
//! asks to unlock a read-only connection or confirms. Start asks where to save, then the job
//! runs on its own, whatever happens to this dialog.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::Collection;
use tessera_core::transfer::export::{ExportFormat, ExportJob, ExportScope, FieldPick, field_safe};
use tessera_widgets::{FilterInput, FilterInputEvent};

use super::super::documents::{facet_fetch, filter_fields};
use super::super::env_badge;
use super::super::overview::grouped;
use super::super::root::RootView;
use crate::actions::{EXPORT_DIALOG, StartTransfer};
use crate::connection::{ConnectionState, error_text};
use crate::documents::DocumentsBackendGlobal;
use crate::jobs::JobsGlobal;

const WIDTH: f32 = 560.;
/// How long the count waits for the next keystroke.
const COUNT_DELAY: Duration = Duration::from_millis(300);

/// Each format and its id, which is also the file extension.
const FORMATS: [(ExportFormat, &str); 3] = [
    (ExportFormat::Jsonl, "jsonl"),
    (ExportFormat::JsonArray, "json"),
    (ExportFormat::Csv, "csv"),
];

/// Which documents the dialog is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every document.
    All,
    /// Those matching the filter typed here.
    Filter,
    /// The rows ticked in the table.
    Selection,
}

/// How the fields are picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fields {
    All,
    Include,
    Exclude,
}

/// `id`, then each top-level field of `c` once: a name up to its first `.`, without the
/// wildcard ones.
pub fn field_names(c: &Collection) -> Vec<String> {
    let mut names = vec!["id".to_string()];
    for field in &c.fields {
        let top = field.name.split('.').next().unwrap_or_default();
        if !top.is_empty() && !top.contains('*') && !names.iter().any(|n| n == top) {
            names.push(top.to_string());
        }
    }
    names
}

/// The name the save dialog proposes.
fn suggested_name(collection: &str, format: ExportFormat) -> String {
    let extension = FORMATS
        .iter()
        .find(|(f, _)| *f == format)
        .map_or("jsonl", |(_, extension)| extension);
    format!("{collection}.{extension}")
}

/// The dialog's state. Closing it drops a count; a started job is not its own.
pub struct DocumentExportDialog {
    connection: Entity<ConnectionState>,
    collection: Collection,
    focus_handle: FocusHandle,
    filter: Entity<FilterInput>,
    selection: Vec<String>,
    scope: Scope,
    fields: Fields,
    names: Vec<String>,
    picked: BTreeSet<String>,
    format: ExportFormat,
    /// The answer for the scope as it stands.
    count: Option<u64>,
    error: Option<SharedString>,
    /// The count waiting or running; a newer one replaces it, which cancels it.
    count_task: Option<Task<()>>,
    /// The save dialog.
    path_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DocumentExportDialog {
    /// Opens the dialog on `collection`, set to the selection, else the filter, else all.
    pub fn open(
        connection: Entity<ConnectionState>,
        collection: Collection,
        filter: Option<String>,
        selection: Vec<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let profile = connection.read(cx).profile.clone();
        let view = cx.new(|cx| Self::new(connection, collection, filter, selection, window, cx));
        let (focus, filter) = (
            view.read(cx).focus_handle.clone(),
            view.read(cx).filter.clone(),
        );
        let filter_first = view.read(cx).scope == Scope::Filter;
        let first_build = Cell::new(true);
        let dialog_view = view.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                if filter_first {
                    filter.update(cx, |f, cx| f.focus_end(window, cx));
                } else {
                    focus.focus(window, cx);
                }
            }
            dialog
                .title(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(t!("transfer.export.title").to_string())
                        .child(
                            div()
                                .id("transfer.export.connection")
                                .test_support()
                                .aria_label(SharedString::from(profile.name.clone()))
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(profile.name.clone()),
                        )
                        .child(
                            div()
                                .id("transfer.export.env")
                                .test_support()
                                .aria_label(SharedString::from(env_badge::label(
                                    &profile.environment,
                                )))
                                .child(env_badge::render(&profile.environment, cx)),
                        ),
                )
                .w(px(WIDTH))
                // Enter would close the dialog; Start is ⌘↩ / Ctrl+↩ instead.
                .on_ok(|_, _, _| false)
                .child(dialog_view.clone())
        });
        view
    }

    fn new(
        connection: Entity<ConnectionState>,
        collection: Collection,
        filter: Option<String>,
        selection: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fetch = facet_fetch(
            connection.clone(),
            Rc::new(RefCell::new(collection.name.clone())),
        );
        let input = cx.new(|cx| FilterInput::new(filter_fields(&collection), fetch, window, cx));
        let subscription = cx.subscribe(&input, |this, _, event: &FilterInputEvent, cx| {
            if matches!(event, FilterInputEvent::Changed(_)) && this.scope == Scope::Filter {
                this.recount(cx);
            }
        });
        let scope = match (&filter, selection.is_empty()) {
            (_, false) => Scope::Selection,
            (Some(_), _) => Scope::Filter,
            _ => Scope::All,
        };
        input.update(cx, |f, cx| {
            f.set_value(filter.as_deref().unwrap_or_default(), window, cx)
        });
        let mut this = Self {
            connection,
            names: field_names(&collection),
            collection,
            focus_handle: cx.focus_handle(),
            filter: input,
            selection,
            scope,
            fields: Fields::All,
            picked: BTreeSet::new(),
            format: ExportFormat::Jsonl,
            count: None,
            error: None,
            count_task: None,
            path_task: None,
            _subscriptions: vec![subscription],
        };
        this.recount(cx);
        this
    }

    /// The scope as the job takes it: an empty filter is no filter, as in the table.
    pub fn export_scope(&self, cx: &App) -> ExportScope {
        match self.scope {
            Scope::All => ExportScope::All,
            Scope::Filter => match self.filter.read(cx).value(cx).trim() {
                "" => ExportScope::All,
                text => ExportScope::Filter(text.to_string()),
            },
            Scope::Selection => ExportScope::Selection(self.selection.clone()),
        }
    }

    /// Drops what was known of the old scope and counts the new one after the wait.
    fn recount(&mut self, cx: &mut Context<Self>) {
        self.count_task = None;
        (self.count, self.error) = (None, None);
        let filter = match self.export_scope(cx) {
            ExportScope::Selection(ids) => {
                self.count = Some(ids.len() as u64);
                return cx.notify();
            }
            ExportScope::All => None,
            ExportScope::Filter(text) => Some(text),
        };
        self.count_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COUNT_DELAY).await;
            let Ok(counting) = this.update(cx, |this, cx| this.counting(filter, cx)) else {
                return;
            };
            let answer = counting.await;
            this.update(cx, |this, cx| {
                (this.count, this.error) = match answer {
                    Ok(count) => (Some(count), None),
                    Err(error) => (None, Some(error)),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn counting(&self, filter: Option<String>, cx: &App) -> Task<Result<u64, SharedString>> {
        let connection = self.connection.read(cx);
        let Some(client) = connection.connected.as_ref().map(|c| c.client.clone()) else {
            let text = t!("status.not_connected").to_string();
            return Task::ready(Err(text.into()));
        };
        let profile = connection.profile.clone();
        let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
        let count = backend.count(client, self.collection.name.clone(), filter);
        cx.background_spawn(async move { count.await.map_err(|e| error_text(&e, &profile).into()) })
    }

    fn set_scope(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            if scope == Scope::Filter {
                self.filter.update(cx, |f, cx| f.focus_end(window, cx));
            }
            self.recount(cx);
        }
    }

    fn set_fields(&mut self, fields: Fields, cx: &mut Context<Self>) {
        self.fields = fields;
        cx.notify();
    }

    fn toggle(&mut self, name: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.picked.insert(name.to_string());
        } else {
            self.picked.remove(name);
        }
        cx.notify();
    }

    /// The picked names, in the order the dialog lists them.
    fn chosen(&self) -> Vec<String> {
        let chosen = self.names.iter().filter(|n| self.picked.contains(*n));
        chosen.cloned().collect()
    }

    /// Whether the pick leaves no field to export.
    fn empty_pick(&self) -> bool {
        match self.fields {
            Fields::All => false,
            Fields::Include => self.picked.is_empty(),
            Fields::Exclude => self.picked.len() >= self.names.len(),
        }
    }

    /// Whether the file will hold no id.
    fn lacks_id(&self) -> bool {
        match self.fields {
            Fields::All => false,
            Fields::Include => !self.picked.contains("id"),
            Fields::Exclude => self.picked.contains("id"),
        }
    }

    fn field_pick(&self) -> FieldPick {
        match self.fields {
            Fields::Exclude if self.picked.is_empty() => FieldPick::All,
            Fields::All => FieldPick::All,
            Fields::Include => FieldPick::Include(self.chosen()),
            Fields::Exclude => FieldPick::Exclude(self.chosen()),
        }
    }

    fn can_start(&self) -> bool {
        self.count.is_some() && !self.empty_pick()
    }

    /// Asks where to save; a cancelled prompt changes nothing.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start() {
            return;
        }
        let dir = std::env::home_dir().unwrap_or_default();
        let name = suggested_name(&self.collection.name, self.format);
        let picked = cx.prompt_for_new_path(&dir, Some(&name));
        self.path_task = Some(
            cx.spawn_in(window, async move |this, cx| match picked.await {
                Ok(Ok(Some(path))) => {
                    this.update_in(cx, |this, window, cx| this.run(path, window, cx))
                        .ok();
                }
                Ok(Err(e)) => {
                    let text = t!("connect.error.picker", detail = e.to_string()).to_string();
                    this.update(cx, |this, cx| {
                        this.error = Some(text.into());
                        cx.notify();
                    })
                    .ok();
                }
                // Cancelled.
                Ok(Ok(None)) | Err(_) => {}
            }),
        );
    }

    fn run(&mut self, target: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let label: SharedString = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let job = ExportJob {
            collection: self.collection.name.clone(),
            scope: self.export_scope(cx),
            fields: self.field_pick(),
            format: self.format,
            target,
            expected: self.count,
        };
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let connection = self.connection.clone();
        jobs.update(cx, |jobs, cx| {
            jobs.start_export(&connection, job, label, cx)
        });
        window.close_dialog(cx);
        RootView::reveal_jobs(window, cx);
    }

    fn render_scope(&self, cx: &Context<Self>) -> impl IntoElement {
        let scopes = [
            (Scope::All, "all"),
            (Scope::Filter, "filter"),
            (Scope::Selection, "selection"),
        ];
        h_flex()
            .gap_2()
            .items_center()
            .child(label("transfer.export.scope.label"))
            .children(scopes.into_iter().map(|(scope, id)| {
                Button::new(SharedString::from(format!("transfer.export.scope.{id}")))
                    .outline()
                    .small()
                    .label(t!(format!("transfer.export.scope.{id}")).to_string())
                    .selected(self.scope == scope)
                    .disabled(scope == Scope::Selection && self.selection.is_empty())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_scope(scope, window, cx)),
                    )
            }))
    }

    fn render_fields(&self, cx: &Context<Self>) -> impl IntoElement {
        let modes = [
            (Fields::All, "all"),
            (Fields::Include, "include"),
            (Fields::Exclude, "exclude"),
        ];
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;
        let unsafe_name = self.names.iter().any(|n| !field_safe(n));
        let list = (self.fields != Fields::All).then(|| {
            h_flex()
                .id("transfer.export.field_list")
                .gap_x_3()
                .gap_y_1()
                .flex_wrap()
                .max_h(px(120.))
                .overflow_y_scroll()
                .children(self.names.iter().map(|name| {
                    let picked = self.picked.contains(name);
                    let owned = name.clone();
                    Checkbox::new(SharedString::from(format!("transfer.export.field.{name}")))
                        .checked(picked)
                        .label(name.clone())
                        .disabled(!field_safe(name))
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle(&owned, *checked, cx);
                        }))
                }))
        });
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(label("transfer.export.fields.label"))
                    .children(modes.into_iter().map(|(mode, id)| {
                        Button::new(SharedString::from(format!("transfer.export.fields.{id}")))
                            .outline()
                            .small()
                            .label(t!(format!("transfer.export.fields.{id}")).to_string())
                            .selected(self.fields == mode)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_fields(mode, cx)))
                    })),
            )
            .children(list)
            .when(unsafe_name && self.fields != Fields::All, |el| {
                let text = t!("transfer.export.field_unsafe").to_string();
                el.child(note("transfer.export.field_unsafe", text, muted))
            })
            .when(self.empty_pick() && self.fields != Fields::All, |el| {
                let text = t!("transfer.export.empty_pick").to_string();
                el.child(note("transfer.export.empty_pick", text, danger))
            })
            .when(self.lacks_id() && !self.empty_pick(), |el| {
                let text = t!("transfer.export.no_id_note").to_string();
                el.child(note("transfer.export.no_id_note", text, muted))
            })
    }

    fn render_format(&self, cx: &Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(label("transfer.export.format.label"))
                    .children(FORMATS.into_iter().map(|(format, id)| {
                        Button::new(SharedString::from(format!("transfer.export.format.{id}")))
                            .outline()
                            .small()
                            .label(t!(format!("transfer.export.format.{id}")).to_string())
                            .selected(self.format == format)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.format = format;
                                cx.notify();
                            }))
                    })),
            )
            .when(self.format == ExportFormat::Csv, |el| {
                el.child(note(
                    "transfer.export.csv_note",
                    t!("transfer.export.csv_note").to_string(),
                    muted,
                ))
                .child(note(
                    "transfer.export.csv_formula_note",
                    t!("transfer.export.csv_formula_note").to_string(),
                    muted,
                ))
            })
    }
}

/// The name of a row of options.
fn label(key: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(90.))
        .text_sm()
        .child(t!(key).to_string())
}

fn note(id: &'static str, text: String, color: gpui_kit::Hsla) -> impl IntoElement {
    let text = SharedString::from(text);
    div()
        .id(id)
        .test_support()
        .aria_label(text.clone())
        .text_xs()
        .text_color(color)
        .child(text)
}

impl Render for DocumentExportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = match (self.scope, self.count) {
            (Scope::Selection, _) => Some(t!(
                "transfer.export.count_selection",
                count = grouped(self.selection.len() as u64)
            )),
            (_, Some(count)) => Some(t!("transfer.export.count", count = grouped(count))),
            _ if self.error.is_some() => None,
            _ => Some(t!("transfer.export.counting")),
        };
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let can_start = self.can_start();
        let scope = self.render_scope(cx);
        let fields = self.render_fields(cx);
        let format = self.render_format(cx);
        v_flex()
            .key_context(EXPORT_DIALOG)
            .track_focus(&self.focus_handle)
            .gap_2()
            .on_action(cx.listener(|this, _: &StartTransfer, window, cx| this.start(window, cx)))
            .child(scope)
            .when(self.scope == Scope::Filter, |el| {
                el.child(
                    div()
                        .id("transfer.export.filter")
                        .test_support()
                        .child(self.filter.clone()),
                )
            })
            .children(count.map(|text| note("transfer.export.count", text.to_string(), muted)))
            .children(
                self.error
                    .clone()
                    .map(|e| note("transfer.export.error", e.to_string(), danger)),
            )
            .child(fields)
            .child(format)
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("transfer.export.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("transfer.export.start")
                            .primary()
                            .label(t!("transfer.export.start").to_string())
                            .children(Kbd::binding_for_action(
                                &StartTransfer,
                                Some(EXPORT_DIALOG),
                                window,
                            ))
                            .disabled(!can_start)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(names: &[&str]) -> Collection {
        let fields: Vec<_> = names
            .iter()
            .map(|n| serde_json::json!({"name": n, "type": "string"}))
            .collect();
        serde_json::from_value(serde_json::json!({"name": "c", "fields": fields})).unwrap()
    }

    #[test]
    fn field_names_are_top_level() {
        let c = collection(&["title", "meta.a", "meta.b", ".*", "tags*", "meta.*", "year"]);
        assert_eq!(field_names(&c), ["id", "title", "meta", "year"]);
    }

    #[test]
    fn suggested_name_has_the_extension() {
        assert_eq!(suggested_name("books", ExportFormat::Jsonl), "books.jsonl");
        assert_eq!(
            suggested_name("books", ExportFormat::JsonArray),
            "books.json"
        );
        assert_eq!(suggested_name("books", ExportFormat::Csv), "books.csv");
    }
}
