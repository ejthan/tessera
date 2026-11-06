// SPDX-License-Identifier: GPL-3.0-or-later

//! JSONL and CSV text for exported documents, and the bulk-action danger level.

use serde_json::{Map, Value};

use super::CONFIRM_ABOVE;

/// How hard a bulk action's confirmation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Danger {
    /// A plain confirmation.
    Medium,
    /// The user types the collection name.
    High,
}

/// `High` above [`CONFIRM_ABOVE`] documents or on prod, else `Medium`.
pub fn danger_level(count: u64, prod: bool) -> Danger {
    if prod || count > CONFIRM_ABOVE {
        Danger::High
    } else {
        Danger::Medium
    }
}

/// One compact JSON document per line, without a trailing newline.
pub fn to_jsonl(docs: &[Value]) -> String {
    docs.iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Appends `text` as one RFC 4180 cell.
pub(crate) fn csv_field(out: &mut String, text: &str) {
    if text.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(text);
    }
}

/// RFC 4180 CSV with `\n` line ends: `columns` as the header, then one row per document.
/// Strings are raw, other values compact JSON, an absent key an empty field.
pub fn to_csv(columns: &[String], docs: &[Map<String, Value>]) -> String {
    let mut out = String::new();
    let mut line = |cells: &mut dyn Iterator<Item = String>| {
        for (i, cell) in cells.enumerate() {
            if i > 0 {
                out.push(',');
            }
            csv_field(&mut out, &cell);
        }
        out.push('\n');
    };
    line(&mut columns.iter().cloned());
    for doc in docs {
        line(&mut columns.iter().map(|c| match doc.get(c) {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn doc(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn cols(c: &[&str]) -> Vec<String> {
        c.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn jsonl_is_one_line_per_document() {
        assert_eq!(
            to_jsonl(&[json!({"a":1}), json!({"b":"x"})]),
            "{\"a\":1}\n{\"b\":\"x\"}"
        );
        assert_eq!(to_jsonl(&[]), "");
    }

    #[test]
    fn csv_header_follows_the_columns() {
        let out = to_csv(&cols(&["b", "a"]), &[doc(json!({"a":1,"b":"x"}))]);
        assert_eq!(out, "b,a\nx,1\n");
    }

    #[test]
    fn csv_quotes_commas_quotes_and_newlines() {
        let out = to_csv(
            &cols(&["a"]),
            &[
                doc(json!({"a":"x,y"})),
                doc(json!({"a":"say \"hi\""})),
                doc(json!({"a":"l1\nl2"})),
            ],
        );
        assert_eq!(out, "a\n\"x,y\"\n\"say \"\"hi\"\"\"\n\"l1\nl2\"\n");
    }

    #[test]
    fn csv_absent_nested_number_and_bool() {
        let out = to_csv(
            &cols(&["n", "o", "x", "b", "gone"]),
            &[doc(json!({"n":1.5,"o":{"k":[1,2]},"x":3,"b":true}))],
        );
        // The nested value holds a comma, so it is quoted.
        assert_eq!(out, "n,o,x,b,gone\n1.5,\"{\"\"k\"\":[1,2]}\",3,true,\n");
    }

    #[test]
    fn danger_levels() {
        assert_eq!(danger_level(1000, false), Danger::Medium);
        assert_eq!(danger_level(1001, false), Danger::High);
        assert_eq!(danger_level(1, true), Danger::High);
    }
}
