// SPDX-License-Identifier: GPL-3.0-or-later

//! Masking of secrets in request and response bodies.

use serde_json::Value;

/// What a masked secret reads as.
pub const MASK: &str = "••••";
const SECRET_KEYS: [&str; 5] = [
    "api_key",
    "value",
    "access_token",
    "refresh_token",
    "client_secret",
];

/// Whether a request parameter of this name would hold a key: `api_key`, `x-typesense-api-key`,
/// `scoped_key` and the like, or a name in `SECRET_KEYS`, in any case and with `-` or `_`.
pub fn is_secret_param(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('-', "_");
    ["api_key", "apikey", "scoped_key"]
        .iter()
        .any(|part| name.contains(part))
        || SECRET_KEYS.contains(&name.as_str())
}

/// Whether `text` holds a mask and so must not be sent: at a value of JSON, or anywhere in text
/// that is not JSON, such as JSONL.
pub fn text_has_mask(text: &str) -> bool {
    serde_json::from_str(text).map_or_else(|_| text.contains(MASK), |v| contains_mask(&v))
}

/// Masks the string values of the keys [`is_secret_param`] names (`api_key`, `value`, the
/// embedding model credentials) at any depth.
///
/// Valid JSON comes back pretty-printed. Text that is not valid JSON (for example a body
/// cut at 1 MiB) is masked textually and otherwise left byte-identical.
pub fn json(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(mut v) => {
            mask_value(&mut v);
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| mask_text(text))
        }
        Err(_) => mask_text(text),
    }
}

/// The display form of a key prefix, for example `abcd••••`.
pub fn key(prefix: &str) -> String {
    format!("{prefix}{MASK}")
}

/// A query string with the value of every parameter [`is_secret_param`] names masked. Typesense
/// accepts the key as a query parameter too.
pub fn query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if is_secret_query_name(name) => format!("{name}={MASK}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Compares the percent-decoded name, so `x%2Dtypesense-api-key` counts too.
fn is_secret_query_name(name: &str) -> bool {
    url::form_urlencoded::parse(name.as_bytes())
        .next()
        .is_some_and(|(k, _)| is_secret_param(&k))
}

/// Free text (for example an error message) with the query of every URL in it passed through
/// [`query`]. A URL ends at whitespace, a quote, a parenthesis or an angle bracket.
pub fn text_query_params(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        match run.split_once('?') {
            Some((head, q)) => out.push_str(&format!("{head}?{}", query(q))),
            None => out.push_str(run),
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() || "()\"'<>".contains(c) {
            flush(&mut run, &mut out);
            out.push(c);
        } else {
            run.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Masks the secret string values in `v` in place, at any depth (for display of a parsed body).
pub fn mask_value(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                if is_secret_param(k) && child.is_string() {
                    *child = Value::String(MASK.into());
                } else {
                    mask_value(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_value),
        _ => {}
    }
}

/// True when a string in `v`, at any depth, is the mask [`mask_value`] puts in: a value
/// that came from a masked view and must be entered again before it is sent.
pub fn contains_mask(v: &Value) -> bool {
    match v {
        Value::String(s) => s == MASK,
        Value::Object(map) => map.values().any(contains_mask),
        Value::Array(items) => items.iter().any(contains_mask),
        _ => false,
    }
}

/// Byte index just past the string literal that starts at `start` (a `"`), or the end of
/// the text if it is unterminated.
fn string_end(b: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Scanner for text that does not parse as JSON: replaces the string after a secret key.
fn mask_text(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        if b[i] != b'"' {
            i += 1;
            continue;
        }
        let end = string_end(b, i);
        let name = text.get(i + 1..end.saturating_sub(1)).unwrap_or("");
        let colon = skip_ws(b, end);
        let val = skip_ws(b, colon + 1);
        if is_secret_param(name)
            && b.get(end - 1) == Some(&b'"')
            && b.get(colon) == Some(&b':')
            && b.get(val) == Some(&b'"')
        {
            let val_end = string_end(b, val);
            out.push_str(&text[copied..val]);
            out.push('"');
            out.push_str(MASK);
            out.push('"');
            copied = val_end;
            i = val_end;
        } else {
            i = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_masks_nested_api_key_and_value() {
        let out = json(r#"{"a":{"api_key":"sk"},"keys":[{"value":"abc"}]}"#);
        assert!(!out.contains("sk\"") && !out.contains("abc"));
        assert_eq!(out.matches(MASK).count(), 2);
    }

    #[test]
    fn text_has_mask_reads_json_and_jsonl() {
        assert!(text_has_mask(r#"{"a":{"api_key":"••••"}}"#));
        assert!(text_has_mask("{\"api_key\":\"••••\"}\n{\"n\":1}"));
        assert!(!text_has_mask(r#"{"q":"a"}"#));
        assert!(!text_has_mask("{\"n\":1}\n{\"n\":2}"));
    }

    #[test]
    fn contains_mask_finds_a_masked_value_at_any_depth() {
        let mut v: Value =
            serde_json::from_str(r#"{"a":[{"model_config":{"api_key":"sk"}}]}"#).unwrap();
        assert!(!contains_mask(&v));
        mask_value(&mut v);
        assert!(contains_mask(&v));
        assert!(!contains_mask(&serde_json::json!({"k": "•••", "n": 1})));
    }

    const EMBED: &str = r#"{"name":"emb","type":"float[]","embed":{"from":["title"],"model_config":{"model_name":"openai/text-embedding-3-small","api_key":"sk-embed-1234","access_token":"at-5678","refresh_token":"rt-9012","client_secret":"cs-3456"}}}"#;
    const EMBED_SECRETS: [&str; 4] = ["sk-embed-1234", "at-5678", "rt-9012", "cs-3456"];

    #[test]
    fn embedding_credentials_are_masked() {
        let body = format!(r#"{{"name":"c","fields":[{EMBED}]}}"#);
        let out = json(&body);
        assert!(EMBED_SECRETS.iter().all(|s| !out.contains(s)), "{out}");
        let v: Value = serde_json::from_str(&out).expect("json");
        let cfg = &v["fields"][0]["embed"]["model_config"];
        for k in ["api_key", "access_token", "refresh_token", "client_secret"] {
            assert_eq!(cfg[k], MASK, "{k}");
        }
        assert_eq!(cfg["model_name"], "openai/text-embedding-3-small");
    }

    #[test]
    fn embedding_credentials_are_masked_in_truncated_text() {
        let out = json(&EMBED[..EMBED.len() - 2]);
        assert!(EMBED_SECRETS.iter().all(|s| !out.contains(s)), "{out}");
    }

    #[test]
    fn query_masks_the_key_parameter_in_any_case() {
        assert_eq!(
            query("q=dune&X-TYPESENSE-API-KEY=sk-live&x-typesense-api-key=s2&k=v"),
            "q=dune&X-TYPESENSE-API-KEY=••••&x-typesense-api-key=••••&k=v"
        );
        assert_eq!(query("q=a=b&flag"), "q=a=b&flag");
    }

    #[test]
    fn query_masks_every_secret_parameter() {
        assert_eq!(
            query("api_key=a&SCOPED_KEY=b&x_typesense_api_key=c&a%70i_key=d&q=e"),
            "api_key=••••&SCOPED_KEY=••••&x_typesense_api_key=••••&a%70i_key=••••&q=e"
        );
    }

    #[test]
    fn json_masks_every_secret_parameter() {
        let body = r#"{"x-typesense-api-key":"a","s":[{"scoped_key":"b"}],"q":"keep"}"#;
        let out = json(body);
        assert_eq!(out.matches(MASK).count(), 2, "{out}");
        assert!(out.contains("keep"));
        let cut = mask_text(&body[..body.len() - 2]);
        assert_eq!(cut.matches(MASK).count(), 2, "{cut}");
    }

    #[test]
    fn query_masks_a_percent_encoded_parameter_name() {
        assert_eq!(
            query("x%2Dtypesense-api-key=sk&a=b"),
            "x%2Dtypesense-api-key=••••&a=b"
        );
    }

    #[test]
    fn text_masks_the_key_in_urls_inside_an_error() {
        let out = text_query_params(
            "error sending request for url (http://h:1/x?a=b&x-typesense-api-key=sk-live): refused",
        );
        assert_eq!(
            out,
            "error sending request for url (http://h:1/x?a=b&x-typesense-api-key=••••): refused"
        );
        assert_eq!(text_query_params("no url here?"), "no url here?");
    }

    #[test]
    fn redact_keeps_non_json() {
        assert_eq!(json("plain text, no json"), "plain text, no json");
    }

    #[test]
    fn redact_masks_secrets_in_truncated_json() {
        let out = json(r#"{"keys":[{"value":"abc"},{"value":"de"#);
        assert!(!out.contains("abc") && !out.contains("de\"") && !out.contains(":\"de"));
        assert!(out.ends_with(r#"{"value":"••••""#));
    }

    #[test]
    fn redact_text_honors_escapes_and_spacing() {
        let out = json(r#"{"api_key" : "a\"b", "x": [1,"#);
        assert_eq!(out, r#"{"api_key" : "••••", "x": [1,"#);
    }

    #[test]
    fn key_appends_mask() {
        assert_eq!(key("abcd"), "abcd••••");
        assert_eq!(key(""), "••••");
    }
}
