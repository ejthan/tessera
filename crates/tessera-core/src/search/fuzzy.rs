// SPDX-License-Identifier: GPL-3.0-or-later

//! Fuzzy matching for the command palette.

/// The palette shows at most this many rows.
pub const PALETTE_LIMIT: usize = 100;

const MATCH: u32 = 1;
const CONSECUTIVE: u32 = 4;
const WORD_START: u32 = 8;
const PREFIX: u32 = 1 << 12;
const EXACT: u32 = 1 << 13;

/// How well `query` matches `text`, higher is better: a case-insensitive subsequence with a
/// bonus for word starts and consecutive characters, above them a prefix, above that the
/// whole text. `None` when `query` is no subsequence. An empty query matches everything.
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().flat_map(|c| c.to_lowercase()).collect();
    // `to_lowercase` can lengthen a text; word starts are then only looked for in `lower`.
    let source = if lower.len() == chars.len() {
        &chars
    } else {
        &lower
    };
    let word_start = |j: usize| {
        j == 0
            || !source[j - 1].is_alphanumeric()
            || (source[j - 1].is_lowercase() && source[j].is_uppercase())
    };
    // Best score of an alignment of the query so far that ends at each position of `lower`.
    let mut ending: Vec<Option<u32>> = vec![None; lower.len()];
    for (i, q) in query.iter().enumerate() {
        let mut next = vec![None; lower.len()];
        let mut before = (i == 0).then_some(0);
        for j in 0..lower.len() {
            let run = j
                .checked_sub(1)
                .and_then(|p| ending[p])
                .map(|s| s + CONSECUTIVE);
            if lower[j] == *q && (before.is_some() || run.is_some()) {
                let start = if word_start(j) { WORD_START } else { 0 };
                next[j] = before.max(run).map(|s| s + MATCH + start);
            }
            before = before.max(ending[j]);
        }
        ending = next;
    }
    let best = ending.into_iter().flatten().max()?;
    let whole = lower.len() == query.len();
    let prefix = lower.starts_with(&query);
    Some(
        best + if !prefix {
            0
        } else if whole {
            EXACT
        } else {
            PREFIX
        },
    )
}

/// `items` that match `query`, best first, at most `limit`. Equal scores keep their order.
pub fn rank<T>(query: &str, items: Vec<T>, text: impl Fn(&T) -> &str, limit: usize) -> Vec<T> {
    let mut scored: Vec<(u32, T)> = items
        .into_iter()
        .filter_map(|item| Some((score(query, text(&item))?, item)))
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.truncate(limit);
    scored.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_beats_prefix_beats_scattered() {
        let exact = score("books", "books");
        let prefix = score("books", "books_archive");
        let scattered = score("books", "big_orange_ok_stats");
        assert!(exact > prefix, "{exact:?} {prefix:?}");
        assert!(prefix > scattered, "{prefix:?} {scattered:?}");
        assert!(scattered.is_some());
    }

    #[test]
    fn word_starts_win() {
        assert!(score("dd", "Delete documents") > score("dd", "Hidden items"));
        assert!(score("nd", "New document") > score("nd", "Standard"));
        assert!(score("to", "tessera_orders") > score("to", "tessera"));
    }

    #[test]
    fn the_best_alignment_counts() {
        // The first d of "add" must not hide the word start.
        assert!(score("do", "Add document") > score("do", "Addendum to"));
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(score("NEW", "new"), score("new", "NEW"));
        assert!(score("i", "İstanbul").is_some());
    }

    #[test]
    fn empty_query_keeps_order() {
        assert_eq!(score("", "anything"), Some(0));
        let ranked = rank("", vec!["c", "a", "b"], |s| s, 10);
        assert_eq!(ranked, ["c", "a", "b"]);
    }

    #[test]
    fn no_subsequence_is_none() {
        assert_eq!(score("xyz", "books"), None);
        assert_eq!(score("skoob", "books"), None);
        assert_eq!(score("a", ""), None);
    }

    #[test]
    fn rank_limits() {
        let ranked = rank("a", vec!["bab", "a", "xa", "ab"], |s| s, 2);
        assert_eq!(ranked, ["a", "ab"]);
    }
}
