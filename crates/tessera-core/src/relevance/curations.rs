// SPDX-License-Identifier: GPL-3.0-or-later

//! A curation item as the form holds it.

use serde_json::{Map, Value, json};

use super::changes::ItemIssue;
use super::synonyms::{ModelError, strings, text};

/// When a curation fires. `extra` keeps unknown keys of the rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleDraft {
    /// Query text.
    pub query: String,
    /// `exact` or `contains`.
    pub r#match: String,
    /// Fires when the search filter equals this.
    pub filter_by: String,
    /// Fires only when `curation_tags` names one of these.
    pub tags: Vec<String>,
    /// 30.2 and later: the rule also matches synonyms of the query.
    pub synonyms: bool,
    /// 30.2 and later: the rule also matches the stem of the query.
    pub stem: bool,
    /// Keys without a field.
    pub extra: Map<String, Value>,
}

/// A pinned document. `extra` keeps keys without a field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Include {
    /// Document id.
    pub id: String,
    /// Its position.
    pub position: i64,
    /// Keys without a field.
    pub extra: Map<String, Value>,
}

/// A hidden document. `extra` keeps keys without a field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exclude {
    /// Document id.
    pub id: String,
    /// Keys without a field.
    pub extra: Map<String, Value>,
}

/// When a rule applies, against a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleTime {
    /// Its start lies ahead.
    Scheduled,
    /// It applies now.
    Active,
    /// Its end has passed.
    Expired,
}

/// A curation item. `extra` keeps every key the form has no control for, `diversity` included.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurationDraft {
    /// Item id.
    pub id: String,
    /// When it fires.
    pub rule: RuleDraft,
    /// Pinned documents and their positions.
    pub includes: Vec<Include>,
    /// Hidden documents.
    pub excludes: Vec<Exclude>,
    /// Filter added to the search.
    pub filter_by: String,
    /// Sort replacing the search's.
    pub sort_by: String,
    /// Query replacing the search's.
    pub replace_query: String,
    /// Drop the matched words from the query.
    pub remove_matched_tokens: bool,
    /// Apply the filter to the pinned documents too.
    pub filter_curated_hits: bool,
    /// Stop at this rule.
    pub stop_processing: bool,
    /// Start of validity, Unix seconds.
    pub effective_from_ts: Option<i64>,
    /// End of validity, Unix seconds.
    pub effective_to_ts: Option<i64>,
    /// Data returned with the results.
    pub metadata: Option<Map<String, Value>>,
    /// Keys without a field.
    pub extra: Map<String, Value>,
}

fn flag(map: &mut Map<String, Value>, key: &'static str) -> Result<Option<bool>, ModelError> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(_) => Err(ModelError::BadField(key)),
    }
}

fn stamp(map: &mut Map<String, Value>, key: &'static str) -> Result<Option<i64>, ModelError> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or(ModelError::BadField(key)),
    }
}

fn objects(
    map: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Vec<Map<String, Value>>, ModelError> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list
            .into_iter()
            .map(|v| match v {
                Value::Object(o) => Ok(o),
                _ => Err(ModelError::BadField(key)),
            })
            .collect(),
        Some(_) => Err(ModelError::BadField(key)),
    }
}

fn with_extra(mut value: Value, extra: &Map<String, Value>) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.extend(extra.clone());
    }
    value
}

impl RuleDraft {
    fn from_map(mut map: Map<String, Value>) -> Result<Self, ModelError> {
        Ok(Self {
            query: text(&mut map, "query")?,
            r#match: text(&mut map, "match")?,
            filter_by: text(&mut map, "filter_by")?,
            tags: strings(&mut map, "tags")?,
            synonyms: flag(&mut map, "synonyms")?.unwrap_or(false),
            stem: flag(&mut map, "stem")?.unwrap_or(false),
            extra: map,
        })
    }

    fn to_value(&self) -> Value {
        let mut map = Map::new();
        for (key, value) in [
            ("query", &self.query),
            ("match", &self.r#match),
            ("filter_by", &self.filter_by),
        ] {
            if !value.is_empty() {
                map.insert(key.into(), json!(value));
            }
        }
        if !self.tags.is_empty() {
            map.insert("tags".into(), json!(self.tags));
        }
        if self.synonyms {
            map.insert("synonyms".into(), json!(true));
        }
        if self.stem {
            map.insert("stem".into(), json!(true));
        }
        map.extend(self.extra.clone());
        Value::Object(map)
    }
}

impl CurationDraft {
    /// A new item: it stops processing, and removes matched tokens as the server does when
    /// there are no includes or excludes.
    pub fn new_default() -> Self {
        Self {
            remove_matched_tokens: true,
            stop_processing: true,
            ..Self::default()
        }
    }

    /// What the server makes of `remove_matched_tokens` when it is not sent: true without
    /// includes and excludes, false with them.
    pub fn default_remove_tokens(&self) -> bool {
        self.includes.is_empty() && self.excludes.is_empty()
    }

    /// Where the rule stands at `now`, Unix seconds.
    pub fn timing(&self, now: i64) -> RuleTime {
        if self.effective_from_ts.is_some_and(|from| from > now) {
            RuleTime::Scheduled
        } else if self.effective_to_ts.is_some_and(|to| to < now) {
            RuleTime::Expired
        } else {
            RuleTime::Active
        }
    }

    /// Reads a stored item. A key that is absent takes the server's default.
    pub fn from_value(value: &Value) -> Result<Self, ModelError> {
        let mut map = value.as_object().ok_or(ModelError::NotObject)?.clone();
        let id = text(&mut map, "id")?;
        let rule = match map.remove("rule") {
            None | Some(Value::Null) => RuleDraft::default(),
            Some(Value::Object(o)) => RuleDraft::from_map(o)?,
            Some(_) => return Err(ModelError::BadField("rule")),
        };
        let includes = objects(&mut map, "includes")?
            .into_iter()
            .map(|mut o| {
                Ok(Include {
                    id: text(&mut o, "id")?,
                    position: stamp(&mut o, "position")?.unwrap_or(0),
                    extra: o,
                })
            })
            .collect::<Result<Vec<_>, ModelError>>()?;
        let excludes = objects(&mut map, "excludes")?
            .into_iter()
            .map(|mut o| {
                Ok(Exclude {
                    id: text(&mut o, "id")?,
                    extra: o,
                })
            })
            .collect::<Result<Vec<_>, ModelError>>()?;
        let metadata = match map.remove("metadata") {
            None | Some(Value::Null) => None,
            Some(Value::Object(o)) => Some(o),
            Some(_) => return Err(ModelError::BadField("metadata")),
        };
        let mut draft = Self {
            id,
            rule,
            includes,
            excludes,
            filter_by: text(&mut map, "filter_by")?,
            sort_by: text(&mut map, "sort_by")?,
            replace_query: text(&mut map, "replace_query")?,
            filter_curated_hits: flag(&mut map, "filter_curated_hits")?.unwrap_or(false),
            stop_processing: flag(&mut map, "stop_processing")?.unwrap_or(true),
            effective_from_ts: stamp(&mut map, "effective_from_ts")?,
            effective_to_ts: stamp(&mut map, "effective_to_ts")?,
            metadata,
            remove_matched_tokens: false,
            extra: Map::new(),
        };
        draft.remove_matched_tokens = flag(&mut map, "remove_matched_tokens")?
            .unwrap_or_else(|| draft.default_remove_tokens());
        draft.extra = map;
        Ok(draft)
    }

    /// The item to send. The three flags the server defaults by content are always written.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".into(), json!(self.id));
        map.insert("rule".into(), self.rule.to_value());
        if !self.includes.is_empty() {
            let list: Vec<Value> = self
                .includes
                .iter()
                .map(|i| with_extra(json!({"id": i.id, "position": i.position}), &i.extra))
                .collect();
            map.insert("includes".into(), Value::Array(list));
        }
        if !self.excludes.is_empty() {
            let list: Vec<Value> = self
                .excludes
                .iter()
                .map(|e| with_extra(json!({"id": e.id}), &e.extra))
                .collect();
            map.insert("excludes".into(), Value::Array(list));
        }
        for (key, value) in [
            ("filter_by", &self.filter_by),
            ("sort_by", &self.sort_by),
            ("replace_query", &self.replace_query),
        ] {
            if !value.is_empty() {
                map.insert(key.into(), json!(value));
            }
        }
        map.insert(
            "remove_matched_tokens".into(),
            json!(self.remove_matched_tokens),
        );
        map.insert(
            "filter_curated_hits".into(),
            json!(self.filter_curated_hits),
        );
        map.insert("stop_processing".into(), json!(self.stop_processing));
        if let Some(from) = self.effective_from_ts {
            map.insert("effective_from_ts".into(), json!(from));
        }
        if let Some(to) = self.effective_to_ts {
            map.insert("effective_to_ts".into(), json!(to));
        }
        if let Some(metadata) = &self.metadata {
            map.insert("metadata".into(), Value::Object(metadata.clone()));
        }
        map.extend(self.extra.clone());
        Value::Object(map)
    }
}

/// Everything the server would store and then ignore. `sort_by` is checked separately, against
/// the attached collections, by `sort_warnings`.
pub fn check_curation(draft: &CurationDraft) -> Vec<ItemIssue> {
    let mut issues = Vec::new();
    if draft.id.is_empty() {
        issues.push(ItemIssue::NoId);
    }
    let rule = &draft.rule;
    if !rule.query.is_empty() && !matches!(rule.r#match.as_str(), "exact" | "contains") {
        issues.push(ItemIssue::BadMatch(rule.r#match.clone()));
    }
    let has_trigger = !rule.query.is_empty() || !rule.tags.is_empty() || !rule.filter_by.is_empty();
    if !has_trigger {
        issues.push(ItemIssue::NoTrigger);
    }
    let has_action = !draft.includes.is_empty()
        || !draft.excludes.is_empty()
        || draft.metadata.as_ref().is_some_and(|m| !m.is_empty())
        || !draft.filter_by.is_empty()
        || !draft.sort_by.is_empty()
        || !draft.replace_query.is_empty()
        || draft.remove_matched_tokens;
    if !has_action {
        issues.push(ItemIssue::NoAction);
    }
    for Include { id, position, .. } in &draft.includes {
        if id.is_empty() {
            issues.push(ItemIssue::BadInclude(id.clone()));
        }
        if *position < 1 {
            issues.push(ItemIssue::BadPosition(*position));
        }
        if !id.is_empty() && draft.excludes.iter().any(|e| &e.id == id) {
            issues.push(ItemIssue::IncludedAndExcluded(id.clone()));
        }
    }
    if let (Some(from), Some(to)) = (draft.effective_from_ts, draft.effective_to_ts)
        && from > to
    {
        issues.push(ItemIssue::DatesReversed);
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_compares_both_ends_with_now() {
        let at = |from, to| CurationDraft {
            effective_from_ts: from,
            effective_to_ts: to,
            ..CurationDraft::default()
        };
        assert_eq!(at(None, None).timing(10), RuleTime::Active);
        assert_eq!(at(Some(10), Some(10)).timing(10), RuleTime::Active);
        assert_eq!(at(Some(11), None).timing(10), RuleTime::Scheduled);
        assert_eq!(at(None, Some(9)).timing(10), RuleTime::Expired);
    }

    fn include(id: &str, position: i64) -> Include {
        Include {
            id: id.into(),
            position,
            ..Include::default()
        }
    }

    fn exclude(id: &str) -> Exclude {
        Exclude {
            id: id.into(),
            ..Exclude::default()
        }
    }

    /// A draft that passes every check.
    fn good() -> CurationDraft {
        CurationDraft {
            id: "c".into(),
            rule: RuleDraft {
                query: "shoes".into(),
                r#match: "exact".into(),
                ..RuleDraft::default()
            },
            includes: vec![include("1", 1)],
            stop_processing: true,
            ..CurationDraft::default()
        }
    }

    #[test]
    fn a_good_draft_has_no_issue() {
        assert!(check_curation(&good()).is_empty());
        assert_eq!(
            check_curation(&CurationDraft::new_default()),
            [ItemIssue::NoId, ItemIssue::NoTrigger]
        );
    }

    #[test]
    fn one_test_per_row_of_the_issue_table() {
        let with = |f: fn(&mut CurationDraft)| {
            let mut d = good();
            f(&mut d);
            check_curation(&d)
        };
        assert_eq!(with(|d| d.id.clear()), [ItemIssue::NoId]);
        assert_eq!(
            with(|d| d.rule.r#match = "wrong".into()),
            [ItemIssue::BadMatch("wrong".into())]
        );
        assert_eq!(
            with(|d| d.rule.r#match.clear()),
            [ItemIssue::BadMatch(String::new())]
        );
        assert_eq!(
            with(|d| d.rule = RuleDraft::default()),
            [ItemIssue::NoTrigger]
        );
        // Tags alone, or a filter alone, are triggers.
        assert!(
            with(|d| d.rule = RuleDraft {
                tags: vec!["t".into()],
                ..RuleDraft::default()
            })
            .is_empty()
        );
        assert!(
            with(|d| d.rule = RuleDraft {
                filter_by: "a:=1".into(),
                ..RuleDraft::default()
            })
            .is_empty()
        );
        // A match without a query is no trigger.
        assert_eq!(
            with(|d| d.rule = RuleDraft {
                r#match: "exact".into(),
                ..RuleDraft::default()
            }),
            [ItemIssue::NoTrigger]
        );
        assert_eq!(with(|d| d.includes.clear()), [ItemIssue::NoAction]);
        assert!(
            with(|d| {
                d.includes.clear();
                d.remove_matched_tokens = true;
            })
            .is_empty()
        );
        assert!(
            with(|d| {
                d.includes.clear();
                d.sort_by = "price:asc".into();
            })
            .is_empty()
        );
        assert_eq!(
            with(|d| d.includes = vec![include("", 1)]),
            [ItemIssue::BadInclude(String::new())]
        );
        assert_eq!(
            with(|d| d.includes = vec![include("1", 0)]),
            [ItemIssue::BadPosition(0)]
        );
        assert_eq!(
            with(|d| d.excludes = vec![exclude("1")]),
            [ItemIssue::IncludedAndExcluded("1".into())]
        );
        assert_eq!(
            with(|d| {
                d.effective_from_ts = Some(20);
                d.effective_to_ts = Some(10);
            }),
            [ItemIssue::DatesReversed]
        );
        assert!(
            with(|d| {
                d.effective_from_ts = Some(10);
                d.effective_to_ts = Some(10);
            })
            .is_empty()
        );
    }

    #[test]
    fn empty_metadata_is_no_action() {
        let mut d = good();
        d.includes.clear();
        d.metadata = Some(Map::new());
        assert_eq!(check_curation(&d), [ItemIssue::NoAction]);
        d.metadata = Some(json!({"k": 1}).as_object().unwrap().clone());
        assert!(check_curation(&d).is_empty());
    }

    #[test]
    fn a_new_draft_stops_processing_and_follows_the_servers_token_rule() {
        let mut d = CurationDraft::new_default();
        assert!(d.stop_processing);
        assert!(d.default_remove_tokens() && d.remove_matched_tokens);
        d.includes.push(include("1", 1));
        assert!(!d.default_remove_tokens());
        d.includes.clear();
        d.excludes.push(exclude("1"));
        assert!(!d.default_remove_tokens());
    }

    #[test]
    fn a_stored_30_2_rule_round_trips_with_diversity_and_an_unknown_key() {
        let stored = json!({
            "id": "r",
            "rule": {"query": "a", "match": "exact", "stem": true, "synonyms": false, "tessera_new": [1]},
            "includes": [{"id": "1", "position": 2}],
            "excludes": [{"id": "9"}],
            "remove_matched_tokens": false,
            "filter_curated_hits": false,
            "stop_processing": true,
            "effective_from_ts": 100,
            "metadata": {"k": "v"},
            "diversity": {"similarity_metric": [{"field": "title", "method": "equality", "weight": 1.0}]},
            "tessera_top": {"x": 1}
        });
        let draft = CurationDraft::from_value(&stored).unwrap();
        assert!(draft.rule.stem && !draft.rule.synonyms);
        assert_eq!(draft.rule.extra["tessera_new"], json!([1]));
        assert_eq!(draft.includes, [include("1", 2)]);
        assert_eq!(draft.extra["diversity"], stored["diversity"]);
        assert_eq!(draft.extra["tessera_top"], json!({"x": 1}));
        let back = draft.to_value();
        // `synonyms: false` is the default and is left out; everything else is the same.
        let mut expected = stored.clone();
        expected["rule"].as_object_mut().unwrap().remove("synonyms");
        assert_eq!(back, expected);
        assert_eq!(CurationDraft::from_value(&back).unwrap(), draft);
    }

    #[test]
    fn unknown_keys_of_includes_and_excludes_stay() {
        let stored = json!({
            "id": "r",
            "rule": {"tags": ["t"]},
            "includes": [{"id": "1", "position": 2, "tessera_new": 1}],
            "excludes": [{"id": "9", "tessera_new": 2}],
            "remove_matched_tokens": false,
            "filter_curated_hits": false,
            "stop_processing": true
        });
        assert_eq!(
            CurationDraft::from_value(&stored).unwrap().to_value(),
            stored
        );
    }

    #[test]
    fn to_value_writes_the_flags_always_and_the_rule_flags_only_when_true() {
        let value = good().to_value();
        assert_eq!(value["remove_matched_tokens"], json!(false));
        assert_eq!(value["filter_curated_hits"], json!(false));
        assert_eq!(value["stop_processing"], json!(true));
        assert!(value["rule"].get("synonyms").is_none());
        assert!(value["rule"].get("stem").is_none());
        assert!(value.get("excludes").is_none());
        assert!(value.get("sort_by").is_none());
        assert_eq!(value["includes"], json!([{"id": "1", "position": 1}]));
        let mut d = good();
        d.rule.stem = true;
        d.excludes = vec![exclude("2")];
        let value = d.to_value();
        assert_eq!(value["rule"]["stem"], json!(true));
        assert_eq!(value["excludes"], json!([{"id": "2"}]));
    }

    #[test]
    fn absent_flags_take_the_servers_defaults() {
        let bare = CurationDraft::from_value(&json!({"id": "a", "rule": {"tags": ["t"]}})).unwrap();
        assert!(bare.remove_matched_tokens && bare.stop_processing && !bare.filter_curated_hits);
        let pinned = CurationDraft::from_value(
            &json!({"id": "a", "rule": {"tags": ["t"]}, "includes": [{"id": "1", "position": 1}]}),
        )
        .unwrap();
        assert!(!pinned.remove_matched_tokens);
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        assert_eq!(
            CurationDraft::from_value(&json!(1)),
            Err(ModelError::NotObject)
        );
        assert_eq!(
            CurationDraft::from_value(&json!({"id": "a", "rule": 1})),
            Err(ModelError::BadField("rule"))
        );
        assert_eq!(
            CurationDraft::from_value(&json!({"id": "a", "includes": [1]})),
            Err(ModelError::BadField("includes"))
        );
        assert_eq!(
            CurationDraft::from_value(&json!({"id": "a", "effective_from_ts": "x"})),
            Err(ModelError::BadField("effective_from_ts"))
        );
    }
}
