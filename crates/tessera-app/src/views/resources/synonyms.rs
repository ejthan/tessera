// SPDX-License-Identifier: GPL-3.0-or-later

//! What the synonym kind adds to [`SetsView`]: the item table and the item dialog.

use std::cell::Cell;

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
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::SetKind;
use tessera_core::relevance::changes::{ItemIssue, Level, dropped_keys};
use tessera_core::relevance::synonyms::{SynonymDraft, check_synonym};
use tessera_widgets::confirm;
use tessera_widgets::json_editor::{JsonEditor, JsonEditorEvent};

use super::sets::{SetsView, issue_text};
use super::stopwords::words_of;
use super::{ListRow, ResourceColumn, client_of, dialog_title, titled};
use crate::actions::{SET_ITEM_DIALOG, SaveResource};
use crate::connection::{ConnectionState, error_text};
use crate::relevance::RelevanceBackendGlobal;

/// Width of the dialog.
const WIDTH: f32 = 480.;
/// Height of the word list.
const WORDS_HEIGHT: f32 = 140.;
/// Height of the raw block.
const RAW_HEIGHT: f32 = 90.;
/// The columns of the item table.
pub(super) const ITEM_COLUMNS: &[ResourceColumn] = &[
    ("id", 140., false),
    ("root", 120., false),
    ("synonyms", 280., false),
    ("locale", 70., false),
];

/// The row of a stored item. One the form cannot read still shows its id.
pub(super) fn item_row(item: &Value) -> ListRow {
    let key = item.get("id").and_then(Value::as_str).unwrap_or_default();
    let draft = SynonymDraft::from_value(item).unwrap_or_default();
    ListRow {
        key: key.to_string(),
        cells: vec![
            key.to_string(),
            draft.root,
            draft.synonyms.join(", "),
            draft.locale,
        ],
        warning: None,
    }
}

/// Adds or edits one item, through the item endpoint: a row is one change and must not overwrite
/// a colleague's other rows (D60).
pub struct SynonymItemDialog {
    view: WeakEntity<SetsView>,
    connection: Entity<ConnectionState>,
    set: String,
    /// The id of the item being edited.
    original: Option<String>,
    id: Entity<InputState>,
    one_way: bool,
    root: Entity<InputState>,
    words: Entity<JsonEditor>,
    locale: Entity<InputState>,
    symbols: Entity<InputState>,
    /// The keys the form has no control for, shown only when there are any.
    raw: Option<Entity<JsonEditor>>,
    error: Option<SharedString>,
    busy: bool,
    check: Option<Task<()>>,
    cancel_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SynonymItemDialog {
    /// Opens it on `item`, or on an empty one.
    pub(super) fn open(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        set: String,
        item: Option<SynonymDraft>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let title = if item.is_some() {
            t!("sets.item.edit_title")
        } else {
            t!("sets.item.add_title")
        }
        .to_string();
        let conn = connection.clone();
        let dialog = cx.new(|cx| Self::new(view, connection, set, item, window, cx));
        let opened = dialog.clone();
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |d, window, cx| {
            if first_build.replace(false) {
                let id = opened.read(cx).id.read(cx).focus_handle(cx);
                id.focus(window, cx);
            }
            d.title(dialog_title(title.clone(), &conn, cx))
                .w(px(WIDTH))
                .child(opened.clone())
        });
        dialog
    }

    fn new(
        view: WeakEntity<SetsView>,
        connection: Entity<ConnectionState>,
        set: String,
        item: Option<SynonymDraft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let original = item.as_ref().map(|d| d.id.clone());
        let draft = item.unwrap_or_default();
        let mut input = |text: &str, cx: &mut Context<Self>| {
            let text = text.to_string();
            cx.new(|cx| InputState::new(window, cx).default_value(text))
        };
        let (id, root) = (input(&draft.id, cx), input(&draft.root, cx));
        let locale = input(&draft.locale, cx);
        let symbols = input(&draft.symbols_to_index.join(", "), cx);
        let words = cx.new(|cx| JsonEditor::plain(&draft.synonyms.join("\n"), window, cx));
        let raw = (!draft.extra.is_empty()).then(|| {
            let text = serde_json::to_string_pretty(&draft.extra).unwrap_or_default();
            cx.new(|cx| JsonEditor::new(&text, false, window, cx))
        });
        let mut subscriptions: Vec<Subscription> = [&id, &root, &locale, &symbols]
            .into_iter()
            .map(|input| cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify()))
            .collect();
        let editors = std::iter::once(&words).chain(raw.iter());
        subscriptions.extend(
            editors.map(|editor| cx.subscribe(editor, |_, _, _: &JsonEditorEvent, cx| cx.notify())),
        );
        Self {
            view,
            connection,
            set,
            original,
            id,
            one_way: !draft.root.trim().is_empty(),
            root,
            words,
            locale,
            symbols,
            raw,
            error: None,
            busy: false,
            check: None,
            cancel_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The word list, for what it holds.
    pub fn words(&self) -> &Entity<JsonEditor> {
        &self.words
    }

    /// The item as the form says now, or why the raw block cannot be read.
    fn draft(&self, cx: &App) -> Result<SynonymDraft, SharedString> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let extra: Map<String, Value> = match &self.raw {
            Some(raw) => serde_json::from_str(&raw.read(cx).text(cx))
                .map_err(|e| t!("sets.item.bad_raw", detail = e.to_string()).to_string())?,
            None => Map::new(),
        };
        let symbols = text(&self.symbols);
        Ok(SynonymDraft {
            id: text(&self.id),
            root: if self.one_way {
                text(&self.root)
            } else {
                String::new()
            },
            synonyms: words_of(&self.words.read(cx).text(cx)),
            locale: text(&self.locale),
            symbols_to_index: symbols
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            extra,
        })
    }

    fn issues(&self, cx: &App) -> Vec<ItemIssue> {
        self.draft(cx).map_or(Vec::new(), |d| check_synonym(&d))
    }

    fn blocked(&self, cx: &App) -> bool {
        self.busy || self.issues(cx).iter().any(|i| i.level() == Level::Error)
    }

    fn set_one_way(&mut self, one_way: bool, cx: &mut Context<Self>) {
        self.one_way = one_way;
        cx.notify();
    }

    fn refuse(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(text.into());
        self.busy = false;
        cx.notify();
    }

    /// Checks the id against a fresh read, because a `PUT` overwrites silently, then saves,
    /// after a confirmation on prod.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let draft = match self.draft(cx) {
            Ok(draft) => draft,
            Err(error) => return self.refuse(error, cx),
        };
        let Some(client) = client_of(&self.connection, cx) else {
            return self.refuse(t!("status.not_connected").to_string(), cx);
        };
        self.busy = true;
        self.error = None;
        let fresh =
            cx.global::<RelevanceBackendGlobal>()
                .0
                .set(client, SetKind::Synonym, self.set.clone());
        self.check = Some(cx.spawn_in(window, async move |this, cx| {
            let fresh = fresh.await;
            this.update_in(cx, |this, window, cx| {
                let profile = this.connection.read(cx).profile.clone();
                let fresh = match fresh {
                    Ok(fresh) => fresh,
                    Err(e) => return this.refuse(error_text(&e, &profile), cx),
                };
                let moved = this.original.as_ref() != Some(&draft.id);
                let taken = fresh
                    .items
                    .iter()
                    .any(|i| i.get("id").and_then(Value::as_str) == Some(&draft.id));
                if moved && taken {
                    let text = t!("sets.item.exists", id = draft.id).to_string();
                    return this.refuse(text, cx);
                }
                this.confirm_save(draft, window, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn confirm_save(&mut self, draft: SynonymDraft, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.connection.read(cx).profile.clone();
        if !profile.environment.is_prod() {
            return self.send(draft, window, cx);
        }
        self.busy = false;
        let this = cx.entity().downgrade();
        let title = t!("sets.item.save_title", id = draft.id).to_string();
        confirm::open(
            titled(&title, &profile),
            t!("sets.item.save_line", name = self.set).to_string(),
            t!("sets.save").to_string(),
            move |window, cx| {
                this.update(cx, |this, cx| this.send(draft.clone(), window, cx))
                    .ok();
            },
            window,
            cx,
        );
        cx.notify();
    }

    /// `save_item` new, then `delete_item` old for a changed id. Detached: closing the tab must
    /// not abort a write, and above all not between the two.
    fn send(&mut self, draft: SynonymDraft, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = client_of(&self.connection, cx) else {
            return self.refuse(t!("status.not_connected").to_string(), cx);
        };
        self.busy = true;
        let backend = cx.global::<RelevanceBackendGlobal>().0.clone();
        let (set, view) = (self.set.clone(), self.view.clone());
        let old = self.original.clone().filter(|old| *old != draft.id);
        let profile = self.connection.read(cx).profile.clone();
        let sent = draft.to_value();
        let id = draft.id;
        let save = backend.save_item(
            client.clone(),
            SetKind::Synonym,
            set.clone(),
            id.clone(),
            sent.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let stored = match save.await {
                Ok(stored) => stored,
                Err(e) => {
                    this.update(cx, |this, cx| this.refuse(error_text(&e, &profile), cx))
                        .ok();
                    return;
                }
            };
            let mut failed = None;
            if let Some(old) = old {
                let delete = backend.delete_item(client, SetKind::Synonym, set, old.clone());
                if let Err(e) = delete.await {
                    let detail = error_text(&e, &profile);
                    let text = t!("sets.item.both_exist", old = old, new = id, detail = detail);
                    failed = Some(SharedString::from(text.to_string()));
                }
            }
            this.update_in(cx, |_, window, cx| window.close_dialog(cx))
                .ok();
            let dropped = dropped_keys(&sent, &stored);
            view.update_in(cx, |view, window, cx| {
                view.item_saved(&dropped, failed, window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for SynonymItemDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let issues = self.issues(cx);
        let blocked = self.blocked(cx);
        let field = |label: String, id: &'static str, state: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(label.clone())
                .child(Input::new(state).id(id).aria_label(label))
        };
        let line = |id: SharedString, text: SharedString| {
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(danger)
                .child(text)
        };
        let kind = ButtonGroup::new("sets.item.kind")
            .outline()
            .small()
            .child(
                Button::new("sets.item.multi")
                    .label(t!("sets.item.multi").to_string())
                    .selected(!self.one_way)
                    .on_click(cx.listener(|this, _, _, cx| this.set_one_way(false, cx))),
            )
            .child(
                Button::new("sets.item.one_way")
                    .label(t!("sets.item.one_way").to_string())
                    .selected(self.one_way)
                    .on_click(cx.listener(|this, _, _, cx| this.set_one_way(true, cx))),
            );
        v_flex()
            .key_context(SET_ITEM_DIALOG)
            .gap_2()
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.submit(window, cx)))
            .child(field(
                t!("sets.item.id").to_string(),
                "sets.item.id",
                &self.id,
            ))
            .child(kind)
            .when(self.one_way, |el| {
                el.child(field(
                    t!("sets.item.root").to_string(),
                    "sets.item.root",
                    &self.root,
                ))
            })
            .child(t!("sets.item.words").to_string())
            .child(div().h(px(WORDS_HEIGHT)).child(self.words.clone()))
            .child(field(
                t!("sets.item.locale").to_string(),
                "sets.item.locale",
                &self.locale,
            ))
            .child(field(
                t!("sets.item.symbols").to_string(),
                "sets.item.symbols",
                &self.symbols,
            ))
            .when_some(self.raw.clone(), |el, raw| {
                el.child(t!("sets.item.raw").to_string())
                    .child(div().h(px(RAW_HEIGHT)).child(raw))
            })
            .children(issues.iter().enumerate().map(|(i, issue)| {
                line(
                    format!("sets.item.issue.{i}").into(),
                    issue_text(issue).into(),
                )
            }))
            .children(
                self.error
                    .clone()
                    .map(|error| line("sets.item.error".into(), error)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("sets.item.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sets.item.save")
                            .primary()
                            .label(t!("sets.save").to_string())
                            .loading(self.busy)
                            .disabled(blocked)
                            .children(Kbd::binding_for_action(
                                &SaveResource,
                                Some(SET_ITEM_DIALOG),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
