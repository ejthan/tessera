// SPDX-License-Identifier: GPL-3.0-or-later

//! Collections and aliases, as the server reports them.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Error;

/// Everything but the unreserved characters, so a name stays one path segment.
pub(crate) const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Refuses what a URL cannot hold as one segment: it drops `.` and `..`, percent-encoded or
/// not, and an empty one is the parent, so the request would address something else.
pub(crate) fn addressable(segment: &str) -> Result<(), Error> {
    if matches!(segment, "" | "." | "..") {
        return Err(Error::UnaddressableId(segment.to_string()));
    }
    Ok(())
}

/// `collections/{name}{rest}` with `name` encoded as one path segment.
pub(crate) fn collection_path(name: &str, rest: &str) -> Result<String, Error> {
    addressable(name)?;
    Ok(format!(
        "collections/{}{rest}",
        utf8_percent_encode(name, SEGMENT)
    ))
}

/// A collection schema. Attributes this version does not know are kept in `extra`.
/// `Debug` masks embedding credentials.
#[derive(Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct Collection {
    /// Collection name.
    pub name: String,
    /// Field definitions.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Field that documents are sorted by when a search names none.
    #[serde(default)]
    pub default_sorting_field: String,
    /// Whether nested objects are indexed.
    #[serde(default)]
    pub enable_nested_fields: bool,
    /// Characters that are indexed although they are symbols.
    #[serde(default)]
    pub symbols_to_index: Vec<String>,
    /// Extra word separators.
    #[serde(default)]
    pub token_separators: Vec<String>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// Number of documents (read only).
    #[serde(default)]
    pub num_documents: u64,
    /// Creation time, Unix seconds (read only).
    #[serde(default)]
    pub created_at: i64,
    /// Synonym sets used by this collection.
    #[serde(default)]
    pub synonym_sets: Vec<String>,
    /// Curation sets used by this collection.
    #[serde(default)]
    pub curation_sets: Vec<String>,
    /// Everything else the server sent.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One field of a [`Collection`]. `Debug` masks embedding credentials.
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Field {
    /// Field name (may be a pattern such as `.*`).
    pub name: String,
    /// Typesense type, for example `string[]`. Kept as sent so unknown types round-trip.
    #[serde(rename = "type")]
    pub r#type: String,
    /// All other attributes, as sent.
    #[serde(flatten)]
    pub attributes: Map<String, Value>,
}

impl Field {
    /// A boolean attribute with the server's default: `index` and `store` are true when
    /// absent, everything else false.
    pub fn flag(&self, name: &str) -> bool {
        self.attr(name)
            .and_then(Value::as_bool)
            .unwrap_or(matches!(name, "index" | "store"))
    }

    /// An attribute as sent, if present.
    pub fn attr(&self, name: &str) -> Option<&Value> {
        self.attributes.get(name)
    }
}

/// Keys whose values `Debug` masks. Mirrors `tessera_core::redact` (this crate cannot depend
/// on tessera-core); keep the two lists equal.
const SECRET_KEYS: [&str; 5] = [
    "api_key",
    "value",
    "access_token",
    "refresh_token",
    "client_secret",
];

/// `map` with the value of every [`SECRET_KEYS`] key masked, at any depth.
fn masked(map: &Map<String, Value>) -> Value {
    fn mask(v: &mut Value) {
        match v {
            Value::Object(map) => {
                for (k, child) in map.iter_mut() {
                    if SECRET_KEYS.contains(&k.as_str()) {
                        *child = Value::String("••••".into());
                    } else {
                        mask(child);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(mask),
            _ => {}
        }
    }
    let mut v = Value::Object(map.clone());
    mask(&mut v);
    v
}

impl std::fmt::Debug for Collection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collection")
            .field("name", &self.name)
            .field("fields", &self.fields)
            .field("default_sorting_field", &self.default_sorting_field)
            .field("enable_nested_fields", &self.enable_nested_fields)
            .field("symbols_to_index", &self.symbols_to_index)
            .field("token_separators", &self.token_separators)
            .field("metadata", &self.metadata.as_ref().map(masked))
            .field("num_documents", &self.num_documents)
            .field("created_at", &self.created_at)
            .field("synonym_sets", &self.synonym_sets)
            .field("curation_sets", &self.curation_sets)
            .field("extra", &masked(&self.extra))
            .finish()
    }
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Field")
            .field("name", &self.name)
            .field("type", &self.r#type)
            .field("attributes", &masked(&self.attributes))
            .finish()
    }
}

/// An alias and the collection it points to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Alias {
    /// Alias name.
    pub name: String,
    /// Collection it resolves to.
    pub collection_name: String,
}

/// Outcome of a schema change (`PATCH /collections/{name}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SchemaChange {
    /// Collection name.
    pub collection: String,
    /// Documents checked against the change.
    #[serde(default)]
    pub validated_docs: u64,
    /// Documents rewritten by the change.
    #[serde(default)]
    pub altered_docs: u64,
}

#[derive(Deserialize)]
pub(crate) struct AliasList {
    pub aliases: Vec<Alias>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_masks_embedding_credentials() {
        let collection: Collection = serde_json::from_value(serde_json::json!({
            "name": "docs",
            "fields": [{
                "name": "embedding",
                "type": "float[]",
                "embed": {
                    "from": ["title"],
                    "model_config": {
                        "model_name": "openai/text-embedding-3-small",
                        "api_key": "sk-field-secret",
                        "access_token": "at-secret",
                        "refresh_token": "rt-secret",
                        "client_secret": "cs-secret"
                    }
                }
            }],
            "metadata": {"nested": [{"value": "meta-secret"}]},
            "voice_query_model": {"api_key": "extra-secret"}
        }))
        .unwrap();
        let shown = format!("{collection:?}");
        for secret in [
            "sk-field-secret",
            "at-secret",
            "rt-secret",
            "cs-secret",
            "meta-secret",
            "extra-secret",
        ] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        assert!(shown.contains("openai/text-embedding-3-small"), "{shown}");
        assert!(!format!("{:?}", collection.fields[0]).contains("sk-field-secret"));
    }
}
