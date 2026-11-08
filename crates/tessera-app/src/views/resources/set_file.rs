// SPDX-License-Identifier: GPL-3.0-or-later

//! Importing a set from a file and exporting one to a file.

use std::cell::Cell;
use std::path::PathBuf;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{ItemSet, SetKind};
use tessera_core::relevance::changes::changes;
use tessera_core::relevance::name_ok;
use tessera_core::relevance::synonym_file::{
    SetFormat, format_of, merged, parse_synonyms, synonyms_to_csv, synonyms_to_json,
};
use tessera_core::relevance::synonyms::{ModelError, SynonymDraft};

use super::sets::{SetsView, exists, file_error_text, send_set, write_after_prod_confirm};
use super::{Danger, client_of, dialog_title};
use crate::actions::{SET_IMPORT_DIALOG, StartTransfer};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// Width of the dialogs.
const WIDTH: f32 = 460.;

fn format_group(
    id: &'static str,
    format: SetFormat,
    pick: impl Fn(SetFormat, &mut Window, &mut App) + Clone + 'static,
) -> ButtonGroup {
    let button = |id: String, label: String, value: SetFormat| {
        let pick = pick.clone();
        Button::new(SharedString::from(id))
            .label(label)
            .selected(format == value)
            .on_click(move |_, window, cx| pick(value, window, cx))
    };
    ButtonGroup::new(id)
        .outline()
        .small()
        .child(button(
            format!("{id}.json"),
            t!("sets.file.format_json").to_string(),
            SetFormat::Json,
        ))
        .child(button(
            format!("{id}.csv"),
            t!("sets.file.format_csv").to_string(),
            SetFormat::Csv,
        ))
}

/// The rows of a CSV, or which item has none and why.
fn csv_rows(items: &[Value]) -> Result<Vec<SynonymDraft>, String> {
    let row = |item: &Value| {
        SynonymDraft::from_value(item).map_err(|error| {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            match error {
                ModelError::NotObject => t!("sets.export.not_object"),
                ModelError::BadField(key) => t!("sets.export.bad_field", id = id, key = key),
            }
            .to_string()
        })
    };
    items.iter().map(row).collect()
}

/// Takes the items of a file into a new set, or into the selected one by Merge or Replace.
pub struct SetImportDialog {
    view: WeakEntity<SetsView>,
    connection: Entity<ConnectionState>,
    text: String,
    format: SetFormat,
    parsed: Result<Vec<Value>, String>,
    /// The set selected when the dialog opened: the other target.
    selected: Option<ItemSet>,
    into_new: bool,
    replace: bool,
    name: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
    check: Option<Task<()>>,
    cancel_focus: FocusHandle,
    _subscription: Subscription,
}

impl SetImportDialog {
    fn open(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        path: PathBuf,
        text: String,
        selected: Option<ItemSet>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let title = t!("sets.import.title").to_string();
        let conn = connection.clone();
        let dialog = cx.new(|cx| {
            let name = cx.new(|cx| InputState::new(window, cx));
            let mut this = Self {
                _subscription: cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()),
                view,
                connection,
                text,
                format: format_of(&path),
                parsed: Ok(Vec::new()),
                into_new: selected.is_none(),
                selected,
                replace: false,
                name,
                error: None,
                busy: false,
                check: None,
                cancel_focus: cx.focus_handle(),
            };
            this.parse();
            this
        });
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |d, window, cx| {
            if first_build.replace(false) {
                let cancel = dialog.read(cx).cancel_focus.clone();
                cancel.focus(window, cx);
            }
            d.title(dialog_title(title.clone(), &conn, cx))
                .w(px(WIDTH))
                .child(dialog.clone())
        });
    }

    fn parse(&mut self) {
        self.parsed = parse_synonyms(&self.text, self.format).map_err(|e| file_error_text(&e));
    }

    fn set_format(&mut self, format: SetFormat, cx: &mut Context<Self>) {
        self.format = format;
        self.parse();
        cx.notify();
    }

    /// The items a save into `current` sends.
    fn items_for(&self, current: &ItemSet, incoming: &[Value]) -> Vec<Value> {
        if self.replace {
            incoming.to_vec()
        } else {
            merged(&current.items, incoming)
        }
    }

    fn counts(&self) -> Option<String> {
        let incoming = self.parsed.as_ref().ok()?;
        if self.into_new {
            return Some(t!("sets.import.count", n = incoming.len()).to_string());
        }
        let current = self.selected.as_ref()?;
        let diff = changes(
            SetKind::Synonym,
            &current.items,
            &self.items_for(current, incoming),
        );
        Some(
            t!(
                "sets.import.counts",
                added = diff.added.len(),
                changed = diff.changed.len(),
                removed = diff.removed.len()
            )
            .to_string(),
        )
    }

    fn refuse(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(text.into());
        self.busy = false;
        cx.notify();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Ok(incoming) = self.parsed.clone() else {
            return;
        };
        self.error = None;
        if self.into_new {
            self.start_new(incoming, window, cx);
        } else {
            self.start_selected(incoming, window, cx);
        }
    }

    fn start_new(&mut self, items: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if !name_ok(&name) {
            return self.refuse(t!("resources.bad_name").to_string(), cx);
        }
        self.busy = true;
        let check = exists(&self.connection, SetKind::Synonym, name.clone(), cx);
        self.check = Some(cx.spawn_in(window, async move |this, cx| {
            let checked = check.await;
            this.update_in(cx, |this, window, cx| {
                if let Err(error) = checked {
                    return this.refuse(error, cx);
                }
                this.busy = false;
                window.close_dialog(cx);
                let view = this.view.clone();
                let danger = Danger {
                    title: t!("sets.new_title").to_string(),
                    lines: vec![t!("sets.new_line", name = name).to_string()],
                    expected: name.clone(),
                    action: t!("sets.import.start").to_string(),
                    typed: false,
                };
                let target = view.clone();
                let run = move |window: &mut Window, cx: &mut App| {
                    send_set(
                        target.clone(),
                        name.clone(),
                        items.clone(),
                        None,
                        window,
                        cx,
                    )
                };
                let connection = this.connection.clone();
                write_after_prod_confirm(danger, &view, &connection, run, window, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Reads the set fresh, so that Merge keeps a colleague's new rows and the review is true. A
    /// set unlike the one the counts came from shows the new counts and asks again: a save with
    /// no removals runs without a review, so these counts are the only one.
    fn start_selected(
        &mut self,
        incoming: Vec<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.selected.as_ref().map(|s| s.name.clone()) else {
            return;
        };
        let Some(client) = client_of(&self.connection, cx) else {
            return self.refuse(t!("status.not_connected").to_string(), cx);
        };
        self.busy = true;
        let fresh = cx
            .global::<RelevanceBackendGlobal>()
            .0
            .set(client, SetKind::Synonym, name);
        self.check = Some(cx.spawn_in(window, async move |this, cx| {
            let fresh = fresh.await;
            this.update_in(cx, |this, window, cx| match fresh {
                Ok(stored) => {
                    this.busy = false;
                    if this.selected.as_ref() != Some(&stored) {
                        this.selected = Some(stored);
                        return this.refuse(t!("sets.import.changed").to_string(), cx);
                    }
                    let items = this.items_for(&stored, &incoming);
                    window.close_dialog(cx);
                    this.view
                        .update(cx, |view, cx| view.review(stored, items, None, window, cx))
                        .ok();
                }
                Err(e) => {
                    let profile = this.connection.read(cx).profile.clone();
                    this.refuse(error_text(&e, &profile), cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for SetImportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let weak = cx.entity().downgrade();
        let formats = format_group("sets.import.format", self.format, move |format, _, cx| {
            weak.update(cx, |this, cx| this.set_format(format, cx)).ok();
        });
        let targets = ButtonGroup::new("sets.import.target")
            .outline()
            .small()
            .child(
                Button::new("sets.import.new")
                    .label(t!("sets.import.new").to_string())
                    .selected(self.into_new)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.into_new = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("sets.import.selected")
                    .label(t!("sets.import.selected").to_string())
                    .selected(!self.into_new)
                    .disabled(self.selected.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.into_new = false;
                        cx.notify();
                    })),
            );
        let modes = ButtonGroup::new("sets.import.mode")
            .outline()
            .small()
            .child(
                Button::new("sets.import.merge")
                    .label(t!("sets.import.merge").to_string())
                    .selected(!self.replace)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replace = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("sets.import.replace")
                    .label(t!("sets.import.replace").to_string())
                    .selected(self.replace)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replace = true;
                        cx.notify();
                    })),
            );
        let line = |id: &'static str, text: SharedString, color| {
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(color)
                .child(text)
        };
        let foreground = cx.theme().foreground;
        let problem = self
            .parsed
            .as_ref()
            .err()
            .map(|e| SharedString::from(e.clone()));
        let blocked = self.parsed.is_err() || self.busy;
        v_flex()
            .key_context(SET_IMPORT_DIALOG)
            .gap_2()
            .on_action(cx.listener(|this, _: &StartTransfer, window, cx| this.start(window, cx)))
            .child(formats)
            .child(targets)
            .when(self.into_new, |el| {
                el.child(
                    Input::new(&self.name)
                        .id("sets.import.name")
                        .aria_label(t!("resources.name").to_string()),
                )
            })
            .when(!self.into_new, |el| el.child(modes))
            .children(
                self.counts()
                    .map(|text| line("sets.import.counts", text.into(), foreground)),
            )
            .children(problem.map(|text| line("sets.import.problem", text, danger)))
            .children(
                self.error
                    .clone()
                    .map(|text| line("sets.import.error", text, danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("sets.import.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sets.import.start")
                            .primary()
                            .label(t!("sets.import.start").to_string())
                            .loading(self.busy)
                            .disabled(blocked)
                            .children(Kbd::binding_for_action(
                                &StartTransfer,
                                Some(SET_IMPORT_DIALOG),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

/// Asks for a format, then a path, and writes what the server holds then.
pub struct SetExportDialog {
    view: WeakEntity<SetsView>,
    format: SetFormat,
    cancel_focus: FocusHandle,
}

impl SetExportDialog {
    fn open(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let title = t!("sets.export.title").to_string();
        let dialog = cx.new(|cx| Self {
            view,
            format: SetFormat::Json,
            cancel_focus: cx.focus_handle(),
        });
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |d, window, cx| {
            if first_build.replace(false) {
                let cancel = dialog.read(cx).cancel_focus.clone();
                cancel.focus(window, cx);
            }
            d.title(dialog_title(title.clone(), &connection, cx))
                .w(px(WIDTH))
                .child(dialog.clone())
        });
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        let format = self.format;
        self.view
            .update(cx, |view, cx| view.export(format, window, cx))
            .ok();
    }
}

impl Render for SetExportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let formats = format_group("sets.export.format", self.format, move |format, _, cx| {
            weak.update(cx, |this, cx| {
                this.format = format;
                cx.notify();
            })
            .ok();
        });
        v_flex()
            .key_context(SET_IMPORT_DIALOG)
            .gap_2()
            .on_action(cx.listener(|this, _: &StartTransfer, window, cx| this.start(window, cx)))
            .child(formats)
            .when(self.format == SetFormat::Csv, |el| {
                let note = SharedString::from(t!("transfer.export.csv_formula_note").to_string());
                el.child(
                    div()
                        .id("sets.export.note")
                        .test_support()
                        .aria_label(note.clone())
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("sets.export.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sets.export.start")
                            .primary()
                            .label(t!("sets.export.start").to_string())
                            .children(Kbd::binding_for_action(
                                &StartTransfer,
                                Some(SET_IMPORT_DIALOG),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

impl SetsView {
    /// Unlocks if needed, asks for a file, reads it off the GPUI thread and opens the dialog.
    pub(super) fn ask_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        self.error = None;
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let picked = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some(t!("sets.import.prompt").to_string().into()),
                });
                let backend = cx.global::<RelevanceBackendGlobal>().0.clone();
                window
                    .spawn(cx, async move |cx| {
                        let Ok(Ok(Some(paths))) = picked.await else {
                            return;
                        };
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let read = backend.read_set_file(path.clone()).await;
                        this.update_in(cx, |view, window, cx| match read {
                            Ok(text) => {
                                let selected = view.current().cloned();
                                let (view, conn) = (cx.entity().downgrade(), connection.clone());
                                SetImportDialog::open(view, conn, path, text, selected, window, cx);
                            }
                            Err(error) => {
                                view.error = Some(file_error_text(&error).into());
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .detach();
            });
        });
    }

    /// Reading needs no unlock.
    pub(super) fn ask_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.followed.is_some() {
            let view = cx.entity().downgrade();
            SetExportDialog::open(view, self.connection.clone(), window, cx);
        }
    }

    fn export(&mut self, format: SetFormat, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(name), Some(client)) = (self.followed.clone(), client_of(&self.connection, cx))
        else {
            return;
        };
        let extension = match format {
            SetFormat::Json => "json",
            SetFormat::Csv => "csv",
        };
        let dir = std::env::home_dir().unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some(&format!("{name}.{extension}")));
        let backend = cx.global::<RelevanceBackendGlobal>().0.clone();
        let kind = self.kind;
        let profile = self.connection.read(cx).profile.clone();
        self.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            // The ids of the items that lost keys, which a CSV has no column for.
            let written: Result<Vec<String>, String> = async {
                let set = backend.set(client, kind, name).await;
                let set = set.map_err(|e| error_text(&e, &profile))?;
                let mut cut = Vec::new();
                let text = match format {
                    SetFormat::Json => synonyms_to_json(&set.items),
                    SetFormat::Csv => {
                        let rows = csv_rows(&set.items)?;
                        let lossy = rows.iter().filter(|d| !d.extra.is_empty());
                        cut.extend(lossy.map(|d| d.id.clone()));
                        synonyms_to_csv(&rows)
                    }
                };
                backend.write_set_file(path.clone(), text).await?;
                Ok(cut)
            }
            .await;
            this.update(cx, |view, cx| {
                match written {
                    Ok(cut) => {
                        let path = path.display().to_string();
                        let mut note = t!("sets.export.done", path = path).to_string();
                        if !cut.is_empty() {
                            let ids = cut.join(", ");
                            note = format!("{note} {}", t!("sets.export.cut", ids = ids));
                        }
                        view.note = Some(note.into());
                    }
                    Err(error) => view.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
