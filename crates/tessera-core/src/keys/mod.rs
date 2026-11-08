// SPDX-License-Identifier: GPL-3.0-or-later

//! API keys: what a form may ask for, when a key ends, and scoped keys.

pub mod actions;
pub mod scoped;

use jiff::Timestamp;
use tessera_client::{ApiKey, NewKey};

pub use actions::{ACTION_GROUPS, ActionGroup};

/// The `expires_at` of a key that was created without one.
pub const NEVER: i64 = 64_723_363_199;
/// Days before its end a key counts as ending soon.
pub const EXPIRY_WARNING_DAYS: i64 = 7;

/// Where a key stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// It has no end.
    Never,
    /// It stopped working.
    Expired,
    /// It ends within [`EXPIRY_WARNING_DAYS`].
    Soon,
    /// It ends later.
    Later,
}

/// Where a key with `expires_at` stands at `now`.
pub fn expiry(expires_at: Option<i64>, now: Timestamp) -> Expiry {
    let Some(at) = expires_at.filter(|at| *at < NEVER) else {
        return Expiry::Never;
    };
    let left = at.saturating_sub(now.as_second());
    if left <= 0 {
        Expiry::Expired
    } else if left <= EXPIRY_WARNING_DAYS * 86_400 {
        Expiry::Soon
    } else {
        Expiry::Later
    }
}

/// A starting point of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// Searches any collection.
    SearchOnly,
    /// Imports into any collection.
    ImportOnly,
    /// May do everything.
    Admin,
}

impl Template {
    /// The actions it fills in.
    pub fn actions(self) -> Vec<&'static str> {
        match self {
            Self::SearchOnly => vec!["documents:search"],
            Self::ImportOnly => vec!["documents:import", "collections:get"],
            Self::Admin => vec!["*"],
        }
    }
}

/// Why a draft cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProblem {
    /// The description is blank.
    NoDescription,
    /// No action is chosen.
    NoActions,
    /// No collection is named.
    NoCollections,
    /// The server takes such a date and makes a key that never works.
    ExpiryPast,
}

/// What stops `draft` from being sent.
pub fn check_new_key(draft: &NewKey, now: Timestamp) -> Vec<KeyProblem> {
    let past = draft.expires_at.is_some_and(|at| at <= now.as_second());
    [
        (
            draft.description.trim().is_empty(),
            KeyProblem::NoDescription,
        ),
        (draft.actions.is_empty(), KeyProblem::NoActions),
        (draft.collections.is_empty(), KeyProblem::NoCollections),
        (past, KeyProblem::ExpiryPast),
    ]
    .into_iter()
    .filter_map(|(found, problem)| found.then_some(problem))
    .collect()
}

/// The collections of `text`: one per line, trimmed. A comma belongs to a regex.
pub fn collections_of(text: &str) -> Vec<String> {
    let lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.map(str::to_string).collect()
}

/// What the key list says about a parent key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentNote {
    /// A listed key of this prefix can sign.
    SearchOnly,
    /// The key of this id holds other actions, so the server refuses what it signs.
    NotSearchOnly(u64),
    /// The key of this id could sign, but it ended.
    Expired(u64),
    /// No listed key has this prefix. The bootstrap key is one of these.
    Unknown,
}

/// What the keys with the prefix of `parent` allow at `now`. Prefixes can repeat, so a fitting
/// key wins over the others.
pub fn parent_note(keys: &[ApiKey], parent: &str, now: Timestamp) -> ParentNote {
    let prefix: String = parent.chars().take(4).collect();
    let same = || {
        keys.iter()
            .filter(|k| k.value_prefix.as_ref() == Some(&prefix))
    };
    let signs = |k: &&ApiKey| k.actions == ["documents:search"];
    let alive = |k: &&ApiKey| expiry(k.expires_at, now) != Expiry::Expired;
    if same().filter(signs).any(|k| alive(&k)) {
        ParentNote::SearchOnly
    } else if let Some(key) = same().find(signs) {
        ParentNote::Expired(key.id)
    } else {
        same()
            .next()
            .map_or(ParentNote::Unknown, |k| ParentNote::NotSearchOnly(k.id))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    fn now() -> Timestamp {
        Timestamp::from_second(NOW).unwrap_or(Timestamp::UNIX_EPOCH)
    }

    fn key(id: u64, prefix: &str, actions: &[&str], expires_at: i64) -> ApiKey {
        ApiKey {
            id,
            description: String::new(),
            actions: actions.iter().map(ToString::to_string).collect(),
            collections: vec!["*".into()],
            expires_at: Some(expires_at),
            value_prefix: Some(prefix.into()),
            extra: Map::new(),
        }
    }

    fn draft() -> NewKey {
        NewKey {
            description: "d".into(),
            actions: vec!["*".into()],
            collections: vec!["*".into()],
            expires_at: None,
        }
    }

    #[test]
    fn expiry_boundaries() {
        for (at, want) in [
            (None, Expiry::Never),
            (Some(NEVER), Expiry::Never),
            (Some(NEVER + 1), Expiry::Never),
            (Some(NEVER - 1), Expiry::Later),
            (Some(NOW - 1), Expiry::Expired),
            (Some(NOW), Expiry::Expired),
            (Some(NOW + 1), Expiry::Soon),
            (Some(NOW + 7 * DAY), Expiry::Soon),
            (Some(NOW + 7 * DAY + 1), Expiry::Later),
            (Some(i64::MIN), Expiry::Expired),
        ] {
            assert_eq!(expiry(at, now()), want, "{at:?}");
        }
    }

    #[test]
    fn collections_of_keeps_a_regex_whole() {
        let text = " a{1,2}\n\n  org_.* \r\nb\n";
        assert_eq!(collections_of(text), ["a{1,2}", "org_.*", "b"]);
    }

    #[test]
    fn a_good_draft_has_no_problem() {
        assert_eq!(check_new_key(&draft(), now()), []);
    }

    #[test]
    fn every_problem_is_named() {
        let bad = NewKey {
            description: "  ".into(),
            actions: Vec::new(),
            collections: Vec::new(),
            expires_at: Some(NOW),
        };
        assert_eq!(
            check_new_key(&bad, now()),
            [
                KeyProblem::NoDescription,
                KeyProblem::NoActions,
                KeyProblem::NoCollections,
                KeyProblem::ExpiryPast
            ]
        );
    }

    #[test]
    fn templates_fill_the_form() {
        assert_eq!(Template::SearchOnly.actions(), ["documents:search"]);
        assert_eq!(
            Template::ImportOnly.actions(),
            ["documents:import", "collections:get"]
        );
        assert_eq!(Template::Admin.actions(), ["*"]);
    }

    #[test]
    fn parent_note_reads_the_list() {
        let search = ["documents:search"];
        let later = NOW + 100 * DAY;
        let keys = [
            key(1, "abcd", &["*"], later),
            key(2, "dead", &search, NOW - 1),
            key(3, "more", &["documents:search", "documents:get"], later),
        ];
        let note = |parent| parent_note(&keys, parent, now());
        assert_eq!(note("abcdefgh"), ParentNote::NotSearchOnly(1));
        assert_eq!(note("deadbeef"), ParentNote::Expired(2));
        assert_eq!(note("moremore"), ParentNote::NotSearchOnly(3));
        assert_eq!(note("xyz"), ParentNote::Unknown);
        assert_eq!(note(""), ParentNote::Unknown);
    }

    #[test]
    fn parent_note_with_two_keys_of_one_prefix() {
        let later = NOW + 100 * DAY;
        let keys = [
            key(1, "abcd", &["*"], later),
            key(2, "abcd", &["documents:search"], later),
        ];
        assert_eq!(
            parent_note(&keys, "abcdefgh", now()),
            ParentNote::SearchOnly
        );
    }
}
