// SPDX-License-Identifier: GPL-3.0-or-later

//! History, saved queries and presets: the lists in the toolbar and what fills them.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    IntoElement, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _,
    Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{MULTI_SEARCH_LIMIT, Preset};
use tessera_core::search::HISTORY_SETTLE_MS;
use tessera_core::search::history::{HistoryEntry, HistoryItem, search_entry};
use tessera_core::search::params::SearchDraft;
use tessera_core::search::presets::{PresetError, preset_drafts, preset_value};
use tessera_core::search::saved::{SavedQuery, savable};
use tessera_widgets::confirm;
use uuid::Uuid;

use super::super::env_badge;
use super::super::resources::titled;
use super::{SearchWorkbench, param_error_text};
use crate::actions::{DeleteSavedQuery, LeavePreset, QUERY_LIST, SaveQuery, WORKBENCH};
use crate::connection::{PresetsChanged, error_text};
use crate::search::SearchBackendGlobal;

/// Height of a row of a list.
const ROW_HEIGHT: f32 = 44.;
/// Rows of a list shown without scrolling.
const ROWS_SHOWN: usize = 8;
/// Width of a list.
const LIST_WIDTH: f32 = 420.;

/// A list in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryList {
    History,
    Saved,
    Presets,
}

impl QueryList {
    /// The start of the ids of this list's elements.
    fn id(self) -> &'static str {
        match self {
            Self::History => "search.history",
            Self::Saved => "search.saved",
            Self::Presets => "search.preset",
        }
    }
}

/// What a row of a list loads.
enum Entry {
    Search {
        union: bool,
        searches: Vec<SearchDraft>,
    },
    Saved(SavedQuery),
    Preset(Preset),
}

impl Entry {
    fn title_and_note(&self) -> (String, String) {
        let collections = |searches: &[SearchDraft]| {
            let names: Vec<&str> = searches.iter().map(|d| d.collection.as_str()).collect();
            names.join(", ")
        };
        match self {
            Self::Search { searches, .. } => {
                let text = |d: &SearchDraft, key| {
                    let value = d.params.get(key).and_then(Value::as_str);
                    value.unwrap_or_default().to_string()
                };
                let queries: Vec<String> = searches.iter().map(|d| text(d, "q")).collect();
                let filter = searches.first().map(|d| text(d, "filter_by"));
                let note = [collections(searches), filter.unwrap_or_default()];
                (queries.join(" | "), note.join(" ").trim().to_string())
            }
            Self::Saved(query) => {
                let tags = query.tags.join(", ");
                let note = [collections(&query.searches), tags];
                (query.name.clone(), note.join(" ").trim().to_string())
            }
            Self::Preset(preset) => (preset.name.clone(), String::new()),
        }
    }
}

/// The lists' state in the workbench.
pub(super) struct Queries {
    open: Option<QueryList>,
    /// Filters the open list; only one is open at a time.
    filter: Entity<InputState>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// The saved list leaves the active collection's filter off.
    all_collections: bool,
    /// The server's presets, fetched each time the list opens.
    presets: Option<Result<Vec<Preset>, SharedString>>,
    presets_task: Option<Task<()>>,
    /// Why the picked preset was not loaded.
    pick_error: Option<SharedString>,
    /// Why the last save or delete failed. The list is gone by then, so it shows in the tab.
    write_error: Option<SharedString>,
}

impl Queries {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<SearchWorkbench>,
    ) -> (Self, Subscription) {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("search.queries.filter").to_string())
        });
        let changed = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.queries.selected = 0;
                cx.notify();
            }
        });
        let queries = Self {
            open: None,
            filter,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            all_collections: false,
            presets: None,
            presets_task: None,
            pick_error: None,
            write_error: None,
        };
        (queries, changed)
    }
}

/// A run on its way into the history.
pub(super) struct PendingHistory {
    entry: HistoryEntry,
    /// It stood long enough; a run the user started does at once.
    settled: bool,
    /// The server answered it.
    answered: bool,
}

/// What is wrong with a preset's value.
pub(crate) fn preset_error_text(error: &PresetError) -> String {
    match error {
        PresetError::NotAnObject => t!("search.preset.not_an_object").to_string(),
        PresetError::TooMany => t!("search.preset.too_many", n = MULTI_SEARCH_LIMIT).to_string(),
        PresetError::Param(error) => param_error_text(error),
    }
}

/// Tags typed as a list: split at commas, trimmed, empties dropped.
fn split_tags(text: &str) -> Vec<String> {
    let tags = text.split(',').map(str::trim).filter(|t| !t.is_empty());
    tags.map(str::to_string).collect()
}

type OnName = Box<dyn Fn(String, Vec<String>, &mut Window, &mut App)>;

/// Asks for a name and, for a saved query, tags.
struct NameDialog {
    name: Entity<InputState>,
    tags: Option<Entity<InputState>>,
    error: Option<SharedString>,
    on_name: OnName,
    cancel_focus: FocusHandle,
    _subscription: Subscription,
}

impl NameDialog {
    fn open(
        title: String,
        with_tags: bool,
        on_name: impl Fn(String, Vec<String>, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = cx.new(|cx| {
            let name = cx.new(|cx| InputState::new(window, cx));
            let enter = |this: &mut Self,
                         event: &InputEvent,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            };
            Self {
                _subscription: cx.subscribe_in(&name, window, move |this, _, event, window, cx| {
                    enter(this, event, window, cx);
                }),
                tags: with_tags.then(|| cx.new(|cx| InputState::new(window, cx))),
                name,
                error: None,
                on_name: Box::new(on_name),
                cancel_focus: cx.focus_handle(),
            }
        });
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                view.read(cx)
                    .name
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx);
            }
            let enter = view.clone();
            dialog
                .title(title.clone())
                .w(px(LIST_WIDTH))
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
                .child(view.clone())
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some(t!("search.save.name_required").to_string().into());
            return cx.notify();
        }
        let tags = self.tags.as_ref().map(|t| split_tags(&t.read(cx).value()));
        window.close_dialog(cx);
        (self.on_name)(name, tags.unwrap_or_default(), window, cx);
    }
}

impl Render for NameDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        v_flex()
            .gap_2()
            .child(t!("search.save.name").to_string())
            .child(
                Input::new(&self.name)
                    .id("search.save.name")
                    .aria_label(t!("search.save.name").to_string()),
            )
            .children(self.tags.as_ref().map(|tags| {
                v_flex()
                    .gap_2()
                    .child(t!("search.save.tags").to_string())
                    .child(
                        Input::new(tags)
                            .id("search.save.tags")
                            .aria_label(t!("search.save.tags").to_string()),
                    )
            }))
            .children(self.error.clone().map(|error| {
                div()
                    .id("search.save.error")
                    .test_support()
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("search.save.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.cancel_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("search.save.ok")
                            .primary()
                            .label(t!("search.save.ok").to_string())
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

impl SearchWorkbench {
    fn profile_id(&self, cx: &App) -> Uuid {
        self.connection.read(cx).profile.id
    }

    /// Follows a run: only `explicit` ones and those that stand for the settle time enter
    /// the history, and only once they answered. Replaces the run followed before.
    pub(super) fn follow_for_history(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let Some(entry) = search_entry(self.union, &self.drafts) else {
            return self.forget_run();
        };
        self.pending_history = Some(PendingHistory {
            entry,
            settled: explicit,
            answered: false,
        });
        self.history_task = (!explicit).then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(HISTORY_SETTLE_MS))
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(pending) = &mut this.pending_history {
                        pending.settled = true;
                    }
                    this.record_when_due(cx);
                })
                .ok();
            })
        });
    }

    /// The followed run answered, or failed, which keeps it out of the history.
    pub(super) fn history_answer(&mut self, succeeded: bool, cx: &mut Context<Self>) {
        match &mut self.pending_history {
            Some(pending) if succeeded => pending.answered = true,
            _ => self.forget_run(),
        }
        self.record_when_due(cx);
    }

    /// Stops following the run.
    pub(super) fn forget_run(&mut self) {
        self.pending_history = None;
        self.history_task = None;
    }

    fn record_when_due(&mut self, cx: &mut Context<Self>) {
        let due = |p: &mut PendingHistory| p.settled && p.answered;
        let Some(pending) = self.pending_history.take_if(due) else {
            return;
        };
        self.history_task = None;
        let at = SystemTime::now().duration_since(UNIX_EPOCH);
        let item = HistoryItem {
            at: at.map_or(0, |d| d.as_secs().cast_signed()),
            profile: self.profile_id(cx),
            entry: pending.entry,
        };
        self.state.update(cx, |s, cx| s.record_history(item, cx));
    }

    /// Opens list `kind`, or closes it when it is open.
    pub(super) fn toggle_list(
        &mut self,
        kind: QueryList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = (self.queries.open != Some(kind)).then_some(kind);
        self.set_list(next, window, cx);
    }

    fn set_list(&mut self, list: Option<QueryList>, window: &mut Window, cx: &mut Context<Self>) {
        self.queries.open = list;
        self.queries.selected = 0;
        self.queries.pick_error = None;
        self.queries
            .filter
            .update(cx, |f, cx| f.set_value("", window, cx));
        if list == Some(QueryList::Presets) {
            self.load_presets(cx);
        }
        cx.notify();
    }

    /// Closes the list for a dialog, which gives the focus back to where it was: not the filter,
    /// which goes with the list.
    fn leave_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_list(None, window, cx);
        self.focus_handle.focus(window, cx);
    }

    fn load_presets(&mut self, cx: &mut Context<Self>) {
        self.queries.presets = None;
        let Some(connected) = &self.connection.read(cx).connected else {
            let error = t!("status.not_connected").to_string().into();
            self.queries.presets = Some(Err(error));
            return;
        };
        let backend = cx.global::<SearchBackendGlobal>().0.clone();
        let fetch = backend.presets(connected.client.clone());
        self.queries.presets_task = Some(cx.spawn(async move |this, cx| {
            let answer = fetch.await;
            this.update(cx, |this, cx| {
                let profile = &this.connection.read(cx).profile;
                let answer = answer.map_err(|e| error_text(&e, profile).into());
                this.queries.presets = Some(answer);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The rows of list `kind` for the filter text.
    fn entries(&self, kind: QueryList, cx: &App) -> Vec<Entry> {
        let needle = self.queries.filter.read(cx).value().to_lowercase();
        let hit = |text: &str| text.to_lowercase().contains(&needle);
        let profile = self.profile_id(cx);
        let state = self.state.read(cx);
        match kind {
            QueryList::History => state
                .history
                .searches(profile, &needle)
                .into_iter()
                .filter_map(|item| match &item.entry {
                    HistoryEntry::Search { union, searches } => Some(Entry::Search {
                        union: *union,
                        searches: searches.clone(),
                    }),
                    HistoryEntry::Raw { .. } => None,
                })
                .collect(),
            QueryList::Saved => {
                let active = &self.draft().collection;
                state
                    .saved
                    .queries
                    .iter()
                    .filter(|q| q.profile == profile)
                    .filter(|q| {
                        self.queries.all_collections
                            || q.searches.iter().any(|d| d.collection == *active)
                    })
                    .filter(|q| hit(&q.name) || q.tags.iter().any(|t| hit(t)))
                    .cloned()
                    .map(Entry::Saved)
                    .collect()
            }
            QueryList::Presets => {
                let presets = self.queries.presets.iter().flatten().flatten();
                let found = presets.filter(|p| hit(&p.name)).cloned();
                found.map(Entry::Preset).collect()
            }
        }
    }

    /// Moves the selection of the open list by `step`.
    pub(super) fn step_list(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(kind) = self.queries.open else {
            return;
        };
        let last = self.entries(kind, cx).len().saturating_sub(1);
        let selected = self.queries.selected.saturating_add_signed(step).min(last);
        self.queries.selected = selected;
        self.queries
            .scroll
            .scroll_to_item(selected, ScrollStrategy::Center);
        cx.notify();
    }

    /// Loads the selected row of the open list and runs nothing.
    pub(super) fn load_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.queries.open else {
            return;
        };
        let entries = self.entries(kind, cx);
        let (drafts, union) = match entries.into_iter().nth(self.queries.selected) {
            Some(Entry::Search { union, searches }) => (searches, union),
            Some(Entry::Saved(query)) => (query.searches, query.union),
            Some(Entry::Preset(preset)) => {
                match preset_drafts(&preset.value, &self.draft().collection) {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        self.queries.pick_error = Some(preset_error_text(&error).into());
                        return cx.notify();
                    }
                }
            }
            None => return,
        };
        self.set_list(None, window, cx);
        self.load(drafts, union, window, cx);
    }

    fn ask_clear_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_list(window, cx);
        let (state, profile) = (self.state.clone(), self.profile_id(cx));
        confirm::open(
            t!("search.history.clear_title").to_string(),
            t!("search.history.clear_body").to_string(),
            t!("search.history.clear").to_string(),
            move |_, cx| state.update(cx, |s, cx| s.clear_history(profile, cx)),
            window,
            cx,
        );
    }

    /// Asks to delete the selected row of the saved list.
    pub(super) fn ask_delete_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queries.open != Some(QueryList::Saved) {
            return;
        }
        let entries = self.entries(QueryList::Saved, cx);
        let Some(Entry::Saved(query)) = entries.into_iter().nth(self.queries.selected) else {
            return;
        };
        self.leave_list(window, cx);
        let this = cx.entity().downgrade();
        confirm::open(
            t!("search.saved.delete_title").to_string(),
            t!("search.saved.delete_body", name = query.name).to_string(),
            t!("search.saved.delete").to_string(),
            move |_, cx| {
                this.update(cx, |this, cx| {
                    let delete = this.state.update(cx, |s, cx| s.delete_query(query.id, cx));
                    this.report_write(delete, cx);
                })
                .ok();
            },
            window,
            cx,
        );
    }

    /// Shows the failure of `write`, which owns the save, when it comes.
    fn report_write(&mut self, write: Task<Result<(), String>>, cx: &mut Context<Self>) {
        self.queries.write_error = None;
        cx.spawn(async move |this, cx| {
            let result = write.await;
            this.update(cx, |this, cx| {
                this.queries.write_error = result.err().map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn save_blocked(&self, cx: &App) -> bool {
        self.run_blocked() || (self.preset.is_none() && self.state.read(cx).saved_error.is_some())
    }

    /// Opens the Save dialog.
    pub(super) fn ask_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_blocked(cx) {
            return;
        }
        let this = cx.entity().downgrade();
        NameDialog::open(
            t!("search.save.title").to_string(),
            true,
            move |name, tags, window, cx| {
                this.update(cx, |this, cx| this.submit_query(name, tags, window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    fn submit_query(
        &mut self,
        name: String,
        tags: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = self.profile_id(cx);
        let saved = &self.state.read(cx).saved.queries;
        let same = saved
            .iter()
            .find(|q| q.profile == profile && q.name == name);
        let replaces = same.map(|q| q.id);
        let (searches, dropped) = savable(&self.drafts);
        let query = SavedQuery {
            id: replaces.unwrap_or_else(Uuid::new_v4),
            profile,
            name,
            tags,
            union: self.union,
            searches,
        };
        if replaces.is_none() {
            return self.store_query(query, dropped, cx);
        }
        let this = cx.entity().downgrade();
        let body = t!("search.save.replace_body", name = query.name).to_string();
        confirm::open(
            t!("search.save.replace_title").to_string(),
            body,
            t!("search.save.replace").to_string(),
            move |_, cx| {
                this.update(cx, |this, cx| this.store_query(query.clone(), dropped, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    fn store_query(&mut self, query: SavedQuery, dropped: bool, cx: &mut Context<Self>) {
        let save = self.state.update(cx, |s, cx| s.save_query(query, cx));
        self.report_write(save, cx);
        if dropped {
            self.queries.write_error = Some(t!("search.save.secrets_dropped").to_string().into());
        }
    }

    /// Unlocks if needed, then asks for the preset's name.
    fn ask_preset_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_list(window, cx);
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                NameDialog::open(
                    t!("search.preset.save_title").to_string(),
                    false,
                    move |name, _, window, cx| {
                        this.update(cx, |this, cx| this.submit_preset(name, window, cx))
                            .ok();
                    },
                    window,
                    cx,
                );
            });
        });
    }

    fn submit_preset(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let (drafts, dropped) = savable(&self.drafts);
        let value = match preset_value(&drafts, self.union, None) {
            Ok(value) => value,
            Err(error) => {
                self.queries.write_error = Some(param_error_text(&error).into());
                return cx.notify();
            }
        };
        let known = self.queries.presets.iter().flatten().flatten();
        if !known.into_iter().any(|p| p.name == name) {
            return self.send_preset(name, value, dropped, cx);
        }
        let this = cx.entity().downgrade();
        let body = t!("search.preset.replace_body", name = name).to_string();
        confirm::open(
            t!("search.preset.replace_title").to_string(),
            body,
            t!("search.preset.replace").to_string(),
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.send_preset(name.clone(), value.clone(), dropped, cx);
                })
                .ok();
            },
            window,
            cx,
        );
    }

    /// Saves to the preset of preset mode, keys left out, asking first on prod.
    pub(super) fn save_preset_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self.preset.clone().filter(|_| !self.save_blocked(cx)) else {
            return;
        };
        let name = preset.name;
        let (drafts, dropped) = savable(&self.drafts);
        let value = match preset_value(&drafts, self.union, preset.wrapper) {
            Ok(value) => value,
            Err(error) => {
                self.queries.write_error = Some(param_error_text(&error).into());
                return cx.notify();
            }
        };
        let this = cx.entity().downgrade();
        let profile = self.connection.read(cx).profile.clone();
        let prod = profile.environment.is_prod();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let send = {
                    let name = name.clone();
                    move |cx: &mut App| {
                        this.update(cx, |this, cx| {
                            this.send_preset(name.clone(), value.clone(), dropped, cx);
                        })
                        .ok();
                    }
                };
                if !prod {
                    return send(cx);
                }
                confirm::open(
                    titled(&t!("presets.save_title", name = name), &profile),
                    t!("presets.save_line", name = name).to_string(),
                    t!("presets.save").to_string(),
                    move |_, cx| send(cx),
                    window,
                    cx,
                );
            });
        });
    }

    /// The band of preset mode.
    pub(super) fn preset_band(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let name = self.preset()?;
        let text = SharedString::from(t!("presets.mode", name = name).to_string());
        let locked = self.connection.read(cx).writes_locked();
        Some(
            h_flex()
                .id("presets.band")
                .test_support()
                .aria_label(text.clone())
                .flex_none()
                .gap_3()
                .px_2()
                .py_1()
                .items_center()
                .text_sm()
                .bg(cx.theme().accent)
                .text_color(cx.theme().accent_foreground)
                .child(div().flex_1().truncate().child(text))
                .child(
                    Button::new("presets.save")
                        .primary()
                        .small()
                        .label(t!("presets.save").to_string())
                        .when(locked, |b| {
                            b.child(env_badge::lock("presets.save", false, cx))
                        })
                        .children(Kbd::binding_for_action(&SaveQuery, Some(WORKBENCH), window))
                        .disabled(self.save_blocked(cx))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_preset_mode(window, cx);
                        })),
                )
                .child(
                    Button::new("presets.leave")
                        .small()
                        .label(t!("presets.leave").to_string())
                        .children(Kbd::binding_for_action(
                            &LeavePreset,
                            Some(WORKBENCH),
                            window,
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.leave_preset(cx))),
                ),
        )
    }

    /// Detached: closing the tab must not abort a write. `dropped`: `value` lost key parameters.
    fn send_preset(&mut self, name: String, value: Value, dropped: bool, cx: &mut Context<Self>) {
        self.queries.write_error = None;
        let Some(connected) = &self.connection.read(cx).connected else {
            self.queries.write_error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        let backend = cx.global::<SearchBackendGlobal>().0.clone();
        let save = backend.save_preset(connected.client.clone(), name.clone(), value.clone());
        let connection = self.connection.clone();
        cx.spawn(async move |this, cx| {
            let result = save.await;
            if result.is_ok() {
                connection.update(cx, |_, cx| cx.emit(PresetsChanged));
            }
            this.update(cx, |this, cx| {
                let profile = &this.connection.read(cx).profile;
                this.queries.write_error = match result {
                    Err(e) => Some(error_text(&e, profile).into()),
                    Ok(stored) if stored != value => {
                        Some(t!("presets.mismatch", name = name).to_string().into())
                    }
                    Ok(_) if dropped => Some(t!("search.save.secrets_dropped").to_string().into()),
                    Ok(_) => None,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The trigger of list `kind` with its list.
    pub(super) fn list_button(
        &self,
        kind: QueryList,
        label: String,
        action: &dyn Action,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Popover {
        let id = kind.id();
        let (on_open, on_content) = (cx.entity(), cx.entity());
        let trigger = Button::new(id)
            .ghost()
            .small()
            .label(label)
            .children(Kbd::binding_for_action(action, Some(WORKBENCH), window));
        Popover::new(SharedString::from(format!("{id}.popover")))
            .open(self.queries.open == Some(kind))
            .track_focus(&self.queries.filter.read(cx).focus_handle(cx))
            .on_open_change(move |open, window, cx| {
                on_open.update(cx, |this, cx| {
                    if *open != (this.queries.open == Some(kind)) {
                        this.toggle_list(kind, window, cx);
                    }
                });
            })
            .trigger(trigger)
            .content(move |_, window, cx| {
                on_content.update(cx, |this, cx| this.list_content(kind, window, cx))
            })
    }

    fn list_rows(&self, kind: QueryList, cx: &mut Context<Self>) -> AnyElement {
        let id = kind.id();
        let rows: Vec<(String, String)> = self
            .entries(kind, cx)
            .iter()
            .map(Entry::title_and_note)
            .collect();
        if rows.is_empty() {
            let note = SharedString::from(t!("search.queries.empty").to_string());
            return div()
                .id("search.queries.empty")
                .test_support()
                .aria_label(note.clone())
                .p_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(note)
                .into_any_element();
        }
        let count = rows.len();
        let selected = self.queries.selected;
        let view = cx.entity();
        let rows = Rc::new(rows);
        let (muted, selected_bg) = (cx.theme().muted_foreground, cx.theme().secondary);
        let list = uniform_list(
            SharedString::from(format!("{id}.list")),
            count,
            move |range, _, _| {
                range
                    .map(|ix| {
                        let (title, note) = rows[ix].clone();
                        let view = view.clone();
                        v_flex()
                            .id(SharedString::from(format!("{id}.row.{ix}")))
                            .test_support()
                            .aria_label(SharedString::from(format!("{title} {note}")))
                            .h(px(ROW_HEIGHT))
                            .px_2()
                            .justify_center()
                            .overflow_hidden()
                            .cursor_pointer()
                            .text_sm()
                            .when(ix == selected, |el| el.bg(selected_bg))
                            .on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.queries.selected = ix;
                                    this.load_selected(window, cx);
                                });
                            })
                            .child(div().truncate().child(title))
                            .child(div().truncate().text_xs().text_color(muted).child(note))
                    })
                    .collect()
            },
        )
        .track_scroll(&self.queries.scroll)
        .h(px(ROW_HEIGHT * count.min(ROWS_SHOWN) as f32));
        list.into_any_element()
    }

    fn list_footer(&self, kind: QueryList, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match kind {
            QueryList::History => {
                let empty = self.entries(kind, cx).is_empty();
                Button::new("search.history.clear")
                    .ghost()
                    .small()
                    .label(t!("search.history.clear").to_string())
                    .disabled(empty)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.ask_clear_history(window, cx);
                    }))
                    .into_any_element()
            }
            QueryList::Saved => {
                let error = self.state.read(cx).saved_error.clone();
                let empty = self.entries(kind, cx).is_empty();
                v_flex()
                    .gap_1()
                    .children(error.map(|error| {
                        let text = SharedString::from(error);
                        div()
                            .id("search.saved.error")
                            .test_support()
                            .aria_label(text.clone())
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(text)
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .justify_between()
                            .child(
                                Checkbox::new("search.saved.all")
                                    .checked(self.queries.all_collections)
                                    .label(t!("search.saved.all").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.queries.all_collections ^= true;
                                        this.queries.selected = 0;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("search.saved.delete")
                                    .danger()
                                    .small()
                                    .label(t!("search.saved.delete").to_string())
                                    .children(Kbd::binding_for_action(
                                        &DeleteSavedQuery,
                                        Some(QUERY_LIST),
                                        window,
                                    ))
                                    .disabled(empty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.ask_delete_saved(window, cx);
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            QueryList::Presets => {
                let list_error = match &self.queries.presets {
                    Some(Err(error)) => Some(error.clone()),
                    _ => None,
                };
                let error = self.queries.pick_error.clone().or(list_error);
                let ready = matches!(self.queries.presets, Some(Ok(_)));
                let locked = self.connection.read(cx).writes_locked();
                v_flex()
                    .gap_1()
                    .children(error.map(|error| {
                        div()
                            .id("search.preset.error")
                            .test_support()
                            .aria_label(error.clone())
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error)
                    }))
                    .when(self.queries.presets.is_none(), |el| {
                        el.child(Spinner::new().small())
                    })
                    .child(
                        Button::new("search.preset.save")
                            .ghost()
                            .small()
                            .label(t!("search.preset.save").to_string())
                            .children(
                                locked.then(|| env_badge::lock("search.preset.save", false, cx)),
                            )
                            // Without the list a name in use would be replaced unasked.
                            .disabled(!ready || self.run_blocked())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.ask_preset_name(window, cx);
                            })),
                    )
                    .into_any_element()
            }
        }
    }

    fn list_content(&self, kind: QueryList, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .key_context(QUERY_LIST)
            .w(px(LIST_WIDTH))
            .gap_2()
            .p_1()
            .child(
                Input::new(&self.queries.filter)
                    .id("search.queries.filter")
                    .aria_label(t!("search.queries.filter").to_string())
                    .small(),
            )
            .child(self.list_rows(kind, cx))
            .child(self.list_footer(kind, window, cx))
            .into_any_element()
    }

    /// Why the last save or delete failed.
    pub(super) fn write_note(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let error = self.queries.write_error.clone()?;
        Some(
            div()
                .id("search.queries.error")
                .test_support()
                .aria_label(error.clone())
                .flex_none()
                .px_2()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn tags_are_trimmed_and_empties_dropped() {
        assert_eq!(split_tags(" a, ,b ,"), ["a", "b"]);
        assert!(split_tags("  ").is_empty());
    }
}
