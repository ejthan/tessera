// SPDX-License-Identifier: GPL-3.0-or-later

//! What the two sides of a set's test panel send.

use tessera_client::SetKind;

use super::param_safe;

/// What a side of the panel means. The app words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideMeaning {
    /// The collection as it is.
    AsConfigured,
    /// The collection plus the set, named for this search only.
    WithSet,
    /// Without any synonyms, because one attached set cannot be left out.
    NoSynonyms,
    /// Without any curations.
    NoCurations,
}

/// The search parameters in which a side differs from the common ones.
pub type Side = (SideMeaning, Vec<(String, String)>);

/// Both sides of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSides {
    /// The "without" side.
    pub left: Side,
    /// The "with" side.
    pub right: Side,
    /// Several sets are attached, so the panel explains what the left side leaves out.
    pub broad: bool,
}

/// Why a set cannot be tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestBlocked {
    /// A curation set works only when attached: a search cannot name one.
    NotAttached,
    /// An unattached synonym set whose name a `synonym_sets=` list cannot carry.
    NameNotSendable,
}

fn param(key: &str, value: &str) -> Vec<(String, String)> {
    vec![(key.to_string(), value.to_string())]
}

/// The sides for `set`, given the sets attached to the chosen collection and the typed tags.
pub fn test_sides(
    kind: SetKind,
    set: &str,
    attached: &[String],
    tags: &str,
) -> Result<TestSides, TestBlocked> {
    let is_attached = attached.iter().any(|n| n == set);
    let broad = attached.len() > 1;
    let (left, right) = match (kind, is_attached) {
        (SetKind::Synonym, false) => {
            if !param_safe(set) {
                return Err(TestBlocked::NameNotSendable);
            }
            (
                (SideMeaning::AsConfigured, Vec::new()),
                (SideMeaning::WithSet, param("synonym_sets", set)),
            )
        }
        (SetKind::Synonym, true) => (
            (SideMeaning::NoSynonyms, param("enable_synonyms", "false")),
            (SideMeaning::AsConfigured, Vec::new()),
        ),
        (SetKind::Curation, false) => return Err(TestBlocked::NotAttached),
        (SetKind::Curation, true) => {
            let tags = tags.trim();
            (
                (SideMeaning::NoCurations, param("enable_curations", "false")),
                (
                    SideMeaning::AsConfigured,
                    if tags.is_empty() {
                        Vec::new()
                    } else {
                        param("curation_tags", tags)
                    },
                ),
            )
        }
    };
    Ok(TestSides { left, right, broad })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn an_unattached_synonym_set_is_added_to_the_right_side() {
        let sides = test_sides(SetKind::Synonym, "s", &names(&["other"]), "").unwrap();
        assert_eq!(sides.left, (SideMeaning::AsConfigured, vec![]));
        assert_eq!(
            sides.right,
            (SideMeaning::WithSet, pairs(&[("synonym_sets", "s")]))
        );
        assert!(!sides.broad);
    }

    #[test]
    fn an_attached_synonym_set_cannot_be_left_out_alone() {
        let alone = test_sides(SetKind::Synonym, "s", &names(&["s"]), "").unwrap();
        assert_eq!(
            alone.left,
            (
                SideMeaning::NoSynonyms,
                pairs(&[("enable_synonyms", "false")])
            )
        );
        assert_eq!(alone.right, (SideMeaning::AsConfigured, vec![]));
        assert!(!alone.broad);
        let broad = test_sides(SetKind::Synonym, "s", &names(&["s", "t"]), "").unwrap();
        assert!(broad.broad);
    }

    #[test]
    fn an_attached_curation_set_sends_tags_on_the_right_side_only() {
        let sides = test_sides(SetKind::Curation, "c", &names(&["c"]), " vip,new ").unwrap();
        assert_eq!(
            sides.left,
            (
                SideMeaning::NoCurations,
                pairs(&[("enable_curations", "false")])
            )
        );
        assert_eq!(
            sides.right,
            (
                SideMeaning::AsConfigured,
                pairs(&[("curation_tags", "vip,new")])
            )
        );
        let none = test_sides(SetKind::Curation, "c", &names(&["c", "d"]), "  ").unwrap();
        assert_eq!(none.right, (SideMeaning::AsConfigured, vec![]));
        assert!(none.broad);
    }

    #[test]
    fn tags_are_ignored_for_synonyms() {
        let sides = test_sides(SetKind::Synonym, "s", &names(&["s"]), "vip").unwrap();
        assert_eq!(sides.right, (SideMeaning::AsConfigured, vec![]));
    }

    #[test]
    fn an_unattached_curation_set_is_blocked() {
        assert_eq!(
            test_sides(SetKind::Curation, "c", &names(&["d"]), ""),
            Err(TestBlocked::NotAttached)
        );
        assert_eq!(
            test_sides(SetKind::Curation, "c", &[], ""),
            Err(TestBlocked::NotAttached)
        );
    }

    #[test]
    fn an_unattached_synonym_set_with_a_comma_cannot_be_sent() {
        assert_eq!(
            test_sides(SetKind::Synonym, "a,b", &[], ""),
            Err(TestBlocked::NameNotSendable)
        );
        // Attached, it is not sent by name.
        assert!(test_sides(SetKind::Synonym, "a,b", &names(&["a,b"]), "").is_ok());
    }
}
