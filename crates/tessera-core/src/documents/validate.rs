// SPDX-License-Identifier: GPL-3.0-or-later

//! Advisory checks of a document against its collection schema (spec D27).

use serde_json::Value;
use tessera_client::{Collection, Field};

/// How serious an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    /// The server will refuse the document.
    Error,
    /// The server coerces the value.
    Warning,
    /// Worth knowing, harmless.
    Info,
}

/// One finding about a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocIssue {
    /// Dotted path of the value, with `[i]` for an array element.
    pub path: String,
    /// Message key: `document.missing_required`, `document.wrong_type`,
    /// `document.wrong_length`, `document.id_not_string` or `document.not_in_schema`.
    pub key: &'static str,
    /// Severity.
    pub severity: IssueSeverity,
    /// The expected type or length.
    pub detail: String,
}

fn issue(path: &str, key: &'static str, severity: IssueSeverity, detail: &str) -> DocIssue {
    DocIssue {
        path: path.into(),
        key,
        severity,
        detail: detail.into(),
    }
}

fn lookup<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(doc, |v, part| v.get(part))
}

/// `Some(Warning)` for a numeric string a numeric field coerces, `Some(Error)` for any other
/// mismatch, `None` when `v` fits the scalar type `ty`.
fn scalar_mismatch(ty: &str, v: &Value) -> Option<IssueSeverity> {
    let ok = match ty {
        "string" => v.is_string(),
        "bool" => v.is_boolean(),
        "int32" => v.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        "int64" => v.is_i64() || v.as_u64().is_some_and(|n| i64::try_from(n).is_ok()),
        "float" => v.is_number(),
        "geopoint" => v
            .as_array()
            .is_some_and(|a| a.len() == 2 && a.iter().all(Value::is_number)),
        "object" => v.is_object(),
        // `auto`, `string*`, `image` and unknown types accept anything.
        _ => true,
    };
    if ok {
        return None;
    }
    let coerced = match (ty, v.as_str()) {
        ("int32", Some(s)) => s.parse::<i32>().is_ok(),
        ("int64", Some(s)) => s.parse::<i64>().is_ok(),
        ("float", Some(s)) => s.parse::<f64>().is_ok(),
        _ => false,
    };
    Some(if coerced {
        IssueSeverity::Warning
    } else {
        IssueSeverity::Error
    })
}

fn check_value(f: &Field, path: &str, v: &Value, out: &mut Vec<DocIssue>) {
    let ty = f.r#type.as_str();
    if v.is_null() {
        return;
    }
    // A `geopoint[]` is an array of pairs; any other `x[]` an array of `x`.
    let Some(element) = ty.strip_suffix("[]") else {
        if let Some(s) = scalar_mismatch(ty, v) {
            out.push(issue(path, "document.wrong_type", s, ty));
        }
        return;
    };
    let Some(items) = v.as_array() else {
        out.push(issue(path, "document.wrong_type", IssueSeverity::Error, ty));
        return;
    };
    if let Some(dim) = f.attr("num_dim").and_then(Value::as_u64)
        && items.len() as u64 != dim
    {
        out.push(issue(
            path,
            "document.wrong_length",
            IssueSeverity::Error,
            &dim.to_string(),
        ));
    }
    let bad = items
        .iter()
        .enumerate()
        .find_map(|(i, item)| scalar_mismatch(element, item).map(|s| (i, s)));
    if let Some((i, s)) = bad {
        out.push(issue(
            &format!("{path}[{i}]"),
            "document.wrong_type",
            s,
            element,
        ));
    }
}

/// Checks `doc` against `c`. Advisory: the server decides.
pub fn validate_document(c: &Collection, doc: &Value) -> Vec<DocIssue> {
    let mut out = Vec::new();
    if !doc.is_object() {
        return out;
    }
    if doc.get("id").is_some_and(|id| !id.is_string()) {
        out.push(issue(
            "id",
            "document.id_not_string",
            IssueSeverity::Error,
            "string",
        ));
    }
    for f in c
        .fields
        .iter()
        .filter(|f| f.name != "id" && !f.name.contains('*'))
    {
        match lookup(doc, &f.name) {
            Some(v) if !v.is_null() => check_value(f, &f.name, v, &mut out),
            _ => {
                let parent_present = f
                    .name
                    .rsplit_once('.')
                    .is_none_or(|(parent, _)| lookup(doc, parent).is_some());
                if !f.flag("optional") && f.attr("embed").is_none() && parent_present {
                    out.push(issue(
                        &f.name,
                        "document.missing_required",
                        IssueSeverity::Error,
                        &f.r#type,
                    ));
                }
            }
        }
    }
    if !c.fields.iter().any(|f| f.name == ".*")
        && let Some(map) = doc.as_object()
    {
        for key in map.keys().filter(|k| *k != "id") {
            let known = c
                .fields
                .iter()
                .any(|f| f.name.split('.').next() == Some(key.as_str()));
            if !known {
                out.push(issue(
                    key,
                    "document.not_in_schema",
                    IssueSeverity::Info,
                    "",
                ));
            }
        }
    }
    out
}

struct Frame {
    key: Option<String>,
    is_object: bool,
}

/// Where the key at dotted `path` sits in the JSON `text`: 0-based line, then the columns (in
/// characters) of its opening quote and just past its closing quote. `None` when the key is
/// absent. Keys inside arrays are never matched.
pub fn key_position(text: &str, path: &str) -> Option<(u32, u32, u32)> {
    let mut stack: Vec<Frame> = Vec::new();
    let (mut line, mut col) = (0u32, 0u32);
    let mut in_string = false;
    let mut escaped = false;
    let mut current = String::new();
    // A finished string that may turn out to be a key: text, line, start and end column.
    let mut pending: Option<(String, u32, u32, u32)> = None;
    let mut start = (0, 0);
    for ch in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
                current.push(ch);
            } else if ch == '\\' {
                escaped = true;
                current.push(ch);
            } else if ch == '"' {
                in_string = false;
                pending = Some((std::mem::take(&mut current), start.0, start.1, col + 1));
            } else {
                current.push(ch);
            }
        } else if !ch.is_whitespace() {
            let candidate = pending.take();
            match ch {
                '"' => {
                    in_string = true;
                    start = (line, col);
                }
                ':' if stack.last().is_some_and(|f| f.is_object) => {
                    if let Some((key, l, c0, c1)) = candidate {
                        let n = stack.len();
                        stack[n - 1].key = Some(key.clone());
                        let keys: Option<Vec<&str>> = stack
                            .iter()
                            .map(|f| f.key.as_deref().filter(|_| f.is_object))
                            .collect();
                        if keys.is_some_and(|k| k.join(".") == path) {
                            return Some((l, c0, c1));
                        }
                    }
                }
                '{' => stack.push(Frame {
                    key: None,
                    is_object: true,
                }),
                '[' => stack.push(Frame {
                    key: None,
                    is_object: false,
                }),
                '}' | ']' => {
                    stack.pop();
                }
                _ => {}
            }
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn c(fields: Value) -> Collection {
        serde_json::from_value(json!({"name":"c","fields":fields})).unwrap()
    }

    fn check(fields: Value, doc: Value) -> Vec<(String, &'static str, IssueSeverity)> {
        validate_document(&c(fields), &doc)
            .into_iter()
            .map(|i| (i.path, i.key, i.severity))
            .collect()
    }

    use IssueSeverity::{Error, Info, Warning};

    #[test]
    fn required_and_optional() {
        let f = json!([{"name":"a","type":"string"},{"name":"b","type":"string","optional":true}]);
        assert_eq!(
            check(f.clone(), json!({})),
            [("a".into(), "document.missing_required", Error)]
        );
        assert!(check(f, json!({"a":"x"})).is_empty());
    }

    #[test]
    fn scalar_types() {
        let wrong = |ty: &str, v: Value| check(json!([{"name":"a","type":ty}]), json!({"a":v}));
        assert_eq!(wrong("int32", json!("x"))[0].2, Error);
        assert_eq!(wrong("int32", json!("12"))[0].2, Warning);
        assert_eq!(wrong("bool", json!("true"))[0].2, Error);
        assert_eq!(wrong("int32", json!(3_000_000_000_i64))[0].2, Error);
        assert_eq!(wrong("int32", json!(1.5))[0].2, Error);
        assert!(wrong("float", json!(3)).is_empty());
        assert!(wrong("auto", json!({"x":1})).is_empty());
        assert!(wrong("string*", json!(5)).is_empty());
        assert!(wrong("geopoint", json!([1.0, 2.0])).is_empty());
        assert_eq!(wrong("geopoint", json!([1.0]))[0].1, "document.wrong_type");
    }

    #[test]
    fn arrays_and_vectors() {
        let arr = check(
            json!([{"name":"t","type":"string[]"}]),
            json!({"t":["a",1]}),
        );
        assert_eq!(arr, [("t[1]".into(), "document.wrong_type", Error)]);
        let vec_f = json!([{"name":"v","type":"float[]","num_dim":3}]);
        let got = validate_document(&c(vec_f.clone()), &json!({"v":[1.0, 2.0]}));
        assert_eq!(
            (got[0].key, got[0].detail.as_str()),
            ("document.wrong_length", "3")
        );
        assert!(check(vec_f, json!({"v":[1,2,3]})).is_empty());
    }

    #[test]
    fn id_schema_and_nested() {
        let f = json!([{"name":"author","type":"object"},{"name":"author.age","type":"int32"}]);
        assert_eq!(
            check(f.clone(), json!({"id":5,"author":{"age":"x"}})),
            [
                ("id".into(), "document.id_not_string", Error),
                ("author.age".into(), "document.wrong_type", Error)
            ]
        );
        assert_eq!(
            check(f.clone(), json!({"author":{"age":1},"extra":1})),
            [("extra".into(), "document.not_in_schema", Info)]
        );
        let mut wild = f.as_array().unwrap().clone();
        wild.push(json!({"name":".*","type":"auto"}));
        assert!(check(json!(wild), json!({"author":{"age":1},"extra":1})).is_empty());
        // Only `.*` is a catch-all; a narrower pattern still leaves other keys unknown.
        let narrow = json!([{"name":"a","type":"string"},{"name":"num_.*","type":"int32"}]);
        assert_eq!(
            check(narrow, json!({"a":"x","extra":1})),
            [("extra".into(), "document.not_in_schema", Info)]
        );
    }

    #[test]
    fn embed_field_not_required() {
        let f = json!([{"name":"e","type":"float[]","embed":{"from":["s"]}}]);
        assert!(check(f, json!({})).is_empty());
    }

    #[test]
    fn key_positions() {
        let text = "{\n  \"a\": {\"b\": 1, \"c\": \"\\\"b\\\": x\"},\n  \"d\": [{\"e\": 1}],\n  \"f\" : 2\n}";
        assert_eq!(key_position(text, "a"), Some((1, 2, 5)));
        assert_eq!(key_position(text, "a.b"), Some((1, 8, 11)));
        // A key-looking text inside a string value, with escaped quotes, is no key.
        assert_eq!(key_position(text, "a.c"), Some((1, 16, 19)));
        assert_eq!(key_position(text, "b"), None);
        assert_eq!(key_position(text, "a.\"b\""), None);
        // Keys in arrays are not matched, and a key after nested containers is.
        assert_eq!(key_position(text, "d.e"), None);
        assert_eq!(key_position(text, "f"), Some((3, 2, 5)));
        assert_eq!(key_position(text, "a.zzz"), None);
    }
}
