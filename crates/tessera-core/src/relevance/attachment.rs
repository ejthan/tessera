// SPDX-License-Identifier: GPL-3.0-or-later

//! Which collection uses which set, and the lists a `PATCH` may send.

use serde_json::{Value, json};
use tessera_client::{Collection, SetKind};

/// The sets of `kind` attached to `collection`.
pub fn attached(collection: &Collection, kind: SetKind) -> &[String] {
    match kind {
        SetKind::Synonym => &collection.synonym_sets,
        SetKind::Curation => &collection.curation_sets,
    }
}

/// The collections that name the set.
pub fn users<'a>(collections: &'a [Collection], kind: SetKind, name: &str) -> Vec<&'a str> {
    collections
        .iter()
        .filter(|c| attached(c, kind).iter().any(|n| n == name))
        .map(|c| c.name.as_str())
        .collect()
}

/// A set that collections name and the server no longer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    /// The set name.
    pub set: String,
    /// The collections that still name it.
    pub collections: Vec<String>,
}

/// Names attached to a collection that are not among the `known` sets. A missing synonym set
/// fails every search on its collection.
pub fn missing(collections: &[Collection], kind: SetKind, known: &[String]) -> Vec<Missing> {
    let mut out: Vec<Missing> = Vec::new();
    for collection in collections {
        for name in attached(collection, kind)
            .iter()
            .filter(|n| !known.contains(n))
        {
            match out.iter_mut().find(|m| &m.set == name) {
                Some(m) => m.collections.push(collection.name.clone()),
                None => out.push(Missing {
                    set: name.clone(),
                    collections: vec![collection.name.clone()],
                }),
            }
        }
    }
    out
}

/// The list to send, and the names left out because the server no longer has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewList {
    /// The names to send.
    pub list: Vec<String>,
    /// Names that are not known: a `PATCH` listing one fails with 404.
    pub dropped: Vec<String>,
}

fn keep_known(current: &[String], known: &[String], skip: &str) -> NewList {
    let (list, dropped) = current
        .iter()
        .filter(|n| n.as_str() != skip)
        .cloned()
        .partition(|n| known.contains(n));
    NewList { list, dropped }
}

/// No set is known while a collection names others: this looks like a failed list, and going
/// on would detach them all. A caller treats a failed list as an error, never as empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no sets are known, yet the collection names some")]
pub struct NoKnownSets;

fn refuse_empty(current: &[String], known: &[String], name: &str) -> Result<(), NoKnownSets> {
    if known.is_empty() && current.iter().any(|n| n != name) {
        return Err(NoKnownSets);
    }
    Ok(())
}

/// `current` plus `name`, in order. `known` and `current` must have been read right before:
/// a stale `known` would detach someone else's new set, a stale `current` undo their attach.
/// A `name` that is not known is dropped, never listed.
pub fn with_set(current: &[String], known: &[String], name: &str) -> Result<NewList, NoKnownSets> {
    refuse_empty(current, known, name)?;
    let mut new = keep_known(current, known, "");
    if !new.list.iter().any(|n| n == name) && !new.dropped.iter().any(|n| n == name) {
        if known.iter().any(|n| n == name) {
            new.list.push(name.to_string());
        } else {
            new.dropped.push(name.to_string());
        }
    }
    Ok(new)
}

/// `current` without `name`, in order. Other names that are not known are dropped.
pub fn without_set(
    current: &[String],
    known: &[String],
    name: &str,
) -> Result<NewList, NoKnownSets> {
    refuse_empty(current, known, name)?;
    Ok(keep_known(current, known, name))
}

/// The body of the `PATCH` that replaces the whole list.
pub fn attachment_patch(kind: SetKind, list: &[String]) -> Value {
    json!({ kind.path(): list })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(name: &str, synonyms: &[&str], curations: &[&str]) -> Collection {
        serde_json::from_value(json!({
            "name": name,
            "synonym_sets": synonyms,
            "curation_sets": curations,
        }))
        .unwrap()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn attached_and_users_follow_the_kind() {
        let all = [
            collection("a", &["s"], &["c"]),
            collection("b", &["s", "t"], &[]),
            collection("c", &[], &["s"]),
        ];
        assert_eq!(attached(&all[1], SetKind::Synonym), ["s", "t"]);
        assert_eq!(users(&all, SetKind::Synonym, "s"), ["a", "b"]);
        assert_eq!(users(&all, SetKind::Curation, "s"), ["c"]);
        assert!(users(&all, SetKind::Curation, "none").is_empty());
    }

    #[test]
    fn missing_groups_the_collections_by_set() {
        let all = [
            collection("a", &["gone", "s"], &[]),
            collection("b", &["gone", "also"], &[]),
        ];
        assert_eq!(
            missing(&all, SetKind::Synonym, &names(&["s"])),
            [
                Missing {
                    set: "gone".into(),
                    collections: names(&["a", "b"])
                },
                Missing {
                    set: "also".into(),
                    collections: names(&["b"])
                },
            ]
        );
        assert!(missing(&all, SetKind::Curation, &[]).is_empty());
    }

    #[test]
    fn with_set_keeps_order_and_never_lists_a_missing_name() {
        let got = with_set(&names(&["b", "gone", "a"]), &names(&["a", "b", "c"]), "c").unwrap();
        assert_eq!(got.list, ["b", "a", "c"]);
        assert_eq!(got.dropped, ["gone"]);
    }

    #[test]
    fn with_set_does_not_list_a_name_twice() {
        let got = with_set(&names(&["b", "a"]), &names(&["a", "b"]), "b").unwrap();
        assert_eq!(got.list, ["b", "a"]);
        assert!(got.dropped.is_empty());
    }

    #[test]
    fn with_set_refuses_a_name_the_server_does_not_have() {
        let got = with_set(&names(&["a"]), &names(&["a"]), "new").unwrap();
        assert_eq!(got.list, ["a"]);
        assert_eq!(got.dropped, ["new"]);
    }

    #[test]
    fn without_set_keeps_order_and_drops_unknown_names() {
        let got = without_set(
            &names(&["b", "gone", "a", "c"]),
            &names(&["a", "b", "c"]),
            "a",
        )
        .unwrap();
        assert_eq!(got.list, ["b", "c"]);
        assert_eq!(got.dropped, ["gone"]);
        // Detaching a set that no longer exists is not reported as a drop of its own.
        let got = without_set(&names(&["gone"]), &[], "gone").unwrap();
        assert!(got.list.is_empty() && got.dropped.is_empty());
    }

    #[test]
    fn a_name_in_the_list_that_the_server_lost_is_dropped_not_listed() {
        let got = with_set(&names(&["a", "gone"]), &names(&["a"]), "gone").unwrap();
        assert_eq!(got.list, ["a"]);
        assert_eq!(got.dropped, ["gone"]);
    }

    #[test]
    fn no_known_sets_never_empties_a_list() {
        let current = names(&["a", "b"]);
        assert_eq!(with_set(&current, &[], "c"), Err(NoKnownSets));
        assert_eq!(with_set(&current, &[], "a"), Err(NoKnownSets));
        assert_eq!(without_set(&current, &[], "a"), Err(NoKnownSets));
        // Nothing attached: there is nothing a failed list could take away.
        assert_eq!(with_set(&[], &[], "c").unwrap().dropped, ["c"]);
    }

    #[test]
    fn the_patch_replaces_the_whole_list_under_the_kinds_key() {
        assert_eq!(
            attachment_patch(SetKind::Synonym, &names(&["a", "b"])),
            json!({"synonym_sets": ["a", "b"]})
        );
        assert_eq!(
            attachment_patch(SetKind::Curation, &[]),
            json!({"curation_sets": []})
        );
    }
}
