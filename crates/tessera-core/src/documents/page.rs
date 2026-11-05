// SPDX-License-Identifier: GPL-3.0-or-later

//! Document pages with prebuilt cell previews, and the page cache.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::CELL_LIMIT;

/// Rows from a page boundary at which the neighbouring page is fetched too.
const LOOKAHEAD: usize = 50;

/// One document with its cell texts.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The document's `id`.
    pub id: Arc<str>,
    /// The full document.
    pub doc: Arc<Map<String, Value>>,
    /// Preview text per top-level key.
    pub cells: HashMap<String, Arc<str>>,
}

/// One fetched page.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentPage {
    /// Rows in server order.
    pub rows: Vec<Row>,
    /// Sorted top-level keys seen on this page.
    pub keys: Vec<String>,
}

impl DocumentPage {
    /// Builds rows and previews; meant for the tokio worker, not the render path.
    pub fn from_documents(docs: Vec<Map<String, Value>>) -> Self {
        let mut keys = BTreeSet::new();
        let rows = docs
            .into_iter()
            .map(|doc| {
                keys.extend(doc.keys().cloned());
                let cells = doc
                    .iter()
                    .map(|(k, v)| (k.clone(), cell_preview(v)))
                    .collect();
                Row {
                    id: doc.get("id").and_then(Value::as_str).unwrap_or("").into(),
                    doc: Arc::new(doc),
                    cells,
                }
            })
            .collect();
        Self {
            rows,
            keys: keys.into_iter().collect(),
        }
    }
}

/// One line of at most [`CELL_LIMIT`] characters.
pub fn cell_preview(v: &Value) -> Arc<str> {
    let text = match v {
        Value::String(s) => s.replace(['\n', '\r', '\t'], " "),
        other => other.to_string(),
    };
    match text.char_indices().nth(CELL_LIMIT) {
        Some((end, _)) => format!("{}…", &text[..end]).into(),
        None => text.into(),
    }
}

/// Strings raw, everything else compact JSON.
pub fn cell_full(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Pages by number; the least recently touched is evicted first.
pub struct PageCache<T> {
    capacity: usize,
    /// Oldest first.
    entries: VecDeque<(u32, T)>,
}

impl<T> PageCache<T> {
    /// An empty cache holding at most `capacity` pages.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    /// The page, without touching it: this runs per cell.
    pub fn get(&self, page: u32) -> Option<&T> {
        self.entries
            .iter()
            .find(|(p, _)| *p == page)
            .map(|(_, v)| v)
    }

    /// Marks the page as most recently used.
    pub fn touch(&mut self, page: u32) {
        if let Some(i) = self.entries.iter().position(|(p, _)| *p == page)
            && let Some(e) = self.entries.remove(i)
        {
            self.entries.push_back(e);
        }
    }

    /// Stores the page (replacing an old one) and evicts down to capacity.
    pub fn insert(&mut self, page: u32, value: T) {
        self.entries.retain(|(p, _)| *p != page);
        self.entries.push_back((page, value));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// Drops every page.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Pages held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no page is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 1-based page and offset of a 0-based row.
pub fn row_position(row: usize, per_page: usize) -> (u32, usize) {
    (
        u32::try_from(row / per_page + 1).unwrap_or(u32::MAX),
        row % per_page,
    )
}

/// Pages under `visible`, plus the neighbour when within 50 rows of a boundary, up to `found`.
pub fn wanted_pages(visible: Range<usize>, per_page: usize, found: u64) -> Vec<u32> {
    if visible.is_empty() {
        return Vec::new();
    }
    let last = u32::try_from(found.div_ceil(per_page as u64)).unwrap_or(u32::MAX);
    let (mut first, start_off) = row_position(visible.start, per_page);
    let (mut end, end_off) = row_position(visible.end - 1, per_page);
    if start_off < LOOKAHEAD && first > 1 {
        first -= 1;
    }
    if per_page - end_off <= LOOKAHEAD {
        end += 1;
    }
    (first..=end.min(last)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cell_preview_is_one_line_and_short() {
        assert_eq!(&*cell_preview(&json!("a\nb\r\tc")), "a b  c");
        let long = cell_preview(&json!("x".repeat(300)));
        assert_eq!(long.chars().count(), CELL_LIMIT + 1);
        assert!(long.ends_with('…'));
        assert_eq!(cell_preview(&json!("é".repeat(200))).chars().count(), 200);
    }

    #[test]
    fn cell_preview_of_object_is_compact_json() {
        assert_eq!(&*cell_preview(&json!({"a": [1, 2]})), r#"{"a":[1,2]}"#);
    }

    #[test]
    fn cell_preview_null_is_empty_marker() {
        assert_eq!(&*cell_preview(&Value::Null), "null");
    }

    #[test]
    fn cell_full_keeps_strings_raw() {
        assert_eq!(cell_full(&json!("a\nb")), "a\nb");
        assert_eq!(cell_full(&json!([1])), "[1]");
    }

    #[test]
    fn from_documents_builds_ids_cells_and_sorted_keys() {
        let docs = vec![
            json!({"id": "1", "title": "a", "n": 3}),
            json!({"title": "b", "extra": true}),
        ]
        .into_iter()
        .map(|v| v.as_object().cloned().expect("object"))
        .collect();
        let page = DocumentPage::from_documents(docs);
        assert_eq!(page.keys, ["extra", "id", "n", "title"]);
        assert_eq!(&*page.rows[0].id, "1");
        assert_eq!(&*page.rows[0].cells["n"], "3");
        assert_eq!(&*page.rows[1].id, "");
        assert_eq!(&*page.rows[1].cells["title"], "b");
    }

    #[test]
    fn page_cache_evicts_least_recently_touched() {
        let mut c = PageCache::new(3);
        for p in 1..=3 {
            c.insert(p, p);
        }
        c.touch(1);
        c.insert(4, 4);
        assert!(c.get(2).is_none());
        assert!(c.get(1).is_some() && c.get(3).is_some() && c.get(4).is_some());
    }

    #[test]
    fn page_cache_never_exceeds_capacity() {
        let mut c = PageCache::new(20);
        for p in 0..100 {
            c.insert(p, p);
            assert!(c.len() <= 20);
        }
        assert_eq!(c.len(), 20);
    }

    #[test]
    fn insert_of_existing_page_replaces_it() {
        let mut c = PageCache::new(3);
        c.insert(1, "old");
        c.insert(1, "new");
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(1), Some(&"new"));
    }

    #[test]
    fn row_position_splits_pages() {
        assert_eq!(row_position(0, 250), (1, 0));
        assert_eq!(row_position(249, 250), (1, 249));
        assert_eq!(row_position(250, 250), (2, 0));
    }

    #[test]
    fn wanted_pages_covers_visible() {
        assert_eq!(wanted_pages(100..130, 250, 10_000), [1]);
    }

    #[test]
    fn wanted_pages_adds_neighbour_near_boundary() {
        assert_eq!(wanted_pages(230..260, 250, 10_000), [1, 2]);
        assert_eq!(wanted_pages(220..240, 250, 10_000), [1, 2]);
        assert_eq!(wanted_pages(260..290, 250, 10_000), [1, 2]);
    }

    #[test]
    fn wanted_pages_stops_at_found() {
        assert_eq!(wanted_pages(220..240, 250, 250), [1]);
        assert_eq!(wanted_pages(0..10, 250, 0), Vec::<u32>::new());
    }
}
