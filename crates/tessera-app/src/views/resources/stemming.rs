// SPDX-License-Identifier: GPL-3.0-or-later

//! The stemming dictionaries of a connection: their words, Import and Delete.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, h_resizable, resizable_panel, v_flex};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use tessera_core::stemming::{ImportCheck, collections_using};

use super::stemming_dialog::StemmingDialog;
use super::{
    Danger, ErrorSlot, ListRow, ResourceColumn, ResourceList, client_of, confirm_write,
    empty_state, list_error_text, notice, report_to, row_button, toolbar,
};
use crate::actions::{DeleteResource, FocusResourceFilter, NewResource, ReloadResource};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, error_text};

/// Where the empty state points to.
const DOCS_URL: &str = "https://typesense.org/docs/30.2/api/stemming.html";
/// The columns of the dictionaries.
const COLUMNS: &[ResourceColumn] = &[("id", 240., false)];
/// The columns of the words.
const WORD_COLUMNS: &[ResourceColumn] = &[("word", 200., false), ("root", 200., false)];
/// Width of the list side.
const LIST_WIDTH: f32 = 320.;
/// The most missing words a result names.
const MISSING_SHOWN: usize = 10;

/// What a finished import says, and the warning about words that are not as they should be.
pub(super) fn result_text(check: &ImportCheck) -> (String, Option<String>) {
    let mut text = t!("stemming.imported", sent = check.sent).to_string();
    if check.kept_root > 0 {
        text.push(' ');
        text.push_str(&t!("stemming.kept_root", kept = check.kept_root));
    }
    let named = check.missing.iter().take(MISSING_SHOWN);
    let named = named.cloned().collect::<Vec<_>>().join(", ");
    let warning = (!check.missing.is_empty()).then(|| {
        t!(
            "stemming.missing",
            count = check.missing.len(),
            words = named
        )
        .to_string()
    });
    (text, warning)
}

/// The content of a stemming tab.
pub struct StemmingView {
    connection: Entity<ConnectionState>,
    list: Entity<ResourceList>,
    words: Entity<ResourceList>,
    /// The first answer arrived.
    loaded: bool,
    list_error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// The dictionary the words belong to.
    followed: Option<String>,
    words_error: Option<SharedString>,
    words_task: Option<Task<()>>,
    /// Why the last write failed, in full.
    error: Option<SharedString>,
    /// What the last import did, and what is wrong with it.
    result: Option<(SharedString, Option<SharedString>)>,
    /// The dialog opened last.
    dialog: Option<Entity<StemmingDialog>>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl StemmingView {
    /// The dictionaries of `connection`; loads them.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ResourceList::new("stemming", COLUMNS, window, cx));
        let subscription = cx.observe(&list, |this, _, cx| this.follow_selection(cx));
        let mut this = Self {
            connection,
            list,
            words: cx.new(|cx| ResourceList::new("stemming.words", WORD_COLUMNS, window, cx)),
            loaded: false,
            list_error: None,
            load_task: None,
            followed: None,
            words_error: None,
            words_task: None,
            error: None,
            result: None,
            dialog: None,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    /// The dialog opened last.
    pub fn dialog(&self) -> Option<&Entity<StemmingDialog>> {
        self.dialog.as_ref()
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        // A delete names the collections that use a dictionary.
        self.connection
            .update(cx, |c, cx| c.load_collection_list(cx));
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let load = cx.global::<AdminBackendGlobal>().0.dictionaries(client);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match answer {
                    Ok(mut ids) => {
                        ids.sort();
                        this.list_error = None;
                        let rows = ids.iter().map(|id| ListRow {
                            key: id.clone(),
                            cells: vec![id.clone()],
                            warning: None,
                        });
                        let rows = rows.collect();
                        this.list.update(cx, |l, cx| l.set_rows(rows, cx));
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

    fn selected(&self, cx: &App) -> Option<String> {
        self.list.read(cx).selected(cx)
    }

    fn follow_selection(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let selected = self.selected(cx);
        if selected != self.followed {
            self.followed = selected;
            self.load_words(cx);
        }
    }

    fn load_words(&mut self, cx: &mut Context<Self>) {
        self.words_error = None;
        self.words.update(cx, |l, cx| l.set_rows(Vec::new(), cx));
        let Some((id, client)) = self.followed.clone().zip(client_of(&self.connection, cx)) else {
            return;
        };
        let load = cx.global::<AdminBackendGlobal>().0.dictionary(client, id);
        self.words_task = Some(cx.spawn(async move |this, cx| {
            let answer = load.await;
            this.update(cx, |this, cx| {
                match answer {
                    Ok(dictionary) => {
                        let rows = dictionary.words.into_iter().map(|w| ListRow {
                            key: w.word.clone(),
                            cells: vec![w.word, w.root],
                            warning: None,
                        });
                        let rows = rows.collect();
                        this.words.update(cx, |l, cx| l.set_rows(rows, cx));
                    }
                    Err(e) => {
                        let profile = &this.connection.read(cx).profile;
                        this.words_error = Some(error_text(&e, profile).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The import of `id` ended: shows it and what it did.
    pub(super) fn imported(&mut self, id: String, check: &ImportCheck, cx: &mut Context<Self>) {
        let (text, warning) = result_text(check);
        self.error = None;
        self.result = Some((text.into(), warning.map(Into::into)));
        // Read again, even when the selection stays.
        self.followed = None;
        self.list.update(cx, |l, cx| l.select(Some(id), cx));
        self.reload(cx);
    }

    fn ask_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected(cx);
        let connection = self.connection.clone();
        let this = cx.entity().downgrade();
        self.dialog = Some(StemmingDialog::open(this, connection, selected, window, cx));
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else {
            return;
        };
        let cached = &self.connection.read(cx).collection_cache.collections;
        let using = collections_using(cached.as_deref().unwrap_or_default(), &id);
        let this = cx.entity().downgrade();
        let connection = self.connection.clone();
        self.error = None;
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let prod = connection.read(cx).profile.environment.is_prod();
                let mut lines = vec![t!("stemming.delete_consequence").to_string()];
                if !using.is_empty() {
                    lines.push(t!("stemming.delete_used", list = using.join(", ")).to_string());
                }
                let danger = Danger {
                    title: t!("stemming.delete_title", name = id).to_string(),
                    lines,
                    expected: id.clone(),
                    action: t!("stemming.delete").to_string(),
                    typed: prod || !using.is_empty(),
                };
                let (view, conn) = (this.clone(), connection.clone());
                let run = move |_: &mut Window, cx: &mut App| {
                    send_delete(view.clone(), conn.clone(), id.clone(), cx)
                };
                confirm_write(danger, &connection, run, report_to(this), window, cx);
            });
        });
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![(self.selected(cx).is_none(), &DeleteResource)]
    }

    fn words_pane(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        if self.selected(cx).is_none() {
            return div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("stemming.pick").to_string())
                .into_any_element();
        }
        v_flex()
            .size_full()
            .child(div().w(px(280.)).p_2().child(ResourceList::filter_input(
                &self.words,
                "stemming.words",
                cx,
            )))
            .children(
                self.words_error
                    .clone()
                    .map(|e| notice("stemming.words_error", e, danger)),
            )
            .child(div().flex_1().min_h_0().child(self.words.clone()))
            .into_any_element()
    }
}

impl Focusable for StemmingView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StemmingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, warning, muted, border) = (
            theme.danger,
            theme.warning,
            theme.muted_foreground,
            theme.border,
        );
        let locked = self.connection.read(cx).writes_locked();
        let empty = self.loaded && self.list_error.is_none() && self.list.read(cx).is_empty(cx);
        let none = self.selected(cx).is_none();
        let right = self.words_pane(cx);
        let (result, missing) = self.result.clone().unzip();
        let left = v_flex()
            .size_full()
            .child(toolbar(
                "stemming",
                &self.list,
                &self.focus_handle,
                locked,
                window,
                cx,
            ))
            .children(
                self.list_error
                    .clone()
                    .map(|e| notice("stemming.list_error", e, danger)),
            )
            .children(result.map(|r| notice("stemming.result", r, muted)))
            .children(
                missing
                    .flatten()
                    .map(|m| notice("stemming.missing", m, warning)),
            )
            .child(if empty {
                empty_state("stemming", DOCS_URL).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.list.clone())
                    .into_any_element()
            })
            .children(
                self.error
                    .clone()
                    .map(|e| notice("stemming.error", e, danger)),
            )
            .child(
                div().flex_none().p_2().child(
                    row_button(
                        "stemming.delete".into(),
                        t!("stemming.delete").to_string(),
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
            .on_action(cx.listener(|this, _: &NewResource, window, cx| {
                this.ask_import(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteResource, window, cx| {
                this.ask_delete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadResource, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &FocusResourceFilter, window, cx| {
                ResourceList::focus_filter(&this.list, window, cx);
            }))
            .child(
                h_resizable("stemming.split")
                    .child(
                        resizable_panel()
                            .size(px(LIST_WIDTH))
                            .border_r_1()
                            .border_color(border)
                            .child(left),
                    )
                    .child(resizable_panel().child(right)),
            )
    }
}

impl ErrorSlot for StemmingView {
    fn error_slot(&mut self) -> &mut Option<SharedString> {
        &mut self.error
    }
}

fn send_delete(
    view: WeakEntity<StemmingView>,
    connection: Entity<ConnectionState>,
    id: String,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(client) = client_of(&connection, cx) else {
        return Task::ready(Err(t!("status.not_connected").to_string().into()));
    };
    let profile = connection.read(cx).profile.clone();
    let delete = cx
        .global::<AdminBackendGlobal>()
        .0
        .delete_dictionary(client, id);
    cx.spawn(async move |cx| {
        delete
            .await
            .map_err(|e| SharedString::from(error_text(&e, &profile)))?;
        view.update(cx, |view, cx| {
            view.error = None;
            view.result = None;
            view.reload(cx);
        })
        .ok();
        Ok(())
    })
}
