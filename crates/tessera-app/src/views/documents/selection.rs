// SPDX-License-Identifier: GPL-3.0-or-later

//! Ticked rows of the documents table, and deleting them.

use std::collections::BTreeSet;

use gpui_kit::{App, Context, SharedString, Task, WeakEntity, Window};
use rust_i18n::t;
use tessera_client::{Client, Error};
use tessera_core::documents::export::{Danger, danger_level};
use tessera_core::documents::ids::id_batches;
use tessera_core::documents::{ID_FILTER_BYTES, MAX_SELECTION};
use tessera_widgets::confirm;
use tessera_widgets::confirm_danger::{self, DangerOptions};

use super::super::overview::grouped;
use super::DocumentsView;
use crate::connection::{error_text, long_error_text};
use crate::documents::{DocumentsBackend, DocumentsBackendGlobal};

/// The ticked rows: a set of ids, so it outlives the pages they were on. Whatever adds to it
/// refuses beyond [`MAX_SELECTION`] and returns true then.
#[derive(Debug, Default)]
pub struct Selection {
    ids: BTreeSet<String>,
    anchor: Option<usize>,
}

impl Selection {
    /// Ticks or unticks `id`, which is at `row`, and anchors ranges there.
    pub fn toggle(&mut self, id: &str, row: usize) -> bool {
        if !self.ids.remove(id) {
            if self.ids.len() >= MAX_SELECTION {
                return true;
            }
            self.ids.insert(id.to_string());
        }
        self.anchor = Some(row);
        false
    }

    /// Ticks the rows `ids_of` knows from the anchor to `row`, or only `row` with no anchor.
    pub fn extend(&mut self, row: usize, ids_of: impl Fn(usize) -> Option<String>) -> bool {
        let anchor = *self.anchor.get_or_insert(row);
        self.add_all((anchor.min(row)..=anchor.max(row)).filter_map(ids_of))
    }

    /// Ticks `ids`.
    pub fn add_all(&mut self, ids: impl IntoIterator<Item = String>) -> bool {
        for id in ids {
            if !self.ids.contains(&id) {
                if self.ids.len() >= MAX_SELECTION {
                    return true;
                }
                self.ids.insert(id);
            }
        }
        false
    }

    /// Whether `id` is ticked.
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// How many rows are ticked.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether no row is ticked.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Unticks everything.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// The ticked ids, sorted.
    pub fn ids(&self) -> Vec<String> {
        self.ids.iter().cloned().collect()
    }
}

/// ⌘⌫ / Ctrl+Backspace: asks, then deletes the ticked rows or the selected cell's row. A draft
/// of a row going away is dropped first, after asking; one of another row stays.
pub fn delete(view: &mut DocumentsView, window: &mut Window, cx: &mut Context<DocumentsView>) {
    let ids = view.delete_targets(cx);
    if ids.is_empty() || view.deleting {
        return;
    }
    let doomed = view.inspector.read(cx).current_id();
    if doomed.is_some_and(|id| ids.iter().any(|i| i == id)) {
        // Deferred: Enter closes the discard dialog after this, and takes the newest one.
        view.after_discard(window, cx, move |_, window, cx| {
            let ids = ids.clone();
            cx.defer_in(window, move |this, window, cx| ask(this, ids, window, cx));
        });
    } else {
        ask(view, ids, window, cx);
    }
}

/// Unlocks if needed, then opens the confirmation that fits `ids`.
fn ask(
    view: &mut DocumentsView,
    ids: Vec<String>,
    window: &mut Window,
    cx: &mut Context<DocumentsView>,
) {
    let this = cx.entity().downgrade();
    let name = view.name.clone();
    view.connection.clone().update(cx, |c, cx| {
        let prod = c.profile.environment.is_prod();
        c.ensure_writable(window, cx, move |window, cx| {
            confirm_delete(this, name, ids, prod, window, cx);
        });
    });
}

fn confirm_delete(
    view: WeakEntity<DocumentsView>,
    name: String,
    ids: Vec<String>,
    prod: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let count = ids.len() as u64;
    let (title, consequence) = match ids.as_slice() {
        [id] => (
            t!("documents.delete.title_one").to_string(),
            t!(
                "documents.delete.consequence_one",
                id = id,
                collection = name
            )
            .to_string(),
        ),
        _ => {
            let shown = grouped(count);
            (
                t!("documents.delete.title_many", count = shown).to_string(),
                t!(
                    "documents.delete.consequence_many",
                    count = shown,
                    collection = name
                )
                .to_string(),
            )
        }
    };
    let action = t!("documents.delete.action").to_string();
    let run = move |window: &mut Window, cx: &mut App| run(view.clone(), ids.clone(), window, cx);
    match danger_level(count, prod) {
        // The dialog is gone by then, so a failure shows in the view.
        Danger::Medium => confirm::open(
            title,
            consequence,
            action,
            move |window, cx| run(window, cx).detach(),
            window,
            cx,
        ),
        Danger::High => confirm_danger::open(
            DangerOptions {
                title: title.into(),
                consequences: vec![consequence.into()],
                expected: name.into(),
                action_label: action.into(),
                prod,
            },
            run,
            window,
            cx,
        ),
    }
}

/// Deletes `ids` outside the view's tasks: "Stop waiting" leaves the batches running.
fn run(
    view: WeakEntity<DocumentsView>,
    ids: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    let Some(view_ref) = view.upgrade() else {
        return Task::ready(Ok(()));
    };
    let (connection, name) = {
        let this = view_ref.read(cx);
        (this.connection.clone(), this.name.clone())
    };
    let Some(client) = connection.read(cx).connected.as_ref() else {
        let text = SharedString::from(t!("status.not_connected").to_string());
        view_ref.update(cx, |this, cx| {
            this.delete_error = Some(text.clone());
            cx.notify();
        });
        return Task::ready(Err(text));
    };
    let client = client.client.clone();
    view_ref.update(cx, |this, cx| {
        this.deleting = true;
        this.delete_error = None;
        cx.notify();
    });
    let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
    window.spawn(cx, async move |cx| {
        let (done, result) = delete_ids(&*backend, client, name.clone(), &ids).await;
        cx.update(|window, cx| {
            let profile = connection.read(cx).profile.clone();
            let error: Option<SharedString> = result.err().map(|e| {
                let detail = if ids.len() == 1 {
                    error_text(&e, &profile)
                } else {
                    long_error_text(&e, &profile)
                };
                let (done, count) = (grouped(done), grouped(ids.len() as u64));
                t!(
                    "documents.delete.partial",
                    done = done,
                    count = count,
                    error = detail
                )
                .to_string()
                .into()
            });
            view.update(cx, |this, cx| {
                this.delete_finished(&ids, error.clone(), window, cx);
            })
            .ok();
            // A failed batch may have deleted some, so this reloads either way.
            connection.update(cx, |c, cx| c.collection_changed(name, cx));
            error.map_or(Ok(()), Err)
        })
        .unwrap_or(Ok(()))
    })
}

/// Deletes one id through its endpoint, several in `id_batches`, in sequence. Stops at the
/// first failure: how many went, and that failure. Ids a filter cannot quote go one by one.
async fn delete_ids(
    backend: &dyn DocumentsBackend,
    client: Client,
    name: String,
    ids: &[String],
) -> (u64, Result<(), Error>) {
    let (filters, single) = if ids.len() == 1 {
        (Vec::new(), ids.to_vec())
    } else {
        let batches = id_batches(ids, ID_FILTER_BYTES);
        (batches.filters, batches.single)
    };
    let mut done = 0;
    for filter in filters {
        match backend
            .delete_by_filter(client.clone(), name.clone(), filter)
            .await
        {
            Ok(deleted) => done += deleted,
            Err(e) => return (done, Err(e)),
        }
    }
    for id in single {
        match backend.delete(client.clone(), name.clone(), id).await {
            // Already gone is what the user wanted.
            Ok(()) | Err(Error::Http { status: 404, .. }) => done += 1,
            Err(e) => return (done, Err(e)),
        }
    }
    (done, Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(row: usize) -> Option<String> {
        (row < 5).then(|| row.to_string())
    }

    #[test]
    fn toggle_ticks_and_unticks() {
        let mut s = Selection::default();
        assert!(!s.toggle("a", 0));
        assert!(s.contains("a"));
        assert!(!s.toggle("a", 0));
        assert!(s.is_empty());
    }

    #[test]
    fn extend_covers_known_rows_from_the_anchor_either_way() {
        let mut s = Selection::default();
        s.toggle("3", 3);
        s.extend(1, numbers);
        assert_eq!(s.ids(), ["1", "2", "3"]);
        s.extend(7, numbers);
        assert_eq!(s.ids(), ["1", "2", "3", "4"]);
    }

    #[test]
    fn extend_without_an_anchor_takes_the_row() {
        let mut s = Selection::default();
        s.extend(2, numbers);
        assert_eq!(s.ids(), ["2"]);
    }

    #[test]
    fn nothing_passes_the_cap_but_ticked_rows_can_go() {
        let mut s = Selection::default();
        assert!(s.add_all((0..MAX_SELECTION + 5).map(|n| n.to_string())));
        assert_eq!(s.len(), MAX_SELECTION);
        assert!(s.toggle("new", 0));
        assert!(!s.contains("new"));
        assert!(!s.add_all(["0".to_string()]), "already ticked");
        assert!(!s.toggle("0", 0));
        assert_eq!(s.len(), MAX_SELECTION - 1);
    }

    #[test]
    fn clear_drops_the_anchor() {
        let mut s = Selection::default();
        s.toggle("1", 1);
        s.clear();
        s.extend(3, numbers);
        assert_eq!(s.ids(), ["3"]);
    }
}
