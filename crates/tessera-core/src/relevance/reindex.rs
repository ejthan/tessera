// SPDX-License-Identifier: GPL-3.0-or-later

//! What to know before copying a collection, and whether the copy is complete.

use serde_json::Value;
use tessera_client::{Alias, Collection};

use super::MAX_NAME;
use crate::schema::SchemaDraft;

/// `_YYYYMMDD_HHMMSS`, the suffix of a name from [`reindex_name`].
const STAMP_LEN: usize = 16;

fn without_stamp(name: &str) -> &str {
    let digits = |bytes: &[u8]| bytes.iter().all(u8::is_ascii_digit);
    let at = name.len().saturating_sub(STAMP_LEN);
    match name.get(at..).map(str::as_bytes) {
        Some(s)
            if s.len() == STAMP_LEN
                && s[0] == b'_'
                && digits(&s[1..9])
                && s[9] == b'_'
                && digits(&s[10..]) =>
        {
            &name[..at]
        }
        _ => name,
    }
}

/// The proposed name of the copy of `source`, cut to fit [`MAX_NAME`]. An old stamp is replaced,
/// so a third reindex does not grow the name.
pub fn reindex_name(source: &str, now: jiff::Timestamp) -> String {
    let base = without_stamp(source);
    let mut end = base.len().min(MAX_NAME - STAMP_LEN);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}_{}", &base[..end], now.strftime("%Y%m%d_%H%M%S"))
}

/// What a reindex of a collection has to warn about or refuse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preflight {
    /// Other aliases on the source. They stay where they are.
    pub other_aliases: Vec<String>,
    /// Fields of the source with `store: false`. An export cannot hold them.
    pub unstored: Vec<String>,
    /// Those of `unstored` that the draft requires: every copied document would fail.
    pub required_unstored: Vec<String>,
    /// `(collection, field)` of every join on the source or an alias of it.
    pub referenced_by: Vec<(String, String)>,
    /// Sets the draft attaches that no longer exist.
    pub missing_sets: Vec<String>,
    /// Fields with `embed`, whose copy could not be verified.
    pub embedded: Vec<String>,
    /// Embedding credentials still to fill in.
    pub blank_secrets: usize,
}

fn draft_curation_sets(draft: &SchemaDraft) -> impl Iterator<Item = &str> {
    draft
        .extra
        .get("curation_sets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// Checks a reindex of `source` behind `alias` into `draft`. All lists must be fresh.
pub fn preflight(
    alias: &str,
    source: &Collection,
    draft: &SchemaDraft,
    collections: &[Collection],
    aliases: &[Alias],
    synonym_sets: &[String],
    curation_sets: &[String],
) -> Preflight {
    let on_source: Vec<&str> = aliases
        .iter()
        .filter(|a| a.collection_name == source.name)
        .map(|a| a.name.as_str())
        .collect();
    let unstored: Vec<String> = source
        .fields
        .iter()
        .filter(|f| !f.flag("store"))
        .map(|f| f.name.clone())
        .collect();
    let required_unstored = unstored
        .iter()
        .filter(|name| draft.fields.iter().any(|f| &f.name == *name && !f.optional))
        .cloned()
        .collect();
    let joins = |c: &'_ Collection| -> Vec<(String, String)> {
        c.fields
            .iter()
            .filter(|f| {
                f.attr("reference")
                    .and_then(Value::as_str)
                    .map(|r| r.split('.').next().unwrap_or(r))
                    .is_some_and(|target| target == source.name || on_source.contains(&target))
            })
            .map(|f| (c.name.clone(), f.name.clone()))
            .collect()
    };
    let missing_synonyms = draft
        .synonym_sets
        .iter()
        .filter(|n| !synonym_sets.contains(n))
        .cloned();
    let missing_curations = draft_curation_sets(draft)
        .filter(|n| !curation_sets.iter().any(|known| known == n))
        .map(str::to_string);
    Preflight {
        other_aliases: on_source
            .iter()
            .filter(|a| **a != alias)
            .map(|a| a.to_string())
            .collect(),
        unstored,
        required_unstored,
        referenced_by: collections.iter().flat_map(joins).collect(),
        missing_sets: missing_synonyms.chain(missing_curations).collect(),
        embedded: draft
            .fields
            .iter()
            .filter(|f| f.embed.is_some())
            .map(|f| f.name.clone())
            .collect(),
        blank_secrets: draft.empty_secrets(),
    }
}

/// Removes the sets `preflight` found missing, since `POST` would answer 404.
// shortcut: `missing_sets` does not say the kind, so a set of the other kind with the same name
// goes too, and the panel names it; split the list if that ever bites.
pub fn without_missing(draft: &mut SchemaDraft, preflight: &Preflight) {
    let missing = &preflight.missing_sets;
    draft.synonym_sets.retain(|n| !missing.contains(n));
    if let Some(Value::Array(list)) = draft.extra.get_mut("curation_sets") {
        list.retain(|v| v.as_str().is_none_or(|n| !missing.iter().any(|m| m == n)));
        if list.is_empty() {
            draft.extra.remove("curation_sets");
        }
    }
}

/// The two sides of a finished copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compared {
    /// The source's fresh count, or for a file source what the job imported.
    pub expected: u64,
    /// The target's fresh count.
    pub target: u64,
    /// Lines the import job reported as failed.
    pub failed: u64,
    /// Lines the import job could not confirm.
    pub unconfirmed: u64,
}

impl Compared {
    /// Whether every document arrived.
    pub fn equal(&self) -> bool {
        self.expected == self.target && self.failed == 0 && self.unconfirmed == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};
    use tessera_client::Field;

    fn stamp() -> jiff::Timestamp {
        "2026-11-06T14:22:33+01:00".parse().unwrap()
    }

    #[test]
    fn reindex_name_is_stamped_in_utc() {
        assert_eq!(
            reindex_name("products", stamp()),
            "products_20261106_132233"
        );
    }

    #[test]
    fn reindex_name_replaces_an_old_stamp() {
        assert_eq!(
            reindex_name("products_20250101_000000", stamp()),
            "products_20261106_132233"
        );
        // Not a stamp: kept.
        assert_eq!(
            reindex_name("p_2025010_000000", stamp()),
            "p_2025010_000000_20261106_132233"
        );
        assert_eq!(
            reindex_name("p_20250101-000000", stamp()),
            "p_20250101-000000_20261106_132233"
        );
    }

    #[test]
    fn reindex_name_fits_the_limit() {
        for source in ["a".repeat(MAX_NAME), "a".repeat(MAX_NAME - 16)] {
            let name = reindex_name(&source, stamp());
            assert_eq!(name.len(), MAX_NAME);
            assert!(crate::relevance::name_ok(&name), "{name}");
        }
    }

    #[test]
    fn reindex_name_cuts_on_a_character() {
        let source = format!("a{}", "é".repeat(MAX_NAME));
        let name = reindex_name(&source, stamp());
        assert!(name.len() <= MAX_NAME, "{name}");
        assert!(name.ends_with("_20261106_132233"));
    }

    fn field(name: &str, attributes: Value) -> Field {
        let Value::Object(attributes) = attributes else {
            panic!("attributes are an object");
        };
        Field {
            name: name.into(),
            r#type: "string".into(),
            attributes,
        }
    }

    fn collection(name: &str, fields: Vec<Field>) -> Collection {
        Collection {
            name: name.into(),
            fields,
            ..Collection::default()
        }
    }

    fn alias(name: &str, target: &str) -> Alias {
        Alias {
            name: name.into(),
            collection_name: target.into(),
        }
    }

    fn run(source: &Collection, collections: &[Collection], aliases: &[Alias]) -> Preflight {
        let draft = SchemaDraft::from_collection(source);
        preflight("shop", source, &draft, collections, aliases, &[], &[])
    }

    #[test]
    fn a_plain_collection_has_nothing_to_say() {
        let source = collection("products", vec![field("a", json!({}))]);
        let got = run(
            &source,
            std::slice::from_ref(&source),
            &[alias("shop", "products")],
        );
        assert_eq!(got, Preflight::default());
    }

    #[test]
    fn other_aliases_are_named() {
        let source = collection("products", vec![]);
        let aliases = [
            alias("shop", "products"),
            alias("shop_v1", "products"),
            alias("else", "other"),
        ];
        assert_eq!(run(&source, &[], &aliases).other_aliases, ["shop_v1"]);
    }

    #[test]
    fn unstored_is_not_required_unstored() {
        let source = collection(
            "products",
            vec![
                field("opt", json!({"store": false, "optional": true})),
                field("req", json!({"store": false, "optional": false})),
                field("kept", json!({"store": true})),
            ],
        );
        let got = run(&source, &[], &[]);
        assert_eq!(got.unstored, ["opt", "req"]);
        assert_eq!(got.required_unstored, ["req"]);
    }

    #[test]
    fn a_required_unstored_field_can_be_dropped_or_made_optional() {
        let source = collection("p", vec![field("req", json!({"store": false}))]);
        let mut draft = SchemaDraft::from_collection(&source);
        draft.fields[0].optional = true;
        let got = preflight("shop", &source, &draft, &[], &[], &[], &[]);
        assert_eq!(got.unstored, ["req"]);
        assert!(got.required_unstored.is_empty());

        draft.fields.clear();
        let got = preflight("shop", &source, &draft, &[], &[], &[], &[]);
        assert_eq!(got.unstored, ["req"]);
        assert!(got.required_unstored.is_empty());
    }

    #[test]
    fn a_reference_counts_by_name_and_by_any_alias() {
        let source = collection("products", vec![]);
        let orders = collection(
            "orders",
            vec![
                field("by_name", json!({"reference": "products.id"})),
                field("by_alias", json!({"reference": "shop_v1.id"})),
                field("elsewhere", json!({"reference": "products_old.id"})),
                field("other_alias", json!({"reference": "other.id"})),
                field("plain", json!({})),
            ],
        );
        let aliases = [
            alias("shop", "products"),
            alias("shop_v1", "products"),
            alias("other", "else"),
        ];
        let got = run(&source, &[source.clone(), orders], &aliases);
        let joins = [("orders", "by_name"), ("orders", "by_alias")]
            .map(|(c, f)| (c.to_string(), f.to_string()));
        assert_eq!(got.referenced_by, joins);
    }

    #[test]
    fn missing_sets_of_both_kinds_are_found_and_removed() {
        let mut source = collection("products", vec![]);
        source.synonym_sets = vec!["syn_ok".into(), "syn_gone".into()];
        source.curation_sets = vec!["cur_ok".into(), "cur_gone".into()];
        let mut draft = SchemaDraft::from_collection(&source);
        let got = preflight(
            "shop",
            &source,
            &draft,
            &[],
            &[],
            &["syn_ok".into()],
            &["cur_ok".into()],
        );
        assert_eq!(got.missing_sets, ["syn_gone", "cur_gone"]);

        without_missing(&mut draft, &got);
        assert_eq!(draft.synonym_sets, ["syn_ok"]);
        assert_eq!(draft.extra["curation_sets"], json!(["cur_ok"]));
    }

    #[test]
    fn removing_the_last_curation_set_leaves_no_key() {
        let mut source = collection("products", vec![]);
        source.curation_sets = vec!["cur_gone".into()];
        let mut draft = SchemaDraft::from_collection(&source);
        let got = preflight("shop", &source, &draft, &[], &[], &[], &[]);
        without_missing(&mut draft, &got);
        assert!(!draft.extra.contains_key("curation_sets"));
    }

    fn embedded() -> Collection {
        let embed = json!({"embed": {"from": ["a"], "model_config": {
            "model_name": "openai/text-embedding-3-small", "api_key": "sk-…abcd"}}});
        collection("products", vec![field("v", embed)])
    }

    #[test]
    fn embed_is_named() {
        assert_eq!(run(&embedded(), &[], &[]).embedded, ["v"]);
    }

    #[test]
    fn blanked_secrets_count_until_filled() {
        let source = embedded();
        let mut draft = SchemaDraft::from_collection(&source);
        let got = preflight("shop", &source, &draft, &[], &[], &[], &[]);
        assert_eq!(got.blank_secrets, 0, "not blanked yet");

        assert_eq!(draft.blank_secrets(), 1);
        let got = preflight("shop", &source, &draft, &[], &[], &[], &[]);
        assert_eq!(got.blank_secrets, 1);

        let config: &mut Map<String, Value> = draft.fields[0]
            .embed
            .as_mut()
            .unwrap()
            .get_mut("model_config")
            .unwrap()
            .as_object_mut()
            .unwrap();
        config.insert("api_key".into(), "sk-real".into());
        let got = preflight("shop", &source, &draft, &[], &[], &[], &[]);
        assert_eq!(got.blank_secrets, 0);
    }

    #[test]
    fn compared_is_equal_only_when_everything_agrees() {
        let base = Compared {
            expected: 5,
            target: 5,
            failed: 0,
            unconfirmed: 0,
        };
        assert!(base.equal());
        for other in [
            Compared { target: 4, ..base },
            Compared { target: 6, ..base },
            Compared { failed: 1, ..base },
            Compared {
                unconfirmed: 1,
                ..base
            },
        ] {
            assert!(!other.equal(), "{other:?}");
        }
    }
}
