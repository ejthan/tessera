// SPDX-License-Identifier: GPL-3.0-or-later

//! The difference between an existing collection and an edited draft; it drives the PATCH.

use serde_json::{Map, Value, json};
use tessera_client::Collection;

use super::{FieldDraft, SchemaDraft};

/// A field whose definition differs.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    /// Field name.
    pub name: String,
    /// The field as it is now.
    pub before: FieldDraft,
    /// The field as edited.
    pub after: FieldDraft,
}

/// A side effect the user should see before applying the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consequence {
    /// Typesense reindexes this many documents.
    RewritesDocuments(u64),
    /// The server clears `default_sorting_field` of this name.
    ClearsDefaultSortingField(String),
    /// Another collection references a field that is dropped or changed.
    BreaksReference {
        /// The referencing collection.
        collection: String,
        /// Its referencing field.
        field: String,
    },
}

/// What a PATCH would change.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchemaDiff {
    /// Fields only in the draft.
    pub added: Vec<FieldDraft>,
    /// Names of fields only in the collection.
    pub dropped: Vec<String>,
    /// Fields in both whose definition differs.
    pub changed: Vec<FieldChange>,
    /// New synonym sets, when they changed.
    pub synonym_sets: Option<Vec<String>>,
    /// New metadata, when it changed (`Some(None)` removes it).
    pub metadata: Option<Option<Map<String, Value>>>,
    /// Side effects of applying.
    pub consequences: Vec<Consequence>,
}

impl SchemaDiff {
    /// True when there is nothing to send.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.dropped.is_empty()
            && self.changed.is_empty()
            && self.synonym_sets.is_none()
            && self.metadata.is_none()
    }

    /// The PATCH body: drops, then drop+add pairs for changes, then plain adds.
    pub fn patch_body(&self) -> Value {
        let mut fields: Vec<Value> = self
            .dropped
            .iter()
            .map(|n| json!({"name": n, "drop": true}))
            .collect();
        for c in &self.changed {
            fields.push(json!({"name": c.name, "drop": true}));
        }
        fields.extend(self.changed.iter().map(|c| c.after.to_value()));
        fields.extend(self.added.iter().map(FieldDraft::to_value));
        let mut body = Map::new();
        if !fields.is_empty() {
            body.insert("fields".into(), fields.into());
        }
        if let Some(s) = &self.synonym_sets {
            body.insert("synonym_sets".into(), json!(s));
        }
        if let Some(m) = &self.metadata {
            body.insert(
                "metadata".into(),
                m.clone().map_or(Value::Null, Value::Object),
            );
        }
        Value::Object(body)
    }
}

/// Diffs `old` against the edited `new`; only patchable parts are compared.
pub fn diff(old: &Collection, new: &SchemaDraft) -> SchemaDiff {
    let old_draft = SchemaDraft::from_collection(old);
    let mut d = SchemaDiff::default();
    for f in &new.fields {
        match old_draft.fields.iter().find(|o| o.name == f.name) {
            None => d.added.push(f.clone()),
            Some(o) if o.to_value() != f.to_value() => d.changed.push(FieldChange {
                name: f.name.clone(),
                before: o.clone(),
                after: f.clone(),
            }),
            Some(_) => {}
        }
    }
    d.dropped = old_draft
        .fields
        .iter()
        .filter(|o| !new.fields.iter().any(|f| f.name == o.name))
        .map(|o| o.name.clone())
        .collect();
    if new.synonym_sets != old_draft.synonym_sets {
        d.synonym_sets = Some(new.synonym_sets.clone());
    }
    if new.metadata != old_draft.metadata {
        d.metadata = Some(new.metadata.clone());
    }
    if !d.added.is_empty() || !d.changed.is_empty() {
        d.consequences
            .push(Consequence::RewritesDocuments(old.num_documents));
    }
    if !old.default_sorting_field.is_empty() && d.dropped.contains(&old.default_sorting_field) {
        d.consequences.push(Consequence::ClearsDefaultSortingField(
            old.default_sorting_field.clone(),
        ));
    }
    d
}

/// Like [`diff`], plus `BreaksReference` for fields of other collections in `all`.
pub fn diff_with_references(old: &Collection, new: &SchemaDraft, all: &[Collection]) -> SchemaDiff {
    let mut d = diff(old, new);
    let gone: Vec<String> = d
        .dropped
        .iter()
        .chain(d.changed.iter().map(|c| &c.name))
        .map(|f| format!("{}.{f}", old.name))
        .collect();
    for c in all.iter().filter(|c| c.name != old.name) {
        for f in &c.fields {
            let r = f.attributes.get("reference").and_then(Value::as_str);
            if r.is_some_and(|r| gone.iter().any(|g| g == r)) {
                d.consequences.push(Consequence::BreaksReference {
                    collection: c.name.clone(),
                    field: f.name.clone(),
                });
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldType;

    fn coll(v: Value) -> Collection {
        serde_json::from_value(v).unwrap()
    }

    fn base() -> Collection {
        coll(
            json!({"name":"books","num_documents":7,"created_at":1,"default_sorting_field":"n",
            "fields":[
              {"name":"title","type":"string","facet":false,"index":true,"optional":false,"store":true},
              {"name":"n","type":"int32","facet":false,"index":true,"optional":false,"sort":true,"store":true},
              {"name":"v","type":"float[]","num_dim":4,"embed":{"from":["title"],"model_config":{"model_name":"ts/x"}},
               "facet":false,"index":true,"optional":true,"store":true}]}),
        )
    }

    #[test]
    fn diff_table() {
        let old = base();
        let same = SchemaDraft::from_collection(&old);
        let d = diff(&old, &same);
        assert!(d.is_empty() && d.consequences.is_empty());
        assert_eq!(d.patch_body(), json!({}));

        // add only
        let mut n = same.clone();
        n.fields.push(FieldDraft::new("x", FieldType::Bool));
        let d = diff(&old, &n);
        assert_eq!(
            d.patch_body(),
            json!({"fields":[{"name":"x","type":"bool"}]})
        );
        assert_eq!(d.consequences, vec![Consequence::RewritesDocuments(7)]);

        // drop only; embed field untouched and absent
        let mut n = same.clone();
        n.fields.retain(|f| f.name != "title");
        let d = diff(&old, &n);
        assert_eq!(
            d.patch_body(),
            json!({"fields":[{"name":"title","drop":true}]})
        );
        assert!(d.consequences.is_empty());

        // type change -> drop + add
        let mut n = same.clone();
        n.fields[1].r#type = FieldType::Int64;
        let d = diff(&old, &n);
        assert_eq!(
            d.patch_body(),
            json!({"fields":[{"name":"n","drop":true},{"name":"n","type":"int64"}]})
        );

        // attribute change
        let mut n = same.clone();
        n.fields[0].facet = true;
        let d = diff(&old, &n);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].name, "title");
        assert!(!d.patch_body().to_string().contains("embed"));

        // synonym sets and metadata
        let mut n = same.clone();
        n.synonym_sets = vec!["s".into()];
        n.metadata = Some(Map::from_iter([("a".to_string(), json!(1))]));
        let d = diff(&old, &n);
        assert_eq!(
            d.patch_body(),
            json!({"synonym_sets":["s"],"metadata":{"a":1}})
        );
        assert!(d.consequences.is_empty());

        // dropping the default sorting field
        let mut n = same.clone();
        n.fields.retain(|f| f.name != "n");
        assert!(
            diff(&old, &n)
                .consequences
                .contains(&Consequence::ClearsDefaultSortingField("n".into()))
        );
    }

    #[test]
    fn dropping_a_referenced_field_breaks_the_reference() {
        let authors = coll(json!({"name":"authors","num_documents":1,"created_at":1,
            "fields":[{"name":"id","type":"string"},{"name":"nm","type":"string"}]}));
        let refs = coll(json!({"name":"books_ref","num_documents":1,"created_at":1,
            "fields":[{"name":"author_id","type":"string","reference":"authors.id"}]}));
        let mut n = SchemaDraft::from_collection(&authors);
        n.fields.retain(|f| f.name != "id");
        let d = diff_with_references(&authors, &n, &[authors.clone(), refs]);
        assert!(d.consequences.contains(&Consequence::BreaksReference {
            collection: "books_ref".into(),
            field: "author_id".into()
        }));
    }
}
