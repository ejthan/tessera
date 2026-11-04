// SPDX-License-Identifier: GPL-3.0-or-later

//! Import preview for typesense-dashboard `config.json` files (design §7.5).
//!
//! [`ImportDialog::open`] picks and reads the file; [`ImportDialog::import_from_text`] shows
//! the candidates. "Import" stores the keys first, then writes all checked profiles: a
//! keychain failure is listed on its row and leaves that profile without a key.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Task, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use secrecy::ExposeSecret as _;
use tessera_core::import::{self, ImportCandidate, ImportNote};
use tessera_core::profile::{Environment, ProfileColor};
use tessera_core::secrets::SecretError;

use super::env_badge;
use crate::runtime::Tokio;
use crate::state::AppState;

/// The environments a row can be given.
const ENVS: [Environment; 4] = [
    Environment::Local,
    Environment::Dev,
    Environment::Staging,
    Environment::Prod,
];

/// One candidate and what happened to it.
struct Row {
    candidate: ImportCandidate,
    imported: bool,
    /// The keychain refused the key; the profile was imported without it.
    error: Option<SharedString>,
}

/// The preview dialog's state.
pub struct ImportDialog {
    state: Entity<AppState>,
    rows: Vec<Row>,
    /// Why the file could not be read or parsed.
    error: Option<SharedString>,
    /// Why `connections.json` could not be written after the import.
    save_error: Option<SharedString>,
    /// The running import. Esc waits for it.
    task: Option<Task<()>>,
    /// The footer buttons, so Enter can activate the focused one.
    cancel_focus: FocusHandle,
    submit_focus: FocusHandle,
}

impl ImportDialog {
    /// Asks for a `config.json`, reads it off the GPUI thread and opens the preview.
    pub fn open(state: Entity<AppState>, window: &mut Window, cx: &mut App) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("import.prompt").to_string().into()),
        });
        let tokio = cx.global::<Tokio>().0.clone();
        window
            .spawn(cx, async move |cx| {
                let text = match picked.await {
                    Ok(Ok(Some(paths))) => {
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let shown = path.display().to_string();
                        let read = tokio
                            .spawn_blocking(move || std::fs::read_to_string(&path))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()));
                        read.map_err(|e| {
                            t!("import.error.read", path = shown, detail = e).to_string()
                        })
                    }
                    Ok(Err(e)) => {
                        Err(t!("connect.error.picker", detail = e.to_string()).to_string())
                    }
                    // Cancelled.
                    Ok(Ok(None)) | Err(_) => return,
                };
                cx.update(|window, cx| match text {
                    Ok(text) => Self::import_from_text(state, &text, window, cx),
                    Err(e) => Self::show(state, Err(e), window, cx),
                })
                .ok();
            })
            .detach();
    }

    /// Parses `text` as a typesense-dashboard `config.json` and opens the preview, which
    /// shows a parse error inline.
    pub fn import_from_text(
        state: Entity<AppState>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        let parsed = import::dashboard_config(text, &state.read(cx).profiles)
            .map_err(|e| t!("import.error.parse", detail = e.to_string()).to_string());
        Self::show(state, parsed, window, cx);
    }

    fn show(
        state: Entity<AppState>,
        parsed: Result<Vec<ImportCandidate>, String>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (rows, error) = match parsed {
            Ok(c) if c.is_empty() => (Vec::new(), Some(t!("import.empty").to_string())),
            Ok(c) => (c, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let view = cx.new(|cx| Self {
            state,
            rows: rows
                .into_iter()
                .map(|candidate| Row {
                    candidate,
                    imported: false,
                    error: None,
                })
                .collect(),
            error: error.map(Into::into),
            save_error: None,
            task: None,
            cancel_focus: cx.focus_handle(),
            submit_focus: cx.focus_handle(),
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let busy = view.clone();
            let enter = view.clone();
            dialog
                .title(t!("import.title").to_string())
                .w(px(720.))
                // The dialog's Enter binding wins over the focused button's own key handling,
                // so Enter activates the focused footer button here; elsewhere it does nothing.
                .on_ok(move |_, window, cx| {
                    enter.update(cx, |this, cx| {
                        if this.submit_focus.is_focused(window) {
                            this.submit(window, cx);
                            false
                        } else {
                            this.cancel_focus.is_focused(window) && this.task.is_none()
                        }
                    })
                })
                .on_cancel(move |_, _, cx| busy.read(cx).task.is_none())
                .child(view.clone())
        });
    }

    /// The rows "Import" writes.
    fn pending(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&i| self.rows[i].candidate.selected && !self.rows[i].imported)
            .collect()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self.pending();
        let blocked = self.state.read(cx).load_error.is_some();
        if self.task.is_some() || picked.is_empty() || blocked {
            return;
        }
        let keys: Vec<_> = picked
            .iter()
            .map(|&i| &self.rows[i].candidate)
            .enumerate()
            .filter(|(_, c)| !c.key.expose_secret().is_empty())
            .map(|(n, c)| (picked[n], c.profile.id, c.key.clone()))
            .collect();
        let rows_with_keys: Vec<usize> = keys.iter().map(|(i, _, _)| *i).collect();
        let secrets = self.state.read(cx).secrets.clone();
        let job = cx.global::<Tokio>().0.spawn_blocking(move || {
            keys.into_iter()
                .map(|(i, id, key)| (i, secrets.set(id, &key)))
                .collect::<Vec<_>>()
        });
        self.save_error = None;
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let stored = job.await.unwrap_or_else(|e| {
                let e = e.to_string();
                rows_with_keys
                    .into_iter()
                    .map(|i| (i, Err(SecretError::new(e.clone()))))
                    .collect()
            });
            let saved = this.update(cx, |view, cx| {
                for (i, result) in stored {
                    if let Err(e) = result {
                        let text = t!("import.error.keychain", detail = e.to_string());
                        view.rows[i].error = Some(text.to_string().into());
                    }
                }
                let profiles: Vec<_> = picked
                    .iter()
                    .map(|&i| {
                        view.rows[i].imported = true;
                        view.rows[i].candidate.profile.clone()
                    })
                    .collect();
                view.state.update(cx, |s, cx| {
                    s.profiles.extend(profiles);
                    cx.notify();
                    s.save(cx)
                })
            });
            let Ok(saved) = saved else {
                return;
            };
            let result = saved.await;
            this.update_in(cx, |view, window, cx| {
                view.task = None;
                match result {
                    Err(e) => {
                        let text = t!("connect.error.save_file", detail = e);
                        view.save_error = Some(text.to_string().into());
                    }
                    Ok(()) if view.rows.iter().all(|r| r.error.is_none()) => {
                        window.close_dialog(cx);
                    }
                    Ok(()) => {}
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_env(&mut self, i: usize, env: Environment, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(i) {
            row.candidate.profile.read_only = env.is_prod();
            row.candidate.profile.environment = env;
            cx.notify();
        }
    }

    fn set_color(&mut self, i: usize, color: ProfileColor, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(i) {
            row.candidate.profile.color = color;
            cx.notify();
        }
    }

    fn render_row(&self, i: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = |part: &str| SharedString::from(format!("import.row.{i}{part}"));
        let c = &row.candidate;
        let name = SharedString::from(c.profile.name.clone());
        // A "SAME" host only works in the web version; such rows cannot be imported.
        // "SAME" hosts only work in the web version, and an invalid node would bypass the
        // connect dialog's host check: such rows cannot be imported.
        let locked = row.imported
            || self.task.is_some()
            || c.notes
                .iter()
                .any(|n| matches!(n, ImportNote::SkippedSameHost | ImportNote::InvalidNode));
        let nodes = c
            .profile
            .nodes
            .iter()
            .map(|n| n.label())
            .collect::<Vec<_>>()
            .join(", ");
        let state = self.state.read(cx);
        let mut notes: Vec<String> = c
            .duplicate_of
            .and_then(|id| state.profiles.iter().find(|p| p.id == id))
            .map(|p| t!("import.note.duplicate", name = p.name.clone()).to_string())
            .into_iter()
            .collect();
        notes.extend(c.notes.iter().map(|n| {
            match n {
                ImportNote::DifferingClusterKeys => t!("import.note.differing_keys"),
                ImportNote::SkippedSameHost => t!("import.note.same_host"),
                ImportNote::InvalidNode => t!("import.note.invalid_node"),
            }
            .to_string()
        }));
        if row.imported {
            notes.push(t!("import.note.imported").to_string());
        }
        let view = cx.entity().downgrade();
        let env = c.profile.environment.clone();
        let env_button = Button::new(id(".env"))
            .outline()
            .small()
            .dropdown_caret(true)
            .disabled(locked)
            .accessibility_label(t!("connect.environment").to_string())
            .child(env_badge::render(&env, cx))
            .dropdown_menu({
                let view = view.clone();
                move |menu, _, _| {
                    ENVS.iter().fold(menu, |menu, choice| {
                        let view = view.clone();
                        let choice = choice.clone();
                        let label = t!(format!("connect.env.{}", choice.label())).to_string();
                        menu.item(PopupMenuItem::new(label).checked(env == choice).on_click(
                            move |_, _, cx| {
                                view.update(cx, |this, cx| this.set_env(i, choice.clone(), cx))
                                    .ok();
                            },
                        ))
                    })
                }
            });
        let color = c.profile.color;
        let color_name = env_badge::COLORS
            .iter()
            .find(|(c, _)| *c == color)
            .map_or("gray", |(_, n)| n);
        let color_button = Button::new(id(".color"))
            .ghost()
            .small()
            .dropdown_caret(true)
            .disabled(locked)
            .accessibility_label(t!(format!("connect.color.{color_name}")).to_string())
            .child(
                div()
                    .size_4()
                    .rounded_full()
                    .bg(env_badge::color(color, &theme)),
            )
            .dropdown_menu(move |menu, _, _| {
                env_badge::COLORS
                    .iter()
                    .fold(menu, |menu, &(choice, name)| {
                        let view = view.clone();
                        let label = t!(format!("connect.color.{name}")).to_string();
                        menu.item(PopupMenuItem::new(label).checked(color == choice).on_click(
                            move |_, _, cx| {
                                view.update(cx, |this, cx| this.set_color(i, choice, cx))
                                    .ok();
                            },
                        ))
                    })
            });
        h_flex()
            .id(id(""))
            .test_support()
            .aria_label(name.clone())
            .gap_3()
            .items_start()
            .py_1()
            .child(
                Checkbox::new(id(".selected"))
                    .accessibility_label(name.clone())
                    .checked(c.selected)
                    .disabled(locked)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if let Some(row) = this.rows.get_mut(i) {
                            row.candidate.selected = *checked;
                            cx.notify();
                        }
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_sm().child(name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(nodes),
                    )
                    .children(
                        notes
                            .into_iter()
                            .map(|n| div().text_xs().text_color(theme.muted_foreground).child(n)),
                    )
                    .children(row.error.clone().map(|e| error_line(id(".error"), e, cx))),
            )
            .child(env_button)
            .child(color_button)
            .into_any_element()
    }
}

impl Render for ImportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.pending().len();
        let busy = self.task.is_some();
        // While connections.json is unreadable nothing would be written, so nothing is imported.
        let blocked = self
            .state
            .read(cx)
            .load_error
            .clone()
            .filter(|_| !self.rows.is_empty())
            .map(|e| SharedString::from(t!("import.blocked", error = e).to_string()));
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| self.render_row(i, row, cx))
            .collect();
        let submit = if count == 1 {
            t!("import.submit_one").to_string()
        } else {
            t!("import.submit", count = count).to_string()
        };
        v_flex()
            .gap_2()
            .children(
                self.error
                    .clone()
                    .map(|e| error_line("import.error".into(), e, cx)),
            )
            .children(rows)
            .children(
                self.save_error
                    .clone()
                    .map(|e| error_line("import.save_error".into(), e, cx)),
            )
            .children(
                blocked
                    .clone()
                    .map(|e| error_line("import.blocked".into(), e, cx)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("import.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .disabled(busy)
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .when(!self.rows.is_empty(), |el| {
                        el.child(
                            Button::new("import.submit")
                                .primary()
                                .label(submit)
                                .loading(busy)
                                .disabled(busy || count == 0 || blocked.is_some())
                                .track_focus(&self.submit_focus)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.submit(window, cx)),
                                ),
                        )
                    }),
            )
    }
}

/// An inline error with its test id.
fn error_line(id: SharedString, text: SharedString, cx: &App) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(text.clone())
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text)
}
