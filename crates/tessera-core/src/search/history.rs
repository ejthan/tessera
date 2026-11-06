// SPDX-License-Identifier: GPL-3.0-or-later

//! The history of searches and raw requests, kept in `history.jsonl`.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::redact;
use crate::search::params::SearchDraft;
use crate::store::{self, StoreError};

/// Entries kept per kind.
pub const HISTORY_CAP: usize = 500;
/// A larger raw body is not kept.
pub const RAW_BODY_KEEP: usize = 64 * 1024;

/// One entry with its time and connection.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistoryItem {
    /// Seconds since the Unix epoch.
    pub at: i64,
    /// The connection it ran on.
    pub profile: Uuid,
    /// What ran.
    pub entry: HistoryEntry,
}

/// What ran.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A search of the workbench.
    Search {
        /// The searches ran as one merged list.
        union: bool,
        /// The drafts.
        searches: Vec<SearchDraft>,
    },
    /// A request of the console. Secrets are masked: see [`raw_entry`].
    Raw {
        /// HTTP method.
        method: String,
        /// Path below the node.
        path: String,
        /// The query string.
        query: Option<String>,
        /// The body, unless it was too large.
        body: Option<String>,
        /// The body was over [`RAW_BODY_KEEP`].
        body_dropped: bool,
    },
}

/// The entries in memory, newest last.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    searches: VecDeque<HistoryItem>,
    raw: VecDeque<HistoryItem>,
}

fn lower_contains(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(needle)
}

/// Whether `item` matches `needle`, which is in lower case: `q`, `filter_by` or the collection
/// of a search, the method, path or query of a raw request.
fn matches(item: &HistoryItem, needle: &str) -> bool {
    match &item.entry {
        HistoryEntry::Search { searches, .. } => searches.iter().any(|draft| {
            let text = |key| draft.params.get(key).and_then(|v| v.as_str()).unwrap_or("");
            [text("q"), text("filter_by"), &draft.collection]
                .iter()
                .any(|t| lower_contains(t, needle))
        }),
        HistoryEntry::Raw {
            method,
            path,
            query,
            ..
        } => [method, path, query.as_deref().unwrap_or("")]
            .iter()
            .any(|t| lower_contains(t, needle)),
    }
}

fn select<'a>(
    items: &'a VecDeque<HistoryItem>,
    profile: Uuid,
    needle: &str,
) -> Vec<&'a HistoryItem> {
    let needle = needle.to_lowercase();
    let mine = items.iter().rev().filter(|i| i.profile == profile);
    mine.filter(|i| matches(i, &needle)).collect()
}

impl History {
    /// Adds `item`, dropping the oldest of its kind above [`HISTORY_CAP`]. `false`: it equals
    /// the newest of its kind, whatever the time, and nothing changed.
    pub fn push(&mut self, item: HistoryItem) -> bool {
        let list = match item.entry {
            HistoryEntry::Search { .. } => &mut self.searches,
            HistoryEntry::Raw { .. } => &mut self.raw,
        };
        let same =
            |newest: &HistoryItem| newest.profile == item.profile && newest.entry == item.entry;
        if list.back().is_some_and(same) {
            return false;
        }
        list.push_back(item);
        if list.len() > HISTORY_CAP {
            list.pop_front();
        }
        true
    }

    /// The searches of `profile` matching `needle`, newest first.
    pub fn searches(&self, profile: Uuid, needle: &str) -> Vec<&HistoryItem> {
        select(&self.searches, profile, needle)
    }

    /// The raw requests of `profile` matching `needle`, newest first.
    pub fn raw(&self, profile: Uuid, needle: &str) -> Vec<&HistoryItem> {
        select(&self.raw, profile, needle)
    }

    /// Forgets everything of `profile`.
    pub fn clear(&mut self, profile: Uuid) {
        self.searches.retain(|i| i.profile != profile);
        self.raw.retain(|i| i.profile != profile);
    }
}

/// `history.jsonl` in the platform data directory.
pub fn history_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .data_dir()
            .join("history.jsonl"),
    )
}

/// Loads the file and counts its lines. A line that does not parse is skipped, so a future
/// format does not lose the file; a file that cannot be read is an empty history.
pub fn load(path: &Path) -> (History, usize) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Default::default(),
        Err(e) => {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            return Default::default();
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut history = History::default();
    let mut lines = 0;
    for line in text.lines() {
        lines += 1;
        match serde_json::from_str(line) {
            Ok(item) => {
                history.push(item);
            }
            // A parse error can quote the line, which holds query text.
            Err(_) => tracing::warn!("skipping line {lines} of {}", path.display()),
        }
    }
    (history, lines)
}

fn to_line(item: &HistoryItem) -> Result<String, StoreError> {
    serde_json::to_string(item).map_err(|e| StoreError::Parse {
        path: PathBuf::new(),
        message: e.to_string(),
    })
}

/// Adds `item` at the end of the file.
pub fn append(path: &Path, item: &HistoryItem) -> Result<(), StoreError> {
    let line = to_line(item)? + "\n";
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Replaces the file with `history`, whole or not at all.
pub fn rewrite(path: &Path, history: &History) -> Result<(), StoreError> {
    let mut text = String::new();
    for item in history.searches.iter().chain(&history.raw) {
        text.push_str(&to_line(item)?);
        text.push('\n');
    }
    store::write_atomic(path, text.as_bytes())
}

/// A console request as history keeps it: the query and body masked, a body over
/// [`RAW_BODY_KEEP`] dropped.
pub fn raw_entry(
    method: &str,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> HistoryEntry {
    let kept = body.filter(|b| b.len() <= RAW_BODY_KEEP);
    HistoryEntry::Raw {
        method: method.to_string(),
        path: path.to_string(),
        query: query.map(redact::query),
        body: kept.map(redact::json),
        body_dropped: body.is_some() && kept.is_none(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    fn search(profile: Uuid, q: &str, filter: &str, collection: &str) -> HistoryItem {
        let mut draft = SearchDraft::new(collection);
        draft.params.insert("q".into(), q.into());
        draft.params.insert("filter_by".into(), filter.into());
        HistoryItem {
            at: 1,
            profile,
            entry: HistoryEntry::Search {
                union: false,
                searches: vec![draft],
            },
        }
    }

    fn raw(profile: Uuid, path: &str) -> HistoryItem {
        HistoryItem {
            at: 1,
            profile,
            entry: raw_entry("GET", path, None, None),
        }
    }

    #[test]
    fn push_drops_an_equal_neighbour() {
        let id = Uuid::new_v4();
        let mut h = History::default();
        assert!(h.push(search(id, "a", "", "books")));
        let mut later = search(id, "a", "", "books");
        later.at = 2;
        assert!(!h.push(later), "the time does not make it new");
        assert!(h.push(search(id, "b", "", "books")));
        assert!(h.push(search(id, "a", "", "books")), "not a neighbour");
        assert!(h.push(search(Uuid::new_v4(), "a", "", "books")));
        assert!(
            h.push(raw(id, "health")),
            "the other kind has its own newest"
        );
        assert_eq!(h.searches(id, "").len(), 3);
    }

    #[test]
    fn cap_holds_500_per_kind() {
        let id = Uuid::new_v4();
        let mut h = History::default();
        for n in 0..=HISTORY_CAP {
            h.push(search(id, &n.to_string(), "", "books"));
            h.push(raw(id, &n.to_string()));
        }
        assert_eq!(h.searches(id, "").len(), HISTORY_CAP);
        assert_eq!(h.raw(id, "").len(), HISTORY_CAP);
        assert_eq!(h.searches(id, "")[0], &search(id, "500", "", "books"));
        assert!(
            h.searches
                .iter()
                .all(|i| *i != search(id, "0", "", "books"))
        );
    }

    #[test]
    fn searches_filters_by_profile_and_needle() {
        let (id, other) = (Uuid::new_v4(), Uuid::new_v4());
        let mut h = History::default();
        h.push(search(id, "Dune", "", "books"));
        h.push(search(id, "x", "year:>1990", "films"));
        h.push(search(other, "dune", "", "books"));
        let q = |h: &History, n: &str| h.searches(id, n).len();
        assert_eq!(q(&h, ""), 2);
        assert_eq!(q(&h, "dUNE"), 1);
        assert_eq!(q(&h, "year"), 1);
        assert_eq!(q(&h, "films"), 1);
        assert_eq!(q(&h, "nope"), 0);
        assert_eq!(
            h.searches(id, "")[0],
            &search(id, "x", "year:>1990", "films")
        );
        h.clear(id);
        assert_eq!((q(&h, ""), h.searches(other, "").len()), (0, 1));
    }

    #[test]
    fn raw_entry_masks_query_and_body() {
        let entry = raw_entry(
            "POST",
            "keys",
            Some("a=1&x-typesense-api-key=topsecret"),
            Some(r#"{"api_key":"topsecret"}"#),
        );
        let item = HistoryItem {
            at: 1,
            profile: Uuid::new_v4(),
            entry,
        };
        let line = to_line(&item).unwrap();
        assert!(!line.contains("topsecret"), "{line}");
        assert!(line.contains("a=1"));
    }

    #[test]
    fn large_raw_body_is_dropped_and_marked() {
        let body = "x".repeat(RAW_BODY_KEEP + 1);
        let entry = raw_entry("POST", "p", None, Some(&body));
        let HistoryEntry::Raw {
            body, body_dropped, ..
        } = entry
        else {
            panic!("a raw entry");
        };
        assert_eq!((body, body_dropped), (None, true));
        let kept = "x".repeat(RAW_BODY_KEEP);
        let entry = raw_entry("POST", "p", None, Some(&kept));
        assert!(matches!(
            entry,
            HistoryEntry::Raw {
                body: Some(_),
                body_dropped: false,
                ..
            }
        ));
    }

    fn file(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("sub").join("history.jsonl")
    }

    #[test]
    fn append_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        let id = Uuid::new_v4();
        append(&path, &search(id, "a", "", "books")).unwrap();
        append(&path, &raw(id, "health")).unwrap();
        let (h, lines) = load(&path);
        assert_eq!(lines, 2);
        assert_eq!(h.searches(id, ""), [&search(id, "a", "", "books")]);
        assert_eq!(h.raw(id, ""), [&raw(id, "health")]);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&file(&dir)), (History::default(), 0));
    }

    #[test]
    fn bad_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let id = Uuid::new_v4();
        let good = to_line(&search(id, "a", "", "books")).unwrap();
        std::fs::write(&path, format!("{{not json\n{good}\n")).unwrap();
        let (h, lines) = load(&path);
        assert_eq!((h.searches(id, "").len(), lines), (1, 2));
    }

    #[test]
    fn unknown_entry_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let id = Uuid::new_v4();
        let future = json!({"at": 1, "profile": id, "entry": {"kind": "chart", "x": 1}});
        let good = to_line(&raw(id, "health")).unwrap();
        std::fs::write(&path, format!("{future}\n{good}\n")).unwrap();
        let (h, lines) = load(&path);
        assert_eq!((h.raw(id, "").len(), lines), (1, 2));
    }

    #[test]
    fn rewrite_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let id = Uuid::new_v4();
        let mut h = History::default();
        h.push(search(id, "a", "", "books"));
        h.push(raw(id, "health"));
        rewrite(&path, &h).unwrap();
        assert_eq!(load(&path), (h.clone(), 2));
        let before = std::fs::read(&path).unwrap();
        // A directory where the temporary file goes makes the write fail.
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        h.clear(id);
        assert!(rewrite(&path, &h).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
