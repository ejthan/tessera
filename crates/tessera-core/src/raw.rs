// SPDX-License-Identifier: GPL-3.0-or-later

//! Logic of the raw console: which path may go out, how dangerous a request is, and which
//! 200 hides a failure.

use percent_encoding::percent_decode_str;
use serde_json::Value;
use tessera_client::{Method, READ_METHODS, encode_query};

use crate::redact;

/// Largest console body read from a file, which is held in memory.
pub const RAW_FILE_MAX: u64 = 64 << 20;

/// Why a console path is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// Nothing but slashes.
    Empty,
    /// A character outside the allowlist.
    Character(char),
    /// An empty segment, or one that decodes to `.` or `..`.
    Segment(String),
    /// A `%` without two hex digits.
    BadEscape,
    /// The first segment could read as a URL scheme.
    Scheme,
}

fn decode(segment: &str) -> String {
    percent_decode_str(segment).decode_utf8_lossy().into_owned()
}

/// Whether `segment` climbs at any depth of encoding, also with `/` or `\` hidden in an escape,
/// as in `..%2f..%2fkeys`. Each round removes a `%`, so the loop ends.
fn climbs(segment: &str) -> bool {
    let mut text = segment.to_string();
    loop {
        if text.split(['/', '\\']).any(|p| matches!(p, "." | "..")) {
            return true;
        }
        let next = decode(&text);
        if next == text {
            return false;
        }
        text = next;
    }
}

/// `input` as a relative path below the node, or why not. An allowlist: a query has its own
/// table, and a space in a segment is typed `%20`.
pub fn parse_path(input: &str) -> Result<String, PathError> {
    let path = input.strip_prefix('/').unwrap_or(input);
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || "-._~%/:@+,=".contains(c);
    if let Some(c) = path.chars().find(|c| !allowed(*c)) {
        return Err(PathError::Character(c));
    }
    let hex = |b: Option<&u8>| b.is_some_and(u8::is_ascii_hexdigit);
    let bytes = path.as_bytes();
    let escapes_ok = (0..bytes.len())
        .filter(|&i| bytes[i] == b'%')
        .all(|i| hex(bytes.get(i + 1)) && hex(bytes.get(i + 2)));
    if !escapes_ok {
        return Err(PathError::BadEscape);
    }
    if let Some(segment) = path
        .split('/')
        .find(|segment| segment.is_empty() || climbs(segment))
    {
        return Err(PathError::Segment(segment.to_string()));
    }
    let first = path.split('/').next().unwrap_or_default();
    if !first
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(PathError::Scheme);
    }
    Ok(path.to_string())
}

/// Whether `key` may be a query parameter: `[A-Za-z0-9_.-]+`, and no parameter that holds a key.
pub fn param_key_ok(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        && !redact::is_secret_param(key)
}

/// The encoded query of `pairs`, `None` without any, or the first key [`param_key_ok`] refuses.
pub fn raw_query(pairs: &[(String, String)]) -> Result<Option<String>, String> {
    if let Some((key, _)) = pairs.iter().find(|(key, _)| !param_key_ok(key)) {
        return Err(key.clone());
    }
    let pairs: Vec<_> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    Ok((!pairs.is_empty()).then(|| encode_query(&pairs)))
}

/// What a query string reads as in the table: the reverse of [`raw_query`].
pub fn query_rows(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// How much a request must be confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawDanger {
    /// Sends at once.
    None,
    /// Asks first.
    Medium,
    /// Asks for a typed name.
    High {
        /// The text to type, percent-decoded.
        expected: String,
    },
}

/// The confirmation `method` on `path`, a [`parse_path`] result, needs. Segments compare
/// decoded, as the server routes them, so `%64ocuments` is `documents`.
pub fn raw_danger(method: &Method, path: &str, prod: bool) -> RawDanger {
    let segments: Vec<String> = path.split('/').map(decode).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let high = |expected: &str| RawDanger::High {
        expected: expected.to_string(),
    };
    let last = segments.last().copied().unwrap_or_default();
    let by_env = |not_prod: RawDanger, on_prod: RawDanger| if prod { on_prod } else { not_prod };
    if READ_METHODS.contains(method) {
        return RawDanger::None;
    }
    if *method == Method::DELETE {
        return match segments[..] {
            ["collections", c] | ["collections", c, "documents"] => high(c),
            _ => by_env(RawDanger::Medium, high(last)),
        };
    }
    if *method == Method::POST {
        match segments[..] {
            ["multi_search"] => return RawDanger::None,
            ["operations", _, ..] => return by_env(RawDanger::Medium, high(last)),
            _ => {}
        }
    }
    by_env(RawDanger::None, RawDanger::Medium)
}

/// Cuts `body` to at most `max` bytes on a char boundary, and gives its length before, if it did.
pub fn cap_body(body: &mut String, max: usize) -> Option<usize> {
    let total = body.len();
    (total > max).then(|| {
        body.truncate(body.floor_char_boundary(max));
        body.shrink_to_fit();
        total
    })
}

/// A failure inside a successful answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutcomeNote {
    /// Lines or searches that failed.
    pub failed: usize,
    /// Lines or searches in the answer.
    pub total: usize,
}

/// What a 2xx answer on `path` hides: failed import lines, or failed searches of a
/// `multi_search`. `None` when nothing failed or the path has no such answer.
pub fn outcome_note(path: &str, status: u16, body: &str) -> Option<OutcomeNote> {
    if !(200..300).contains(&status) {
        return None;
    }
    let is_failure = |line: &str| {
        serde_json::from_str::<Value>(line).is_ok_and(|v| v["success"] == Value::Bool(false))
    };
    let (failed, total) = if path.ends_with("/documents/import") {
        let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
        (lines.iter().filter(|l| is_failure(l)).count(), lines.len())
    } else if path.trim_start_matches('/') == "multi_search" {
        let value: Value = serde_json::from_str(body).ok()?;
        match value["results"].as_array() {
            Some(results) => (
                results.iter().filter(|r| r.get("error").is_some()).count(),
                results.len(),
            ),
            None => (usize::from(value.get("error").is_some()), 1),
        }
    } else {
        return None;
    };
    (failed > 0).then_some(OutcomeNote { failed, total })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parse_path_table() {
        use PathError::*;
        let ok = |s: &str| Ok(s.to_string());
        let seg = |s: &str| Err(Segment(s.to_string()));
        for (input, expected) in [
            ("/collections", ok("collections")),
            (
                "collections/a%20b/documents",
                ok("collections/a%20b/documents"),
            ),
            ("health", ok("health")),
            ("metrics.json", ok("metrics.json")),
            (
                "collections/c/documents/a%2Fb",
                ok("collections/c/documents/a%2Fb"),
            ),
            (
                "collections/c/documents/a:b@c,d=e+f~g",
                ok("collections/c/documents/a:b@c,d=e+f~g"),
            ),
            ("", Err(Empty)),
            ("/", Err(Empty)),
            ("//evil", seg("")),
            ("///evil", seg("")),
            ("a//b", seg("")),
            ("a/", seg("")),
            ("http://x", seg("")),
            ("a/../b", seg("..")),
            ("a/./b", seg(".")),
            ("..", seg("..")),
            ("../keys", seg("..")),
            ("a/%2e%2E/b", seg("%2e%2E")),
            ("a/%252e%252e/b", seg("%252e%252e")),
            ("a/%25252e%25252e/b", seg("%25252e%25252e")),
            ("%2e%2e/keys", seg("%2e%2e")),
            (".%2e/keys", seg(".%2e")),
            ("collections/..%2f..%2fkeys", seg("..%2f..%2fkeys")),
            ("collections/..%5c..%5ckeys", seg("..%5c..%5ckeys")),
            ("collections/%2e%2e%2fkeys", seg("%2e%2e%2fkeys")),
            (
                "collections/%252e%252e%252fkeys",
                seg("%252e%252e%252fkeys"),
            ),
            ("a\\b", Err(Character('\\'))),
            ("/\\evil", Err(Character('\\'))),
            ("\\\\evil", Err(Character('\\'))),
            ("a?b", Err(Character('?'))),
            ("a#b", Err(Character('#'))),
            ("a b", Err(Character(' '))),
            ("a\tb", Err(Character('\t'))),
            ("/\n/evil", Err(Character('\n'))),
            ("ü", Err(Character('ü'))),
            ("a%2", Err(BadEscape)),
            ("a%", Err(BadEscape)),
            ("a%zz", Err(BadEscape)),
            ("http:x", Err(Scheme)),
            ("https:x", Err(Scheme)),
            ("localhost:8108/keys", Err(Scheme)),
            ("%68ttp:x", Err(Scheme)),
            ("%2fevil", Err(Scheme)),
            ("@evil.example", Err(Scheme)),
        ] {
            assert_eq!(parse_path(input), expected, "{input:?}");
        }
    }

    #[test]
    fn raw_danger_table() {
        let high = |e: &str| RawDanger::High {
            expected: e.to_string(),
        };
        use RawDanger::{Medium, None};
        for (method, path, not_prod, prod) in [
            (Method::GET, "keys", None, None),
            (Method::HEAD, "collections/a", None, None),
            (Method::POST, "multi_search", None, None),
            (Method::DELETE, "collections/a", high("a"), high("a")),
            (
                Method::DELETE,
                "collections/a%20b",
                high("a b"),
                high("a b"),
            ),
            (
                Method::DELETE,
                "collections/a/documents",
                high("a"),
                high("a"),
            ),
            (
                Method::DELETE,
                "collections/a/%64ocuments",
                high("a"),
                high("a"),
            ),
            (
                Method::DELETE,
                "collections/a/documents/7",
                Medium,
                high("7"),
            ),
            (Method::DELETE, "keys/12", Medium, high("12")),
            (Method::DELETE, "aliases/a%20b", Medium, high("a b")),
            (
                Method::POST,
                "operations/db/compact",
                Medium,
                high("compact"),
            ),
            (
                Method::POST,
                "operations/snapshot",
                Medium,
                high("snapshot"),
            ),
            (Method::POST, "operations", None, Medium),
            (Method::POST, "collections", None, Medium),
            (Method::PUT, "presets/a", None, Medium),
            (Method::PUT, "multi_search", None, Medium),
            (Method::PATCH, "collections/a", None, Medium),
        ] {
            assert_eq!(
                raw_danger(&method, path, false),
                not_prod,
                "{method} {path}"
            );
            assert_eq!(
                raw_danger(&method, path, true),
                prod,
                "{method} {path} on prod"
            );
        }
    }

    #[test]
    fn outcome_note_finds_failed_import_lines() {
        let mut lines = vec![r#"{"success":true}"#; 38];
        lines.extend([r#"{"success":false,"error":"x"}"#; 2]);
        let note = outcome_note("collections/c/documents/import", 200, &lines.join("\n"));
        assert_eq!(
            note,
            Some(OutcomeNote {
                failed: 2,
                total: 40
            })
        );
        let good = r#"{"success":true}"#.to_string() + "\n";
        assert_eq!(
            outcome_note("collections/c/documents/import", 200, &good),
            Option::None
        );
    }

    #[test]
    fn outcome_note_reads_multi_search() {
        let multi = r#"{"results":[{"hits":[]},{"code":404,"error":"nope"}]}"#;
        assert_eq!(
            outcome_note("multi_search", 200, multi),
            Some(OutcomeNote {
                failed: 1,
                total: 2
            })
        );
        let union = r#"{"code":400,"error":"nope"}"#;
        assert_eq!(
            outcome_note("multi_search", 200, union),
            Some(OutcomeNote {
                failed: 1,
                total: 1
            })
        );
        assert_eq!(
            outcome_note("multi_search", 200, r#"{"results":[{"hits":[]}]}"#),
            Option::None
        );
    }

    #[test]
    fn cap_body_cuts_on_a_char_boundary() {
        let mut body = "éé".to_string();
        assert_eq!(cap_body(&mut body, 3), Some(4));
        assert_eq!(body, "é");
        assert_eq!(cap_body(&mut body, 2), None);
        assert_eq!(body, "é");
    }

    #[test]
    fn outcome_note_is_silent_elsewhere() {
        let body = r#"{"success":false}"#;
        assert_eq!(outcome_note("collections", 200, body), Option::None);
        assert_eq!(
            outcome_note("collections/c/documents/import", 400, body),
            Option::None
        );
        assert_eq!(outcome_note("multi_search", 200, "not json"), Option::None);
    }

    #[test]
    fn param_key_ok_table() {
        for key in ["q", "filter_by", "a.b", "per-page"] {
            assert!(param_key_ok(key), "{key}");
        }
        for key in [
            "x-typesense-api-key",
            "X-TYPESENSE-API-KEY",
            "x_typesense_api_key",
            "api_key",
            "a b",
            "a=b",
            "a&b",
            "a%3Db",
            "",
        ] {
            assert!(!param_key_ok(key), "{key:?}");
        }
    }

    #[test]
    fn query_rows_read_what_raw_query_wrote() {
        let pairs = vec![
            ("q".to_string(), "a&b c".to_string()),
            ("flag".to_string(), String::new()),
        ];
        let query = raw_query(&pairs).unwrap().unwrap();
        assert_eq!(query_rows(&query), pairs);
        assert!(query_rows("").is_empty());
    }

    #[test]
    fn raw_query_encodes_values_and_refuses_keys() {
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(raw_query(&[]), Ok(Option::None));
        assert_eq!(
            raw_query(&[pair("q", "a&b=c d"), pair("page", "2")]),
            Ok(Some("q=a%26b%3Dc%20d&page=2".to_string()))
        );
        assert_eq!(
            raw_query(&[pair("q", "a"), pair("X-TYPESENSE-API-KEY", "x")]),
            Err("X-TYPESENSE-API-KEY".to_string())
        );
    }
}
