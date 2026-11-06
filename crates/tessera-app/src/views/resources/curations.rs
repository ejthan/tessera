// SPDX-License-Identifier: GPL-3.0-or-later

//! What the curation kind adds to [`SetsView`](super::sets::SetsView): the item table.

use rust_i18n::t;
use serde_json::Value;
use tessera_core::relevance::curations::{CurationDraft, RuleTime};

use super::{ListRow, ResourceColumn};

/// The columns of the item table.
pub(super) const ITEM_COLUMNS: &[ResourceColumn] = &[
    ("id", 130., false),
    ("trigger", 170., false),
    ("includes", 70., true),
    ("excludes", 70., true),
    ("active", 90., false),
    ("stops", 60., false),
];

fn trigger(draft: &CurationDraft) -> String {
    let rule = &draft.rule;
    if !rule.query.is_empty() {
        format!("{} ({})", rule.query, rule.r#match)
    } else if !rule.tags.is_empty() {
        rule.tags.join(", ")
    } else {
        rule.filter_by.clone()
    }
}

/// The row of a stored item at `now`. One the form cannot read still shows its id.
pub(super) fn item_row(item: &Value, now: i64) -> ListRow {
    let key = item.get("id").and_then(Value::as_str).unwrap_or_default();
    let Ok(draft) = CurationDraft::from_value(item) else {
        return ListRow {
            key: key.to_string(),
            cells: vec![
                key.to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
            warning: None,
        };
    };
    let active = match draft.timing(now) {
        RuleTime::Scheduled => t!("curations.active.scheduled"),
        RuleTime::Active => t!("curations.active.active"),
        RuleTime::Expired => t!("curations.active.expired"),
    };
    let stops = if draft.stop_processing {
        t!("curations.stops.yes")
    } else {
        t!("curations.stops.no")
    };
    ListRow {
        key: key.to_string(),
        cells: vec![
            key.to_string(),
            trigger(&draft),
            draft.includes.len().to_string(),
            draft.excludes.len().to_string(),
            active.to_string(),
            stops.to_string(),
        ],
        warning: None,
    }
}
