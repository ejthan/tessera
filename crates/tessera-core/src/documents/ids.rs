// SPDX-License-Identifier: GPL-3.0-or-later

//! Splits document ids into `id:[...]` filters short enough for one request.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Filters to send and ids that no filter can express.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdBatches {
    /// ``id:[`a`,`b`]`` filters, each at most `max_bytes` once percent-encoded.
    pub filters: Vec<String>,
    /// Ids `filter_by` cannot name exactly; these go one by one.
    pub single: Vec<String>,
}

/// Percent-encoded length. Encodes every non-alphanumeric, so it never undercounts the client.
fn encoded_len(s: &str) -> usize {
    utf8_percent_encode(s, NON_ALPHANUMERIC).map(str::len).sum()
}

/// Whether an `id:[...]` filter would miss `id` or hit others: a backtick cannot be quoted,
/// `*` matches every document, and the server trims ids, so `a ` means the document `a`.
fn needs_its_own_request(id: &str) -> bool {
    id.contains(['`', '*']) || id.trim() != id
}

/// Groups `ids` in order into filters of at most `max_bytes` after percent-encoding.
/// An id that alone exceeds `max_bytes` still gets a filter of its own.
pub fn id_batches(ids: &[String], max_bytes: usize) -> IdBatches {
    const OPEN: &str = "id:[";
    let overhead = encoded_len(OPEN) + encoded_len("]");
    let mut out = IdBatches::default();
    let mut items: Vec<String> = Vec::new();
    let mut len = overhead;
    let flush = |items: &mut Vec<String>, filters: &mut Vec<String>| {
        if !items.is_empty() {
            filters.push(format!("{OPEN}{}]", items.join(",")));
            items.clear();
        }
    };
    for id in ids {
        if needs_its_own_request(id) {
            out.single.push(id.clone());
            continue;
        }
        let item = format!("`{id}`");
        // A comma separates items, so every item but the first pays for one.
        let cost = encoded_len(&item)
            + if items.is_empty() {
                0
            } else {
                encoded_len(",")
            };
        if !items.is_empty() && len + cost > max_bytes {
            flush(&mut items, &mut out.filters);
            len = overhead;
            items.push(item);
            len += encoded_len(items.last().map_or("", String::as_str));
        } else {
            len += cost;
            items.push(item);
        }
    }
    flush(&mut items, &mut out.filters);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("doc {i}")).collect()
    }

    #[test]
    fn empty_input_gives_nothing() {
        assert_eq!(id_batches(&[], 100), IdBatches::default());
    }

    #[test]
    fn ids_are_backtick_quoted() {
        let b = id_batches(&["a".into(), "b,c".into()], 1000);
        assert_eq!(b.filters, vec!["id:[`a`,`b,c`]"]);
    }

    #[test]
    fn backtick_ids_go_to_single() {
        let b = id_batches(&["a".into(), "x`y".into()], 1000);
        assert_eq!(b.filters, vec!["id:[`a`]"]);
        assert_eq!(b.single, vec!["x`y"]);
    }

    #[test]
    fn ids_a_filter_would_not_match_exactly_go_to_single() {
        // `*` matches every document, and the server trims ids in a filter, so `keep1 ` would
        // delete `keep1` and leave itself.
        let odd = [
            "*", " * ", "a*", "*a", "keep1 ", " lead", "trail ", "  ", "tab\t", "\nx",
        ];
        let mut all: Vec<String> = odd.map(String::from).to_vec();
        all.push("plain".into());
        let b = id_batches(&all, 1000);
        assert_eq!(b.filters, vec!["id:[`plain`]"]);
        assert_eq!(b.single, odd);
    }

    #[test]
    fn inner_spaces_and_dots_stay_in_filters() {
        let b = id_batches(&["a b".into(), ".".into(), "..".into()], 1000);
        assert_eq!(b.filters, vec!["id:[`a b`,`.`,`..`]"]);
        assert!(b.single.is_empty());
    }

    #[test]
    fn a_batch_never_exceeds_the_limit_and_keeps_every_id_once() {
        let all = ids(500);
        let max = 400;
        let b = id_batches(&all, max);
        assert!(b.filters.len() > 1);
        let mut seen = Vec::new();
        for f in &b.filters {
            assert!(encoded_len(f) <= max, "{} > {max}", encoded_len(f));
            let inner = f.strip_prefix("id:[").and_then(|f| f.strip_suffix(']'));
            seen.extend(
                inner
                    .unwrap()
                    .split(',')
                    .map(|s| s.trim_matches('`').to_string()),
            );
        }
        assert_eq!(seen, all);
    }

    #[test]
    fn an_oversized_id_gets_its_own_filter() {
        let b = id_batches(&["a".repeat(100), "b".into()], 50);
        assert_eq!(b.filters.len(), 2);
    }
}
