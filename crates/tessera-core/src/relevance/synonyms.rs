// SPDX-License-Identifier: GPL-3.0-or-later

//! A synonym item as the form holds it.

use serde_json::{Map, Value, json};

use super::changes::ItemIssue;

/// An item that cannot be opened in the form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// The item is not a JSON object.
    #[error("the item is not an object")]
    NotObject,
    /// A key holds the wrong kind of value.
    #[error("`{0}` has the wrong type")]
    BadField(&'static str),
}

/// A synonym item. `extra` keeps every key the form has no control for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SynonymDraft {
    /// Item id.
    pub id: String,
    /// The root of a one-way synonym; empty for a multi-way one.
    pub root: String,
    /// The words.
    pub synonyms: Vec<String>,
    /// Locale, empty if unset.
    pub locale: String,
    /// Symbols to index.
    pub symbols_to_index: Vec<String>,
    /// Keys without a field.
    pub extra: Map<String, Value>,
}

pub(super) fn text(map: &mut Map<String, Value>, key: &'static str) -> Result<String, ModelError> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(ModelError::BadField(key)),
    }
}

pub(super) fn strings(
    map: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Vec<String>, ModelError> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list
            .into_iter()
            .map(|v| match v {
                Value::String(s) => Ok(s),
                _ => Err(ModelError::BadField(key)),
            })
            .collect(),
        Some(_) => Err(ModelError::BadField(key)),
    }
}

impl SynonymDraft {
    /// Reads a stored item. `root: ""`, which the server stores for a multi-way item, is no root.
    pub fn from_value(value: &Value) -> Result<Self, ModelError> {
        let mut map = value.as_object().ok_or(ModelError::NotObject)?.clone();
        Ok(Self {
            id: text(&mut map, "id")?,
            synonyms: strings(&mut map, "synonyms")?,
            root: text(&mut map, "root")?,
            locale: text(&mut map, "locale")?,
            symbols_to_index: strings(&mut map, "symbols_to_index")?,
            extra: map,
        })
    }

    /// The item to send: unset keys are left out.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".into(), json!(self.id));
        map.insert("synonyms".into(), json!(self.synonyms));
        if !self.root.trim().is_empty() {
            map.insert("root".into(), json!(self.root));
        }
        if !self.locale.is_empty() {
            map.insert("locale".into(), json!(self.locale));
        }
        if !self.symbols_to_index.is_empty() {
            map.insert("symbols_to_index".into(), json!(self.symbols_to_index));
        }
        map.extend(self.extra.clone());
        Value::Object(map)
    }
}

/// What stops this item from working. One synonym alone is accepted and does nothing.
pub fn check_synonym(draft: &SynonymDraft) -> Vec<ItemIssue> {
    let mut issues = Vec::new();
    if draft.id.is_empty() {
        issues.push(ItemIssue::NoId);
    }
    let words = draft
        .synonyms
        .iter()
        .filter(|w| !w.trim().is_empty())
        .count();
    if draft.root.trim().is_empty() {
        if words < 2 {
            issues.push(ItemIssue::TooFewSynonyms);
        }
    } else if words == 0 {
        issues.push(ItemIssue::NoSynonyms);
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(root: &str, words: &[&str]) -> SynonymDraft {
        SynonymDraft {
            id: "x".into(),
            root: root.into(),
            synonyms: words.iter().map(ToString::to_string).collect(),
            ..SynonymDraft::default()
        }
    }

    #[test]
    fn a_stored_multi_way_item_has_no_root() {
        let stored = json!({"id": "x", "root": "", "synonyms": ["a", "b"], "locale": "",
            "symbols_to_index": [], "tessera_future": {"k": 1}});
        let draft = SynonymDraft::from_value(&stored).unwrap();
        assert_eq!(draft.root, "");
        assert_eq!(draft.extra["tessera_future"], json!({"k": 1}));
        assert_eq!(
            draft.to_value(),
            json!({"id": "x", "synonyms": ["a", "b"], "tessera_future": {"k": 1}})
        );
    }

    #[test]
    fn a_blank_root_is_not_written() {
        assert!(draft("  ", &["a", "b"]).to_value().get("root").is_none());
    }

    #[test]
    fn root_locale_and_symbols_round_trip() {
        let item = json!({"id": "x", "synonyms": ["a"], "root": "r", "locale": "en",
            "symbols_to_index": ["+"]});
        let draft = SynonymDraft::from_value(&item).unwrap();
        assert_eq!(draft.to_value(), item);
    }

    #[test]
    fn a_wrong_type_is_an_error_not_a_loss() {
        assert_eq!(
            SynonymDraft::from_value(&json!([])),
            Err(ModelError::NotObject)
        );
        assert_eq!(
            SynonymDraft::from_value(&json!({"id": "x", "synonyms": "a"})),
            Err(ModelError::BadField("synonyms"))
        );
        assert_eq!(
            SynonymDraft::from_value(&json!({"id": 1})),
            Err(ModelError::BadField("id"))
        );
    }

    #[test]
    fn a_multi_way_item_needs_two_words() {
        assert_eq!(
            check_synonym(&draft("", &["a"])),
            [ItemIssue::TooFewSynonyms]
        );
        assert_eq!(check_synonym(&draft("", &[])), [ItemIssue::TooFewSynonyms]);
        assert_eq!(
            check_synonym(&draft("", &["a", " "])),
            [ItemIssue::TooFewSynonyms]
        );
        assert!(check_synonym(&draft("", &["a", "b"])).is_empty());
    }

    #[test]
    fn a_one_way_item_needs_one_word() {
        assert_eq!(check_synonym(&draft("r", &[])), [ItemIssue::NoSynonyms]);
        assert!(check_synonym(&draft("r", &["a"])).is_empty());
    }

    #[test]
    fn an_empty_id_is_named() {
        let mut d = draft("", &["a", "b"]);
        d.id.clear();
        assert_eq!(check_synonym(&d), [ItemIssue::NoId]);
    }
}
