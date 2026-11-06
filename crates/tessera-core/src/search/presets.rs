// SPDX-License-Identifier: GPL-3.0-or-later

//! Stored presets: what a value means, and drafts to and from one.

use serde_json::{Value, json};
use tessera_client::MULTI_SEARCH_LIMIT;

use super::params::{ParamError, SearchDraft, parse_params, to_search};

/// Why a preset's value cannot become drafts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetError {
    /// The value, or one of its searches, is not a JSON object.
    NotAnObject,
    /// It holds more searches than a `multi_search` takes.
    TooMany,
    /// A parameter was refused.
    Param(ParamError),
}

/// What a preset's value holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetKind {
    /// One search.
    Search,
    /// This many searches.
    Multi(usize),
    /// This many searches, as a union.
    Union(usize),
    /// Not something a search can use.
    Invalid(PresetError),
}

/// `value` of a preset as drafts and whether they are a union. Without a collection a search
/// stays on `current`.
pub fn preset_drafts(
    value: &Value,
    current: &str,
) -> Result<(Vec<SearchDraft>, bool), PresetError> {
    fn draft(search: &Value, current: &str) -> Result<SearchDraft, PresetError> {
        let Value::Object(search) = search else {
            return Err(PresetError::NotAnObject);
        };
        let mut search = search.clone();
        let collection = search.shift_remove("collection");
        let collection = collection.as_ref().and_then(Value::as_str);
        let params =
            parse_params(&Value::Object(search).to_string()).map_err(PresetError::Param)?;
        Ok(SearchDraft {
            collection: collection.unwrap_or(current).to_string(),
            params,
        })
    }
    match value.get("searches").and_then(Value::as_array) {
        Some(searches) if searches.len() > MULTI_SEARCH_LIMIT => Err(PresetError::TooMany),
        Some(searches) if !searches.is_empty() => {
            let drafts: Result<Vec<_>, _> = searches.iter().map(|s| draft(s, current)).collect();
            let union = value.get("union").and_then(Value::as_bool);
            Ok((drafts?, union.unwrap_or(false)))
        }
        _ => Ok((vec![draft(value, current)?], false)),
    }
}

/// The `searches` wrapper of a stored `value`: `Some(union)`, or `None` for a flat search.
pub fn preset_wrapper(value: &Value) -> Option<bool> {
    let searches = value.get("searches")?.as_array()?;
    (!searches.is_empty()).then(|| value.get("union").and_then(Value::as_bool) == Some(true))
}

/// What a preset of `drafts` stores: one search without its collection, or all of them, with
/// `union` when it is on. `wrapper`, from [`preset_wrapper`], keeps the shape of the preset being
/// edited for one search: a flat preset fails on `POST /multi_search?preset=`.
pub fn preset_value(
    drafts: &[SearchDraft],
    union: bool,
    wrapper: Option<bool>,
) -> Result<Value, ParamError> {
    let mut searches = drafts
        .iter()
        .map(to_search)
        .collect::<Result<Vec<_>, _>>()?;
    if let ([search], None) = (&mut searches[..], wrapper) {
        search.shift_remove("collection");
        return Ok(Value::Object(std::mem::take(search)));
    }
    // The workbench clears `union` for one draft, so the wrapper carries it.
    let union = union || (searches.len() == 1 && wrapper == Some(true));
    let mut value = json!({ "searches": searches });
    if union {
        value["union"] = true.into();
    }
    Ok(value)
}

/// What `value` holds, read the way the workbench loads it.
pub fn preset_kind(value: &Value) -> PresetKind {
    match preset_drafts(value, "") {
        Err(error) => PresetKind::Invalid(error),
        Ok(_) if value.get("searches").is_none() => PresetKind::Search,
        Ok((drafts, true)) => PresetKind::Union(drafts.len()),
        Ok((drafts, false)) => PresetKind::Multi(drafts.len()),
    }
}

/// `q` and `query_by`, or the collections, joined by `, `.
pub fn preset_summary(value: &Value) -> String {
    let text = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
    let parts: Vec<String> = match value.get("searches").and_then(Value::as_array) {
        Some(searches) => searches
            .iter()
            .filter_map(|s| s.get("collection"))
            .map(text)
            .collect(),
        None => ["q", "query_by"]
            .iter()
            .filter_map(|key| value.get(key))
            .map(text)
            .collect(),
    };
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_preset_of_one_search_keeps_no_collection() {
        let value = preset_value(&[SearchDraft::new("books")], false, None).unwrap();
        assert_eq!(value, json!({"q": "*", "enable_analytics": false}));
    }

    #[test]
    fn a_preset_of_several_lists_them() {
        let drafts = [SearchDraft::new("a"), SearchDraft::new("b")];
        let value = preset_value(&drafts, false, None).unwrap();
        assert_eq!(value["searches"][1]["collection"], "b");
        assert!(value.get("union").is_none());
    }

    #[test]
    fn a_union_survives_a_preset_both_ways() {
        let drafts = [SearchDraft::new("a"), SearchDraft::new("b")];
        let value = preset_value(&drafts, true, None).unwrap();
        assert_eq!(value["union"], true);
        let (loaded, union) = preset_drafts(&value, "other").unwrap();
        assert!(union);
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn an_unedited_wrapper_of_one_search_round_trips() {
        let plain = json!({"searches": [{"collection": "books", "q": "a"}]});
        let union = json!({"searches": [{"collection": "books", "q": "a"}], "union": true});
        for value in [plain, union] {
            let (drafts, union) = preset_drafts(&value, "other").unwrap();
            // As the workbench loads them: `union` needs two drafts.
            let saved = preset_value(&drafts, union && drafts.len() > 1, preset_wrapper(&value));
            assert_eq!(saved.unwrap()["searches"][0]["collection"], "books");
            let saved = preset_value(&drafts, false, preset_wrapper(&value)).unwrap();
            assert_eq!(saved["union"], value["union"]);
            assert_eq!(preset_kind(&saved), preset_kind(&value));
        }
    }

    #[test]
    fn wrappers_of_a_stored_value() {
        assert_eq!(preset_wrapper(&json!({"q": "a"})), None);
        assert_eq!(preset_wrapper(&json!({"searches": []})), None);
        assert_eq!(preset_wrapper(&json!({"searches": [{}]})), Some(false));
        let union = json!({"searches": [{}], "union": true});
        assert_eq!(preset_wrapper(&union), Some(true));
    }

    #[test]
    fn a_preset_over_the_limit_is_refused() {
        let searches = vec![json!({"q": "a"}); MULTI_SEARCH_LIMIT + 1];
        let error = preset_drafts(&json!({ "searches": searches }), "books").unwrap_err();
        assert_eq!(error, PresetError::TooMany);
        let searches = vec![json!({"q": "a"}); MULTI_SEARCH_LIMIT];
        assert!(preset_drafts(&json!({ "searches": searches }), "books").is_ok());
    }

    #[test]
    fn a_search_without_collection_stays_on_the_current_one() {
        let (drafts, _) = preset_drafts(&json!({"searches": [{"q": "a"}]}), "books").unwrap();
        assert_eq!(drafts[0].collection, "books");
    }

    #[test]
    fn a_refused_parameter_changes_nothing() {
        let value = json!({"searches": [{"q": "a"}, {"collection": "b", "x": [1]}]});
        let error = preset_drafts(&value, "books").unwrap_err();
        assert!(matches!(error, PresetError::Param(_)));
    }

    #[test]
    fn kinds_of_a_preset() {
        assert_eq!(preset_kind(&json!({})), PresetKind::Search);
        assert_eq!(preset_kind(&json!({"q": "a"})), PresetKind::Search);
        assert_eq!(
            preset_kind(&json!({"searches": [{}]})),
            PresetKind::Multi(1)
        );
        let union = json!({"searches": [{}, {}], "union": true});
        assert_eq!(preset_kind(&union), PresetKind::Union(2));
        assert_eq!(
            preset_kind(&json!("str")),
            PresetKind::Invalid(PresetError::NotAnObject)
        );
        let many = json!({ "searches": vec![json!({}); 100] });
        assert_eq!(
            preset_kind(&many),
            PresetKind::Invalid(PresetError::TooMany)
        );
        let bad = json!({"x": [1]});
        assert!(matches!(
            preset_kind(&bad),
            PresetKind::Invalid(PresetError::Param(_))
        ));
    }

    #[test]
    fn summaries_of_a_preset() {
        let one = json!({"q": "dune", "query_by": "title", "per_page": 5});
        assert_eq!(preset_summary(&one), "dune, title");
        let many = json!({"searches": [{"collection": "a"}, {"q": "x"}, {"collection": "b"}]});
        assert_eq!(preset_summary(&many), "a, b");
        assert_eq!(preset_summary(&json!("str")), "");
        assert_eq!(preset_summary(&json!({})), "");
    }
}
