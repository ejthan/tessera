// SPDX-License-Identifier: GPL-3.0-or-later

//! Table columns derived from a collection schema and the documents seen.

use tessera_client::{Collection, Field};

/// One table column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    /// Top-level document key.
    pub key: String,
    /// Whether the server can sort by it.
    pub sortable: bool,
    /// Right-aligned.
    pub numeric: bool,
    /// Declared in the schema (otherwise only seen in documents).
    pub in_schema: bool,
}

pub(crate) fn is_vector(f: &Field) -> bool {
    f.r#type == "float[]" && (f.attr("num_dim").is_some() || f.attr("embed").is_some())
}

pub(crate) fn is_sortable(f: &Field) -> bool {
    matches!(
        f.r#type.as_str(),
        "int32" | "int64" | "float" | "bool" | "string"
    ) && f.flag("sort")
        && f.flag("index")
}

/// `id` first, then the schema's top-level keys in schema order.
pub fn columns_from_schema(c: &Collection) -> Vec<ColumnDef> {
    let mut cols = vec![ColumnDef {
        key: "id".into(),
        sortable: false,
        numeric: false,
        in_schema: true,
    }];
    for f in &c.fields {
        if f.name.contains('*') || f.name == "id" || is_vector(f) {
            continue;
        }
        let key = f.name.split('.').next().unwrap_or(&f.name);
        // A nested field `a.b` collapses into `a`, which cannot be sorted by.
        let sortable = key == f.name && is_sortable(f);
        if cols.iter().any(|col| col.key == key) {
            continue;
        }
        cols.push(ColumnDef {
            key: key.to_string(),
            sortable,
            numeric: matches!(f.r#type.as_str(), "int32" | "int64" | "float"),
            in_schema: true,
        });
    }
    cols
}

/// Appends a column for each key not yet present; true when one was added.
pub fn merge_keys(columns: &mut Vec<ColumnDef>, keys: &[String]) -> bool {
    let before = columns.len();
    for key in keys {
        if !columns.iter().any(|c| &c.key == key) {
            columns.push(ColumnDef {
                key: key.clone(),
                sortable: false,
                numeric: false,
                in_schema: false,
            });
        }
    }
    columns.len() > before
}

/// Comma-separated vector fields, to keep embeddings out of pages.
pub fn exclude_fields(c: &Collection) -> Option<String> {
    let names: Vec<&str> = c
        .fields
        .iter()
        .filter(|f| is_vector(f))
        .map(|f| f.name.as_str())
        .collect();
    (!names.is_empty()).then(|| names.join(","))
}

/// `price:desc` style `sort_by` value.
pub fn sort_key(column: &str, ascending: bool) -> String {
    format!("{column}:{}", if ascending { "asc" } else { "desc" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn coll(fields: Value) -> Collection {
        serde_json::from_value(json!({"name": "c", "fields": fields})).expect("collection")
    }

    fn keys(c: &Collection) -> Vec<String> {
        columns_from_schema(c).into_iter().map(|c| c.key).collect()
    }

    #[test]
    fn id_comes_first_then_schema_order() {
        let c = coll(json!([
            {"name": "title", "type": "string"},
            {"name": "price", "type": "float"},
        ]));
        assert_eq!(keys(&c), ["id", "title", "price"]);
    }

    #[test]
    fn nested_fields_collapse_to_one_column() {
        let c = coll(json!([
            {"name": "author", "type": "object"},
            {"name": "author.name", "type": "string"},
        ]));
        assert_eq!(keys(&c), ["id", "author"]);
        let c = coll(json!([{"name": "author.name", "type": "string", "sort": true}]));
        let cols = columns_from_schema(&c);
        assert_eq!(cols[1].key, "author");
        assert!(!cols[1].sortable);
    }

    #[test]
    fn wildcards_make_no_column() {
        let c = coll(json!([
            {"name": ".*", "type": "auto"},
            {"name": "tag_.*", "type": "string"},
        ]));
        assert_eq!(keys(&c), ["id"]);
    }

    #[test]
    fn vector_fields_make_no_column_and_are_excluded() {
        let c = coll(json!([
            {"name": "embedding", "type": "float[]", "num_dim": 4},
            {"name": "other", "type": "float[]", "embed": {"from": ["t"]}},
            {"name": "t", "type": "string"},
        ]));
        assert_eq!(keys(&c), ["id", "t"]);
        assert_eq!(exclude_fields(&c).as_deref(), Some("embedding,other"));
        assert_eq!(
            exclude_fields(&coll(json!([{"name": "t", "type": "string"}]))),
            None
        );
    }

    #[test]
    fn plain_float_array_stays() {
        let c = coll(json!([{"name": "scores", "type": "float[]"}]));
        assert_eq!(keys(&c), ["id", "scores"]);
        assert_eq!(exclude_fields(&c), None);
    }

    #[test]
    fn sortable_table() {
        let cases = [
            (json!({"name": "a", "type": "string", "sort": true}), true),
            (json!({"name": "a", "type": "string", "sort": false}), false),
            (json!({"name": "a", "type": "int32", "sort": true}), true),
            (json!({"name": "a", "type": "bool", "sort": true}), true),
            (
                json!({"name": "a", "type": "geopoint", "sort": true}),
                false,
            ),
            (
                json!({"name": "a", "type": "string[]", "sort": true}),
                false,
            ),
            (
                json!({"name": "a", "type": "string", "sort": true, "index": false}),
                false,
            ),
        ];
        for (f, want) in cases {
            let c = coll(json!([f.clone()]));
            assert_eq!(columns_from_schema(&c)[1].sortable, want, "{f}");
        }
        assert!(!columns_from_schema(&coll(json!([])))[0].sortable);
    }

    #[test]
    fn merge_keys_appends_unknown_and_reports() {
        let mut cols = columns_from_schema(&coll(json!([])));
        let keys = vec!["extra".to_string(), "id".to_string()];
        assert!(merge_keys(&mut cols, &keys));
        assert!(!merge_keys(&mut cols, &keys));
        assert_eq!(cols.len(), 2);
        assert!(!cols[1].in_schema);
    }

    #[test]
    fn sort_key_gives_direction() {
        assert_eq!(sort_key("price", false), "price:desc");
        assert_eq!(sort_key("price", true), "price:asc");
    }
}
