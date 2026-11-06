// SPDX-License-Identifier: GPL-3.0-or-later

//! A search as a JSON object of parameters: the form is a view onto it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tessera_client::Collection;

use crate::documents::columns::{is_sortable, is_vector};

/// What the user is searching and with which parameters.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SearchDraft {
    /// The collection to search.
    pub collection: String,
    /// The search parameters, in the order the user sees them.
    pub params: Map<String, Value>,
}

impl SearchDraft {
    /// `q=*`, and `enable_analytics=false` so tuning relevance does not pollute the
    /// server's popular-queries statistics.
    pub fn new(collection: &str) -> Self {
        let mut params = Map::new();
        params.insert("q".into(), "*".into());
        params.insert("enable_analytics".into(), false.into());
        Self {
            collection: collection.to_string(),
            params,
        }
    }
}

/// Why a parameter map is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParamError {
    /// The key is not `[a-z][a-z0-9_]*`.
    #[error("`{0}` is not a valid parameter name")]
    BadKey(String),
    /// The collection picker and the mode own this key.
    #[error("`{0}` is set by the form, not by the parameters")]
    Reserved(String),
    /// The value is not a string, number or boolean.
    #[error("`{0}` must be a string, a number or true/false")]
    BadValue(String),
    /// The text is not a JSON object.
    #[error("the parameters must be a JSON object")]
    NotAnObject,
}

fn key_ok(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Keys pass an allowlist, which also keeps `encode_query`'s unencoded keys safe and keeps
/// `x-typesense-api-key` out of drafts and history.
fn check(params: &Map<String, Value>) -> Result<(), ParamError> {
    for (key, value) in params {
        if !key_ok(key) {
            return Err(ParamError::BadKey(key.clone()));
        }
        if matches!(key.as_str(), "collection" | "searches" | "union") {
            return Err(ParamError::Reserved(key.clone()));
        }
        if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
            return Err(ParamError::BadValue(key.clone()));
        }
    }
    Ok(())
}

/// Parses and checks a JSON object of parameters.
pub fn parse_params(text: &str) -> Result<Map<String, Value>, ParamError> {
    // shortcut: a syntax error is reported as NotAnObject, the editor shows the JSON error
    // itself; give ParamError a variant if a caller needs to tell them apart.
    let Ok(Value::Object(params)) = serde_json::from_str(text) else {
        return Err(ParamError::NotAnObject);
    };
    check(&params)?;
    Ok(params)
}

/// The parameters as pretty JSON.
pub fn params_text(params: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(params).unwrap_or_default()
}

/// `q` as text, `*` when blank or absent.
fn q_text(params: &Map<String, Value>) -> String {
    match params.get("q") {
        Some(Value::String(q)) if !q.trim().is_empty() => q.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => "*".into(),
    }
}

/// The parameters as `k=v` pairs, `q` first and `*` when blank, so a blank draft browses.
pub fn to_pairs(params: &Map<String, Value>) -> Result<Vec<(String, String)>, ParamError> {
    check(params)?;
    let mut pairs = vec![("q".to_string(), q_text(params))];
    for (key, value) in params.iter().filter(|(k, _)| *k != "q") {
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        pairs.push((key.clone(), text));
    }
    Ok(pairs)
}

/// One entry of a `multi_search`: `collection`, `q`, then the rest, with numbers and
/// booleans kept as such.
pub fn to_search(draft: &SearchDraft) -> Result<Map<String, Value>, ParamError> {
    check(&draft.params)?;
    let mut search = Map::new();
    search.insert("collection".into(), draft.collection.clone().into());
    search.insert("q".into(), q_text(&draft.params).into());
    for (key, value) in draft.params.iter().filter(|(k, _)| *k != "q") {
        search.insert(key.clone(), value.clone());
    }
    Ok(search)
}

/// How the form edits a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Free text.
    Text,
    /// A `filter_by` expression.
    Filter,
    /// A `sort_by` expression.
    Sort,
    /// Comma-separated field names.
    Fields,
    /// A whole number.
    Int,
    /// A checkbox.
    Bool,
    /// One of these values.
    Choice(&'static [&'static str]),
}

/// A parameter the form has a control for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamDef {
    /// The parameter name.
    pub key: &'static str,
    /// Its control.
    pub kind: ParamKind,
    /// Shown at once; the rest fold under "More".
    pub common: bool,
}

const fn def(key: &'static str, kind: ParamKind, common: bool) -> ParamDef {
    ParamDef { key, kind, common }
}

/// The form's parameters in the form's order. `num_typos` and `prefix` are text because they
/// take one value per `query_by` field.
pub const FORM_PARAMS: &[ParamDef] = &[
    def("q", ParamKind::Text, true),
    def("query_by", ParamKind::Fields, true),
    def("query_by_weights", ParamKind::Text, true),
    def("filter_by", ParamKind::Filter, true),
    def("sort_by", ParamKind::Sort, true),
    def("facet_by", ParamKind::Fields, true),
    def("page", ParamKind::Int, true),
    def("per_page", ParamKind::Int, true),
    def("max_facet_values", ParamKind::Int, false),
    def("facet_query", ParamKind::Text, false),
    def("group_by", ParamKind::Fields, false),
    def("group_limit", ParamKind::Int, false),
    def("include_fields", ParamKind::Fields, false),
    def("exclude_fields", ParamKind::Fields, false),
    def("highlight_fields", ParamKind::Fields, false),
    def("num_typos", ParamKind::Text, false),
    def("prefix", ParamKind::Text, false),
    def(
        "infix",
        ParamKind::Choice(&["off", "always", "fallback"]),
        false,
    ),
    def("drop_tokens_threshold", ParamKind::Int, false),
    def("typo_tokens_threshold", ParamKind::Int, false),
    def(
        "text_match_type",
        ParamKind::Choice(&["max_score", "max_weight"]),
        false,
    ),
    def("prioritize_exact_match", ParamKind::Bool, false),
    def("exhaustive_search", ParamKind::Bool, false),
    def("vector_query", ParamKind::Text, false),
    def("synonym_sets", ParamKind::Text, false),
    def("curation_tags", ParamKind::Text, false),
    def("preset", ParamKind::Text, false),
    def("use_cache", ParamKind::Bool, false),
    def("search_cutoff_ms", ParamKind::Int, false),
    def("limit_hits", ParamKind::Int, false),
    def("enable_analytics", ParamKind::Bool, false),
];

/// Fields `query_by` can name: indexed text and vector fields. Dotted nested names stay;
/// `id` and wildcards go.
pub fn query_fields(c: &Collection) -> Vec<String> {
    c.fields
        .iter()
        .filter(|f| {
            f.flag("index")
                && f.name != "id"
                && !f.name.contains('*')
                && (matches!(f.r#type.as_str(), "string" | "string[]" | "string*") || is_vector(f))
        })
        .map(|f| f.name.clone())
        .collect()
}

/// Fields and expressions `sort_by` can name.
pub fn sortable_fields(c: &Collection) -> Vec<String> {
    let mut names: Vec<String> = c
        .fields
        .iter()
        .filter(|f| is_sortable(f) && f.name != "id" && !f.name.contains('*'))
        .map(|f| f.name.clone())
        .collect();
    names.push("_text_match".into());
    names.push("_eval".into());
    if c.fields.iter().any(is_vector) {
        names.push("_vector_distance".into());
    }
    names
}

/// Something a search does besides answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffect {
    /// The query is recorded in the server's search analytics.
    Analytics,
    /// A conversation is stored and an LLM is called.
    Conversation,
    /// An LLM turns the text into a query.
    NaturalLanguage,
    /// `q` is sent to a remote embedding API.
    RemoteEmbedding,
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

/// What running this search would do besides reading. An absent `enable_analytics` counts,
/// because the server's default is on.
pub fn side_effects(params: &Map<String, Value>, schema: Option<&Collection>) -> Vec<SideEffect> {
    let mut found = Vec::new();
    if params.get("enable_analytics").is_none() || truthy(params.get("enable_analytics")) {
        found.push(SideEffect::Analytics);
    }
    if truthy(params.get("conversation")) {
        found.push(SideEffect::Conversation);
    }
    if truthy(params.get("nl_query")) {
        found.push(SideEffect::NaturalLanguage);
    }
    let embeds = |query_by: &str| {
        schema.is_some_and(|c| {
            query_by.split(',').map(str::trim).any(|name| {
                c.fields
                    .iter()
                    .any(|f| f.name == name && f.attr("embed").is_some())
            })
        })
    };
    if let Some(Value::String(query_by)) = params.get("query_by")
        && embeds(query_by)
    {
        found.push(SideEffect::RemoteEmbedding);
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn collection(fields: Value) -> Collection {
        serde_json::from_value(json!({"name":"c","fields":fields})).unwrap()
    }

    #[test]
    fn new_draft_turns_analytics_off() {
        let draft = SearchDraft::new("books");
        assert_eq!(draft.collection, "books");
        assert_eq!(
            serde_json::to_string(&draft.params).unwrap(),
            r#"{"q":"*","enable_analytics":false}"#
        );
    }

    #[test]
    fn to_pairs_contains_analytics_false() {
        let pairs = to_pairs(&SearchDraft::new("c").params).unwrap();
        assert_eq!(
            pairs,
            [
                ("q".to_string(), "*".to_string()),
                ("enable_analytics".to_string(), "false".to_string())
            ]
        );
    }

    #[test]
    fn param_keys() {
        let cases = [
            ("q", None),
            ("a1_b", None),
            ("Q", Some(ParamError::BadKey("Q".into()))),
            ("a b", Some(ParamError::BadKey("a b".into()))),
            ("a&b", Some(ParamError::BadKey("a&b".into()))),
            (
                "x-typesense-api-key",
                Some(ParamError::BadKey("x-typesense-api-key".into())),
            ),
            (
                "collection",
                Some(ParamError::Reserved("collection".into())),
            ),
            ("union", Some(ParamError::Reserved("union".into()))),
            ("searches", Some(ParamError::Reserved("searches".into()))),
        ];
        for (key, want) in cases {
            let params = map(json!({ key: "v" }));
            assert_eq!(to_pairs(&params).err(), want, "{key}");
            assert_eq!(
                parse_params(&json!({ key: "v" }).to_string()).err(),
                want,
                "{key}"
            );
        }
    }

    #[test]
    fn values_must_be_scalar() {
        for value in [json!(null), json!([1]), json!({"a":1})] {
            let text = json!({"filter_by": value}).to_string();
            assert_eq!(
                parse_params(&text).unwrap_err(),
                ParamError::BadValue("filter_by".into())
            );
        }
        assert_eq!(parse_params("[1]").unwrap_err(), ParamError::NotAnObject);
        assert_eq!(parse_params("nope").unwrap_err(), ParamError::NotAnObject);
    }

    #[test]
    fn blank_q_is_a_star() {
        for params in [json!({"q":"  "}), json!({"q":""}), json!({"per_page":5})] {
            let pairs = to_pairs(&map(params.clone())).unwrap();
            assert_eq!(pairs[0], ("q".to_string(), "*".to_string()), "{params}");
        }
        let draft = SearchDraft {
            collection: "c".into(),
            params: map(json!({"q":" "})),
        };
        assert_eq!(to_search(&draft).unwrap()["q"], "*");
    }

    #[test]
    fn q_comes_first_and_pairs_stringify() {
        let params = map(json!({"per_page":5,"q":"dune","exhaustive_search":true,"x":1.5}));
        let pairs = to_pairs(&params).unwrap();
        let shown: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(
            shown,
            ["q=dune", "per_page=5", "exhaustive_search=true", "x=1.5"]
        );
    }

    #[test]
    fn numbers_stay_numbers_in_a_search_object() {
        let draft = SearchDraft {
            collection: "books".into(),
            params: map(json!({"q":"dune","per_page":5,"exhaustive_search":true})),
        };
        let search = to_search(&draft).unwrap();
        assert_eq!(search["collection"], "books");
        assert_eq!(search["per_page"], json!(5));
        assert_eq!(search["exhaustive_search"], json!(true));
        let keys: Vec<&String> = search.keys().collect();
        assert_eq!(keys[..2], ["collection", "q"]);
    }

    #[test]
    fn unknown_key_is_kept() {
        let params = parse_params(r#"{"q":"x","typo_param":"y"}"#).unwrap();
        assert!(
            to_pairs(&params)
                .unwrap()
                .contains(&("typo_param".into(), "y".into()))
        );
    }

    #[test]
    fn params_text_round_trips() {
        let params = map(json!({"q":"x","per_page":5,"enable_analytics":false,"f":1.5}));
        assert_eq!(parse_params(&params_text(&params)).unwrap(), params);
    }

    #[test]
    fn form_params_start_with_the_common_ones() {
        let common: Vec<&str> = FORM_PARAMS
            .iter()
            .take_while(|d| d.common)
            .map(|d| d.key)
            .collect();
        assert_eq!(
            common,
            [
                "q",
                "query_by",
                "query_by_weights",
                "filter_by",
                "sort_by",
                "facet_by",
                "page",
                "per_page"
            ]
        );
        assert_eq!(FORM_PARAMS.iter().filter(|d| d.common).count(), 8);
        assert_eq!(FORM_PARAMS.len(), 31);
        assert!(FORM_PARAMS.iter().all(|d| key_ok(d.key)));
    }

    #[test]
    fn side_effects_table() {
        let embedding = collection(json!([
            {"name":"title","type":"string"},
            {"name":"vec","type":"float[]","embed":{"from":["title"],"model_config":{"model_name":"openai/text-embedding-3-small","api_key":"k"}}}
        ]));
        use SideEffect::*;
        let cases: [(Value, Option<&Collection>, Vec<SideEffect>); 8] = [
            (json!({"q":"x"}), None, vec![Analytics]),
            (json!({"enable_analytics":false}), None, vec![]),
            (json!({"enable_analytics":true}), None, vec![Analytics]),
            (json!({"enable_analytics":"true"}), None, vec![Analytics]),
            (
                json!({"enable_analytics":false,"conversation":true}),
                None,
                vec![Conversation],
            ),
            (
                json!({"enable_analytics":false,"nl_query":"true"}),
                None,
                vec![NaturalLanguage],
            ),
            (
                json!({"enable_analytics":false,"query_by":"title, vec"}),
                Some(&embedding),
                vec![RemoteEmbedding],
            ),
            (
                json!({"enable_analytics":false,"query_by":"title"}),
                Some(&embedding),
                vec![],
            ),
        ];
        for (params, schema, want) in cases {
            assert_eq!(side_effects(&map(params.clone()), schema), want, "{params}");
        }
    }

    #[test]
    fn query_fields_skip_unindexed_and_id() {
        let c = collection(json!([
            {"name":"id","type":"string"},
            {"name":"title","type":"string"},
            {"name":"tags","type":"string[]"},
            {"name":"note","type":"string","index":false},
            {"name":"year","type":"int32"},
            {"name":"meta.author","type":"string"},
            {"name":".*","type":"auto"},
            {"name":"vec","type":"float[]","num_dim":3}
        ]));
        assert_eq!(query_fields(&c), ["title", "tags", "meta.author", "vec"]);
    }

    #[test]
    fn sortable_fields_add_the_virtual_ones() {
        let c = collection(json!([
            {"name":"title","type":"string","sort":true},
            {"name":"tags","type":"string[]"},
            {"name":"year","type":"int32","sort":true},
            {"name":"vec","type":"float[]","num_dim":3}
        ]));
        assert_eq!(
            sortable_fields(&c),
            ["title", "year", "_text_match", "_eval", "_vector_distance"]
        );
        let plain = collection(json!([{"name":"year","type":"int32","sort":true}]));
        assert_eq!(sortable_fields(&plain), ["year", "_text_match", "_eval"]);
    }
}
