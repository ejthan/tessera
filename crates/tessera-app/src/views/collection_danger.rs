// SPDX-License-Identifier: GPL-3.0-or-later

//! Truncate and delete a collection, each behind a type-the-name dialog (spec §7.8).

use gpui_kit::{App, Entity, SharedString, Task, Window};
use rust_i18n::t;
use tessera_client::{Alias, Client, Error};
use tessera_widgets::confirm_danger::{self, DangerOptions};

use super::overview::grouped;
use crate::collections::{CollectionsBackendGlobal, aliases_of};
use crate::connection::{ConnectionState, error_text};
use crate::state::AppState;

/// Which of the two actions.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Truncate,
    Delete,
}

/// Asks for unlock if needed, then for the name, then empties `name`, which holds `documents`.
pub fn truncate(
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    documents: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let conn = connection.clone();
    conn.update(cx, |c, cx| {
        c.ensure_writable(window, cx, move |window, cx| {
            let lines = vec![
                t!(
                    "danger.truncate.consequence",
                    count = grouped(documents),
                    name = name
                )
                .to_string()
                .into(),
            ];
            open(Kind::Truncate, state, connection, name, lines, window, cx);
        })
    });
}

/// Asks for unlock if needed, reloads the aliases, asks for the name, then deletes `name`,
/// which holds `documents` (as of the fresh list when it has the collection).
pub fn delete(
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    documents: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let conn = connection.clone();
    conn.update(cx, |c, cx| {
        c.ensure_writable(window, cx, move |window, cx| {
            let Some(client) = client(&connection, cx) else {
                return;
            };
            let list = cx.global::<CollectionsBackendGlobal>().0.list(client);
            window
                .spawn(cx, async move |cx| {
                    let fresh = list.await;
                    cx.update(|window, cx| {
                        let cached;
                        let (count, aliases, unchecked): (u64, &[Alias], Option<String>) =
                            match &fresh {
                                Ok((all, aliases)) => {
                                    let count = all.iter().find(|c| c.name == name);
                                    (count.map_or(documents, |c| c.num_documents), aliases, None)
                                }
                                Err(e) => {
                                    // The cache may be stale; say so rather than hide the aliases.
                                    cached = connection.read(cx).collection_cache.aliases.clone();
                                    let profile = &connection.read(cx).profile;
                                    (documents, &cached, Some(error_text(e, profile)))
                                }
                            };
                        let mut lines: Vec<SharedString> = vec![
                            t!(
                                "danger.delete.consequence",
                                name = name,
                                count = grouped(count)
                            )
                            .to_string()
                            .into(),
                        ];
                        let dangling: Vec<&str> = aliases_of(aliases, &name).collect();
                        if !dangling.is_empty() {
                            lines.push(
                                t!("danger.delete.aliases", aliases = dangling.join(", "))
                                    .to_string()
                                    .into(),
                            );
                        }
                        if let Some(detail) = unchecked {
                            lines.push(
                                t!("danger.delete.aliases_unchecked", detail = detail)
                                    .to_string()
                                    .into(),
                            );
                        }
                        open(Kind::Delete, state, connection, name, lines, window, cx);
                    })
                    .ok();
                })
                .detach();
        })
    });
}

fn client(connection: &Entity<ConnectionState>, cx: &App) -> Option<Client> {
    connection
        .read(cx)
        .connected
        .as_ref()
        .map(|c| c.client.clone())
}

fn open(
    kind: Kind,
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    name: String,
    consequences: Vec<SharedString>,
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
                    Err(e) => return Err(error_text(&e, &profile).into()),
                    Ok(()) => {}
                }
                connection.update(cx, |c, cx| match kind {
                    // Views of the collection reload and show 0 documents.
                    Kind::Truncate => c.collection_changed(name.clone(), cx),
                    Kind::Delete => c.invalidate_collections(cx),
                });
                if kind == Kind::Delete {
                    state.update(cx, |s, cx| s.close_collection(profile.id, name, cx));
                }
                Ok(())
            })
        },
        window,
        cx,
    );
}
