// SPDX-License-Identifier: GPL-3.0-or-later

//! The actions a key form offers. The names were measured against 30.0 and 30.2: the server
//! takes any text, and many documented names grant nothing.

/// Actions that belong together in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionGroup {
    /// The suffix of its locale key under `keys.group.`.
    pub key: &'static str,
    /// The actions, as the server spells them.
    pub actions: &'static [&'static str],
}

const fn group(key: &'static str, actions: &'static [&'static str]) -> ActionGroup {
    ActionGroup { key, actions }
}

/// Every action of the form but the plain `*`, which has its own control.
pub const ACTION_GROUPS: &[ActionGroup] = &[
    group(
        "collections",
        &[
            "collections:create",
            "collections:get",
            "collections:list",
            "collections:update",
            "collections:delete",
            "collections:*",
        ],
    ),
    group(
        "documents",
        &[
            "documents:search",
            "documents:get",
            "documents:create",
            "documents:upsert",
            "documents:update",
            "documents:delete",
            "documents:import",
            "documents:export",
            "documents:*",
        ],
    ),
    group(
        "aliases",
        &[
            "aliases:list",
            "aliases:get",
            "aliases:upsert",
            "aliases:delete",
            "aliases:*",
        ],
    ),
    group(
        "synonym_sets",
        &[
            "synonym_sets:list",
            "synonym_sets:get",
            "synonym_sets:upsert",
            "synonym_sets:delete",
            "synonym_sets:*",
        ],
    ),
    group(
        "synonym_items",
        &[
            "synonym_sets/items:list",
            "synonym_sets/items:get",
            "synonym_sets/items:upsert",
            "synonym_sets/items:delete",
            "synonym_sets/items:*",
        ],
    ),
    group(
        "curation_sets",
        &[
            "curation_sets:list",
            "curation_sets:get",
            "curation_sets:upsert",
            "curation_sets:delete",
            "curation_sets:*",
        ],
    ),
    group(
        "curation_items",
        &[
            "curation_sets/items:list",
            "curation_sets/items:get",
            "curation_sets/items:upsert",
            "curation_sets/items:delete",
            "curation_sets/items:*",
        ],
    ),
    group(
        "stopwords",
        &[
            "stopwords:list",
            "stopwords:get",
            "stopwords:upsert",
            "stopwords:delete",
            "stopwords:*",
        ],
    ),
    group(
        "presets",
        &[
            "presets:list",
            "presets:get",
            "presets:upsert",
            "presets:delete",
            "presets:*",
        ],
    ),
    group(
        "keys",
        &[
            "keys:list",
            "keys:get",
            "keys:create",
            "keys:delete",
            "keys:*",
        ],
    ),
    group(
        "analytics_rules",
        &[
            "analytics/rules:list",
            "analytics/rules:get",
            "analytics/rules:create",
            "analytics/rules:upsert",
            "analytics/rules:delete",
            "analytics/rules:*",
        ],
    ),
    group(
        "analytics_events",
        &[
            "analytics/events:create",
            "analytics/events:list",
            "analytics/status:list",
            "analytics/flush:create",
        ],
    ),
    group(
        "stemming",
        &[
            "stemming/dictionaries:list",
            "stemming/dictionaries:get",
            "stemming/dictionaries:delete",
            "stemming/dictionaries/import:create",
        ],
    ),
    group(
        "conversation_models",
        &[
            "conversations/models:list",
            "conversations/models:get",
            "conversations/models:create",
            "conversations/models:upsert",
            "conversations/models:delete",
        ],
    ),
    group(
        "nl_models",
        &[
            "nl_search_models:list",
            "nl_search_models:get",
            "nl_search_models:create",
            "nl_search_models:upsert",
            "nl_search_models:delete",
        ],
    ),
    group(
        "operations",
        &[
            "operations/snapshot:create",
            "operations/vote:create",
            "operations/cache/clear:create",
            "operations/db/compact:create",
            "operations/schema_changes:list",
            "config:create",
        ],
    ),
    group(
        "metrics",
        &["metrics.json:list", "stats.json:list", "debug:list"],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> impl Iterator<Item = &'static str> {
        ACTION_GROUPS.iter().flat_map(|g| g.actions.iter().copied())
    }

    #[test]
    fn catalog_has_no_documented_only_names() {
        for name in [
            "aliases:create",
            "stopwords:create",
            "synonym_sets:create",
            "stemming/dictionaries:create",
            "operations/schema_changes:get",
        ] {
            assert!(!all().any(|a| a == name), "{name}");
        }
        assert!(!all().any(|a| a.starts_with("analytics:")));
    }

    #[test]
    fn catalog_holds_the_measured_names() {
        for name in [
            "aliases:upsert",
            "stopwords:upsert",
            "synonym_sets:upsert",
            "stemming/dictionaries/import:create",
            "analytics/rules:*",
        ] {
            assert!(all().any(|a| a == name), "{name}");
        }
    }

    #[test]
    fn the_plain_wildcard_is_no_group_member() {
        assert!(!all().any(|a| a == "*"));
    }
}
