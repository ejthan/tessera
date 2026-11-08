// SPDX-License-Identifier: GPL-3.0-or-later

//! A starting document for a collection.

use serde_json::{Map, Value, json};
use tessera_client::{Collection, Field};

fn placeholder(ty: &str) -> Value {
    match ty {
        "int32" | "int64" => json!(0),
        "float" => json!(0.0),
        "bool" => json!(false),
        "object" => json!({}),
        "geopoint" => json!([0.0, 0.0]),
        t if t.ends_with("[]") => json!([]),
        _ => json!(""),
    }
}

fn wanted(f: &Field) -> bool {
    f.name != "id" && !f.name.contains('*') && !f.flag("optional") && f.attr("embed").is_none()
}

/// Puts `value` at `path` below `node`, creating objects on the way. An existing value at the
/// leaf stays, so an `object` field listed after its nested fields does not wipe them.
fn insert(node: &mut Map<String, Value>, path: &[&str], value: Value) {
    match path {
        [] => {}
        [leaf] => {
            node.entry(leaf.to_string()).or_insert(value);
        }
        [head, rest @ ..] => {
            let child = node
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(child) = child {
                insert(child, rest, value);
            }
        }
    }
}

/// One key per required field, with a placeholder of its type. `a.b` nests as `{"a":{"b":…}}`.
pub fn template(c: &Collection) -> Value {
    let mut root = Map::new();
    for f in c.fields.iter().filter(|f| wanted(f)) {
        let path: Vec<&str> = f.name.split('.').collect();
        insert(&mut root, &path, placeholder(&f.r#type));
    }
    Value::Object(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::validate::{IssueSeverity, validate_document};

    fn collection(fields: Value) -> Collection {
        serde_json::from_value(json!({"name":"c","fields":fields})).unwrap()
    }

    #[test]
    fn placeholders_per_type() {
        let c = collection(json!([
            {"name":"s","type":"string"},{"name":"i","type":"int32"},{"name":"l","type":"int64"},
            {"name":"f","type":"float"},{"name":"b","type":"bool"},{"name":"t","type":"string[]"},
            {"name":"o","type":"object"},{"name":"g","type":"geopoint"},
            {"name":"opt","type":"string","optional":true},
            {"name":"e","type":"float[]","embed":{"from":["s"]}},
            {"name":"id","type":"string"},{"name":".*","type":"auto"}
        ]));
        let t = template(&c);
        assert_eq!(
            t,
            json!({"s":"","i":0,"l":0,"f":0.0,"b":false,"t":[],"o":{},"g":[0.0,0.0]})
        );
        assert!(
            validate_document(&c, &t)
                .iter()
                .all(|i| i.severity != IssueSeverity::Error)
        );
    }

    #[test]
    fn nested_required_builds_objects() {
        let c = collection(json!([
            {"name":"a.b","type":"string"},{"name":"a","type":"object"}
        ]));
        assert_eq!(template(&c), json!({"a":{"b":""}}));
    }
}
