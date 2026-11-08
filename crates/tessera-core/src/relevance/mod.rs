// SPDX-License-Identifier: GPL-3.0-or-later

//! Aliases, sets and the other resources that shape search results.

use tessera_client::{Alias, Collection};

/// The longest name Tessera creates. The server has no limit; a URL and a tab should.
pub const MAX_NAME: usize = 128;

/// Whether Tessera creates a resource called `name`.
pub fn name_ok(name: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    name.len() <= MAX_NAME
        && name.chars().next().is_some_and(word)
        && name.chars().all(|c| word(c) || matches!(c, '.' | '-'))
}

/// Whether `name` fits a search parameter, which lists names split at commas.
pub fn param_safe(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.chars().any(|c| c == ',' || c.is_control())
}

/// What is wrong with an alias, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasState {
    /// It resolves to a collection.
    Ok,
    /// Its target does not exist.
    TargetMissing,
    /// Its target is empty.
    TargetEmpty,
    /// Its target is another alias, which does not resolve.
    TargetIsAlias,
    /// A collection of the same name wins over it.
    Shadowed,
}

/// The state of `alias` among the loaded `collections` and `aliases`.
pub fn alias_state(alias: &Alias, collections: &[Collection], aliases: &[Alias]) -> AliasState {
    let is_collection = |name: &str| collections.iter().any(|c| c.name == name);
    let target = alias.collection_name.as_str();
    if is_collection(&alias.name) {
        AliasState::Shadowed
    } else if target.is_empty() {
        AliasState::TargetEmpty
    } else if is_collection(target) {
        AliasState::Ok
    } else if aliases
        .iter()
        .any(|a| a.name == target && a.name != alias.name)
    {
        AliasState::TargetIsAlias
    } else {
        AliasState::TargetMissing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_ok_is_the_allowlist() {
        let longest = "a".repeat(MAX_NAME);
        let too_long = "a".repeat(MAX_NAME + 1);
        for good in ["a", "_a", "a.b-c_1", "0", longest.as_str()] {
            assert!(name_ok(good), "{good}");
        }
        for bad in ["", ".a", "-a", "a b", "a,b", "é", too_long.as_str()] {
            assert!(!name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn param_safe_refuses_what_a_list_cannot_hold() {
        for good in ["a", "a b", "é", "a/b", ".a"] {
            assert!(param_safe(good), "{good}");
        }
        for bad in ["", "a,b", " a", "a ", "a\nb", "a\tb", "\u{7f}a"] {
            assert!(!param_safe(bad), "{bad:?}");
        }
    }

    fn alias(name: &str, target: &str) -> Alias {
        Alias {
            name: name.into(),
            collection_name: target.into(),
        }
    }

    fn collection(name: &str) -> Collection {
        Collection {
            name: name.into(),
            ..Collection::default()
        }
    }

    #[test]
    fn an_alias_on_a_collection_is_ok() {
        let state = alias_state(&alias("a", "c"), &[collection("c")], &[alias("a", "c")]);
        assert_eq!(state, AliasState::Ok);
    }

    #[test]
    fn a_missing_target_shows() {
        let state = alias_state(
            &alias("a", "gone"),
            &[collection("c")],
            &[alias("a", "gone")],
        );
        assert_eq!(state, AliasState::TargetMissing);
    }

    #[test]
    fn an_empty_target_shows() {
        assert_eq!(
            alias_state(&alias("a", ""), &[], &[]),
            AliasState::TargetEmpty
        );
    }

    #[test]
    fn a_target_that_is_an_alias_shows() {
        let aliases = [alias("a", "b"), alias("b", "c")];
        let state = alias_state(&aliases[0], &[collection("c")], &aliases);
        assert_eq!(state, AliasState::TargetIsAlias);
    }

    #[test]
    fn a_collection_of_the_targets_name_wins_over_an_alias_of_it() {
        let aliases = [alias("a", "b"), alias("b", "c")];
        let state = alias_state(&aliases[0], &[collection("b")], &aliases);
        assert_eq!(state, AliasState::Ok);
    }

    #[test]
    fn a_collection_of_the_same_name_shadows_whatever_the_target() {
        for target in ["", "gone", "b", "a"] {
            let aliases = [alias("a", target), alias("b", "c")];
            let state = alias_state(&aliases[0], &[collection("a")], &aliases);
            assert_eq!(state, AliasState::Shadowed, "{target}");
        }
    }
}
