// SPDX-License-Identifier: GPL-3.0-or-later

//! What is wrong with an item, and what a save changes.

use std::collections::HashMap;

use serde_json::Value;
use tessera_client::{Collection, SetKind};

use super::curations::CurationDraft;
use super::synonyms::SynonymDraft;

/// Whether an issue stops a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The server would store it without complaint, so Tessera refuses it.
    Error,
    /// Worth a look; the save goes ahead.
    Warning,
}

/// One thing wrong with an item. The app words it; core only names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemIssue {
    /// The id is empty or missing.
    NoId,
    /// Two rows (1-based) hold the same id; the server keeps the last one silently.
    DuplicateId {
        /// The id.
        id: String,
        /// The first and the later row.
        rows: (usize, usize),
    },
    /// Fewer than two synonyms and no root.
    TooFewSynonyms,
    /// A root and no synonym.
    NoSynonyms,
    /// `rule.match` is not `exact` or `contains`; holds the value.
    BadMatch(String),
    /// A rule with no query plus match, no tags and no `filter_by`.
    NoTrigger,
    /// No action at all.
    NoAction,
    /// An include without a document id; holds the id, empty when missing.
    BadInclude(String),
    /// An include whose position is below 1.
    BadPosition(i64),
    /// Two includes hold this position; the server answers 200 and curates only one of them.
    DuplicatePosition(i64),
    /// The same document is included and excluded; holds its id.
    IncludedAndExcluded(String),
    /// `effective_from_ts` is after `effective_to_ts`.
    DatesReversed,
    /// `sort_by` names a field an attached collection lacks.
    UnknownSortField {
        /// The field.
        field: String,
        /// The collection.
        collection: String,
    },
    /// An issue of the item in this row (1-based) of a file or paste.
    AtRow(usize, Box<ItemIssue>),
}

impl ItemIssue {
    /// Whether the issue stops a save.
    pub fn level(&self) -> Level {
        match self {
            Self::UnknownSortField { .. } => Level::Warning,
            Self::AtRow(_, issue) => issue.level(),
            _ => Level::Error,
        }
    }
}

/// Items without an id, and ids that occur twice, which the server reduces to the last one.
pub fn check_items(items: &[Value]) -> Vec<ItemIssue> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut issues = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let row = i + 1;
        match item.get("id").and_then(Value::as_str) {
            None | Some("") => issues.push(ItemIssue::NoId),
            Some(id) => {
                if let Some(first) = seen.insert(id, row) {
                    issues.push(ItemIssue::DuplicateId {
                        id: id.to_string(),
                        rows: (first, row),
                    });
                }
            }
        }
    }
    issues
}

/// What a whole-set `PUT` does, by item id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Ids that are new.
    pub added: Vec<String>,
    /// Ids whose item differs.
    pub changed: Vec<String>,
    /// Ids that are gone.
    pub removed: Vec<String>,
}

impl Changes {
    /// Whether the `PUT` changes nothing.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

fn id_of(item: &Value) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

/// `item` as the form would send it, or as it is if the form cannot read it. The server adds
/// `"root":""` and the flags that `to_value` leaves out or fills in.
fn normal(kind: SetKind, item: &Value) -> Value {
    let read = match kind {
        SetKind::Synonym => SynonymDraft::from_value(item).map(|d| d.to_value()),
        SetKind::Curation => CurationDraft::from_value(item).map(|d| d.to_value()),
    };
    read.unwrap_or_else(|_| item.clone())
}

/// Compares two lists by `id`. A different order alone is no change, and neither is a form the
/// server stores differently.
pub fn changes(kind: SetKind, old: &[Value], new: &[Value]) -> Changes {
    let old_by: HashMap<&str, &Value> = old.iter().filter_map(|v| Some((id_of(v)?, v))).collect();
    let new_by: HashMap<&str, &Value> = new.iter().filter_map(|v| Some((id_of(v)?, v))).collect();
    let mut out = Changes::default();
    for item in new {
        let Some(id) = id_of(item) else { continue };
        match old_by.get(id) {
            None => out.added.push(id.to_string()),
            Some(before) if normal(kind, before) != normal(kind, item) => {
                out.changed.push(id.to_string());
            }
            Some(_) => {}
        }
    }
    for item in old {
        if let Some(id) = id_of(item).filter(|id| !new_by.contains_key(id)) {
            out.removed.push(id.to_string());
        }
    }
    out
}

const NOT_FIELDS: [&str; 5] = [
    "_text_match",
    "_eval",
    "_vector_distance",
    "_seq_id",
    "_group_found",
];

/// The schema fields a `sort_by` names, without the built-in scores, `_eval(…)` and `$` joins.
pub fn sort_fields(sort_by: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in sort_by.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                terms.push(&sort_by[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    terms.push(&sort_by[start..]);
    let mut fields: Vec<String> = Vec::new();
    for term in terms {
        let term = term.trim();
        let name = term.split(['(', ':']).next().unwrap_or_default().trim();
        if name.is_empty()
            || term.starts_with('$')
            || NOT_FIELDS.contains(&name)
            || fields.iter().any(|f| f == name)
        {
            continue;
        }
        fields.push(name.to_string());
    }
    fields
}

/// A warning for each field of `sort_by` that an attached collection lacks. A rule like that
/// fails every search it matches.
pub fn sort_warnings(sort_by: &str, collections: &[Collection]) -> Vec<ItemIssue> {
    let fields = sort_fields(sort_by);
    let mut issues = Vec::new();
    for collection in collections {
        for field in &fields {
            // shortcut: a field pattern such as `.*` is not matched, only a plain name is
            let known = collection.fields.iter().any(|f| &f.name == field);
            if !known {
                issues.push(ItemIssue::UnknownSortField {
                    field: field.clone(),
                    collection: collection.name.clone(),
                });
            }
        }
    }
    issues
}

fn missing_keys(sent: &Value, kept: &Value) -> Vec<String> {
    let (Some(sent), Some(kept)) = (sent.as_object(), kept.as_object()) else {
        return Vec::new();
    };
    let lost = sent.keys().filter(|k| !kept.contains_key(*k));
    lost.cloned().collect()
}

/// Keys of `sent` that `stored` lacks, as dotted paths: one level into `rule` and `diversity`, and
/// into each row of `includes` and `excludes`, named by its id. The server drops unknown
/// attributes silently, and its answer to a `PUT` does not show it.
pub fn dropped_keys(sent: &Value, stored: &Value) -> Vec<String> {
    let (Some(sent), Some(stored)) = (sent.as_object(), stored.as_object()) else {
        return Vec::new();
    };
    let mut dropped = Vec::new();
    for (key, value) in sent {
        match (stored.get(key), key.as_str()) {
            (None, _) => dropped.push(key.clone()),
            (Some(kept), "rule" | "diversity") => {
                let lost = missing_keys(value, kept);
                dropped.extend(lost.into_iter().map(|k| format!("{key}.{k}")));
            }
            (Some(kept), "includes" | "excludes") => {
                let rows = value.as_array().map_or(&[][..], Vec::as_slice);
                let kept = kept.as_array().map_or(&[][..], Vec::as_slice);
                for (id, row) in rows.iter().filter_map(|row| Some((id_of(row)?, row))) {
                    match kept.iter().find(|k| id_of(k) == Some(id)) {
                        Some(kept) => {
                            let lost = missing_keys(row, kept);
                            dropped.extend(lost.into_iter().map(|k| format!("{key}.{id}.{k}")));
                        }
                        None => dropped.push(format!("{key}.{id}")),
                    }
                }
            }
            _ => {}
        }
    }
    dropped
}

/// [`dropped_keys`] of every sent item the server still holds, each as `id: key`.
pub fn dropped_by_id(sent: &[Value], stored: &[Value]) -> Vec<String> {
    let stored: HashMap<&str, &Value> =
        stored.iter().filter_map(|v| Some((id_of(v)?, v))).collect();
    let mut dropped = Vec::new();
    for item in sent {
        if let Some((id, kept)) = id_of(item).and_then(|id| Some((id, *stored.get(id)?))) {
            let keys = dropped_keys(item, kept);
            dropped.extend(keys.into_iter().map(|key| format!("{id}: {key}")));
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn items_without_an_id_or_with_a_repeated_one_are_named() {
        let items = [
            json!({"id": "a"}),
            json!({"synonyms": []}),
            json!({"id": ""}),
            json!({"id": "a"}),
            json!({"id": "b"}),
            json!({"id": "a"}),
        ];
        assert_eq!(
            check_items(&items),
            [
                ItemIssue::NoId,
                ItemIssue::NoId,
                ItemIssue::DuplicateId {
                    id: "a".into(),
                    rows: (1, 4)
                },
                ItemIssue::DuplicateId {
                    id: "a".into(),
                    rows: (4, 6)
                },
            ]
        );
        assert!(check_items(&[json!({"id": "a"}), json!({"id": "b"})]).is_empty());
    }

    #[test]
    fn only_a_sort_warning_lets_a_save_go_on() {
        let warning = ItemIssue::UnknownSortField {
            field: "f".into(),
            collection: "c".into(),
        };
        assert_eq!(warning.level(), Level::Warning);
        for error in [
            ItemIssue::NoId,
            ItemIssue::TooFewSynonyms,
            ItemIssue::NoSynonyms,
            ItemIssue::BadMatch("x".into()),
            ItemIssue::NoTrigger,
            ItemIssue::NoAction,
            ItemIssue::BadInclude(String::new()),
            ItemIssue::BadPosition(0),
            ItemIssue::IncludedAndExcluded("1".into()),
            ItemIssue::DatesReversed,
        ] {
            assert_eq!(error.level(), Level::Error, "{error:?}");
        }
    }

    #[test]
    fn changes_sorts_by_id_and_ignores_order() {
        let old = [
            json!({"id": "a", "synonyms": ["x", "y"]}),
            json!({"id": "b"}),
            json!({"id": "c"}),
        ];
        let new = [
            json!({"id": "c"}),
            json!({"id": "a", "synonyms": ["x", "z"]}),
            json!({"id": "d"}),
        ];
        let got = changes(SetKind::Synonym, &old, &new);
        assert_eq!(got.added, ["d"]);
        assert_eq!(got.changed, ["a"]);
        assert_eq!(got.removed, ["b"]);
        assert!(!got.is_empty());
    }

    #[test]
    fn a_reorder_alone_changes_nothing() {
        let old = [json!({"id": "a", "x": 1, "y": 2}), json!({"id": "b"})];
        let new = [json!({"y": 2, "id": "a", "x": 1}), json!({"id": "b"})];
        assert!(changes(SetKind::Synonym, &old, &new).is_empty());
        let swapped = [old[1].clone(), old[0].clone()];
        assert!(changes(SetKind::Synonym, &old, &swapped).is_empty());
    }

    #[test]
    fn a_stored_item_equals_its_export() {
        // As `GET /synonym_sets/x` answers on 30.2.
        let stored = [json!({"id": "m", "root": "", "synonyms": ["a", "b"]})];
        let exported = [json!({"id": "m", "synonyms": ["a", "b"]})];
        assert!(changes(SetKind::Synonym, &stored, &exported).is_empty());
        let edited = [json!({"id": "m", "root": "", "synonyms": ["a", "c"]})];
        assert_eq!(changes(SetKind::Synonym, &stored, &edited).changed, ["m"]);
    }

    #[test]
    fn a_stored_rule_equals_its_form() {
        let rule = json!({"query": "a", "match": "exact"});
        let stored = [
            json!({"id": "r", "rule": rule, "remove_matched_tokens": true,
            "filter_curated_hits": false, "stop_processing": true}),
        ];
        let sent = [json!({"id": "r", "rule": rule})];
        assert!(changes(SetKind::Curation, &stored, &sent).is_empty());
    }

    #[test]
    fn sort_fields_skips_what_is_no_schema_field() {
        assert_eq!(
            sort_fields("_text_match(buckets:10):desc, price:asc,_eval(x:1):desc"),
            ["price"]
        );
        assert_eq!(
            sort_fields("$a(b:asc), rating:desc, rating:asc"),
            ["rating"]
        );
        assert_eq!(
            sort_fields(
                "_eval([(a:1):3, (b:2):2]):desc,_vector_distance:asc,_seq_id:desc,_group_found:desc"
            ),
            Vec::<String>::new()
        );
        assert_eq!(sort_fields("loc(48.8, 2.3, precision: 2mi):asc"), ["loc"]);
        assert!(sort_fields("").is_empty());
    }

    fn collection(name: &str, fields: &[&str]) -> Collection {
        serde_json::from_value(json!({
            "name": name,
            "fields": fields.iter().map(|f| json!({"name": f, "type": "int32"})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn sort_warnings_name_each_collection_that_lacks_a_field() {
        let collections = [collection("a", &["price"]), collection("b", &["rating"])];
        assert_eq!(
            sort_warnings("price:asc,_text_match:desc", &collections),
            [ItemIssue::UnknownSortField {
                field: "price".into(),
                collection: "b".into()
            }]
        );
        assert!(sort_warnings("", &collections).is_empty());
    }

    #[test]
    fn dropped_by_id_names_the_item() {
        let sent = [
            json!({"id": "a", "lost": 1, "kept": 1}),
            json!({"id": "b", "kept": 1}),
            json!({"id": "gone", "lost": 1}),
            json!({"lost": 1}),
        ];
        let stored = [json!({"id": "b", "kept": 1}), json!({"id": "a", "kept": 1})];
        assert_eq!(dropped_by_id(&sent, &stored), ["a: lost"]);
    }

    #[test]
    fn dropped_keys_look_into_rows_and_diversity() {
        let sent = json!({
            "id": "a",
            "includes": [{"id": "1", "position": 1, "note": "x"}, {"id": "2", "position": 2}],
            "excludes": [{"id": "3", "note": "x"}],
            "diversity": {"similarity_metric": [], "future": 1},
        });
        let stored = json!({
            "id": "a",
            "includes": [{"id": "1", "position": 1}],
            "excludes": [{"id": "3"}],
            "diversity": {"similarity_metric": []},
        });
        assert_eq!(
            dropped_keys(&sent, &stored),
            [
                "includes.1.note",
                "includes.2",
                "excludes.3.note",
                "diversity.future"
            ]
        );
        assert!(dropped_keys(&sent, &sent).is_empty());
    }

    #[test]
    fn dropped_keys_look_one_level_into_the_rule() {
        let sent = json!({"id": "a", "typo": 1, "rule": {"query": "q", "stem": true, "x": 2}});
        let stored = json!({"id": "a", "rule": {"query": "q"}, "stop_processing": true});
        assert_eq!(
            dropped_keys(&sent, &stored),
            ["typo", "rule.stem", "rule.x"]
        );
        assert!(dropped_keys(&stored, &stored).is_empty());
        assert!(dropped_keys(&json!(1), &stored).is_empty());
    }
}
