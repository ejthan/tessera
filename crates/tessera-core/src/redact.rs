// SPDX-License-Identifier: GPL-3.0-or-later

//! Masking of secrets in request and response bodies.

use serde_json::Value;

const MASK: &str = "••••";
const SECRET_KEYS: [&str; 2] = ["api_key", "value"];

/// Masks the string values of the keys `api_key` and `value` at any depth.
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

fn mask_value(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                if SECRET_KEYS.contains(&k.as_str()) && child.is_string() {
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
        if SECRET_KEYS.contains(&name)
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
