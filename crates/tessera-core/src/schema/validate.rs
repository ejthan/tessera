// SPDX-License-Identifier: GPL-3.0-or-later
//! Client-side checks of a [`SchemaDraft`]; the server stays the authority.

use super::{FieldType, SchemaDraft};
use std::collections::HashSet;

/// Where an issue applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssuePath {
    /// The collection as a whole.
    Collection,
    /// Field at this index.
    Field(usize),
}

/// How serious an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks "Create".
    Error,
    /// Shown, does not block.
    Warning,
}

/// One finding. `key` is an i18n key under `schema.issue.*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issue {
    /// What it concerns.
    pub path: IssuePath,
    /// Message key for the app to translate.
    pub key: &'static str,
    /// Severity.
    pub severity: Severity,
}

/// True when any issue is an error.
pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

/// Checks a draft (design §5.2).
pub fn validate(draft: &SchemaDraft) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut add = |path, key, severity| {
        out.push(Issue {
            path,
            key,
            severity,
        })
    };
    use IssuePath::*;
    use Severity::*;

    if draft.name.is_empty() {
        add(Collection, "schema.issue.name_empty", Error);
    }
    if draft.fields.is_empty() {
        add(Collection, "schema.issue.no_fields", Error);
    }
    let dsf = &draft.default_sorting_field;
    if !dsf.is_empty() {
        let sortable = draft.fields.iter().find(|f| &f.name == dsf).map(|f| {
            matches!(
                f.r#type,
                FieldType::Int32 | FieldType::Int64 | FieldType::Float
            ) || (f.r#type == FieldType::String && f.sort == Some(true))
        });
        match sortable {
            None => add(Collection, "schema.issue.sorting_field_missing", Error),
            Some(false) => add(Collection, "schema.issue.sorting_field_type", Error),
            Some(true) => {}
        }
    }
    let mut seen = HashSet::new();
    for (i, f) in draft.fields.iter().enumerate() {
        let p = Field(i);
        if f.name.is_empty() {
            add(p, "schema.issue.field_name_empty", Error);
        } else if !seen.insert(f.name.as_str()) {
            add(p, "schema.issue.field_name_twice", Error);
        }
        let vector = f.r#type == FieldType::FloatArray;
        if (f.num_dim.is_some() || f.vec_dist.is_some()) && !vector {
            add(p, "schema.issue.vector_type", Error);
        }
        if !f.reference.is_empty() {
            let ok = f
                .reference
                .split_once('.')
                .is_some_and(|(c, g)| !c.is_empty() && !g.is_empty() && !g.contains('.'));
            if !ok {
                add(p, "schema.issue.reference_form", Error);
            }
        }
        if let Some(e) = &f.embed {
            if !vector {
                add(p, "schema.issue.embed_type", Error);
            }
            if !e
                .get("from")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty())
            {
                add(p, "schema.issue.embed_from", Error);
            }
        }
        if f.name == "id" && f.r#type != FieldType::String {
            add(p, "schema.issue.id_type", Warning);
        }
        if f.infix
            && !matches!(
                f.r#type,
                FieldType::String | FieldType::StringArray | FieldType::StringStar
            )
        {
            add(p, "schema.issue.infix_type", Warning);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldDraft;
    use serde_json::json;

    fn draft(fields: Vec<FieldDraft>) -> SchemaDraft {
        let mut d = SchemaDraft::from_value(&json!({"name":"c","fields":[]})).unwrap();
        d.fields = fields;
        d
    }
    fn f(n: &str, t: FieldType) -> FieldDraft {
        FieldDraft::new(n, t)
    }

    #[test]
    fn validation_table() {
        use IssuePath::*;
        use Severity::*;
        let ok = || draft(vec![f("t", FieldType::String)]);
        type Row = (
            &'static str,
            SchemaDraft,
            Vec<(IssuePath, &'static str, Severity)>,
        );
        let mut rows: Vec<Row> = vec![];
        rows.push(("valid", ok(), vec![]));
        let mut d = ok();
        d.name.clear();
        rows.push((
            "name",
            d,
            vec![(Collection, "schema.issue.name_empty", Error)],
        ));
        rows.push((
            "no fields",
            draft(vec![]),
            vec![(Collection, "schema.issue.no_fields", Error)],
        ));
        rows.push((
            "empty field",
            draft(vec![f("", FieldType::String)]),
            vec![(Field(0), "schema.issue.field_name_empty", Error)],
        ));
        rows.push((
            "dup",
            draft(vec![f("a", FieldType::String), f("a", FieldType::Int32)]),
            vec![(Field(1), "schema.issue.field_name_twice", Error)],
        ));
        let mut d = ok();
        d.default_sorting_field = "nope".into();
        rows.push((
            "dsf missing",
            d,
            vec![(Collection, "schema.issue.sorting_field_missing", Error)],
        ));
        let mut d = ok();
        d.default_sorting_field = "t".into();
        rows.push((
            "dsf plain string",
            d,
            vec![(Collection, "schema.issue.sorting_field_type", Error)],
        ));
        let mut d = ok();
        d.default_sorting_field = "t".into();
        d.fields[0].sort = Some(true);
        rows.push(("dsf sorted string", d, vec![]));
        let mut d = draft(vec![f("n", FieldType::Int64)]);
        d.default_sorting_field = "n".into();
        rows.push(("dsf int64", d, vec![]));
        let mut x = f("v", FieldType::String);
        x.num_dim = Some(3);
        rows.push((
            "num_dim",
            draft(vec![x]),
            vec![(Field(0), "schema.issue.vector_type", Error)],
        ));
        let mut x = f("v", FieldType::Int32);
        x.vec_dist = Some("cosine".into());
        rows.push((
            "vec_dist",
            draft(vec![x]),
            vec![(Field(0), "schema.issue.vector_type", Error)],
        ));
        let mut x = f("v", FieldType::FloatArray);
        x.num_dim = Some(3);
        rows.push(("vector ok", draft(vec![x]), vec![]));
        for r in ["authors", "a.", ".b", "a.b.c"] {
            let mut x = f("r", FieldType::String);
            x.reference = r.into();
            rows.push((
                "reference",
                draft(vec![x]),
                vec![(Field(0), "schema.issue.reference_form", Error)],
            ));
        }
        let mut x = f("r", FieldType::String);
        x.reference = "authors.id".into();
        rows.push(("reference ok", draft(vec![x]), vec![]));
        let mut x = f("e", FieldType::String);
        x.embed = Some(json!({"from":["t"]}));
        rows.push((
            "embed type",
            draft(vec![x]),
            vec![(Field(0), "schema.issue.embed_type", Error)],
        ));
        let mut x = f("e", FieldType::FloatArray);
        x.embed = Some(json!({"model_config":{}}));
        rows.push((
            "embed from",
            draft(vec![x]),
            vec![(Field(0), "schema.issue.embed_from", Error)],
        ));
        for e in [json!({"from":[]}), json!({"from":null})] {
            let mut x = f("e", FieldType::FloatArray);
            x.embed = Some(e);
            rows.push((
                "embed from empty",
                draft(vec![x]),
                vec![(Field(0), "schema.issue.embed_from", Error)],
            ));
        }
        let mut x = f("a", FieldType::String);
        x.num_dim = Some(3);
        rows.push((
            "several",
            draft(vec![f("a", FieldType::String), x]),
            vec![
                (Field(1), "schema.issue.field_name_twice", Error),
                (Field(1), "schema.issue.vector_type", Error),
            ],
        ));
        rows.push((
            "id type",
            draft(vec![f("id", FieldType::Int32)]),
            vec![(Field(0), "schema.issue.id_type", Warning)],
        ));
        let mut x = f("i", FieldType::Int32);
        x.infix = true;
        rows.push((
            "infix",
            draft(vec![x]),
            vec![(Field(0), "schema.issue.infix_type", Warning)],
        ));

        for (name, d, want) in rows {
            let got: Vec<_> = validate(&d)
                .iter()
                .map(|i| (i.path, i.key, i.severity))
                .collect();
            assert_eq!(got, want, "{name}");
        }
        assert!(has_errors(&validate(&draft(vec![]))));
        assert!(!has_errors(&validate(&draft(vec![f(
            "id",
            FieldType::Int32
        )]))));
    }
}
