// SPDX-License-Identifier: GPL-3.0-or-later

//! Splits document ids into `id:[...]` filters short enough for one request.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Filters to send and ids that no filter can express.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdBatches {
    /// ``id:[`a`,`b`]`` filters, each at most `max_bytes` once percent-encoded.
    pub filters: Vec<String>,
    /// Ids with a backtick, which `filter_by` cannot quote; delete these one by one.
    pub single: Vec<String>,
}

/// Percent-encoded length. Encodes every non-alphanumeric, so it never undercounts the client.
fn encoded_len(s: &str) -> usize {
    utf8_percent_encode(s, NON_ALPHANUMERIC).map(str::len).sum()
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
        if id.contains('`') {
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
