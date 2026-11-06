// SPDX-License-Identifier: GPL-3.0-or-later

//! A search answer as the workbench shows it. Parsing is tolerant: every field the server may
//! leave out has a default, and only a hit's `document` is required.

use std::ops::Range;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

/// Joins the highlighted elements of an array field.
const ARRAY_JOIN: &str = " · ";

/// Lines a hit without highlights shows instead.
const FALLBACK_LINES: usize = 3;

/// One search's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// Documents matching.
    pub found: u64,
    /// Documents in the collection.
    pub out_of: u64,
    /// The page the server answered.
    pub page: u32,
    /// Server-side search time.
    pub search_time_ms: u64,
    /// Matching documents of a grouped answer, where `found` counts groups.
    pub found_docs: Option<u64>,
    /// The hits of this page (empty for a grouped answer).
    pub hits: Vec<Hit>,
    /// The groups of a grouped answer.
    pub groups: Vec<HitGroup>,
    /// Facet counts, in the order sent.
    pub facets: Vec<Facet>,
}

/// One hit.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The document's `id`, empty when it has none.
    pub id: Arc<str>,
    /// The document.
    pub doc: Arc<Map<String, Value>>,
    /// One line per highlighted field, in the order sent.
    pub lines: Vec<HitLine>,
    /// The rank score. Values reach 2^59, so not a float.
    pub text_match: Option<u64>,
    /// How the score came about.
    pub text_match_info: Option<Value>,
    /// A curation rule put this hit here.
    pub curated: bool,
    /// Union only: the collection the hit comes from.
    pub collection: Option<String>,
    /// Union only: which search found the hit.
    pub search_index: Option<usize>,
}

/// A highlighted field of a hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitLine {
    /// Field name, nested ones dotted.
    pub field: String,
    /// The text without highlight tags.
    pub text: String,
    /// Byte ranges of `text` that matched.
    pub marks: Vec<Range<usize>>,
}

/// The hits sharing one `group_by` value.
#[derive(Debug, Clone, PartialEq)]
pub struct HitGroup {
    /// The value of each `group_by` field.
    pub key: Vec<String>,
    /// Documents in the group.
    pub found: u64,
    /// The group's hits.
    pub hits: Vec<Hit>,
}

/// The counts of one facet field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facet {
    /// Field name.
    pub field: String,
    /// `(value, count)` in the order sent.
    pub counts: Vec<(String, u64)>,
    /// The counts come from a sample of the matches.
    pub sampled: bool,
}

/// What a multi-search returned.
#[derive(Debug, Clone, PartialEq)]
pub enum MultiResult {
    /// One answer, or the error of that search, per search, in the order sent.
    Federated(Vec<Result<SearchResult, ItemError>>),
    /// One merged answer.
    Union(SearchResult),
}

/// A federated search that failed while its neighbours did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemError {
    /// The code the server reported, 0 when it sent none.
    pub code: u16,
    /// The server's message.
    pub message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawResult {
    found: u64,
    out_of: u64,
    page: u32,
    search_time_ms: u64,
    found_docs: Option<u64>,
    hits: Vec<RawHit>,
    grouped_hits: Vec<RawGroup>,
    facet_counts: Vec<RawFacet>,
}

#[derive(Deserialize)]
struct RawHit {
    document: Map<String, Value>,
    #[serde(default)]
    highlight: Value,
    #[serde(default)]
    text_match: Value,
    #[serde(default)]
    text_match_info: Option<Value>,
    #[serde(default)]
    curated: bool,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    search_index: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawGroup {
    group_key: Vec<Value>,
    found: u64,
    hits: Vec<RawHit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawFacet {
    field_name: String,
    counts: Vec<RawCount>,
    sampled: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawCount {
    value: Value,
    count: u64,
}

/// `value` as the text a user would read.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `snippet` without the tags, and the byte ranges the tags enclosed. A start tag without an
/// end tag marks to the end of the text.
pub fn marks(snippet: &str, tags: (&str, &str)) -> (String, Vec<Range<usize>>) {
    let (start_tag, end_tag) = tags;
    let mut text = String::with_capacity(snippet.len());
    let mut ranges = Vec::new();
    if start_tag.is_empty() || end_tag.is_empty() {
        return (snippet.to_string(), ranges);
    }
    let mut rest = snippet;
    while let Some(at) = rest.find(start_tag) {
        text.push_str(&rest[..at]);
        rest = &rest[at + start_tag.len()..];
        let (inner, after) = match rest.find(end_tag) {
            Some(end) => (&rest[..end], &rest[end + end_tag.len()..]),
            None => (rest, ""),
        };
        let from = text.len();
        text.push_str(inner);
        if text.len() > from {
            ranges.push(from..text.len());
        }
        rest = after;
    }
    text.push_str(rest);
    (text, ranges)
}

fn line(field: &str, snippet: &str, tags: (&str, &str)) -> HitLine {
    let (text, marks) = marks(snippet, tags);
    HitLine {
        field: field.to_string(),
        text,
        marks,
    }
}

/// Walks `highlight`: `{"snippet"}` is a line, a list is an array field whose matched
/// elements are joined, any other object nests.
fn walk(path: &str, value: &Value, tags: (&str, &str), out: &mut Vec<HitLine>) {
    match value {
        Value::Object(map) => match map.get("snippet") {
            Some(Value::String(snippet)) => out.push(line(path, snippet, tags)),
            _ => {
                for (key, child) in map {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(&child_path, child, tags, out);
                }
            }
        },
        Value::Array(items) => {
            let mut matched = Vec::new();
            let mut nested = Vec::new();
            for item in items {
                match item.get("snippet").and_then(Value::as_str) {
                    Some(snippet) => {
                        let has_tokens = item["matched_tokens"]
                            .as_array()
                            .is_some_and(|tokens| !tokens.is_empty());
                        if has_tokens {
                            matched.push(snippet);
                        }
                    }
                    // An array of objects: `addresses.city` keeps one line per element.
                    None => walk(path, item, tags, &mut nested),
                }
            }
            if !matched.is_empty() {
                out.push(line(path, &matched.join(ARRAY_JOIN), tags));
            }
            out.extend(nested);
        }
        _ => {}
    }
}

fn hit(raw: RawHit, tags: (&str, &str)) -> Hit {
    let mut lines = Vec::new();
    walk("", &raw.highlight, tags, &mut lines);
    if lines.is_empty() {
        lines = raw
            .document
            .iter()
            .filter_map(|(k, v)| Some((k, v.as_str()?)))
            .take(FALLBACK_LINES)
            .map(|(field, text)| HitLine {
                field: field.clone(),
                text: text.to_string(),
                marks: Vec::new(),
            })
            .collect();
    }
    Hit {
        id: raw
            .document
            .get("id")
            .map(text_of)
            .unwrap_or_default()
            .into(),
        lines,
        text_match: raw.text_match.as_u64(),
        text_match_info: raw.text_match_info,
        curated: raw.curated,
        collection: raw.collection,
        search_index: raw.search_index,
        doc: Arc::new(raw.document),
    }
}

fn result(raw: RawResult, tags: (&str, &str)) -> SearchResult {
    let hits = |hits: Vec<RawHit>| hits.into_iter().map(|h| hit(h, tags)).collect();
    SearchResult {
        found: raw.found,
        out_of: raw.out_of,
        page: raw.page,
        search_time_ms: raw.search_time_ms,
        found_docs: raw.found_docs,
        hits: hits(raw.hits),
        groups: raw
            .grouped_hits
            .into_iter()
            .map(|g| HitGroup {
                key: g.group_key.iter().map(text_of).collect(),
                found: g.found,
                hits: hits(g.hits),
            })
            .collect(),
        facets: raw
            .facet_counts
            .into_iter()
            .map(|f| Facet {
                field: f.field_name,
                counts: f
                    .counts
                    .iter()
                    .map(|c| (text_of(&c.value), c.count))
                    .collect(),
                sampled: f.sampled,
            })
            .collect(),
    }
}

/// Parses the answer of one search. `tags` are the draft's highlight tags.
pub fn parse_result(text: &str, tags: (&str, &str)) -> Result<SearchResult, serde_json::Error> {
    Ok(result(serde_json::from_str(text)?, tags))
}

/// Parses a multi-search answer. `tags` apply to all searches.
pub fn parse_multi(
    text: &str,
    union: bool,
    tags: (&str, &str),
) -> Result<MultiResult, serde_json::Error> {
    if union {
        return parse_result(text, tags).map(MultiResult::Union);
    }
    #[derive(Deserialize)]
    struct Federated {
        results: Vec<Value>,
    }
    let answer: Federated = serde_json::from_str(text)?;
    let items = answer
        .results
        .into_iter()
        .map(|item| match item.get("error").and_then(Value::as_str) {
            Some(message) => Ok(Err(ItemError {
                code: item["code"]
                    .as_u64()
                    .and_then(|c| u16::try_from(c).ok())
                    .unwrap_or(0),
                message: message.to_string(),
            })),
            None => serde_json::from_value(item).map(|raw| Ok(result(raw, tags))),
        })
        .collect::<Result<_, serde_json::Error>>()?;
    Ok(MultiResult::Federated(items))
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // expected byte ranges, one per mark
mod tests {
    use super::*;

    const TAGS: (&str, &str) = ("<mark>", "</mark>");

    const HIT_PAPER: &str = r#"{"document":{"genre":"sci fi","id":"1","tags":["winter","paper"],"title":"harbor paper silent","year":1965},"highlight":{"tags":[{"matched_tokens":[],"snippet":"winter"},{"matched_tokens":["paper"],"snippet":"<mark>paper</mark>"}],"title":{"matched_tokens":["paper"],"snippet":"harbor <mark>paper</mark> silent"}},"text_match":578730123373578354,"text_match_info":{"fields_matched":2,"score":"578730123373578354"}}"#;

    fn answer(hits: &str) -> String {
        format!(
            r#"{{"facet_counts":[{{"counts":[{{"count":1,"highlighted":"crime","value":"crime"}},{{"count":3,"highlighted":"1965","value":1965}}],"field_name":"genre","sampled":true,"stats":{{"total_values":2}}}}],"found":2,"out_of":9,"page":1,"search_time_ms":4,"hits":[{hits}]}}"#
        )
    }

    #[test]
    fn a_search_answer_fills_every_part() {
        let r = parse_result(&answer(HIT_PAPER), TAGS).unwrap();
        assert_eq!((r.found, r.out_of, r.page, r.search_time_ms), (2, 9, 1, 4));
        assert_eq!(r.found_docs, None);
        let hit = &r.hits[0];
        assert_eq!(&*hit.id, "1");
        assert_eq!(hit.doc["year"], 1965);
        assert!(!hit.curated && hit.collection.is_none() && hit.search_index.is_none());
        assert_eq!(hit.text_match_info.as_ref().unwrap()["fields_matched"], 2);
        assert_eq!(
            r.facets,
            [Facet {
                field: "genre".into(),
                counts: vec![("crime".into(), 1), ("1965".into(), 3)],
                sampled: true
            }]
        );
    }

    #[test]
    fn highlight_lines_follow_the_two_shapes() {
        let r = parse_result(&answer(HIT_PAPER), TAGS).unwrap();
        let lines = &r.hits[0].lines;
        assert_eq!(lines.len(), 2);
        // An array field joins only the elements that matched.
        assert_eq!(
            (lines[0].field.as_str(), lines[0].text.as_str()),
            ("tags", "paper")
        );
        assert_eq!(lines[0].marks, [0..5]);
        assert_eq!(lines[1].text, "harbor paper silent");
        assert_eq!(lines[1].marks, [7..12]);
    }

    #[test]
    fn nested_and_joined_array_highlights() {
        let hit = r#"{"document":{"id":"7"},"highlight":{"meta":{"author":{"matched_tokens":["al"],"snippet":"<mark>Al</mark>ice"}},"tags":[{"matched_tokens":["a"],"snippet":"<mark>a</mark>x"},{"matched_tokens":[],"snippet":"no"},{"matched_tokens":["b"],"snippet":"<mark>b</mark>y"}]}}"#;
        let r = parse_result(&answer(hit), TAGS).unwrap();
        let lines = &r.hits[0].lines;
        assert_eq!(lines[0].field, "meta.author");
        assert_eq!(lines[1].text, "ax · by");
        assert_eq!(lines[1].marks, [0..1, 6..7]);
    }

    #[test]
    fn an_array_of_objects_keeps_its_lines() {
        let hit = r#"{"document":{"id":"7"},"highlight":{"addresses":[{"city":{"matched_tokens":["ber"],"snippet":"<mark>Ber</mark>lin"}},{"city":{"matched_tokens":[],"snippet":"Rome"}}]}}"#;
        let r = parse_result(&answer(hit), TAGS).unwrap();
        let lines = &r.hits[0].lines;
        assert_eq!(lines[0].field, "addresses.city");
        assert_eq!(
            (lines[0].text.as_str(), lines[0].marks.clone()),
            ("Berlin", vec![0..3])
        );
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn a_hit_without_highlight_shows_three_strings() {
        let hit = r#"{"document":{"id":"1","n":5,"a":"x","b":"y","c":"z"},"highlight":{}}"#;
        let r = parse_result(&answer(hit), TAGS).unwrap();
        let fields: Vec<&str> = r.hits[0].lines.iter().map(|l| l.field.as_str()).collect();
        assert_eq!(fields, ["id", "a", "b"]);
        assert!(r.hits[0].lines.iter().all(|l| l.marks.is_empty()));
    }

    #[test]
    fn union_hits_carry_collection_and_index() {
        let hit = r#"{"collection":"books","search_index":1,"curated":true,"document":{"id":"b"},"highlight":{},"text_match":1}"#;
        let r = parse_multi(&answer(hit), true, TAGS).unwrap();
        let MultiResult::Union(r) = r else {
            panic!("union")
        };
        let hit = &r.hits[0];
        assert_eq!(hit.collection.as_deref(), Some("books"));
        assert_eq!(hit.search_index, Some(1));
        assert!(hit.curated);
    }

    #[test]
    fn grouped_answer_fills_groups_and_found_docs() {
        let text = format!(
            r#"{{"found":2,"found_docs":7,"out_of":9,"page":1,"search_time_ms":1,"grouped_hits":[{{"found":4,"group_key":["crime"],"hits":[{HIT_PAPER}]}},{{"found":3,"group_key":[1965,true],"hits":[]}}]}}"#
        );
        let r = parse_result(&text, TAGS).unwrap();
        assert_eq!(r.found_docs, Some(7));
        assert!(r.hits.is_empty());
        assert_eq!(r.groups[0].key, ["crime"]);
        assert_eq!((r.groups[0].found, r.groups[0].hits.len()), (4, 1));
        assert_eq!(r.groups[1].key, ["1965", "true"]);
    }

    #[test]
    fn federated_keeps_good_results_next_to_an_error() {
        let text = format!(
            r#"{{"results":[{},{{"code":404,"error":"Collection not found"}}]}}"#,
            answer(HIT_PAPER)
        );
        let MultiResult::Federated(items) = parse_multi(&text, false, TAGS).unwrap() else {
            panic!("federated")
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_ref().unwrap().hits.len(), 1);
        assert_eq!(
            items[1],
            Err(ItemError {
                code: 404,
                message: "Collection not found".into()
            })
        );
    }

    #[test]
    fn custom_tags() {
        let hit = r#"{"document":{"id":"1"},"highlight":{"t":{"matched_tokens":["x"],"snippet":"a [[x]] b"}}}"#;
        let r = parse_result(&answer(hit), ("[[", "]]")).unwrap();
        assert_eq!(r.hits[0].lines[0].text, "a x b");
        assert_eq!(r.hits[0].lines[0].marks, [2..3]);
    }

    #[test]
    fn unterminated_mark() {
        let (text, ranges) = marks("ab <mark>cd", TAGS);
        assert_eq!((text.as_str(), ranges), ("ab cd", vec![3..5]));
        assert_eq!(marks("plain", TAGS), ("plain".to_string(), vec![]));
        assert_eq!(marks("a<mark></mark>b", TAGS).1, Vec::<Range<usize>>::new());
    }

    #[test]
    fn marks_are_byte_ranges() {
        let (text, ranges) = marks("größe <mark>Straße</mark> ok", TAGS);
        assert_eq!(text, "größe Straße ok");
        assert_eq!(&text[ranges[0].clone()], "Straße");
        let (text, ranges) = marks("<mark>é</mark><mark>ü</mark>", TAGS);
        assert_eq!(ranges, [0..2, 2..4]);
        assert_eq!(text, "éü");
    }

    #[test]
    fn big_text_match() {
        let hit = r#"{"document":{"id":"1"},"highlight":{},"text_match":578730123373578361}"#;
        let r = parse_result(&answer(hit), TAGS).unwrap();
        assert_eq!(r.hits[0].text_match, Some(578_730_123_373_578_361));
    }

    #[test]
    fn a_hit_needs_its_document_and_the_rest_may_be_missing() {
        assert!(parse_result(r#"{"hits":[{"highlight":{}}]}"#, TAGS).is_err());
        let r = parse_result(r#"{"hits":[{"document":{"x":1}}]}"#, TAGS).unwrap();
        assert_eq!(&*r.hits[0].id, "");
        assert_eq!(r.found, 0);
    }
}
