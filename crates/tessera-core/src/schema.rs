// SPDX-License-Identifier: GPL-3.0-or-later

//! The form model behind the create, clone and edit dialog, with a lossless JSON round trip.

use serde_json::{Map, Value};
use tessera_client::Collection;

/// Credential keys inside `embed.model_config`.
const SECRET_KEYS: [&str; 4] = ["api_key", "access_token", "refresh_token", "client_secret"];

/// A Typesense field type. Unknown strings pass through as [`FieldType::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)] // the variants are the Typesense type names
pub enum FieldType {
    String,
    StringArray,
    StringStar,
    Int32,
    Int32Array,
    Int64,
    Int64Array,
    Float,
    FloatArray,
    Bool,
    BoolArray,
    Geopoint,
    GeopointArray,
    Geopolygon,
    Object,
    ObjectArray,
    Image,
    Auto,
    /// A type this version does not know, kept verbatim.
    Other(String),
}

impl FieldType {
    /// All known types, in the order the type picker shows them.
    pub const ALL: [FieldType; 18] = [
        FieldType::String,
        FieldType::StringArray,
        FieldType::StringStar,
        FieldType::Int32,
        FieldType::Int32Array,
        FieldType::Int64,
        FieldType::Int64Array,
        FieldType::Float,
        FieldType::FloatArray,
        FieldType::Bool,
        FieldType::BoolArray,
        FieldType::Geopoint,
        FieldType::GeopointArray,
        FieldType::Geopolygon,
        FieldType::Object,
        FieldType::ObjectArray,
        FieldType::Image,
        FieldType::Auto,
    ];

    /// The type as Typesense spells it.
    pub fn as_str(&self) -> &str {
        match self {
            Self::String => "string",
            Self::StringArray => "string[]",
            Self::StringStar => "string*",
            Self::Int32 => "int32",
            Self::Int32Array => "int32[]",
            Self::Int64 => "int64",
            Self::Int64Array => "int64[]",
            Self::Float => "float",
            Self::FloatArray => "float[]",
            Self::Bool => "bool",
            Self::BoolArray => "bool[]",
            Self::Geopoint => "geopoint",
            Self::GeopointArray => "geopoint[]",
            Self::Geopolygon => "geopolygon",
            Self::Object => "object",
            Self::ObjectArray => "object[]",
            Self::Image => "image",
            Self::Auto => "auto",
            Self::Other(s) => s,
        }
    }

    /// Parses a type string; unknown strings become [`FieldType::Other`].
    pub fn parse(s: &str) -> FieldType {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .unwrap_or_else(|| FieldType::Other(s.to_string()))
    }
}

/// Whether the server sorts a field of this type by default.
// shortcut: scalars only, arrays are not confirmed against the docs; revisit if a clone shows `sort` noise.
fn default_sort(t: &FieldType) -> bool {
    matches!(
        t,
        FieldType::Int32 | FieldType::Int64 | FieldType::Float | FieldType::Bool
    )
}

/// One field of a [`SchemaDraft`].
#[derive(Clone, PartialEq)]
pub struct FieldDraft {
    /// Field name or pattern.
    pub name: String,
    /// Field type.
    pub r#type: FieldType,
    /// Facet on this field.
    pub facet: bool,
    /// Documents may omit the field.
    pub optional: bool,
    /// Index the field (server default true).
    pub index: bool,
    /// Sorting; `None` means the default for the type.
    pub sort: Option<bool>,
    /// Infix search.
    pub infix: bool,
    /// Stemming.
    pub stem: bool,
    /// Store the value (server default true).
    pub store: bool,
    /// Range index.
    pub range_index: bool,
    /// Language locale, empty for none.
    pub locale: String,
    /// Reference as `collection.field`, empty for none.
    pub reference: String,
    /// Asynchronous reference.
    pub async_reference: bool,
    /// Vector dimension.
    pub num_dim: Option<u32>,
    /// Vector distance metric.
    pub vec_dist: Option<String>,
    /// Embedding config, edited in JSON mode only.
    pub embed: Option<Value>,
    /// Field attributes the form does not edit.
    pub extra: Map<String, Value>,
}

impl std::fmt::Debug for FieldDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut embed = self.embed.clone();
        if let Some(e) = &mut embed {
            crate::redact::mask_value(e);
        }
        let mut extra = Value::Object(self.extra.clone());
        crate::redact::mask_value(&mut extra);
        f.debug_struct("FieldDraft")
            .field("name", &self.name)
            .field("type", &self.r#type)
            .field("facet", &self.facet)
            .field("optional", &self.optional)
            .field("index", &self.index)
            .field("sort", &self.sort)
            .field("infix", &self.infix)
            .field("stem", &self.stem)
            .field("store", &self.store)
            .field("range_index", &self.range_index)
            .field("locale", &self.locale)
            .field("reference", &self.reference)
            .field("async_reference", &self.async_reference)
            .field("num_dim", &self.num_dim)
            .field("vec_dist", &self.vec_dist)
            .field("embed", &embed)
            .field("extra", &extra)
            .finish()
    }
}

/// Dynamic fields (`string*`, `auto`, names with `*`) are optional by server default.
fn default_optional(name: &str, t: &FieldType) -> bool {
    matches!(t, FieldType::StringStar | FieldType::Auto) || name.contains('*')
}

impl FieldDraft {
    /// A field with the server defaults.
    pub fn new(name: impl Into<String>, r#type: FieldType) -> Self {
        let name = name.into();
        let optional = default_optional(&name, &r#type);
        Self {
            name,
            r#type,
            facet: false,
            optional,
            index: true,
            sort: None,
            infix: false,
            stem: false,
            store: true,
            range_index: false,
            locale: String::new(),
            reference: String::new(),
            async_reference: false,
            num_dim: None,
            vec_dist: None,
            embed: None,
            extra: Map::new(),
        }
    }

    fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), self.name.clone().into());
        m.insert("type".into(), self.r#type.as_str().into());
        if self.async_reference {
            m.insert("async_reference".into(), true.into());
        }
        if let Some(e) = &self.embed {
            m.insert("embed".into(), e.clone());
        }
        if self.facet {
            m.insert("facet".into(), true.into());
        }
        if !self.index {
            m.insert("index".into(), false.into());
        }
        if self.infix {
            m.insert("infix".into(), true.into());
        }
        if !self.locale.is_empty() {
            m.insert("locale".into(), self.locale.clone().into());
        }
        if let Some(n) = self.num_dim {
            m.insert("num_dim".into(), n.into());
        }
        let dyn_default = default_optional(&self.name, &self.r#type);
        if self.optional != dyn_default {
            m.insert("optional".into(), self.optional.into());
        }
        if self.range_index {
            m.insert("range_index".into(), true.into());
        }
        if !self.reference.is_empty() {
            m.insert("reference".into(), self.reference.clone().into());
        }
        if let Some(s) = self.sort {
            m.insert("sort".into(), s.into());
        }
        if self.stem {
            m.insert("stem".into(), true.into());
        }
        if !self.store {
            m.insert("store".into(), false.into());
        }
        if let Some(v) = &self.vec_dist {
            m.insert("vec_dist".into(), v.clone().into());
        }
        m.extend(self.extra.clone());
        Value::Object(m)
    }

    /// `server_defaults` drops values the server fills in (clone and modify).
    fn from_map(mut m: Map<String, Value>, server_defaults: bool) -> Result<Self, DraftError> {
        let name = take_str(&mut m, "name")?.unwrap_or_default();
        let ty = FieldType::parse(&take_str(&mut m, "type")?.unwrap_or_default());
        let mut f = FieldDraft::new(name, ty);
        f.async_reference = take_bool(&mut m, "async_reference")?.unwrap_or(false);
        f.embed = m.remove("embed");
        f.facet = take_bool(&mut m, "facet")?.unwrap_or(false);
        f.index = take_bool(&mut m, "index")?.unwrap_or(true);
        f.infix = take_bool(&mut m, "infix")?.unwrap_or(false);
        f.locale = take_str(&mut m, "locale")?.unwrap_or_default();
        f.num_dim = match m.remove("num_dim") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|&n| n > 0)
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| DraftError::msg("`num_dim` must be a positive integer"))?,
            ),
        };
        f.optional = take_bool(&mut m, "optional")?.unwrap_or(f.optional);
        f.range_index = take_bool(&mut m, "range_index")?.unwrap_or(false);
        f.reference = take_str(&mut m, "reference")?.unwrap_or_default();
        f.sort = take_bool(&mut m, "sort")?;
        f.stem = take_bool(&mut m, "stem")?.unwrap_or(false);
        f.store = take_bool(&mut m, "store")?.unwrap_or(true);
        f.vec_dist = take_str(&mut m, "vec_dist")?;
        if server_defaults {
            // Server-filled defaults that the form does not model.
            if m.get("stem_dictionary") == Some(&"".into()) {
                m.remove("stem_dictionary");
            }
            if m.get("truncate_len") == Some(&100.into()) {
                m.remove("truncate_len");
            }
            if f.sort == Some(default_sort(&f.r#type)) {
                f.sort = None;
            }
        }
        f.extra = m;
        Ok(f)
    }
}

/// The create, clone and edit form model of a collection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchemaDraft {
    /// Collection name.
    pub name: String,
    /// Fields.
    pub fields: Vec<FieldDraft>,
    /// Default sorting field, empty for none.
    pub default_sorting_field: String,
    /// Extra word separators.
    pub token_separators: Vec<String>,
    /// Symbols that are indexed.
    pub symbols_to_index: Vec<String>,
    /// Index nested objects.
    pub enable_nested_fields: bool,
    /// Synonym sets.
    pub synonym_sets: Vec<String>,
    /// Free-form metadata.
    pub metadata: Option<Map<String, Value>>,
    /// Top-level keys the form does not edit.
    pub extra: Map<String, Value>,
}

/// Why a schema text could not become a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftError {
    /// 1-based line of a syntax error.
    pub line: Option<usize>,
    /// 1-based column of a syntax error.
    pub column: Option<usize>,
    /// What is wrong.
    pub message: String,
}

impl DraftError {
    fn msg(m: impl Into<String>) -> Self {
        Self {
            line: None,
            column: None,
            message: m.into(),
        }
    }
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DraftError {}

fn take_str(m: &mut Map<String, Value>, k: &str) -> Result<Option<String>, DraftError> {
    match m.remove(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(DraftError::msg(format!("`{k}` must be a string"))),
    }
}

fn take_bool(m: &mut Map<String, Value>, k: &str) -> Result<Option<bool>, DraftError> {
    match m.remove(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(_) => Err(DraftError::msg(format!("`{k}` must be true or false"))),
    }
}

fn take_strings(m: &mut Map<String, Value>, k: &str) -> Result<Vec<String>, DraftError> {
    let bad = || DraftError::msg(format!("`{k}` must be a list of strings"));
    match m.remove(k) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .into_iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(bad))
            .collect(),
        Some(_) => Err(bad()),
    }
}

impl SchemaDraft {
    /// The request body: only what differs from the server default, in a fixed key order.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), self.name.clone().into());
        m.insert(
            "fields".into(),
            self.fields.iter().map(FieldDraft::to_value).collect(),
        );
        if !self.default_sorting_field.is_empty() {
            m.insert(
                "default_sorting_field".into(),
                self.default_sorting_field.clone().into(),
            );
        }
        if self.enable_nested_fields {
            m.insert("enable_nested_fields".into(), true.into());
        }
        if let Some(md) = &self.metadata {
            m.insert("metadata".into(), md.clone().into());
        }
        if !self.symbols_to_index.is_empty() {
            m.insert(
                "symbols_to_index".into(),
                self.symbols_to_index.clone().into(),
            );
        }
        if !self.synonym_sets.is_empty() {
            m.insert("synonym_sets".into(), self.synonym_sets.clone().into());
        }
        if !self.token_separators.is_empty() {
            m.insert(
                "token_separators".into(),
                self.token_separators.clone().into(),
            );
        }
        m.extend(self.extra.clone());
        Value::Object(m)
    }

    /// Parses schema text; syntax errors carry line and column.
    pub fn from_json(text: &str) -> Result<SchemaDraft, DraftError> {
        let v: Value = serde_json::from_str(text).map_err(|e| DraftError {
            line: Some(e.line()),
            column: Some(e.column()),
            message: e.to_string(),
        })?;
        Self::from_value(&v)
    }

    /// Builds a draft from a parsed schema; unknown keys land in `extra`.
    pub fn from_value(v: &Value) -> Result<SchemaDraft, DraftError> {
        let Value::Object(m) = v else {
            return Err(DraftError::msg("the schema must be a JSON object"));
        };
        Self::from_map(m.clone(), false)
    }

    fn from_map(mut m: Map<String, Value>, server_defaults: bool) -> Result<Self, DraftError> {
        let fields = match m.remove("fields") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .into_iter()
                .map(|f| match f {
                    Value::Object(fm) => FieldDraft::from_map(fm, server_defaults),
                    _ => Err(DraftError::msg("every field must be a JSON object")),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(DraftError::msg("`fields` must be a list")),
        };
        let metadata = match m.remove("metadata") {
            None | Some(Value::Null) => None,
            Some(Value::Object(o)) => Some(o),
            Some(_) => return Err(DraftError::msg("`metadata` must be an object")),
        };
        Ok(SchemaDraft {
            name: take_str(&mut m, "name")?.unwrap_or_default(),
            fields,
            default_sorting_field: take_str(&mut m, "default_sorting_field")?.unwrap_or_default(),
            token_separators: take_strings(&mut m, "token_separators")?,
            symbols_to_index: take_strings(&mut m, "symbols_to_index")?,
            enable_nested_fields: take_bool(&mut m, "enable_nested_fields")?.unwrap_or(false),
            synonym_sets: take_strings(&mut m, "synonym_sets")?,
            metadata,
            extra: m,
        })
    }

    /// The draft for clone and modify: server-filled defaults are dropped.
    pub fn from_collection(c: &Collection) -> SchemaDraft {
        let fields = c
            .fields
            .iter()
            .map(|f| {
                let mut m = f.attributes.clone();
                m.insert("name".into(), f.name.clone().into());
                m.insert("type".into(), f.r#type.clone().into());
                // Cannot fail for well-typed server output; fall back to a bare field.
                FieldDraft::from_map(m, true).unwrap_or_else(|_| {
                    // Unexpected attribute types: keep everything the server sent.
                    let mut d = FieldDraft::new(f.name.clone(), FieldType::parse(&f.r#type));
                    d.extra = f.attributes.clone();
                    d
                })
            })
            .collect();
        let mut extra = c.extra.clone();
        if !c.curation_sets.is_empty() {
            extra.insert("curation_sets".into(), c.curation_sets.clone().into());
        }
        SchemaDraft {
            name: c.name.clone(),
            fields,
            default_sorting_field: c.default_sorting_field.clone(),
            token_separators: c.token_separators.clone(),
            symbols_to_index: c.symbols_to_index.clone(),
            enable_nested_fields: c.enable_nested_fields,
            synonym_sets: c.synonym_sets.clone(),
            metadata: c.metadata.clone(),
            extra,
        }
    }

    /// Blanks the credentials under `embed.model_config`; returns how many were blanked.
    pub fn blank_secrets(&mut self) -> usize {
        let mut n = 0;
        for f in &mut self.fields {
            let cfg = f
                .embed
                .as_mut()
                .and_then(|e| e.get_mut("model_config"))
                .and_then(Value::as_object_mut);
            if let Some(cfg) = cfg {
                for k in SECRET_KEYS {
                    if let Some(v) = cfg.get_mut(k) {
                        *v = "".into();
                        n += 1;
                    }
                }
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sorted(mut v: Value) -> Value {
        v["fields"]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|f| f["name"].as_str().unwrap().to_string());
        v
    }

    fn check(draft: SchemaDraft, text: &str) {
        let typed = SchemaDraft::from_json(text).unwrap();
        assert_eq!(sorted(draft.to_json()), sorted(typed.to_json()));
        assert_eq!(
            draft.to_json(),
            serde_json::from_str::<Value>(text).unwrap()
        );
    }

    fn field(name: &str, t: FieldType) -> FieldDraft {
        FieldDraft::new(name, t)
    }

    #[test]
    fn form_and_json_are_equal() {
        let mut title = field("title", FieldType::String);
        title.infix = true;
        let mut genre = field("genre", FieldType::String);
        genre.facet = true;
        check(
            SchemaDraft {
                name: "books".into(),
                fields: vec![title, genre],
                ..Default::default()
            },
            r#"{"name":"books","fields":[{"name":"title","type":"string","infix":true},
                {"name":"genre","type":"string","facet":true}]}"#,
        );

        check(
            SchemaDraft {
                name: "n".into(),
                fields: vec![
                    field("year", FieldType::Int32),
                    field("r", FieldType::Float),
                ],
                default_sorting_field: "year".into(),
                ..Default::default()
            },
            r#"{"name":"n","fields":[{"name":"year","type":"int32"},{"name":"r","type":"float"}],
                "default_sorting_field":"year"}"#,
        );

        let mut tags = field("tags", FieldType::StringArray);
        tags.optional = true;
        check(
            SchemaDraft {
                name: "t".into(),
                fields: vec![tags],
                ..Default::default()
            },
            r#"{"name":"t","fields":[{"name":"tags","type":"string[]","optional":true}]}"#,
        );

        let mut v = field("vec", FieldType::FloatArray);
        v.num_dim = Some(4);
        v.vec_dist = Some("cosine".into());
        check(
            SchemaDraft {
                name: "v".into(),
                fields: vec![v],
                ..Default::default()
            },
            r#"{"name":"v","fields":[{"name":"vec","type":"float[]","num_dim":4,"vec_dist":"cosine"}]}"#,
        );

        let mut r = field("book_id", FieldType::String);
        r.reference = "books.id".into();
        r.async_reference = true;
        check(
            SchemaDraft {
                name: "r".into(),
                fields: vec![r],
                ..Default::default()
            },
            r#"{"name":"r","fields":[{"name":"book_id","type":"string",
                "async_reference":true,"reference":"books.id"}]}"#,
        );

        check(
            SchemaDraft {
                name: "o".into(),
                fields: vec![field("author", FieldType::Object)],
                enable_nested_fields: true,
                ..Default::default()
            },
            r#"{"name":"o","fields":[{"name":"author","type":"object"}],"enable_nested_fields":true}"#,
        );
    }

    #[test]
    fn defaults_are_omitted() {
        assert_eq!(
            FieldDraft::new("t", FieldType::String).to_value(),
            json!({"name":"t","type":"string"})
        );
    }

    #[test]
    fn unknown_type_and_attribute_round_trip() {
        let v = json!({"name":"c","fields":[{"name":"x","type":"int8[]","future":1}],"future_top":true});
        assert_eq!(SchemaDraft::from_value(&v).unwrap().to_json(), v);
        assert_eq!(
            FieldType::parse("int8[]"),
            FieldType::Other("int8[]".into())
        );
    }

    #[test]
    fn from_collection_drops_server_defaults() {
        let c: Collection = serde_json::from_value(json!({
            "name":"m2probe","num_documents":0,"created_at":1,"default_sorting_field":"n",
            "enable_nested_fields":false,"symbols_to_index":[],"token_separators":[],
            "synonym_sets":[],"curation_sets":[],
            "fields":[
              {"facet":false,"index":true,"infix":false,"locale":"","name":"title","optional":false,
               "sort":false,"stem":false,"stem_dictionary":"","store":true,"truncate_len":100,"type":"string"},
              {"facet":true,"index":true,"infix":false,"locale":"","name":"n","optional":false,
               "sort":true,"stem":false,"stem_dictionary":"","store":true,"truncate_len":100,"type":"int32"},
              {"facet":false,"index":true,"infix":false,"locale":"","name":".*","optional":true,
               "sort":false,"stem":false,"stem_dictionary":"","store":true,"truncate_len":100,"type":"auto"}]
        }))
        .unwrap();
        assert_eq!(
            SchemaDraft::from_collection(&c).to_json(),
            json!({"name":"m2probe","fields":[
                {"name":"title","type":"string"},
                {"name":"n","type":"int32","facet":true},
                {"name":".*","type":"auto"}],
              "default_sorting_field":"n"})
        );
    }

    #[test]
    fn clone_blanks_embedding_secrets() {
        let mut d = SchemaDraft::from_json(
            r#"{"name":"e","fields":[{"name":"emb","type":"float[]","embed":{"from":["t"],
              "model_config":{"model_name":"openai/x","api_key":"sk-1"}}}]}"#,
        )
        .unwrap();
        assert_eq!(d.blank_secrets(), 1);
        let j = d.to_json();
        let cfg = &j["fields"][0]["embed"]["model_config"];
        assert_eq!(cfg["api_key"], "");
        assert_eq!(cfg["model_name"], "openai/x");
    }

    #[test]
    fn clone_keeps_curation_sets() {
        let c = Collection {
            name: "a".into(),
            curation_sets: vec!["c1".into()],
            ..Default::default()
        };
        assert_eq!(
            SchemaDraft::from_collection(&c).to_json()["curation_sets"],
            json!(["c1"])
        );
    }

    #[test]
    fn clone_keeps_attributes_of_odd_fields() {
        let c: Collection = serde_json::from_value(json!({"name":"a","fields":[
            {"name":"x","type":"string","facet":"yes","reference":"b.id","future":1}]}))
        .unwrap();
        let j = SchemaDraft::from_collection(&c).to_json();
        assert_eq!(
            j["fields"][0],
            json!({"name":"x","type":"string","facet":"yes","reference":"b.id","future":1})
        );
    }

    #[test]
    fn debug_hides_embedding_secrets() {
        let d = SchemaDraft::from_json(
            r#"{"name":"e","fields":[{"name":"emb","type":"float[]","embed":{"from":["t"],
              "model_config":{"api_key":"sk-SECRET"}}}]}"#,
        )
        .unwrap();
        assert!(!format!("{d:?}").contains("sk-SECRET"));
    }

    #[test]
    fn dynamic_fields_are_optional_by_default() {
        let d = SchemaDraft::from_json(
            r#"{"name":"d","fields":[{"name":"a.*","type":"auto"},
              {"name":"b","type":"string*","optional":false},{"name":"c","type":"string","optional":true}]}"#,
        )
        .unwrap();
        let j = d.to_json();
        assert_eq!(j["fields"][0], json!({"name":"a.*","type":"auto"}));
        assert_eq!(j["fields"][1]["optional"], false);
        assert_eq!(j["fields"][2]["optional"], true);
    }

    #[test]
    fn nested_name_with_dot_is_not_dynamic() {
        let d = SchemaDraft::from_json(
            r#"{"name":"d","fields":[{"name":"address.city","type":"string"},
              {"name":"address.zip","type":"string","optional":true},{"name":"num_.*","type":"int32"}]}"#,
        )
        .unwrap();
        assert!(!d.fields[0].optional);
        assert!(d.fields[2].optional);
        let j = d.to_json();
        assert_eq!(
            j["fields"][0],
            json!({"name":"address.city","type":"string"})
        );
        assert_eq!(j["fields"][1]["optional"], true);
        assert!(j["fields"][2].get("optional").is_none());
    }

    #[test]
    fn debug_hides_secrets_in_extra() {
        let mut f = FieldDraft::new("a", FieldType::String);
        f.extra.insert("api_key".into(), "sk-EXTRA".into());
        assert!(!format!("{f:?}").contains("sk-EXTRA"));
    }

    #[test]
    fn key_order_is_fixed() {
        let d = SchemaDraft::from_json(
            r#"{"zz":1,"token_separators":["-"],"default_sorting_field":"n","name":"k",
              "fields":[{"future":1,"vec_dist":"ip","facet":true,"type":"float[]","name":"n","num_dim":2}]}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&d.to_json()).unwrap(),
            r#"{"name":"k","fields":[{"name":"n","type":"float[]","facet":true,"num_dim":2,"vec_dist":"ip","future":1}],"default_sorting_field":"n","token_separators":["-"],"zz":1}"#
        );
    }

    #[test]
    fn zero_num_dim_is_rejected() {
        assert!(
            SchemaDraft::from_json(
                r#"{"name":"a","fields":[{"name":"v","type":"float[]","num_dim":0}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn json_error_has_line() {
        let e = SchemaDraft::from_json("{\n\"name\": \"a\",\n}").unwrap_err();
        assert_eq!(e.line, Some(3));
    }
}
