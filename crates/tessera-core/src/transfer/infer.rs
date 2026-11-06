// SPDX-License-Identifier: GPL-3.0-or-later

//! Proposes a collection schema from the first rows of a file.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::Path;

use serde_json::{Number, Value};

use super::SAMPLE_ROWS;
use super::source::{ReadError, ReadOptions, Source, read_documents};
use crate::schema::{FieldDraft, FieldType, SchemaDraft};

/// A schema proposal and what to tell the user about it.
#[derive(Debug, Clone, PartialEq)]
pub struct InferredSchema {
    /// Fields in first-seen order. `id` is never one.
    pub fields: Vec<FieldDraft>,
    /// Whether a field holds objects, so the collection needs `enable_nested_fields`.
    pub nested: bool,
    /// Where the proposal is a guess.
    pub notes: Vec<InferNote>,
    /// Rows that were looked at.
    pub sampled: u64,
}

/// Something about a field the user should look at. Carries the field name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferNote {
    /// Pairs of numbers within ±90 and ±180, which may be a `geopoint`. Never decided here.
    MaybeGeopoint(String),
    /// Values of several types; the field is `string` or `string[]`.
    MixedTypes(String),
    /// Only empty arrays seen; the field is `string[]`.
    EmptyArray(String),
    /// Only `null` seen; the field is an optional `string`.
    OnlyNull(String),
    /// A key the server would mishandle (empty, `*`, control characters, edge spaces).
    SkippedName(String),
    /// The `id` is no string; the server refuses such a document.
    NonStringId,
}

/// What was seen of one value, as flags.
#[derive(Default, Clone, Copy)]
struct Kinds {
    bool_: bool,
    int: bool,
    float: bool,
    string: bool,
    object: bool,
    /// An array inside an array.
    other: bool,
}

impl Kinds {
    fn add(&mut self, v: &Value) {
        match v {
            Value::Bool(_) => self.bool_ = true,
            Value::Number(n) if is_int(n) => self.int = true,
            Value::Number(_) => self.float = true,
            Value::String(_) => self.string = true,
            Value::Object(_) => self.object = true,
            Value::Array(_) => self.other = true,
            Value::Null => {}
        }
    }

    fn none(&self) -> bool {
        !(self.bool_ || self.int || self.float || self.string || self.object || self.other)
    }

    /// The base type name and whether the values did not agree on one.
    fn resolve(&self) -> (&'static str, bool) {
        let number = self.int || self.float;
        let kinds = [self.bool_, number, self.string, self.object, self.other];
        if kinds.into_iter().filter(|k| *k).count() > 1 || self.other {
            return ("string", true);
        }
        let name = if self.bool_ {
            "bool"
        } else if self.float {
            "float"
        } else if self.int {
            "int64"
        } else if self.object {
            "object"
        } else {
            "string"
        };
        (name, false)
    }
}

fn is_int(n: &Number) -> bool {
    n.is_i64()
}

/// Everything seen of one key.
#[derive(Default)]
struct Seen {
    scalar: Kinds,
    elements: Kinds,
    arrays: u64,
    /// Arrays that held an element.
    filled_arrays: u64,
    /// Arrays that are two numbers within ±90 and ±180.
    pair_arrays: u64,
    /// Rows with a value that is not `null`.
    present: u64,
}

impl Seen {
    fn add(&mut self, v: &Value) {
        if v.is_null() {
            return;
        }
        self.present += 1;
        let Value::Array(items) = v else {
            self.scalar.add(v);
            return;
        };
        self.arrays += 1;
        if !items.is_empty() {
            self.filled_arrays += 1;
        }
        items.iter().for_each(|item| self.elements.add(item));
        if let [Value::Number(lat), Value::Number(lng)] = items.as_slice() {
            let within = |n: &Number, max: f64| n.as_f64().is_some_and(|f| f.abs() <= max);
            if within(lat, 90.0) && within(lng, 180.0) {
                self.pair_arrays += 1;
            }
        }
    }
}

/// Whether the server handles `name` as a plain field name.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('*')
        && !name.contains(char::is_control)
        && name == name.trim()
}

/// Looks at the first [`SAMPLE_ROWS`] records. A row that is no JSON object is left out of
/// the proposal and of `sampled`. Read errors end the proposal.
pub fn infer_schema(source: &Source, options: &ReadOptions) -> Result<InferredSchema, ReadError> {
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashMap<String, Seen> = HashMap::new();
    let mut notes: Vec<InferNote> = Vec::new();
    let (mut sampled, mut records) = (0u64, 0usize);
    read_documents(source, options, |record| {
        records += 1;
        if let Some(Value::Object(row)) = record
            .doc
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            sampled += 1;
            for (key, value) in &row {
                if key == "id" {
                    if !value.is_string() && !notes.contains(&InferNote::NonStringId) {
                        notes.push(InferNote::NonStringId);
                    }
                } else if !plain_name(key) {
                    let note = InferNote::SkippedName(key.clone());
                    if !notes.contains(&note) {
                        notes.push(note);
                    }
                } else {
                    if !seen.contains_key(key) {
                        order.push(key.clone());
                    }
                    seen.entry(key.clone()).or_default().add(value);
                }
            }
        }
        if records >= SAMPLE_ROWS {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })?;
    let mut fields = Vec::new();
    let mut nested = false;
    for name in order {
        let seen = &seen[&name];
        let (ty, mixed, only_null, empty_array) = field_type(seen);
        let field_type = match ty.as_str() {
            "string" => FieldType::String,
            "string[]" => FieldType::StringArray,
            "int64" => FieldType::Int64,
            "int64[]" => FieldType::Int64Array,
            "float" => FieldType::Float,
            "float[]" => FieldType::FloatArray,
            "bool" => FieldType::Bool,
            "bool[]" => FieldType::BoolArray,
            "object" => FieldType::Object,
            _ => FieldType::ObjectArray,
        };
        nested |= matches!(field_type, FieldType::Object | FieldType::ObjectArray);
        if mixed {
            notes.push(InferNote::MixedTypes(name.clone()));
        }
        if only_null {
            notes.push(InferNote::OnlyNull(name.clone()));
        }
        if empty_array {
            notes.push(InferNote::EmptyArray(name.clone()));
        }
        let pairs =
            seen.arrays > 0 && seen.pair_arrays == seen.arrays && seen.present == seen.arrays;
        if pairs && matches!(field_type, FieldType::FloatArray | FieldType::Int64Array) {
            notes.push(InferNote::MaybeGeopoint(name.clone()));
        }
        let mut draft = FieldDraft::new(name, field_type);
        draft.optional = seen.present < sampled || only_null;
        fields.push(draft);
    }
    Ok(InferredSchema {
        fields,
        nested,
        notes,
        sampled,
    })
}

/// The type name, then whether values disagreed, were only `null`, or only empty arrays.
fn field_type(seen: &Seen) -> (String, bool, bool, bool) {
    if seen.present == 0 {
        return ("string".into(), false, true, false);
    }
    if seen.arrays > 0 {
        if !seen.scalar.none() {
            return ("string".into(), true, false, false);
        }
        if seen.filled_arrays == 0 {
            return ("string[]".into(), false, false, true);
        }
        let (base, mixed) = seen.elements.resolve();
        return (format!("{base}[]"), mixed, false, false);
    }
    let (base, mixed) = seen.scalar.resolve();
    (base.into(), mixed, false, false)
}

impl InferredSchema {
    /// A draft named `name` with these fields.
    pub fn to_draft(&self, name: &str) -> SchemaDraft {
        SchemaDraft {
            name: name.to_string(),
            fields: self.fields.clone(),
            enable_nested_fields: self.nested,
            ..SchemaDraft::default()
        }
    }
}

/// A proposal for a collection name: the file stem with everything but `A-Z a-z 0-9 _ -`
/// replaced by `_`, or `imported` when nothing usable is left.
pub fn collection_name_from(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.chars().any(|c| c.is_ascii_alphanumeric()) {
        name
    } else {
        "imported".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::schema::validate;
    use crate::transfer::source::Format;

    fn infer_text(text: &str, format: Format) -> InferredSchema {
        let source = Source::Bytes(Arc::from(text.as_bytes()));
        infer_schema(&source, &ReadOptions::new(format)).expect("infers")
    }

    fn infer(lines: &[&str]) -> InferredSchema {
        infer_text(&lines.join("\n"), Format::Jsonl)
    }

    fn types(s: &InferredSchema) -> Vec<(String, String, bool)> {
        s.fields
            .iter()
            .map(|f| (f.name.clone(), f.r#type.as_str().to_string(), f.optional))
            .collect()
    }

    fn one(lines: &[&str]) -> (String, bool) {
        let s = infer(lines);
        let f = &s.fields[0];
        (f.r#type.as_str().to_string(), f.optional)
    }

    #[test]
    fn integers_are_int64() {
        assert_eq!(one(&[r#"{"n":1}"#, r#"{"n":-5}"#]), ("int64".into(), false));
    }

    #[test]
    fn integer_and_float_mix_to_float() {
        assert_eq!(one(&[r#"{"n":1}"#, r#"{"n":1.5}"#]).0, "float");
    }

    #[test]
    fn huge_number_is_float() {
        assert_eq!(one(&[r#"{"n":18446744073709551615}"#]).0, "float");
        assert_eq!(one(&[r#"{"n":9223372036854775808}"#]).0, "float");
    }

    #[test]
    fn bool_stays_bool() {
        assert_eq!(one(&[r#"{"b":true}"#, r#"{"b":false}"#]).0, "bool");
    }

    #[test]
    fn string_and_number_are_string_with_a_note() {
        let s = infer(&[r#"{"a":"x"}"#, r#"{"a":1}"#]);
        assert_eq!(types(&s), [("a".into(), "string".into(), false)]);
        assert_eq!(s.notes, [InferNote::MixedTypes("a".into())]);
    }

    #[test]
    fn bool_and_number_are_string_with_a_note() {
        let s = infer(&[r#"{"a":true}"#, r#"{"a":1}"#]);
        assert_eq!(s.fields[0].r#type, FieldType::String);
        assert_eq!(s.notes, [InferNote::MixedTypes("a".into())]);
    }

    #[test]
    fn arrays_take_the_element_type() {
        let s = infer(&[r#"{"s":["a"],"i":[1,2],"f":[1,2.5],"b":[true],"o":[{"x":1}]}"#]);
        let got: Vec<String> = s
            .fields
            .iter()
            .map(|f| f.r#type.as_str().to_string())
            .collect();
        assert_eq!(
            got,
            ["string[]", "int64[]", "float[]", "bool[]", "object[]"]
        );
        assert!(s.nested);
    }

    #[test]
    fn empty_arrays_are_string_arrays_with_a_note() {
        let s = infer(&[r#"{"a":[]}"#, r#"{"a":[]}"#]);
        assert_eq!(s.fields[0].r#type, FieldType::StringArray);
        assert_eq!(s.notes, [InferNote::EmptyArray("a".into())]);
    }

    #[test]
    fn object_sets_nested() {
        let s = infer(&[r#"{"o":{"a":1}}"#]);
        assert_eq!(s.fields[0].r#type, FieldType::Object);
        assert!(s.nested);
        assert!(s.to_draft("c").enable_nested_fields);
        assert!(!infer(&[r#"{"a":1}"#]).nested);
    }

    #[test]
    fn missing_or_null_is_optional() {
        let s = infer(&[r#"{"a":1,"b":1}"#, r#"{"a":2}"#, r#"{"a":3,"b":null}"#]);
        assert_eq!(
            types(&s),
            [
                ("a".into(), "int64".into(), false),
                ("b".into(), "int64".into(), true)
            ]
        );
    }

    #[test]
    fn only_null_is_optional_string_with_a_note() {
        let s = infer(&[r#"{"a":null}"#, r#"{"a":null}"#]);
        assert_eq!(types(&s), [("a".into(), "string".into(), true)]);
        assert_eq!(s.notes, [InferNote::OnlyNull("a".into())]);
    }

    #[test]
    fn coordinate_pairs_are_suggested_not_decided() {
        let s = infer(&[r#"{"loc":[48.1,11.5]}"#, r#"{"loc":[52.5,13.4]}"#]);
        assert_eq!(s.fields[0].r#type, FieldType::FloatArray);
        assert_eq!(s.notes, [InferNote::MaybeGeopoint("loc".into())]);
        let ints = infer(&[r#"{"loc":[48,11]}"#]);
        assert_eq!(ints.fields[0].r#type, FieldType::Int64Array);
        assert_eq!(ints.notes, [InferNote::MaybeGeopoint("loc".into())]);
        assert!(infer(&[r#"{"p":[100,200]}"#]).notes.is_empty());
        assert!(infer(&[r#"{"p":[1,2,3]}"#]).notes.is_empty());
        assert!(
            infer(&[r#"{"p":[1,2]}"#, r#"{"p":[100,2]}"#])
                .notes
                .is_empty()
        );
        let object = infer(&[r#"{"p":{"lat":1,"lng":2}}"#]);
        assert_eq!(object.fields[0].r#type, FieldType::Object);
        assert!(object.notes.is_empty());
    }

    #[test]
    fn id_makes_no_field() {
        let s = infer(&[r#"{"id":"1","a":1}"#]);
        assert_eq!(types(&s), [("a".into(), "int64".into(), false)]);
        assert!(s.notes.is_empty());
    }

    #[test]
    fn numeric_id_notes() {
        let s = infer(&[r#"{"id":1}"#, r#"{"id":2}"#]);
        assert!(s.fields.is_empty());
        assert_eq!(s.notes, [InferNote::NonStringId]);
    }

    #[test]
    fn skipped_names() {
        let s = infer(&[r#"{"":1,"x*":1,".*":1," a":1,"a\nb":1,"ok":1}"#]);
        assert_eq!(types(&s), [("ok".into(), "int64".into(), false)]);
        let skipped: Vec<&str> = s
            .notes
            .iter()
            .map(|n| match n {
                InferNote::SkippedName(name) => name.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(skipped, ["", "x*", ".*", " a", "a\nb"]);
    }

    #[test]
    fn first_seen_order() {
        let s = infer(&[r#"{"b":1,"a":1}"#, r#"{"c":1,"a":1}"#]);
        let names: Vec<&str> = s.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "c"]);
    }

    #[test]
    fn sampled_stops_at_1000() {
        let mut lines = vec![r#"{"n":1}"#; SAMPLE_ROWS];
        lines.push(r#"{"n":"text","late":1}"#);
        let s = infer(&lines);
        assert_eq!(s.sampled, SAMPLE_ROWS as u64);
        assert_eq!(types(&s), [("n".into(), "int64".into(), false)]);
    }

    #[test]
    fn unparsable_rows_are_not_sampled() {
        let s = infer(&[r#"{"a":1}"#, "not json", "[1]", r#"{"a":2}"#]);
        assert_eq!(s.sampled, 2);
        assert_eq!(types(&s), [("a".into(), "int64".into(), false)]);
    }

    #[test]
    fn csv_cells_are_guessed() {
        let s = infer_text("i,f,b,s\n12,1.5,true,007\n", Format::Csv);
        let got: Vec<String> = s
            .fields
            .iter()
            .map(|f| f.r#type.as_str().to_string())
            .collect();
        assert_eq!(got, ["int64", "float", "bool", "string"]);
    }

    #[test]
    fn to_draft_passes_validate() {
        let s = infer(&[
            r#"{"id":"1","title":"a","n":1,"price":1.5,"on":true,"tags":["x"],"o":{"a":1},"loc":[48.1,11.5],"opt":null}"#,
            r#"{"id":"2","title":"b","n":2,"price":2.5,"on":false,"tags":[],"o":{"a":2},"loc":[1,2]}"#,
        ]);
        let draft = s.to_draft("books");
        assert_eq!(draft.name, "books");
        let issues = validate(&draft);
        assert!(!crate::schema::has_errors(&issues), "{issues:?}");
    }

    #[test]
    fn collection_name_from_file_names() {
        let cases = [
            ("my data.v2.jsonl", "my_data_v2"),
            ("日本.csv", "imported"),
            ("-x.json", "-x"),
            ("dir/books.json", "books"),
            ("", "imported"),
        ];
        for (path, want) in cases {
            assert_eq!(collection_name_from(Path::new(path)), want, "{path:?}");
        }
    }
}
