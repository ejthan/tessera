// SPDX-License-Identifier: GPL-3.0-or-later

//! The search workbench's logic: drafts, sort rows, results and facet filters.

pub mod facet;
pub mod params;
pub mod results;
pub mod sort;

/// An instant search waits this long after the last keystroke.
pub const INSTANT_DEBOUNCE_MS: u64 = 150;

/// An instant search enters history only when it stood this long.
pub const HISTORY_SETTLE_MS: u64 = 2000;
