// SPDX-License-Identifier: GPL-3.0-or-later

//! Document browser logic: columns, pages, column preferences and filter completion.

pub mod columns;
pub mod edit;
pub mod filter;
pub mod page;
pub mod template;
pub mod validate;
pub mod views;

/// Documents per page request (the server's maximum).
pub const PER_PAGE: u32 = 250;
/// Pages kept in memory.
pub const PAGE_CACHE: usize = 20;
/// Longest cell preview, in characters.
pub const CELL_LIMIT: usize = 200;
/// Deleting or updating more documents than this needs a confirmation.
pub const CONFIRM_ABOVE: u64 = 1000;
/// Most documents that can be selected at once.
pub const MAX_SELECTION: usize = 1000;
/// Longest `id: [...]` filter, in bytes.
pub const ID_FILTER_BYTES: usize = 7500;
