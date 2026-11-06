// SPDX-License-Identifier: GPL-3.0-or-later

//! Analytics rules: what the form edits, what it sends and what stops it. Words nothing, the
//! app translates the enums.

use serde_json::{Map, Value, json};

use crate::relevance::name_ok;

/// The kinds of rule Tessera knows. Another kind stays listed and deletable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleType {
    /// Counts searches into a destination collection.
    #[default]
    PopularQueries,
    /// Counts searches without hits into a destination collection.
    NohitsQueries,
    /// Adds a weight to a counter field of a document.
    Counter,
    /// Stores the events.
    Log,
}

impl RuleType {
    /// The name the server uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PopularQueries => "popular_queries",
            Self::NohitsQueries => "nohits_queries",
            Self::Counter => "counter",
            Self::Log => "log",
        }
    }

    /// The type with this server name.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        [
            Self::PopularQueries,
            Self::NohitsQueries,
            Self::Counter,
            Self::Log,
        ]
        .into_iter()
        .find(|t| t.as_str() == s)
    }

    fn is_query(self) -> bool {
        matches!(self, Self::PopularQueries | Self::NohitsQueries)
    }
}

/// The event types a rule of this type takes.
pub fn event_types(t: RuleType) -> &'static [&'static str] {
    match t {
        RuleType::PopularQueries | RuleType::NohitsQueries => &["search"],
        RuleType::Counter => &["click", "conversion", "visit"],
        RuleType::Log => &["click", "conversion", "visit", "search"],
    }
}

/// A rule as the form holds it. Whatever the form has no control for stays in `extra` (top
/// level) and `extra_params` (inside `params`), and goes back unchanged.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleDraft {
    /// Rule name.
    pub name: String,
    /// Rule type.
    pub rule_type: RuleType,
    /// Source collection.
    pub collection: String,
    /// Event type the rule listens to.
    pub event_type: String,
    /// Optional tag.
    pub rule_tag: String,
    /// Destination collection (query and counter rules).
    pub destination: String,
    /// Most queries kept (query rules).
    pub limit: Option<u32>,
    /// Field that counts (counter rules).
    pub counter_field: String,
    /// Weight added per event (counter rules).
    pub weight: Option<i32>,
    /// Whether searches feed the rule (query rules).
    pub capture_search_requests: bool,
    /// Whether the expanded query is kept (query rules).
    pub expand_query: bool,
    /// Search parameters kept with a query.
    pub meta_fields: Vec<String>,
    /// Top-level keys the form does not know.
    pub extra: Map<String, Value>,
    /// Keys of `params` the form does not know.
    pub extra_params: Map<String, Value>,
}

impl RuleDraft {
    /// Reads a rule the server returned, or `None` for a type this version does not know.
    pub fn from_rule(rule: &Map<String, Value>) -> Option<Self> {
        let mut extra = rule.clone();
        let text = |m: &mut Map<String, Value>, key: &str| match m.remove(key) {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        let name = text(&mut extra, "name");
        let rule_type = RuleType::from_str(&text(&mut extra, "type"))?;
        let collection = text(&mut extra, "collection");
        let event_type = text(&mut extra, "event_type");
        let rule_tag = text(&mut extra, "rule_tag");
        let mut p = match extra.remove("params") {
            Some(Value::Object(p)) => p,
            _ => Map::new(),
        };
        let destination = text(&mut p, "destination_collection");
        // What this type's form does not show stays in `extra_params`, and goes back.
        let (query, counter) = (rule_type.is_query(), rule_type == RuleType::Counter);
        let take = |p: &mut Map<String, Value>, key: &str, shown: bool| {
            if shown { p.remove(key) } else { None }
        };
        let counter_field = match take(&mut p, "counter_field", counter) {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        let limit = take(&mut p, "limit", query)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok());
        let weight = take(&mut p, "weight", counter)
            .and_then(|v| v.as_i64())
            .and_then(|n| i32::try_from(n).ok());
        let capture_search_requests = take(&mut p, "capture_search_requests", query)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let expand_query = take(&mut p, "expand_query", query)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let meta_fields = match take(&mut p, "meta_fields", query) {
            Some(Value::Array(a)) => a
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        Some(Self {
            name,
            rule_type,
            collection,
            event_type,
            rule_tag,
            destination,
            limit,
            counter_field,
            weight,
            capture_search_requests,
            expand_query,
            meta_fields,
            extra,
            extra_params: p,
        })
    }

    /// The body of `POST /analytics/rules`. Empty optionals are left out.
    pub fn to_body(&self) -> Value {
        let mut body = self.extra.clone();
        body.insert("name".into(), self.name.clone().into());
        body.insert("type".into(), self.rule_type.as_str().into());
        body.insert("collection".into(), self.collection.clone().into());
        body.insert("event_type".into(), self.event_type.clone().into());
        if !self.rule_tag.is_empty() {
            body.insert("rule_tag".into(), self.rule_tag.clone().into());
        }
        let mut params = self.extra_params.clone();
        match self.rule_type {
            RuleType::PopularQueries | RuleType::NohitsQueries => {
                params.insert(
                    "destination_collection".into(),
                    self.destination.clone().into(),
                );
                if let Some(limit) = self.limit {
                    params.insert("limit".into(), limit.into());
                }
                params.insert(
                    "capture_search_requests".into(),
                    self.capture_search_requests.into(),
                );
                params.insert("expand_query".into(), self.expand_query.into());
                if !self.meta_fields.is_empty() {
                    params.insert("meta_fields".into(), json!(self.meta_fields));
                }
            }
            RuleType::Counter => {
                params.insert(
                    "destination_collection".into(),
                    self.destination.clone().into(),
                );
                params.insert("counter_field".into(), self.counter_field.clone().into());
                if let Some(weight) = self.weight {
                    params.insert("weight".into(), weight.into());
                }
            }
            RuleType::Log => {}
        }
        if !params.is_empty() {
            body.insert("params".into(), Value::Object(params));
        }
        Value::Object(body)
    }

    /// The body of `PUT /analytics/rules/{name}`: the name is in the path and the type cannot
    /// change. The `PUT` merges, so a cleared tag or field list goes out empty. A limit or
    /// weight has no such form: the server answers 400 to `null`, and [`check_rule`] asks for
    /// both.
    pub fn update_body(&self) -> Value {
        let mut body = self.to_body();
        if let Some(o) = body.as_object_mut() {
            o.remove("name");
            o.remove("type");
            o.insert("rule_tag".into(), self.rule_tag.clone().into());
            if self.rule_type.is_query()
                && let Some(params) = o.get_mut("params").and_then(Value::as_object_mut)
            {
                params.insert("meta_fields".into(), json!(self.meta_fields));
            }
        }
        body
    }
}

/// What stops a rule from being sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleProblem {
    /// The name is not one Tessera creates.
    BadName,
    /// No source collection.
    NoCollection,
    /// A query or counter rule without a destination.
    NoDestination,
    /// A query rule without a limit.
    BadLimit,
    /// A counter rule without a counter field.
    NoCounterField,
    /// A counter rule without a weight.
    BadWeight,
}

/// Everything wrong with a draft.
pub fn check_rule(d: &RuleDraft) -> Vec<RuleProblem> {
    let mut problems = Vec::new();
    if !name_ok(&d.name) {
        problems.push(RuleProblem::BadName);
    }
    if d.collection.is_empty() {
        problems.push(RuleProblem::NoCollection);
    }
    let counter = d.rule_type == RuleType::Counter;
    if (d.rule_type.is_query() || counter) && d.destination.is_empty() {
        problems.push(RuleProblem::NoDestination);
    }
    if d.rule_type.is_query() && d.limit.is_none_or(|l| l == 0) {
        problems.push(RuleProblem::BadLimit);
    }
    if counter && d.counter_field.is_empty() {
        problems.push(RuleProblem::NoCounterField);
    }
    if counter && d.weight.is_none() {
        problems.push(RuleProblem::BadWeight);
    }
    problems
}

/// An event that feeds `rule`: a `q` for a search, a `doc_id` otherwise. It carries the rule's
/// own event type, because the server counts any other one anyway.
pub fn test_event(rule: &RuleDraft, user_id: &str, target: &str) -> Value {
    let key = if rule.event_type == "search" {
        "q"
    } else {
        "doc_id"
    };
    json!({
        "name": rule.name,
        "event_type": rule.event_type,
        "data": { "user_id": user_id, key: target },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("object")
    }

    #[test]
    fn popular_queries_round_trip_keeps_extra() {
        let src = json!({
            "name": "q", "type": "popular_queries", "collection": "products",
            "event_type": "search", "rule_tag": "web", "future": {"a": 1},
            "params": {
                "destination_collection": "popular", "limit": 100,
                "capture_search_requests": true, "expand_query": false,
                "meta_fields": ["filter_by"], "later": 7
            }
        });
        let draft = RuleDraft::from_rule(&rule(src.clone())).expect("known");
        assert_eq!(draft.rule_type, RuleType::PopularQueries);
        assert_eq!(draft.limit, Some(100));
        assert_eq!(draft.extra["future"], json!({"a": 1}));
        assert_eq!(draft.extra_params["later"], 7);
        assert_eq!(draft.to_body(), src);
    }

    #[test]
    fn nohits_round_trip() {
        let src = json!({
            "name": "n", "type": "nohits_queries", "collection": "products",
            "event_type": "search",
            "params": {
                "destination_collection": "nohits", "limit": 10,
                "capture_search_requests": true, "expand_query": true
            }
        });
        let draft = RuleDraft::from_rule(&rule(src.clone())).expect("known");
        assert_eq!(draft.to_body(), src);
    }

    #[test]
    fn counter_round_trip() {
        let src = json!({
            "name": "c", "type": "counter", "collection": "products", "event_type": "click",
            "params": {"destination_collection": "products", "counter_field": "popularity", "weight": 3}
        });
        let draft = RuleDraft::from_rule(&rule(src.clone())).expect("known");
        assert_eq!(
            (draft.counter_field.as_str(), draft.weight),
            ("popularity", Some(3))
        );
        assert_eq!(draft.to_body(), src);
    }

    #[test]
    fn log_has_no_params_unless_extra() {
        let src =
            json!({"name": "l", "type": "log", "collection": "products", "event_type": "click"});
        let mut draft = RuleDraft::from_rule(&rule(src.clone())).expect("known");
        assert_eq!(draft.to_body(), src);
        draft.extra_params.insert("k".into(), 1.into());
        assert_eq!(draft.to_body()["params"], json!({"k": 1}));
    }

    #[test]
    fn update_body_has_no_name_or_type() {
        let draft = RuleDraft::from_rule(&rule(json!({
            "name": "l", "type": "log", "collection": "c", "event_type": "click"
        })))
        .expect("known");
        let body = draft.update_body();
        assert!(body.get("name").is_none() && body.get("type").is_none());
        assert_eq!(body["collection"], "c");
    }

    #[test]
    fn update_body_clears_the_tag_and_the_meta_fields() {
        let mut draft = RuleDraft::from_rule(&rule(json!({
            "name": "q", "type": "popular_queries", "collection": "c", "event_type": "search",
            "rule_tag": "web",
            "params": {"destination_collection": "d", "limit": 10, "meta_fields": ["filter_by"]}
        })))
        .expect("known");
        draft.rule_tag.clear();
        draft.meta_fields.clear();
        let body = draft.update_body();
        assert_eq!(body["rule_tag"], "");
        assert_eq!(body["params"]["meta_fields"], json!([]));
    }

    #[test]
    fn update_body_clears_the_tag_of_a_counter_and_a_log() {
        for t in [RuleType::Counter, RuleType::Log] {
            let draft = RuleDraft {
                rule_type: t,
                ..good_counter()
            };
            let body = draft.update_body();
            assert_eq!(body["rule_tag"], "");
            assert!(
                body.get("params")
                    .is_none_or(|p| p.get("meta_fields").is_none())
            );
        }
    }

    #[test]
    fn a_cleared_limit_or_weight_is_refused_not_sent() {
        let mut query = good_query();
        query.limit = None;
        assert_eq!(check_rule(&query), [RuleProblem::BadLimit]);
        assert!(query.update_body()["params"].get("limit").is_none());
        let mut counter = good_counter();
        counter.weight = None;
        assert_eq!(check_rule(&counter), [RuleProblem::BadWeight]);
        assert!(counter.update_body()["params"].get("weight").is_none());
    }

    #[test]
    fn a_counter_keeps_the_keys_of_a_query_rule() {
        let src = json!({
            "name": "c", "type": "counter", "collection": "products", "event_type": "click",
            "params": {
                "destination_collection": "products", "counter_field": "popularity", "weight": 3,
                "capture_search_requests": true, "expand_query": true
            }
        });
        let draft = RuleDraft::from_rule(&rule(src.clone())).expect("known");
        assert_eq!(draft.to_body(), src);
    }

    #[test]
    fn null_params_read_as_empty_and_empty_tag_is_dropped() {
        let src = rule(json!({
            "name": "l", "type": "log", "collection": "c", "event_type": "click",
            "rule_tag": "", "params": null
        }));
        let draft = RuleDraft::from_rule(&src).expect("known");
        assert!(draft.extra_params.is_empty() && draft.rule_tag.is_empty());
        let body = draft.to_body();
        assert!(body.get("rule_tag").is_none() && body.get("params").is_none());
        let missing =
            rule(json!({"name": "l", "type": "log", "collection": "c", "event_type": "click"}));
        assert_eq!(RuleDraft::from_rule(&missing), Some(draft));
    }

    #[test]
    fn unknown_type_gives_none() {
        assert!(RuleDraft::from_rule(&rule(json!({"name": "x", "type": "wow"}))).is_none());
        assert!(RuleDraft::from_rule(&rule(json!({"name": "x"}))).is_none());
    }

    #[test]
    fn type_names_and_events() {
        for t in [
            RuleType::PopularQueries,
            RuleType::NohitsQueries,
            RuleType::Counter,
            RuleType::Log,
        ] {
            assert_eq!(RuleType::from_str(t.as_str()), Some(t));
        }
        assert_eq!(event_types(RuleType::PopularQueries), ["search"]);
        assert_eq!(
            event_types(RuleType::Counter),
            ["click", "conversion", "visit"]
        );
        assert_eq!(event_types(RuleType::Log).len(), 4);
    }

    fn good_query() -> RuleDraft {
        RuleDraft {
            name: "q".into(),
            collection: "c".into(),
            event_type: "search".into(),
            destination: "d".into(),
            limit: Some(10),
            ..RuleDraft::default()
        }
    }

    fn good_counter() -> RuleDraft {
        RuleDraft {
            rule_type: RuleType::Counter,
            event_type: "click".into(),
            counter_field: "n".into(),
            weight: Some(1),
            limit: None,
            ..good_query()
        }
    }

    #[test]
    fn check_rule_per_problem() {
        assert!(check_rule(&good_query()).is_empty());
        assert!(check_rule(&good_counter()).is_empty());
        let with = |f: fn(&mut RuleDraft)| {
            let mut d = good_query();
            f(&mut d);
            check_rule(&d)
        };
        assert_eq!(with(|d| d.name = "a b".into()), [RuleProblem::BadName]);
        assert_eq!(with(|d| d.collection.clear()), [RuleProblem::NoCollection]);
        assert_eq!(
            with(|d| d.destination.clear()),
            [RuleProblem::NoDestination]
        );
        assert_eq!(with(|d| d.limit = None), [RuleProblem::BadLimit]);
        assert_eq!(with(|d| d.limit = Some(0)), [RuleProblem::BadLimit]);
        let mut c = good_counter();
        c.counter_field.clear();
        c.weight = None;
        assert_eq!(
            check_rule(&c),
            [RuleProblem::NoCounterField, RuleProblem::BadWeight]
        );
        c.destination.clear();
        assert!(check_rule(&c).contains(&RuleProblem::NoDestination));
        let log = RuleDraft {
            rule_type: RuleType::Log,
            destination: String::new(),
            limit: None,
            ..good_query()
        };
        assert!(check_rule(&log).is_empty());
    }

    #[test]
    fn test_event_uses_q_for_search() {
        assert_eq!(
            test_event(&good_query(), "u", "shoes"),
            json!({"name": "q", "event_type": "search", "data": {"user_id": "u", "q": "shoes"}})
        );
        assert_eq!(
            test_event(&good_counter(), "u", "42"),
            json!({"name": "q", "event_type": "click", "data": {"user_id": "u", "doc_id": "42"}})
        );
    }
}
