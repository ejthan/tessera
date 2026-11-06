// SPDX-License-Identifier: GPL-3.0-or-later

//! The search workbench's logic: drafts, sort rows, results and facet filters.

pub mod export;
pub mod facet;
pub mod history;
pub mod params;
pub mod results;
pub mod saved;
pub mod sort;

/// An instant search waits this long after the last keystroke.
pub const INSTANT_DEBOUNCE_MS: u64 = 150;

/// An instant search enters history only when it stood this long.
pub const HISTORY_SETTLE_MS: u64 = 2000;

/// Splits `text` at `sep` outside backticks, parentheses and brackets. `None` when they do
/// not balance.
pub(crate) fn split_top<'a>(text: &'a str, sep: &str) -> Option<Vec<&'a str>> {
    let (mut depth, mut tick, mut start, mut i) = (0usize, false, 0, 0);
    let mut parts = Vec::new();
    while i < text.len() {
        let c = text[i..].chars().next()?;
        if c == '`' {
            tick = !tick;
        } else if !tick {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.checked_sub(1)?,
                _ if depth == 0 && text[i..].starts_with(sep) => {
                    parts.push(&text[start..i]);
                    i += sep.len();
                    start = i;
                    continue;
                }
                _ => {}
            }
        }
        i += c.len_utf8();
    }
    if tick || depth != 0 {
        return None;
    }
    parts.push(&text[start..]);
    Some(parts)
}
