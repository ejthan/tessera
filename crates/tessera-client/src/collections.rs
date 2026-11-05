// SPDX-License-Identifier: GPL-3.0-or-later

//! Collections and aliases, as the server reports them.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Everything but the unreserved characters, so a name stays one path segment.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// `collections/{name}{rest}` with `name` encoded as one path segment.
pub(crate) fn collection_path(name: &str, rest: &str) -> String {
    format!("collections/{}{rest}", utf8_percent_encode(name, SEGMENT))
}

/// A collection schema. Attributes this version does not know are kept in `extra`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
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
    #[serde(default)]
    pub metadata: Map<String, Value>,
    /// Number of documents (read only).
    #[serde(default)]
    pub num_documents: u64,
    /// Creation time, Unix seconds (read only).
    #[serde(default)]
    pub created_at: u64,
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

/// One field of a [`Collection`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
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
