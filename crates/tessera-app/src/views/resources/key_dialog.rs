// SPDX-License-Identifier: GPL-3.0-or-later

//! Creates an API key, and shows its value the one time there is.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{Local, NaiveDate, TimeZone as _};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::date_picker::{DatePicker, DatePickerState, DateTime as PickedTime};
use gpui_kit::component::dialog::{Cancel, Confirm};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, WeakEntity, Window, div,
    prelude::*, px,
};
use jiff::Timestamp;
use rust_i18n::t;
use secrecy::{ExposeSecret as _, SecretString};
use tessera_client::{CreatedKey, NewKey};
use tessera_core::keys::{ACTION_GROUPS, KeyProblem, Template, check_new_key, collections_of};
use uuid::Uuid;

use super::keys::KeysView;
use super::{client_of, dialog_title, notice};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::state::AppState;
use crate::views::connect_dialog::ConnectDialog;
use crate::views::env_badge;

/// Width of the form.
const WIDTH: f32 = 640.;
/// Height of the action matrix.
const MATRIX_HEIGHT: f32 = 260.;
/// Width of the reveal.
const REVEAL_WIDTH: f32 = 520.;
/// How long the Copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_secs(2);

/// The end of the picked day in the local zone, which a key should still reach.
fn end_of_day(picked: PickedTime) -> Option<i64> {
    let PickedTime::Single(Some(at)) = picked else {
        return None;
    };
    let end = at.date().and_hms_opt(23, 59, 59)?;
    let local = Local.from_local_datetime(&end).earliest()?;
    Some(local.timestamp())
}

fn problem_text(problem: KeyProblem) -> String {
    match problem {
        KeyProblem::NoDescription => t!("keys.problem.no_description"),
        KeyProblem::NoActions => t!("keys.problem.no_actions"),
        KeyProblem::NoCollections => t!("keys.problem.no_collections"),
        KeyProblem::ExpiryPast => t!("keys.problem.expiry_past"),
    }
    .to_string()
}

/// The form of a new key.
pub struct KeyDialog {
    connection: Entity<ConnectionState>,
    state: Entity<AppState>,
    keys: WeakEntity<KeysView>,
    description: Entity<InputState>,
    /// The plain `*`.
    wildcard: bool,
    actions: BTreeSet<&'static str>,
    all_collections: bool,
    collections: Entity<TextareaState>,
    date: Entity<DatePickerState>,
    problems: Vec<KeyProblem>,
    error: Option<SharedString>,
    saving: bool,
    cancel_focus: FocusHandle,
}

impl KeyDialog {
    /// Opens the form, and answers it for the tests.
    pub(super) fn open(
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        keys: WeakEntity<KeysView>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let view = cx.new(|cx| {
            let today = Local::now().date_naive();
            let past = move |day: &NaiveDate| *day < today;
            let hint = t!("keys.collections_hint").to_string();
            Self {
                connection: connection.clone(),
                state,
                keys,
                description: cx.new(|cx| InputState::new(window, cx)),
                wildcard: false,
                actions: BTreeSet::new(),
                all_collections: true,
                collections: cx.new(|cx| TextareaState::new(window, cx).rows(3).placeholder(hint)),
                date: cx.new(|cx| DatePickerState::new(window, cx).disabled_matcher(past)),
                problems: Vec::new(),
                error: None,
                saving: false,
                cancel_focus: cx.focus_handle(),
            }
        });
        let shown = view.clone();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                let description = shown.read(cx).description.read(cx);
                description.focus_handle(cx).focus(window, cx);
            }
            let enter = shown.clone();
            // Only the running create's answer holds the value, and it closes the form itself.
            let open = !shown.read(cx).saving;
            dialog
                .title(dialog_title(
                    t!("keys.new_title").to_string(),
                    &connection,
                    cx,
                ))
                .w(px(WIDTH))
                .close_button(open)
                .overlay_closable(open)
                .keyboard(open)
                .on_ok(move |_, window, cx| {
                    enter.update(cx, |this, cx| {
                        let cancel = this.cancel_focus.is_focused(window);
                        if !cancel {
                            this.save(window, cx);
                        }
                        cancel
                    })
                })
                .child(shown.clone())
        });
        view
    }

    /// The picker, for the tests.
    pub fn date(&self) -> &Entity<DatePickerState> {
        &self.date
    }

    fn apply(&mut self, template: Template, cx: &mut Context<Self>) {
        let actions = template.actions();
        self.wildcard = actions.contains(&"*");
        self.actions = actions.into_iter().filter(|a| *a != "*").collect();
        self.all_collections = true;
        self.problems.clear();
        cx.notify();
    }

    fn draft(&self, cx: &App) -> NewKey {
        let wildcard = self.wildcard.then_some("*");
        let catalog = ACTION_GROUPS.iter().flat_map(|g| g.actions.iter().copied());
        let picked = catalog.filter(|a| self.actions.contains(a));
        NewKey {
            description: self.description.read(cx).value().trim().to_string(),
            actions: wildcard
                .into_iter()
                .chain(picked)
                .map(String::from)
                .collect(),
            collections: if self.all_collections {
                vec!["*".to_string()]
            } else {
                collections_of(&self.collections.read(cx).value())
            },
            expires_at: end_of_day(self.date.read(cx).date_time()),
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let draft = self.draft(cx);
        self.problems = check_new_key(&draft, Timestamp::now());
        self.error = None;
        cx.notify();
        if !self.problems.is_empty() {
            return;
        }
        let view = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                send_create(&view, draft.clone(), window, cx);
            });
        });
    }

    fn templates(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let button = |id: &'static str, label: String, template: Template| {
            Button::new(id)
                .small()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.apply(template, cx)))
        };
        h_flex()
            .gap_2()
            .child(button(
                "keys.template.search_only",
                t!("keys.template.search_only").to_string(),
                Template::SearchOnly,
            ))
            .child(button(
                "keys.template.import_only",
                t!("keys.template.import_only").to_string(),
                Template::ImportOnly,
            ))
            .child(button(
                "keys.template.admin",
                t!("keys.template.admin").to_string(),
                Template::Admin,
            ))
    }

    fn matrix(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let groups = ACTION_GROUPS.iter().map(|group| {
            let boxes = group.actions.iter().copied().map(|action| {
                Checkbox::new(SharedString::from(format!("keys.action.{action}")))
                    .small()
                    .checked(self.actions.contains(action))
                    .label(action.to_string())
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if *checked {
                            this.actions.insert(action);
                        } else {
                            this.actions.remove(action);
                        }
                        cx.notify();
                    }))
            });
            let boxes: Vec<_> = boxes.collect();
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .child(t!(format!("keys.group.{}", group.key)).to_string()),
                )
                .child(h_flex().flex_wrap().gap_2().children(boxes))
        });
        let groups: Vec<_> = groups.collect();
        div()
            .id("keys.matrix")
            .h(px(MATRIX_HEIGHT))
            .overflow_y_scroll()
            .child(v_flex().gap_2().children(groups))
    }
}

impl Render for KeyDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, warning) = (theme.danger, theme.warning);
        let locked = self.connection.read(cx).writes_locked();
        let problems = self.problems.iter().enumerate().map(|(i, problem)| {
            notice(
                format!("keys.problem.{i}"),
                problem_text(*problem).into(),
                danger,
            )
        });
        let problems: Vec<_> = problems.collect();
        v_flex()
            .gap_2()
            .child(t!("keys.description").to_string())
            .child(
                Input::new(&self.description)
                    .id("keys.form.description")
                    .aria_label(t!("keys.description").to_string()),
            )
            .child(self.templates(cx))
            .child(
                Checkbox::new("keys.wildcard")
                    .checked(self.wildcard)
                    .label(t!("keys.wildcard").to_string())
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.wildcard = *checked;
                        cx.notify();
                    })),
            )
            .when(self.wildcard, |el| {
                el.child(notice(
                    "keys.wildcard_warning",
                    t!("keys.wildcard_warning").to_string().into(),
                    warning,
                ))
            })
            .child(self.matrix(cx))
            .child(
                Checkbox::new("keys.all_collections")
                    .checked(self.all_collections)
                    .label(t!("keys.all_collections").to_string())
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.all_collections = *checked;
                        cx.notify();
                    })),
            )
            .child(
                div().id("keys.form.collections").test_support().child(
                    Textarea::new(&self.collections)
                        .aria_label(t!("keys.collections_hint").to_string())
                        .h(px(64.))
                        .disabled(self.all_collections),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(t!("keys.expires_on").to_string())
                    .child(
                        DatePicker::new(&self.date)
                            .small()
                            .cleanable(true)
                            .placeholder(t!("keys.never_expires").to_string()),
                    ),
            )
            .children(problems)
            .children(
                self.error
                    .clone()
                    .map(|e| notice("keys.form.error", e, danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("keys.form.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .disabled(self.saving)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("keys.form.save")
                            .primary()
                            .label(t!("keys.create").to_string())
                            .when(locked, |b| {
                                b.child(env_badge::lock("keys.form.save", false, cx))
                            })
                            .children(Kbd::binding_for_action(
                                &Confirm { secondary: false },
                                Some("Dialog"),
                                window,
                            ))
                            .loading(self.saving)
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// Sends the draft. Runs on through a closed tab: the value exists only in this answer, so it
/// holds what the reveal needs. The form stays open meanwhile.
fn send_create(view: &Entity<KeyDialog>, draft: NewKey, window: &mut Window, cx: &mut App) {
    let (connection, state, keys) = {
        let v = view.read(cx);
        (v.connection.clone(), v.state.clone(), v.keys.clone())
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
    let create = cx
        .global::<AdminBackendGlobal>()
        .0
        .create_key(client, draft);
    view.update(cx, |v, cx| {
        v.saving = true;
        cx.notify();
    });
    let form = view.downgrade();
    window
        .spawn(cx, async move |cx| {
            let created = create.await;
            cx.update(|window, cx| match created {
                Ok(created) => {
                    window.close_dialog(cx);
                    RevealDialog::open(created, connection, state, keys, window, cx);
                }
                Err(e) => {
                    let text = SharedString::from(error_text(&e, &profile));
                    form.update(cx, |v, cx| {
                        v.saving = false;
                        v.error = Some(text);
                        cx.notify();
                    })
                    .ok();
                }
            })
            .ok();
        })
        .detach();
}

/// Shows a new key's value. It is gone with this view, and only Done closes it.
pub struct RevealDialog {
    value: SecretString,
    id: u64,
    description: String,
    input: Entity<InputState>,
    masked: bool,
    copied: bool,
    copy_reset: Option<Task<()>>,
    copy_focus: FocusHandle,
    connection: Entity<ConnectionState>,
    state: Entity<AppState>,
    keys: WeakEntity<KeysView>,
}

impl RevealDialog {
    fn open(
        created: CreatedKey,
        connection: Entity<ConnectionState>,
        state: Entity<AppState>,
        keys: WeakEntity<KeysView>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let title_connection = connection.clone();
        let owner = keys.clone();
        let view = cx.new(|cx| {
            let shown = created.value.expose_secret().to_string();
            Self {
                input: cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .default_value(shown)
                }),
                value: created.value,
                id: created.key.id,
                description: created.key.description,
                masked: true,
                copied: false,
                copy_reset: None,
                copy_focus: cx.focus_handle(),
                connection,
                state,
                keys,
            }
        });
        let weak = view.downgrade();
        owner.update(cx, |keys, _| keys.set_reveal(weak)).ok();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                let copy = view.read(cx).copy_focus.clone();
                copy.focus(window, cx);
            }
            dialog
                .title(dialog_title(
                    t!("keys.reveal_title").to_string(),
                    &title_connection,
                    cx,
                ))
                .w(px(REVEAL_WIDTH))
                .close_button(false)
                .overlay_closable(false)
                .keyboard(false)
                .on_ok(|_, _, _| false)
                .child(view.clone())
        });
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let value = self.value.expose_secret().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = true;
        cx.notify();
        self.copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |view, cx| {
                view.copied = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.masked = !self.masked;
        self.input.update(cx, |i, cx| i.toggle_masked(window, cx));
        cx.notify();
    }

    fn save_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = self.connection.read(cx).profile.clone();
        profile.id = Uuid::new_v4();
        profile.name = format!("{} · {}", profile.name, self.description);
        let prefill = Some((profile, Some(self.value.clone())));
        ConnectDialog::open_new(self.state.clone(), prefill, window, cx);
    }

    fn done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        let id = self.id;
        self.keys.update(cx, |keys, cx| keys.created(id, cx)).ok();
    }
}

impl Render for RevealDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let warning = cx.theme().warning;
        let label = if self.masked {
            t!("connect.key.show")
        } else {
            t!("connect.key.hide")
        };
        let copy = if self.copied {
            t!("keys.copied")
        } else {
            t!("keys.copy")
        };
        v_flex()
            .gap_2()
            .child(notice(
                "keys.reveal_warning",
                t!("keys.reveal_warning").to_string().into(),
                warning,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.input)
                                .id("keys.reveal_value")
                                .aria_label(t!("keys.reveal_title").to_string())
                                .disabled(true),
                        ),
                    )
                    .child(
                        Button::new("keys.reveal_toggle")
                            .ghost()
                            .icon(if self.masked {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            })
                            .accessibility_label(label.to_string())
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("keys.reveal_profile")
                            .label(t!("keys.reveal_profile").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_profile(window, cx);
                            })),
                    )
                    .child(
                        Button::new("keys.reveal_copy")
                            .label(copy.to_string())
                            .track_focus(&self.copy_focus)
                            .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                    )
                    .child(
                        Button::new("keys.reveal_done")
                            .primary()
                            .label(t!("keys.reveal_done").to_string())
                            .on_click(cx.listener(|this, _, window, cx| this.done(window, cx))),
                    ),
            )
    }
}
