// SPDX-License-Identifier: GPL-3.0-or-later

//! Imports a dictionary file. The server stores the lines before a bad one and an import cannot
//! be undone, so the file is checked whole here and nothing goes out while a line is bad.

use std::cell::Cell;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::StemmingWord;
use tessera_core::relevance::name_ok;
use tessera_core::stemming::{BadLine, LineProblem};

use super::stemming::StemmingView;
use super::{Danger, ErrorSlot, client_of, confirm_write, dialog_title, notice, report_to};
use crate::actions::{PickImportFile, STEMMING_DIALOG, SaveResource};
use crate::admin::{AdminBackendGlobal, DictionaryFileError};
use crate::connection::{ConnectionState, error_text, long_error_text};

/// Width of the dialog.
const WIDTH: f32 = 480.;

fn file_error_text(error: &DictionaryFileError) -> String {
    match error {
        DictionaryFileError::TooLarge(size) => {
            t!("stemming.too_large", size = size.to_string()).to_string()
        }
        DictionaryFileError::Read(detail) => t!("stemming.unreadable", detail = detail).to_string(),
        DictionaryFileError::Lines(_) => String::new(),
    }
}

fn line_text(problem: LineProblem) -> String {
    match problem {
        LineProblem::NotJson => t!("stemming.line.not_json"),
        LineProblem::NotObject => t!("stemming.line.not_object"),
        LineProblem::MissingWord => t!("stemming.line.missing_word"),
        LineProblem::MissingRoot => t!("stemming.line.missing_root"),
        LineProblem::NotText => t!("stemming.line.not_text"),
        LineProblem::Empty => t!("stemming.line.empty"),
    }
    .to_string()
}

/// The import form.
pub struct StemmingDialog {
    view: WeakEntity<StemmingView>,
    connection: Entity<ConnectionState>,
    id: Entity<InputState>,
    /// The dictionary selected when it opened.
    selected: Option<String>,
    /// The file parsed clean.
    words: Option<Vec<StemmingWord>>,
    bad: Vec<BadLine>,
    /// Why the file cannot be used.
    file_error: Option<SharedString>,
    error: Option<SharedString>,
    busy: bool,
    /// Asking whether the id exists. Closing now would not stop the import that follows.
    checking: bool,
    read_task: Option<Task<()>>,
    cancel_focus: FocusHandle,
    _subscription: Subscription,
}

impl StemmingDialog {
    /// Opens it, with the id of the selected dictionary.
    pub(super) fn open(
        view: WeakEntity<StemmingView>,
        connection: Entity<ConnectionState>,
        selected: Option<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let shown = connection.clone();
        let dialog = cx.new(|cx| {
            let id = cx.new(|cx| {
                let mut id = InputState::new(window, cx);
                id.set_value(selected.clone().unwrap_or_default(), window, cx);
                id
            });
            let subscription = cx.subscribe_in(
                &id,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit(window, cx);
                    }
                    cx.notify();
                },
            );
            Self {
                view,
                connection,
                id,
                selected,
                words: None,
                bad: Vec::new(),
                file_error: None,
                error: None,
                busy: false,
                checking: false,
                read_task: None,
                cancel_focus: cx.focus_handle(),
                _subscription: subscription,
            }
        });
        let entity = dialog.clone();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |d, window, cx| {
            if first_build.replace(false) {
                let id = dialog.read(cx).id.clone();
                id.read(cx).focus_handle(cx).focus(window, cx);
            }
            let (enter, cancel) = (dialog.clone(), dialog.clone());
            d.title(dialog_title(
                t!("stemming.import_title").to_string(),
                &shown,
                cx,
            ))
            .w(px(WIDTH))
            // A failing import reports here, so the form stays until it ends.
            .on_cancel(move |_, _, cx| !cancel.read(cx).busy)
            // The dialog's Enter binding wins over the focused button's own key handling.
            .on_ok(move |_, window, cx| {
                enter.update(cx, |this, cx| {
                    let cancel = this.cancel_focus.is_focused(window);
                    if !cancel {
                        this.submit(window, cx);
                    }
                    cancel
                })
            })
            .child(dialog.clone())
        });
        entity
    }

    fn id_value(&self, cx: &App) -> String {
        self.id.read(cx).value().trim().to_string()
    }

    /// Whether the id and the file allow an import.
    pub fn can_import(&self, cx: &App) -> bool {
        let id = self.id_value(cx);
        (name_ok(&id) || self.selected.as_ref() == Some(&id)) && self.words.is_some() && !self.busy
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("stemming.prompt").to_string().into()),
        });
        self.read_task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.read_file(path, cx)).ok();
        }));
    }

    /// Reads and checks the file at `path`.
    pub fn read_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let read = cx
            .global::<AdminBackendGlobal>()
            .0
            .read_dictionary_file(path);
        self.read_task = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                this.words = None;
                this.bad.clear();
                this.file_error = None;
                match read {
                    Ok(words) if words.is_empty() => {
                        this.file_error = Some(t!("stemming.file_empty").to_string().into());
                    }
                    Ok(words) => this.words = Some(words),
                    Err(DictionaryFileError::Lines(bad)) => this.bad = bad,
                    Err(e) => this.file_error = Some(file_error_text(&e).into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_import(cx) {
            return;
        }
        let Some(words) = self.words.clone() else {
            return;
        };
        let id = self.id_value(cx);
        self.error = None;
        // Here, not in the deferred write, so a second Enter finds it set. A locked connection
        // asks first, and cancelling that leaves the form usable.
        self.busy = !self.connection.read(cx).writes_locked();
        cx.notify();
        let dialog = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                check_id(&dialog, id, words, window, cx);
            });
        });
    }
}

impl ErrorSlot for StemmingDialog {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

/// Asks the server whether `id` exists: what the form opened with can be old, and a merge cannot
/// be undone. Nothing goes out when the answer does not arrive.
fn check_id(
    dialog: &Entity<StemmingDialog>,
    id: String,
    words: Vec<StemmingWord>,
    window: &mut Window,
    cx: &mut App,
) {
    let connection = dialog.read(cx).connection.clone();
    let report = report_to(dialog.downgrade());
    let Some(client) = client_of(&connection, cx) else {
        dialog.update(cx, |d, _| d.busy = false);
        return report(t!("status.not_connected").to_string().into(), cx);
    };
    let read = cx.global::<AdminBackendGlobal>().0.dictionaries(client);
    dialog.update(cx, |d, cx| {
        (d.busy, d.checking) = (true, true);
        cx.notify();
    });
    let dialog = dialog.clone();
    window
        .spawn(cx, async move |cx| {
            let ids = read.await;
            cx.update(|window, cx| {
                dialog.update(cx, |d, cx| {
                    (d.busy, d.checking) = (false, false);
                    cx.notify();
                });
                match ids {
                    Ok(ids) if ids.contains(&id) => ask_merge(dialog, id, words, window, cx),
                    Ok(_) => {
                        let send = send_from_dialog(&dialog, id, words, window, cx);
                        cx.spawn(async move |cx| {
                            if let Err(text) = send.await {
                                cx.update(|cx| report(text, cx));
                            }
                        })
                        .detach();
                    }
                    Err(e) => {
                        let text = error_text(&e, &connection.read(cx).profile);
                        report(text.into(), cx);
                    }
                }
            })
            .ok();
        })
        .detach();
}

/// Confirms over the form, which stays with its file when the user cancels.
fn ask_merge(
    dialog: Entity<StemmingDialog>,
    id: String,
    words: Vec<StemmingWord>,
    window: &mut Window,
    cx: &mut App,
) {
    let connection = dialog.read(cx).connection.clone();
    let danger = Danger {
        title: t!("stemming.merge_title", name = id).to_string(),
        lines: vec![t!("stemming.merge").to_string()],
        expected: id.clone(),
        action: t!("stemming.import").to_string(),
        typed: connection.read(cx).profile.environment.is_prod(),
    };
    let report = report_to(dialog.downgrade());
    let run = move |window: &mut Window, cx: &mut App| {
        send_from_dialog(&dialog, id.clone(), words.clone(), window, cx)
    };
    confirm_write(danger, &connection, run, report, window, cx);
}

/// Imports from the open form. When it worked the form goes, with a confirmation over it.
fn send_from_dialog(
    dialog: &Entity<StemmingDialog>,
    id: String,
    words: Vec<StemmingWord>,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let (view, connection) = {
        let d = dialog.read(cx);
        (d.view.clone(), d.connection.clone())
    };
    let send = send_import(view, connection, id, words, cx);
    dialog.update(cx, |d, cx| {
        d.busy = true;
        cx.notify();
    });
    let form = dialog.downgrade();
    window.spawn(cx, async move |cx| {
        let sent = send.await;
        cx.update(|window, cx| {
            form.update(cx, |d, cx| {
                d.busy = false;
                cx.notify();
            })
            .ok();
            if sent.is_ok() {
                window.close_all_dialogs(cx);
            }
        })
        .ok();
        sent
    })
}

/// Imports and reports on the tab. Runs on through a closed tab.
fn send_import(
    view: WeakEntity<StemmingView>,
    connection: Entity<ConnectionState>,
    id: String,
    words: Vec<StemmingWord>,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let import = cx
        .global::<AdminBackendGlobal>()
        .0
        .import_dictionary(client, id.clone(), words);
    cx.spawn(async move |cx| {
        let check = import
            .await
            .map_err(|e| SharedString::from(long_error_text(&e, &profile)))?;
        view.update(cx, |view, cx| view.imported(id, &check, cx))
            .ok();
        Ok(())
    })
}

impl Render for StemmingDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let locked = self.connection.read(cx).writes_locked();
        let can = self.can_import(cx);
        let bad = self.bad.iter().map(|line| {
            let problem = line_text(line.problem);
            t!("stemming.bad_line", line = line.line, problem = problem).to_string()
        });
        let bad = bad.collect::<Vec<_>>().join("\n");
        let count = self.words.as_ref().map(Vec::len);
        v_flex()
            .key_context(STEMMING_DIALOG)
            .gap_2()
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.submit(window, cx)))
            .on_action(cx.listener(|this, _: &PickImportFile, window, cx| {
                this.pick_file(window, cx);
            }))
            .child(t!("stemming.id").to_string())
            .child(
                Input::new(&self.id)
                    .id("stemming.dialog.id")
                    .aria_label(t!("stemming.id").to_string()),
            )
            .child(
                h_flex().child(
                    Button::new("stemming.dialog.file")
                        .label(t!("stemming.choose_file").to_string())
                        .children(Kbd::binding_for_action(
                            &PickImportFile,
                            Some(STEMMING_DIALOG),
                            window,
                        ))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx))),
                ),
            )
            .children(count.map(|n| {
                let text = t!("stemming.file_words", count = n).to_string();
                notice("stemming.dialog.words", text.into(), muted)
            }))
            .children((!bad.is_empty()).then(|| {
                div()
                    .id("stemming.dialog.lines_scroll")
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .child(notice("stemming.dialog.lines", bad.into(), danger))
            }))
            .children(
                self.file_error
                    .clone()
                    .map(|e| notice("stemming.dialog.file_error", e, danger)),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("stemming.dialog.error", e, danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("stemming.dialog.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .disabled(self.checking)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("stemming.dialog.import")
                            .primary()
                            .label(t!("stemming.import").to_string())
                            .when(locked, |b| {
                                b.child(super::env_badge::lock("stemming.dialog.import", false, cx))
                            })
                            .children(Kbd::binding_for_action(
                                &SaveResource,
                                Some(STEMMING_DIALOG),
                                window,
                            ))
                            .loading(self.busy)
                            .disabled(!can)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
