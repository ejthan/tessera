// SPDX-License-Identifier: GPL-3.0-or-later

//! `sort_by` as rows of the form.

/// Typesense accepts at most three sort criteria.
const MAX_ROWS: usize = 3;

/// One sort criterion: a field or an expression such as `_eval(x:>1)`, and its direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortRow {
    /// Field name or expression, without the direction.
    pub expr: String,
    /// Descending.
    pub desc: bool,
}

/// Splits at commas outside `(…)`, `[…]` and backticks. `None` when they do not balance.
fn split_top(text: &str) -> Option<Vec<&str>> {
    let (mut depth, mut tick, mut start) = (0usize, false, 0);
    let mut parts = Vec::new();
    for (i, c) in text.char_indices() {
        match c {
            '`' => tick = !tick,
            _ if tick => {}
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if tick || depth != 0 {
        return None;
    }
    parts.push(&text[start..]);
    Some(parts)
}

/// The rows of `text`. `None` when it has more than three criteria, an empty one, or
/// unbalanced brackets. A criterion without a direction counts as descending, the
/// server's default for a bare field.
pub fn parse_sort(text: &str) -> Option<Vec<SortRow>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    let parts = split_top(text)?;
    if parts.len() > MAX_ROWS {
        return None;
    }
    parts
        .into_iter()
        .map(|part| {
            let part = part.trim();
            let (expr, desc) = match part.rsplit_once(':') {
                Some((expr, "asc")) => (expr, false),
                Some((expr, "desc")) => (expr, true),
                _ => (part, true),
            };
            let expr = expr.trim();
            (!expr.is_empty()).then(|| SortRow {
                expr: expr.to_string(),
                desc,
            })
        })
        .collect()
}

/// `a:desc,b:asc`.
pub fn sort_text(rows: &[SortRow]) -> String {
    rows.iter()
        .map(|r| format!("{}:{}", r.expr, if r.desc { "desc" } else { "asc" }))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(expr: &str, desc: bool) -> SortRow {
        SortRow {
            expr: expr.into(),
            desc,
        }
    }

    #[test]
    fn parse_sort_table() {
        let cases: [(&str, Option<Vec<SortRow>>); 8] = [
            ("a:desc,b:asc", Some(vec![row("a", true), row("b", false)])),
            ("_text_match:desc", Some(vec![row("_text_match", true)])),
            ("_eval(x:>1):desc", Some(vec![row("_eval(x:>1)", true)])),
            (
                "_eval([(a:1):2,(b:2):1]):desc",
                Some(vec![row("_eval([(a:1):2,(b:2):1])", true)]),
            ),
            ("a:asc,b:asc,c:asc,d:asc", None),
            ("a", Some(vec![row("a", true)])),
            ("a,,b", None),
            ("_eval(a:1:desc", None),
        ];
        for (text, want) in cases {
            assert_eq!(parse_sort(text), want, "{text}");
        }
        assert_eq!(parse_sort(""), Some(vec![]));
    }

    #[test]
    fn a_bare_field_is_written_descending() {
        assert_eq!(sort_text(&parse_sort("a").unwrap()), "a:desc");
    }

    #[test]
    fn sort_text_round_trips() {
        for text in [
            "a:desc,b:asc",
            "_eval([(a:1):2,(b:2):1]):desc",
            "_text_match:desc,n:asc,_eval(x:>1):asc",
        ] {
            assert_eq!(sort_text(&parse_sort(text).unwrap()), text);
        }
    }
}
