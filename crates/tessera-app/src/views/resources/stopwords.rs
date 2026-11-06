// SPDX-License-Identifier: GPL-3.0-or-later

//! The stopword sets of a connection: a list and an editor of one word per line.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::StopwordSet;
use tessera_widgets::confirm;
use tessera_widgets::json_editor::JsonEditor;

use super::name_dialog::NameDialog;
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, row_button, toolbar,
};
use crate::actions::{
    DeleteResource, FocusResourceFilter, NewResource, NextControl, PreviousControl, RAW_BLOCK,
    RESOURCES, ReloadResource, SaveResource,
};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/stopwords.html";
/// The columns of the table.
const COLUMNS: &[ResourceColumn] = &[
    ("id", 180., false),
    ("words", 220., false),
    ("locale", 80., false),
];
/// Width of the list side.
const LIST_WIDTH: f32 = 500.;

/// The words of `text`: one per line, trimmed, empties dropped.
pub(super) fn words_of(text: &str) -> Vec<String> {
    let lines = text.lines().map(str::trim).filter(|w| !w.is_empty());
    lines.map(str::to_string).collect()
}

/// `words` lowercased and sorted, the form both sides are compared and shown in. The stored
/// order is the server's.
fn sorted(words: &[String]) -> Vec<String> {
    let mut words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    words.sort();
    words
}

/// The content of a stopwords tab.
pub struct StopwordsView {
    connection: Entity<ConnectionState>,
    list: Entity<ResourceList>,
    sets: Vec<StopwordSet>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// The set in the editor, which may not exist yet.
    editing: Option<String>,
    /// The selection the editor followed last, so that only a new one replaces it.
    followed: Option<String>,
    editor: Entity<JsonEditor>,
    locale: Entity<InputState>,
    /// A save is running.
    saving: bool,
    /// Why the last write failed, or what it left.
    error: Option<SharedString>,
    note: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl StopwordsView {
    /// The sets of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("stopwords", COLUMNS, window, cx));
        let subscription = cx.observe_in(&list, window, |this, _, window, cx| {
            this.follow_selection(window, cx);
        });
        let mut this = Self {
            connection,
            list,
            sets: Vec::new(),
            loaded: false,
            list_error: None,
            load_task: None,
            editing: None,
            followed: None,
            editor: cx.new(|cx| {
                let mut editor = JsonEditor::plain("", window, cx);
                editor.set_read_only(false, cx);
                editor
            }),
            locale: cx.new(|cx| InputState::new(window, cx)),
            saving: false,
            error: None,
            note: None,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    /// The editor, for what it holds.
    pub fn editor(&self) -> &Entity<JsonEditor> {
        &self.editor
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx.global::<RelevanceBackendGlobal>().0.stopwords(client);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match answer {
                    Ok(sets) => {
                        this.list_error = None;
                        this.take_sets(sets, cx);
                    }
                    Err(e) => {
                        let profile = &this.connection.read(cx).profile;
                        this.list_error = Some(list_error_text(&e, profile));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn take_sets(&mut self, mut sets: Vec<StopwordSet>, cx: &mut Context<Self>) {
        sets.sort_by(|a, b| a.id.cmp(&b.id));
        let rows = sets.iter().map(|set| ListRow {
            key: set.id.clone(),
            cells: vec![
                set.id.clone(),
                sorted(&set.stopwords).join(", "),
                set.locale.clone().unwrap_or_default(),
            ],
            warning: None,
        });
        let rows: Vec<ListRow> = rows.collect();
        self.sets = sets;
        self.list.update(cx, |l, cx| l.set_rows(rows, cx));
    }

    /// Whether the editor differs from what the server holds.
    fn dirty(&self, cx: &App) -> bool {
        let Some(id) = &self.editing else {
            return false;
        };
        let words = sorted(&words_of(&self.editor.read(cx).text(cx)));
        let locale = self.locale.read(cx).value();
        match self.sets.iter().find(|s| &s.id == id) {
            Some(set) => {
                words != sorted(&set.stopwords)
                    || locale.trim() != set.locale.as_deref().unwrap_or_default()
            }
            None => !words.is_empty() || !locale.trim().is_empty(),
        }
    }

    /// Runs `then`, after a Discard confirmation while the editor holds unsaved text.
    fn discarding(
        &mut self,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dirty(cx) {
            return then(self, window, cx);
        }
        let view = cx.entity().downgrade();
        confirm::open(
            t!("stopwords.discard.title").to_string(),
            t!("stopwords.discard.body").to_string(),
            t!("stopwords.discard.confirm").to_string(),
            move |window, cx| {
                view.update(cx, |this, cx| then(this, window, cx)).ok();
            },
            window,
            cx,
        );
    }

    /// A newly selected row replaces the editor.
    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.list.read(cx).selected(cx);
        cx.notify();
        if selected.is_none() || selected == self.followed {
            return;
        }
        let set = self.sets.iter().find(|s| Some(&s.id) == selected.as_ref());
        let Some(set) = set.cloned() else {
            return;
        };
        if self.dirty(cx) {
            // Until Discard, the row stays the one in the editor: Delete acts on the row.
            let shown = self.followed.clone();
            self.list.update(cx, |l, cx| l.select(shown, cx));
        }
        let follow = move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.followed.clone_from(&selected);
            this.list.update(cx, |l, cx| l.select(selected.clone(), cx));
            this.show(&set, window, cx);
        };
        self.discarding(follow, window, cx);
    }

    fn show(&mut self, set: &StopwordSet, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = Some(set.id.clone());
        self.error = None;
        self.note = None;
        let text = sorted(&set.stopwords).join("\n");
        self.editor
            .update(cx, |e, cx| e.set_text(&text, window, cx));
        let locale = set.locale.clone().unwrap_or_default();
        self.locale
            .update(cx, |l, cx| l.set_value(locale, window, cx));
    }

    fn ask_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let check = {
                    let connection = connection.clone();
                    move |name: String, cx: &mut App| exists(&connection, name, cx)
                };
                let on_name = move |name: String, window: &mut Window, cx: &mut App| {
                    let new = StopwordSet {
                        id: name,
                        stopwords: Vec::new(),
                        locale: None,
                    };
                    let new = new.clone();
                    let start =
                        move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                            // No row shows this set yet, and Delete must not act on another.
                            this.followed = None;
                            this.list.update(cx, |l, cx| l.select(None, cx));
                            this.show(&new, window, cx);
                            this.editor.update(cx, |e, cx| e.focus(window, cx));
                        };
                    this.update(cx, |this, cx| this.discarding(start, window, cx))
                        .ok();
                };
                let title = t!("stopwords.new_title").to_string();
                NameDialog::open(title, connection, check, on_name, window, cx);
            });
        });
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone().filter(|_| !self.saving) else {
            return;
        };
        let words = words_of(&self.editor.read(cx).text(cx));
        if words.is_empty() {
            self.error = Some(t!("stopwords.empty_words").to_string().into());
            return cx.notify();
        }
        let locale = self.locale.read(cx).value().trim().to_string();
        let locale = (!locale.is_empty()).then_some(locale);
        self.error = None;
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let send = {
                    let view = this.clone();
                    move |window: &mut Window, cx: &mut App| {
                        send_save(
                            view.clone(),
                            id.clone(),
                            words.clone(),
                            locale.clone(),
                            window,
                            cx,
                        )
                    }
                };
                if !connection.read(cx).profile.environment.is_prod() {
                    return send(window, cx).detach();
                }
                let danger = Danger {
                    title: t!("stopwords.save_title").to_string(),
                    lines: vec![t!("stopwords.save_line").to_string()],
                    expected: String::new(),
                    action: t!("stopwords.save").to_string(),
                    typed: false,
                };
                confirm_write(danger, &connection, send, report_to(this), window, cx);
            });
        });
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.list.read(cx).selected(cx) else {
            return;
        };
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let prod = connection.read(cx).profile.environment.is_prod();
                let danger = Danger {
                    title: t!("stopwords.delete.title", name = id).to_string(),
                    lines: vec![t!("stopwords.delete.line").to_string()],
                    expected: id.clone(),
                    action: t!("stopwords.delete").to_string(),
                    typed: prod,
                };
                let view = this.clone();
                let conn = connection.clone();
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), conn.clone(), id.clone(), cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![
            (self.list.read(cx).selected(cx).is_none(), &DeleteResource),
            (self.editing.is_none(), &SaveResource),
        ]
    }

    fn editor_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locked = self.connection.read(cx).writes_locked();
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let Some(id) = self.editing.clone() else {
            return div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("stopwords.pick").to_string())
                .into_any_element();
        };
        v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .child(
                div()
                    .id("stopwords.editing")
                    .test_support()
                    .aria_label(SharedString::from(id.clone()))
                    .child(id),
            )
            .child(
                div()
                    .key_context(RAW_BLOCK)
                    .flex_1()
                    .min_h_0()
                    .child(self.editor.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(t!("stopwords.col.locale").to_string())
                    .child(
                        div().w(px(120.)).child(
                            Input::new(&self.locale)
                                .id("stopwords.locale")
                                .aria_label(t!("stopwords.col.locale").to_string()),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("stopwords.save")
                            .primary()
                            .small()
                            .label(t!("stopwords.save").to_string())
                            .loading(self.saving)
                            .disabled(self.saving)
                            .when(locked, |b| {
                                b.child(super::super::env_badge::lock("stopwords.save", false, cx))
                            })
                            .children(Kbd::binding_for_action(
                                &SaveResource,
                                Some(RESOURCES),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("stopwords.error", e, danger)),
            )
            .children(
                self.note
                    .clone()
                    .map(|e| notice("stopwords.note", e, muted)),
            )
            .into_any_element()
    }
}

impl Focusable for StopwordsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StopwordsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, border) = (theme.danger, theme.border);
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.list.read(cx).is_empty(cx);
        let none = self.list.read(cx).selected(cx).is_none();
        let editor = self.editor_pane(window, cx);
        let left = v_flex()
            .size_full()
            .child(toolbar(
                "stopwords",
                &self.list,
                &self.focus_handle,
                locked,
                window,
                cx,
            ))
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("stopwords.list_error", e, danger)),
            )
            .child(if empty && self.editing.is_none() {
                empty_state("stopwords", DOCS_URL).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            })
            .child(
                div().flex_none().p_2().child(
                    row_button(
                        "stopwords.delete".into(),
                        t!("stopwords.delete").to_string(),
                        &DeleteResource,
                        (none, locked),
                        &self.focus_handle,
                        window,
                        cx,
                    )
                    .danger(),
                ),
            );
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewResource, window, cx| this.ask_new(window, cx)))
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.ask_delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(|_: &NextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousControl, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(
                h_resizable("stopwords.split")
                    .child(
                        resizable_panel()
                            .size(px(LIST_WIDTH))
                            .border_r_1()
                            .border_color(border)
                            .child(left),
                    )
                    .child(resizable_panel().child(editor)),
            )
    }
}

/// Refuses a name that exists, after a fresh list.
fn exists(
    connection: &Entity<ConnectionState>,
    name: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let list = cx.global::<RelevanceBackendGlobal>().0.stopwords(client);
    cx.spawn(async move |_| {
        let sets = list.await.map_err(|e| list_error_text(&e, &profile))?;
        match sets.iter().find(|s| s.id == name) {
            Some(set) => {
                let n = set.stopwords.len();
                Err(t!("stopwords.exists", name = name, n = n)
                    .to_string()
                    .into())
            }
            None => Ok(()),
        }
    })
}

impl ErrorSlot for StopwordsView {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

/// Saves and shows what was stored, which is not what was sent, unless the user moved on. Runs
/// on through a closed tab.
fn send_save(
    view: WeakEntity<StopwordsView>,
    id: String,
    words: Vec<String>,
    locale: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some((connection, false)) = view
        .read_with(cx, |v, _| (v.connection.clone(), v.saving))
        .ok()
    else {
        return Task::ready(Ok(()));
    };
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let sent = sorted(&words);
    let save =
        cx.global::<RelevanceBackendGlobal>()
            .0
            .save_stopwords(client, id.clone(), words, locale);
    view.update(cx, |v, cx| {
        v.saving = true;
        cx.notify();
    })
    .ok();
    window.spawn(cx, async move |cx| {
        let stored = save
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)));
        view.update_in(cx, |view, window, cx| {
            view.saving = false;
            match stored {
                Ok(set) => {
                    let text = words_of(&view.editor.read(cx).text(cx));
                    let same = view.editing.as_ref() == Some(&id) && sorted(&text) == sent;
                    view.sets.retain(|s| s.id != set.id);
                    view.sets.push(set.clone());
                    if same {
                        view.followed = Some(id);
                        view.list
                            .update(cx, |l, cx| l.select(view.followed.clone(), cx));
                        view.show(&set, window, cx);
                        if sorted(&set.stopwords) != sent {
                            let n = set.stopwords.len();
                            view.note = Some(t!("stopwords.normalised", n = n).to_string().into());
                        }
                    }
                    view.reload(cx);
                }
                Err(error) => view.error = Some(error),
            }
            cx.notify();
        })
        .ok();
        // Shown on the view above, which also holds the editor.
        Ok(())
    })
}

fn send_delete(
    view: WeakEntity<StopwordsView>,
    connection: Entity<ConnectionState>,
    id: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx
        .global::<RelevanceBackendGlobal>()
        .0
        .delete_stopwords(client, id.clone());
    cx.spawn(async move |cx| {
        delete
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        view.update(cx, |view, cx| {
            if view.editing.as_ref() == Some(&id) {
                view.editing = None;
                view.followed = None;
            }
            view.reload(cx);
            cx.notify();
        })
        .ok();
        Ok(())
    })
}
