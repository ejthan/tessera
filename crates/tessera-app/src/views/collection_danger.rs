// SPDX-License-Identifier: GPL-3.0-or-later

//! Truncate and delete a collection, each behind a type-the-name dialog (spec §7.8).

use std::rc::Rc;

use gpui_kit::{App, Entity, SharedString, Task, Window};
use rust_i18n::t;
use tessera_client::{Alias, Client, Error};
use tessera_widgets::confirm_danger::{self, DangerOptions};

use super::overview::grouped;
use crate::collections::{CollectionsBackendGlobal, aliases_of};
use crate::connection::{ConnectionState, error_text, long_error_text};
use crate::state::AppState;

/// What a caller runs after the delete.
type After = dyn Fn(&mut App);

/// Which of the two actions.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Truncate,
    Delete,
}

/// Asks for unlock if needed, reloads the collection, asks for the name, then empties `name`.
/// `documents` is the count to show if the reload fails.
pub fn truncate(
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    documents: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let after: Rc<After> = Rc::new(|_| {});
    prepare(
        Kind::Truncate,
        state,
        connection,
        name,
        documents,
        after,
        window,
        cx,
    );
}

/// Asks for unlock if needed, reloads the collection and aliases, asks for the name, then
/// deletes `name`. `documents` is the count to show if the reload fails.
pub fn delete(
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    documents: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let after: Rc<After> = Rc::new(|_| {});
    prepare(
        Kind::Delete,
        state,
        connection,
        name,
        documents,
        after,
        window,
        cx,
    );
}

/// [`delete`], then `after` once the collection is gone.
pub fn delete_then(
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    documents: u64,
    after: impl Fn(&mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let after: Rc<After> = Rc::new(after);
    prepare(
        Kind::Delete,
        state,
        connection,
        name,
        documents,
        after,
        window,
        cx,
    );
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    kind: Kind,
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    documents: u64,
    after: Rc<After>,
    window: &mut Window,
    cx: &mut App,
) {
    // A second keypress while the first dialog is on its way must not open another.
    if connection.read(cx).danger_loading {
        return;
    }
    let conn = connection.clone();
    conn.update(cx, |c, cx| {
        c.ensure_writable(window, cx, move |window, cx| {
            let Some(client) = client(&connection, cx) else {
                let detail = t!("status.not_connected").to_string();
                let lines = lines(kind, &name, documents, Ok(&[]), Some(detail));
                return open(kind, state, connection, name, lines, after, window, cx);
            };
            let list = cx.global::<CollectionsBackendGlobal>().0.list(client);
            connection.update(cx, |c, _| c.danger_loading = true);
            window
                .spawn(cx, async move |cx| {
                    let fresh = list.await;
                    cx.update(|window, cx| {
                        connection.update(cx, |c, _| c.danger_loading = false);
                        let lines = match &fresh {
                            Ok((all, aliases)) => {
                                let count = all.iter().find(|c| c.name == name);
                                let count = count.map_or(documents, |c| c.num_documents);
                                let profile = &connection.read(cx).profile;
                                let aliases = match aliases {
                                    Ok(aliases) => Ok(aliases.as_slice()),
                                    Err(e) => Err(error_text(e, profile)),
                                };
                                lines(kind, &name, count, aliases, None)
                            }
                            Err(e) => {
                                // The cache may be stale; say so rather than hide it.
                                let conn = connection.read(cx);
                                let detail = error_text(e, &conn.profile);
                                let cache = &conn.collection_cache;
                                let aliases = match &cache.aliases_error {
                                    Some(e) => Err(e.to_string()),
                                    None => Ok(cache.aliases.as_slice()),
                                };
                                lines(kind, &name, documents, aliases, Some(detail))
                            }
                        };
                        open(kind, state, connection, name, lines, after, window, cx);
                    })
                    .ok();
                })
                .detach();
        })
    });
}

/// The consequence lines; `aliases` is `Err` with why they failed to load, `unchecked` is why
/// the numbers could not be reloaded.
fn lines(
    kind: Kind,
    name: &str,
    documents: u64,
    aliases: Result<&[Alias], String>,
    unchecked: Option<String>,
) -> Vec<SharedString> {
    let count = grouped(documents);
    let mut lines: Vec<SharedString> = Vec::new();
    if kind == Kind::Truncate {
        lines.push(
            t!("danger.truncate.consequence", count = count, name = name)
                .to_string()
                .into(),
        );
    } else {
        lines.push(
            t!("danger.delete.consequence", name = name, count = count)
                .to_string()
                .into(),
        );
        match aliases {
            Ok(aliases) => {
                let dangling: Vec<&str> = aliases_of(aliases, name).collect();
                if !dangling.is_empty() {
                    let text = t!("danger.delete.aliases", aliases = dangling.join(", "));
                    lines.push(text.to_string().into());
                }
            }
            Err(detail) => {
                let text = t!(
                    "danger.delete.aliases_unchecked",
                    name = name,
                    detail = detail
                );
                lines.push(text.to_string().into());
            }
        }
    }
    if let Some(detail) = unchecked {
        let text = t!("danger.unchecked", detail = detail);
        lines.push(text.to_string().into());
    }
    lines
}

fn client(connection: &Entity<ConnectionState>, cx: &App) -> Option<Client> {
    connection
        .read(cx)
        .connected
        .as_ref()
        .map(|c| c.client.clone())
}

#[allow(clippy::too_many_arguments)]
fn open(
    kind: Kind,
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    consequences: Vec<SharedString>,
    after: Rc<After>,
    window: &mut Window,
    cx: &mut App,
) {
    let key = if kind == Kind::Truncate {
        "truncate"
    } else {
        "delete"
    };
    let profile = connection.read(cx).profile.clone();
    let options = DangerOptions {
        title: t!(format!("danger.{key}.title"), name = name)
            .to_string()
            .into(),
        consequences,
        expected: name.clone().into(),
        action_label: t!(format!("danger.{key}.action")).to_string().into(),
        prod: profile.environment.is_prod(),
    };
    confirm_danger::open(
        options,
        move |_, cx| {
            let (state, connection, name) = (state.clone(), connection.clone(), name.clone());
            let after = after.clone();
            let Some(client) = client(&connection, cx) else {
                let text = t!("status.not_connected").to_string();
                return Task::ready(Err(text.into()));
            };
            let backend = cx.global::<CollectionsBackendGlobal>().0.clone();
            let profile = profile.clone();
            let run: crate::collections::BoxFuture<Result<(), Error>> = match kind {
                Kind::Truncate => {
                    let fut = backend.truncate(client, name.clone());
                    Box::pin(async move { fut.await.map(drop) })
                }
                Kind::Delete => backend.delete(client, name.clone()),
            };
            cx.spawn(async move |cx| {
                match run.await {
                    // Already gone is what the user wanted; DELETE is retried on another node
                    // after a network error, so the retry can find it deleted.
                    Err(Error::Http { status: 404, .. }) if kind == Kind::Delete => {}
                    Err(e) => {
                        // It may have landed (a timeout) and the user may have stopped
                        // waiting: show what the server has now, keeping the old list meanwhile.
                        connection.update(cx, |c, cx| c.load_collection_list(cx));
                        return Err(long_error_text(&e, &profile).into());
                    }
                    Ok(()) => {}
                }
                connection.update(cx, |c, cx| match kind {
                    // Views of the collection reload and show 0 documents.
                    Kind::Truncate => c.collection_changed(name.clone(), cx),
                    Kind::Delete => c.invalidate_collections(cx),
                });
                if kind == Kind::Delete {
                    // A favorite of a deleted collection would leave a sidebar entry that opens
                    // a 404 (UX rule 4).
                    let unstarred = state.update(cx, |s, cx| {
                        s.close_collection(profile.id, name.clone(), cx);
                        s.set_favorite(profile.id, &name, false, cx)
                    });
                    if let Err(detail) = unstarred.await {
                        let text = t!("danger.delete.favorite_error", name = name, detail = detail);
                        return Err(text.to_string().into());
                    }
                    cx.update(|cx| after(cx));
                }
                Ok(())
            })
        },
        window,
        cx,
    );
}
