// SPDX-License-Identifier: GPL-3.0-or-later

//! Merge patches and diffs between two versions of a document.

use serde_json::{Map, Value};

/// The RFC 7386 merge patch that turns `old` into `new`: changed or added keys carry their new
/// value, removed keys carry `null`, nested objects only carry what changed, arrays and
/// scalars are replaced whole. `id` is never included.
pub fn merge_patch(old: &Map<String, Value>, new: &Map<String, Value>) -> Map<String, Value> {
    patch(old, new, true)
}

// `top` marks the document level, the only place `id` is special.
fn patch(old: &Map<String, Value>, new: &Map<String, Value>, top: bool) -> Map<String, Value> {
    let keep = |k: &&String| !(top && *k == "id");
    let mut out = Map::new();
    for key in old.keys().filter(keep).filter(|k| !new.contains_key(*k)) {
        out.insert(key.clone(), Value::Null);
    }
    for (key, value) in new.iter().filter(|(k, _)| keep(k)) {
        match (old.get(key), value) {
            (Some(Value::Object(before)), Value::Object(after)) => {
                let nested = patch(before, after, false);
                if !nested.is_empty() {
                    out.insert(key.clone(), Value::Object(nested));
                }
            }
            (Some(before), _) if before == value => {}
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

/// One changed leaf between two documents.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffEntry {
    /// Dotted path, for example `author.age`.
    pub path: String,
    /// The value before, `None` when the key was added.
    pub old: Option<Value>,
    /// The value after, `None` when the key was removed.
    pub new: Option<Value>,
}

/// One entry per changed leaf, sorted by path. Empty when the documents are equal.
pub fn diff_entries(old: &Map<String, Value>, new: &Map<String, Value>) -> Vec<DiffEntry> {
    let mut out = Vec::new();
    walk("", old, new, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn walk(
    prefix: &str,
    old: &Map<String, Value>,
    new: &Map<String, Value>,
    out: &mut Vec<DiffEntry>,
) {
    let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match (old.get(key), new.get(key)) {
            (Some(Value::Object(a)), Some(Value::Object(b))) => walk(&path, a, b, out),
            (a, b) if a == b => {}
            (a, b) => out.push(DiffEntry {
                path,
                old: a.cloned(),
                new: b.cloned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn patch(old: Value, new: Value) -> Value {
        Value::Object(merge_patch(&obj(old), &obj(new)))
    }

    #[test]
    fn patch_scalars_and_keys() {
        assert_eq!(
            patch(json!({"a":1,"b":2}), json!({"a":5,"b":2})),
            json!({"a":5})
        );
        assert_eq!(patch(json!({"a":1}), json!({"a":1,"b":2})), json!({"b":2}));
        assert_eq!(
            patch(json!({"a":1,"b":2}), json!({"a":1})),
            json!({"b":null})
        );
        assert_eq!(patch(json!({"a":1}), json!({"a":1})), json!({}));
    }

    #[test]
    fn patch_nested() {
        let old = json!({"a":{"b":1,"c":2}});
        assert_eq!(
            patch(old.clone(), json!({"a":{"b":9,"c":2}})),
            json!({"a":{"b":9}})
        );
        assert_eq!(
            patch(old.clone(), json!({"a":{"c":2}})),
            json!({"a":{"b":null}})
        );
        assert_eq!(patch(old.clone(), json!({})), json!({"a":null}));
        assert_eq!(patch(old, json!({"a":3})), json!({"a":3}));
    }

    #[test]
    fn patch_arrays_whole_and_id_never() {
        assert_eq!(
            patch(json!({"t":[1,2]}), json!({"t":[1,3]})),
            json!({"t":[1,3]})
        );
        assert_eq!(
            patch(json!({"id":"a","x":1}), json!({"id":"b","x":1})),
            json!({})
        );
        assert_eq!(patch(json!({"id":"a"}), json!({})), json!({}));
        // Below the top level `id` is an ordinary key.
        assert_eq!(
            patch(json!({"o":{"id":1}}), json!({"o":{"id":2}})),
            json!({"o":{"id":2}})
        );
    }

    #[test]
    fn diff_paths_sorted() {
        let old = obj(json!({"z":1,"a":{"b":1,"c":2},"gone":true}));
        let new = obj(json!({"z":2,"a":{"b":1,"c":3,"d":4},"new":"x"}));
        let d = diff_entries(&old, &new);
        let paths: Vec<_> = d.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["a.c", "a.d", "gone", "new", "z"]);
        assert_eq!(d[1].old, None);
        assert_eq!(d[1].new, Some(json!(4)));
        assert_eq!(d[2].new, None);
        assert_eq!(d[2].old, Some(json!(true)));
        assert!(diff_entries(&old, &old).is_empty());
    }
}
