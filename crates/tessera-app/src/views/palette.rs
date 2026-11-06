// SPDX-License-Identifier: GPL-3.0-or-later

//! The command palette and quick switch.

use std::cell::Cell;
use std::collections::HashSet;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_core::profile::Environment;
use tessera_core::search::fuzzy::{PALETTE_LIMIT, rank};
use uuid::Uuid;

use super::env_badge;
use crate::actions::action_label;
use crate::state::AppState;

/// Width of the dialog.
const WIDTH: f32 = 560.;

/// What the palette lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Actions, collections, saved queries and connections.
    All,
    /// Collections only: quick switch.
    Collections,
}

/// What a row does.
enum PaletteTarget {
    Action(Box<dyn Action>),
    Collection(Uuid, String),
    Saved(Uuid),
    Connection(Uuid),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Actions,
    Collections,
    Saved,
    Connections,
}

impl Group {
    fn label(self) -> String {
        match self {
            Self::Actions => t!("palette.group.actions"),
            Self::Collections => t!("palette.group.collections"),
            Self::Saved => t!("palette.group.saved"),
            Self::Connections => t!("palette.group.connections"),
        }
        .to_string()
    }
}

/// One row: its text, the connection it belongs to, and what it does.
struct PaletteEntry {
    group: Group,
    label: SharedString,
    connection: Option<String>,
    environment: Option<Environment>,
    /// The element id of the row.
    id: SharedString,
    target: PaletteTarget,
}

/// The dialog's content.
struct CommandPalette {
    command: Entity<CommandState>,
    state: Entity<AppState>,
    entries: Vec<PaletteEntry>,
    /// Indices into `entries`, best first.
    shown: Vec<usize>,
    previous_focus: Option<FocusHandle>,
}

/// Opens the palette on what the focused view offers now.
pub fn open(mode: PaletteMode, state: Entity<AppState>, window: &mut Window, cx: &mut App) {
    let previous_focus = window.focused(cx);
    // Before the dialog takes the focus, which would hide the focused view's actions.
    let stack = window.context_stack();
    let disabled = |action: &dyn Action| stack.iter().any(|c| c.contains(action.name()));
    let actions: Vec<(&'static str, Box<dyn Action>)> = match mode {
        PaletteMode::All => window
            .available_actions(cx)
            .into_iter()
            .filter(|action| !disabled(action.as_ref()))
            .filter_map(|action| Some((action_label(action.name())?, action)))
            .collect(),
        PaletteMode::Collections => Vec::new(),
    };
    let entries = entries(mode, actions, &state, cx);
    let view = cx.new(|cx| {
        let shown = (0..entries.len().min(PALETTE_LIMIT)).collect();
        CommandPalette {
            command: cx.new(|cx| CommandState::new(window, cx)),
            state,
            entries,
            shown,
            previous_focus,
        }
    });
    let first_build = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_build.replace(false) {
            let command = view.read(cx).command.clone();
            command.update(cx, |c, cx| c.focus(window, cx));
        }
        dialog.w(px(WIDTH)).p_0().child(view.clone())
    });
}

/// `actions` first, in their label's order, then what they lead to.
fn entries(
    mode: PaletteMode,
    actions: Vec<(&'static str, Box<dyn Action>)>,
    state: &Entity<AppState>,
    cx: &App,
) -> Vec<PaletteEntry> {
    let state = state.read(cx);
    let mut entries: Vec<PaletteEntry> = actions
        .into_iter()
        .map(|(key, action)| PaletteEntry {
            group: Group::Actions,
            label: t!(key).to_string().into(),
            connection: None,
            environment: None,
            id: format!("palette.action.{}", action.name()).into(),
            target: PaletteTarget::Action(action),
        })
        .collect();
    entries.sort_by(|a, b| a.label.cmp(&b.label));
    for conn in &state.connections {
        let conn = conn.read(cx);
        if conn.connected.is_none() {
            continue;
        }
        let id = conn.profile.id;
        // The poller's list stands in until a tab has loaded the full one.
        let cached = conn.collection_cache.collections.iter().flatten();
        let polled = conn.collections.iter().flatten();
        let names: Vec<String> = if conn.collection_cache.collections.is_some() {
            cached.map(|c| c.name.clone()).collect()
        } else {
            polled.map(|c| c.name.clone()).collect()
        };
        entries.extend(names.into_iter().map(|name| PaletteEntry {
            group: Group::Collections,
            label: name.clone().into(),
            connection: Some(conn.profile.name.clone()),
            environment: Some(conn.profile.environment.clone()),
            id: format!("palette.collection.{id}.{name}").into(),
            target: PaletteTarget::Collection(id, name),
        }));
    }
    if mode == PaletteMode::Collections {
        return entries;
    }
    let selected = state
        .selected
        .and_then(|id| state.connection(id, cx))
        .filter(|c| c.read(cx).connected.is_some());
    if let Some(conn) = selected {
        let profile = &conn.read(cx).profile;
        let queries = state.saved.queries.iter();
        entries.extend(
            queries
                .filter(|q| q.profile == profile.id)
                .map(|query| PaletteEntry {
                    group: Group::Saved,
                    label: query.name.clone().into(),
                    connection: Some(profile.name.clone()),
                    environment: Some(profile.environment.clone()),
                    id: format!("palette.saved.{}", query.id).into(),
                    target: PaletteTarget::Saved(query.id),
                }),
        );
    }
    let open = state.connections.iter().map(|c| &c.read(cx).profile);
    let temporary = state.temporary.iter().map(|(p, _)| p);
    let mut seen = HashSet::new();
    let profiles = state.profiles.iter().chain(temporary).chain(open);
    entries.extend(
        profiles
            .filter(|p| seen.insert(p.id))
            .map(|profile| PaletteEntry {
                group: Group::Connections,
                label: profile.name.clone().into(),
                connection: None,
                environment: Some(profile.environment.clone()),
                id: format!("palette.connection.{}", profile.id).into(),
                target: PaletteTarget::Connection(profile.id),
            }),
    );
    entries
}

impl CommandPalette {
    fn query_changed(&mut self, query: &str, cx: &mut Context<Self>) {
        let all = self.entries.iter().enumerate().collect();
        let ranked = rank(query, all, |(_, e)| e.label.as_ref(), PALETTE_LIMIT);
        self.shown = ranked.into_iter().map(|(i, _)| i).collect();
        cx.notify();
    }

    /// `shown` by group, the group of the best match first, so Enter takes that match.
    fn groups(&self) -> Vec<(Group, Vec<usize>)> {
        let mut groups: Vec<(Group, Vec<usize>)> = Vec::new();
        for &i in &self.shown {
            let group = self.entries[i].group;
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, rows)) => rows.push(i),
                None => groups.push((group, vec![i])),
            }
        }
        groups
    }

    fn item(&self, entry: &PaletteEntry) -> CommandItem {
        let (id, label) = (entry.id.clone(), entry.label.clone());
        let (connection, environment) = (entry.connection.clone(), entry.environment.clone());
        let action = match &entry.target {
            PaletteTarget::Action(action) => Some(action.boxed_clone()),
            _ => None,
        };
        let previous = self.previous_focus.clone();
        CommandItem::new()
            .label(label.clone())
            .child(move |window, cx| {
                let kbd = action.as_ref().and_then(|action| match &previous {
                    Some(focus) => Kbd::binding_for_action_in(action.as_ref(), focus, window),
                    None => Kbd::global_binding_for_action(action.as_ref(), window),
                });
                let muted = cx.theme().muted_foreground;
                h_flex()
                    .id(id.clone())
                    .test_support()
                    .aria_label(label.clone())
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .children(
                        connection
                            .clone()
                            .map(|name| div().text_xs().text_color(muted).child(name)),
                    )
                    .when_some(environment.as_ref(), |el, environment| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("{id}.env")))
                                .test_support()
                                .aria_label(SharedString::from(env_badge::label(environment)))
                                .child(env_badge::render(environment, cx)),
                        )
                    })
                    .when_some(kbd, |el, kbd| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("{id}.kbd")))
                                .test_support()
                                .child(kbd),
                        )
                    })
            })
    }
}

/// Runs row `path` of the palette, which is closed by then, in the focus it opened from.
fn confirm(view: &Entity<CommandPalette>, path: IndexPath, window: &mut Window, cx: &mut App) {
    let palette = view.read(cx);
    let groups = palette.groups();
    let Some(&i) = groups
        .get(path.section)
        .and_then(|(_, rows)| rows.get(path.row))
    else {
        return;
    };
    let previous = palette.previous_focus.clone();
    let state = palette.state.clone();
    let target = match &palette.entries[i].target {
        PaletteTarget::Action(action) => PaletteTarget::Action(action.boxed_clone()),
        PaletteTarget::Collection(id, name) => PaletteTarget::Collection(*id, name.clone()),
        PaletteTarget::Saved(id) => PaletteTarget::Saved(*id),
        PaletteTarget::Connection(id) => PaletteTarget::Connection(*id),
    };
    window.close_dialog(cx);
    window.defer(cx, move |window, cx| {
        if let Some(previous) = previous {
            previous.focus(window, cx);
        }
        match target {
            PaletteTarget::Action(action) => window.dispatch_action(action, cx),
            PaletteTarget::Collection(id, name) => {
                state.update(cx, |s, cx| s.open_collection(id, name, cx));
            }
            PaletteTarget::Saved(id) => {
                let saved = state.read(cx).saved.queries.iter();
                let Some(query) = saved.into_iter().find(|q| q.id == id).cloned() else {
                    return;
                };
                let loaded = Some((query.searches, query.union));
                state.update(cx, |s, cx| s.open_search(query.profile, loaded, cx));
            }
            PaletteTarget::Connection(id) => state.update(cx, |s, cx| s.connect(id, None, cx)),
        }
    });
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let on_confirm = view.clone();
        let mut command = Command::new(&self.command)
            .filterable(false)
            .bordered(false)
            .placeholder(t!("palette.placeholder").to_string())
            .on_query(move |query, _, cx| {
                view.update(cx, |this, cx| this.query_changed(query, cx));
            })
            .on_confirm(move |path, window, cx| confirm(&on_confirm, path, window, cx));
        for (group, rows) in self.groups() {
            let items = rows.iter().map(|&i| self.item(&self.entries[i]));
            command = command.group(CommandGroup::new().label(group.label()).items(items));
        }
        div().id("palette").test_support().child(command)
    }
}
