// SPDX-License-Identifier: GPL-3.0-or-later

//! Turns the text of a CSV cell into a JSON value, by the type of its field.

use serde_json::{Map, Number, Value};

/// `-?(0|[1-9][0-9]*)` that fits `i64`.
fn integer(cell: &str) -> Option<i64> {
    let digits = cell.strip_prefix('-').unwrap_or(cell);
    let plain = digits == "0" || digits.starts_with(|c: char| ('1'..='9').contains(&c));
    (plain && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| cell.parse().ok())
        .flatten()
}

/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` with a fraction or an exponent, finite.
fn decimal(cell: &str) -> Option<f64> {
    let digits_end = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let s = cell.strip_prefix('-').unwrap_or(cell);
    let (int, mut rest) = s.split_at(digits_end(s));
    if int.is_empty() || (int.len() > 1 && int.starts_with('0')) {
        return None;
    }
    let mut marked = false;
    if let Some(after) = rest.strip_prefix('.') {
        let n = digits_end(after);
        if n == 0 {
            return None;
        }
        rest = &after[n..];
        marked = true;
    }
    if let Some(exp) = rest.strip_prefix(['e', 'E']) {
        let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if exp.is_empty() || digits_end(exp) != exp.len() {
            return None;
        }
        rest = "";
        marked = true;
    }
    (marked && rest.is_empty())
        .then(|| cell.parse::<f64>().ok())
        .flatten()
        .filter(|f| f.is_finite())
}

fn number(f: f64) -> Value {
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// A value for a cell of a column the schema does not know: an integer, a float with a dot
/// or an exponent, `true` or `false`, else the text. `007` stays text.
pub fn guess(cell: &str) -> Value {
    if let Some(i) = integer(cell) {
        Value::from(i)
    } else if let Some(f) = decimal(cell) {
        number(f)
    } else {
        match cell {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(cell.to_string()),
        }
    }
}

/// The value of `cell` for a field of type `ty`, or `None` for an empty cell, so the key is
/// left out. A cell that does not fit its type stays text, for the server to judge under
/// `dirty_values`.
pub fn convert(cell: &str, ty: Option<&str>) -> Option<Value> {
    if cell.is_empty() {
        return None;
    }
    let text = || Value::String(cell.to_string());
    Some(match ty {
        None | Some("auto") => guess(cell),
        Some("string" | "string*" | "image") => text(),
        Some("int32" | "int64") => integer(cell).map_or_else(text, Value::from),
        Some("float") => match cell.parse::<f64>() {
            Ok(f) if f.is_finite() => number(f),
            _ => text(),
        },
        Some("bool") => match cell.to_ascii_lowercase().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => text(),
        },
        Some(ty) if ty.ends_with("[]") || matches!(ty, "object" | "geopoint") => {
            serde_json::from_str(cell).unwrap_or_else(|_| text())
        }
        Some(_) => guess(cell),
    })
}

/// Sets `doc["a"]["b"]["c"]` for `path` `a.b.c`, making the objects on the way. A path with
/// an empty part (`a..b`, `.a`) stays one flat key.
pub fn nest(doc: &mut Map<String, Value>, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        doc.insert(path.to_string(), value);
        return;
    }
    let (last, parents) = parts.split_last().expect("split yields one part");
    let mut at = doc;
    for part in parents {
        let entry = at
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        let Value::Object(next) = entry else {
            return;
        };
        at = next;
    }
    at.insert(last.to_string(), value);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn convert_by_type() {
        let cases: &[(&str, Option<&str>, Option<Value>)] = &[
            ("", Some("int32"), None),
            ("", None, None),
            ("hi", Some("string"), Some(json!("hi"))),
            ("12", Some("string"), Some(json!("12"))),
            ("12", Some("string*"), Some(json!("12"))),
            ("12", Some("int32"), Some(json!(12))),
            ("-7", Some("int64"), Some(json!(-7))),
            ("007", Some("int32"), Some(json!("007"))),
            ("1.5", Some("int32"), Some(json!("1.5"))),
            ("9223372036854775807", Some("int64"), Some(json!(i64::MAX))),
            (
                "9223372036854775808",
                Some("int64"),
                Some(json!("9223372036854775808")),
            ),
            ("1.5", Some("float"), Some(json!(1.5))),
            ("1e3", Some("float"), Some(json!(1000.0))),
            ("NaN", Some("float"), Some(json!("NaN"))),
            ("inf", Some("float"), Some(json!("inf"))),
            ("x", Some("float"), Some(json!("x"))),
            ("true", Some("bool"), Some(json!(true))),
            ("TRUE", Some("bool"), Some(json!(true))),
            ("False", Some("bool"), Some(json!(false))),
            ("yes", Some("bool"), Some(json!("yes"))),
            (r#"["a"]"#, Some("string[]"), Some(json!(["a"]))),
            ("x", Some("string[]"), Some(json!("x"))),
            ("[1,2]", Some("int64[]"), Some(json!([1, 2]))),
            (r#"{"a":1}"#, Some("object"), Some(json!({"a": 1}))),
            ("[48.1,11.5]", Some("geopoint"), Some(json!([48.1, 11.5]))),
            ("[[1,2]]", Some("geopoint[]"), Some(json!([[1, 2]]))),
            ("12", Some("auto"), Some(json!(12))),
            ("12", None, Some(json!(12))),
        ];
        for (cell, ty, want) in cases {
            assert_eq!(&convert(cell, *ty), want, "{cell:?} as {ty:?}");
        }
    }

    #[test]
    fn guess_cells() {
        let cases = [
            ("12", json!(12)),
            ("-3", json!(-3)),
            ("1.5", json!(1.5)),
            ("-0.25", json!(-0.25)),
            ("1e3", json!(1000.0)),
            ("2E-2", json!(0.02)),
            ("true", json!(true)),
            ("false", json!(false)),
            ("TRUE", json!("TRUE")),
            ("007", json!("007")),
            ("007.5", json!("007.5")),
            (".5", json!(".5")),
            ("5.", json!("5.")),
            ("1e", json!("1e")),
            ("1e400", json!("1e400")),
            ("NaN", json!("NaN")),
            ("-", json!("-")),
            ("1.2.3", json!("1.2.3")),
            ("9223372036854775808", json!("9223372036854775808")),
            ("abc", json!("abc")),
        ];
        for (cell, want) in cases {
            assert_eq!(guess(cell), want, "{cell:?}");
        }
    }

    #[test]
    fn nest_builds_objects() {
        let mut doc = Map::new();
        nest(&mut doc, "a.b.c", json!(1));
        nest(&mut doc, "a.b.d", json!(2));
        nest(&mut doc, "a.e", json!(3));
        nest(&mut doc, "flat", json!(4));
        nest(&mut doc, "a..x", json!(5));
        assert_eq!(
            Value::Object(doc),
            json!({"a": {"b": {"c": 1, "d": 2}, "e": 3}, "flat": 4, "a..x": 5})
        );
    }

    #[test]
    fn nest_replaces_a_value_in_the_way() {
        let mut doc = Map::new();
        nest(&mut doc, "a", json!(1));
        nest(&mut doc, "a.b", json!(2));
        assert_eq!(Value::Object(doc), json!({"a": {"b": 2}}));
    }
}
