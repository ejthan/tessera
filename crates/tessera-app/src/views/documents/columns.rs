// SPDX-License-Identifier: GPL-3.0-or-later

//! Which columns the documents table shows, pins and how wide: the rules and the picker.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{Entity, ParentElement as _, SharedString, Styled as _, Window};
use rust_i18n::t;
use tessera_core::documents::columns::ColumnDef;
use tessera_core::documents::views::CollectionView;

use super::DocumentsView;
use crate::actions::{DOCUMENTS, ToggleColumns};

/// Narrowest a column may be made.
pub const MIN_WIDTH: f32 = 60.;
const MAX_WIDTH: f32 = 1200.;
/// What a keyboard resize changes a width by.
pub const STEP: f32 = 20.;

/// Pinned columns in pin order, then the others in schema order; hidden ones left out.
pub fn visible_columns(all: &[ColumnDef], prefs: &CollectionView) -> Vec<ColumnDef> {
    let shown = |c: &&ColumnDef| !prefs.hidden.contains(&c.key);
    let pinned = prefs
        .pinned
        .iter()
        .filter_map(|key| all.iter().find(|c| c.key == *key))
        .filter(shown);
    let rest = all
        .iter()
        .filter(|c| !prefs.pinned.contains(&c.key))
        .filter(shown);
    let visible: Vec<_> = pinned.chain(rest).cloned().collect();
    // Saved settings that hide every column left of the schema would leave a blank table.
    if visible.is_empty() {
        all.to_vec()
    } else {
        visible
    }
}

/// Hides or shows `key`. False when it would hide the last visible column.
pub fn toggle_hidden(all: &[ColumnDef], prefs: &mut CollectionView, key: &str) -> bool {
    if let Some(i) = prefs.hidden.iter().position(|k| k == key) {
        prefs.hidden.remove(i);
        return true;
    }
    if visible_columns(all, prefs).len() <= 1 {
        return false;
    }
    prefs.hidden.push(key.to_string());
    true
}

/// Pins `key` after the other pinned columns, or unpins it.
pub fn toggle_pin(prefs: &mut CollectionView, key: &str) {
    match prefs.pinned.iter().position(|k| k == key) {
        Some(i) => {
            prefs.pinned.remove(i);
        }
        None => prefs.pinned.push(key.to_string()),
    }
}

/// Sets the width of `key` to `width`, kept within the limits.
pub fn set_width(prefs: &mut CollectionView, key: &str, width: f32) {
    prefs
        .widths
        .insert(key.to_string(), width.clamp(MIN_WIDTH, MAX_WIDTH));
}

/// The "Columns" button and its popover: a checkbox and a pin per column, and Reset.
pub fn picker(
    view: Entity<DocumentsView>,
    open: bool,
    all: Vec<ColumnDef>,
    prefs: CollectionView,
    window: &Window,
) -> Popover {
    let visible = visible_columns(&all, &prefs).len();
    let trigger = Button::new("documents.columns")
        .ghost()
        .small()
        .icon(IconName::Columns3)
        .label(t!("documents.columns").to_string())
        .children(Kbd::binding_for_action(
            &ToggleColumns,
            Some(DOCUMENTS),
            window,
        ));
    let on_open = view.clone();
    Popover::new("documents.columns.popover")
        .open(open)
        .on_open_change(move |open, _, cx| {
            on_open.update(cx, |v, cx| v.set_columns_open(*open, cx));
        })
        .trigger(trigger)
        .content(move |_, _, _| {
            let rows = all.iter().map(|c| {
                let key = c.key.clone();
                let hidden = prefs.hidden.contains(&key);
                let toggle = view.clone();
                let pin = view.clone();
                let (toggle_key, pin_key) = (key.clone(), key.clone());
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        Checkbox::new(SharedString::from(format!("documents.column.{key}")))
                            .label(key.clone())
                            .checked(!hidden)
                            // The last visible column stays.
                            .disabled(!hidden && visible <= 1)
                            .on_click(move |_, _, cx| {
                                toggle.update(cx, |v, cx| v.toggle_hidden(&toggle_key, cx));
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("documents.pin.{key}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Pin)
                            .selected(prefs.pinned.contains(&key))
                            .accessibility_label(t!("documents.columns_pin", key = key).to_string())
                            .on_click(move |_, _, cx| {
                                pin.update(cx, |v, cx| v.toggle_pin(&pin_key, cx));
                            }),
                    )
            });
            let reset = view.clone();
            v_flex().gap_1().p_1().children(rows).child(
                Button::new("documents.columns.reset")
                    .ghost()
                    .small()
                    .label(t!("documents.columns_reset").to_string())
                    .on_click(move |_, _, cx| reset.update(cx, |v, cx| v.reset_columns(cx))),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(key: &str) -> ColumnDef {
        ColumnDef {
            key: key.into(),
            sortable: false,
            numeric: false,
            in_schema: true,
        }
    }

    fn keys(cols: &[ColumnDef]) -> Vec<&str> {
        cols.iter().map(|c| c.key.as_str()).collect()
    }

    #[test]
    fn pinned_first_in_pin_order_hidden_left_out() {
        let all = [col("id"), col("a"), col("b"), col("c")];
        let prefs = CollectionView {
            pinned: vec!["c".into(), "gone".into(), "id".into()],
            hidden: vec!["a".into(), "id".into()],
            ..Default::default()
        };
        assert_eq!(keys(&visible_columns(&all, &prefs)), ["c", "b"]);
    }

    #[test]
    fn the_last_visible_column_cannot_be_hidden() {
        let all = [col("id"), col("a")];
        let mut prefs = CollectionView::default();
        assert!(toggle_hidden(&all, &mut prefs, "id"));
        assert!(!toggle_hidden(&all, &mut prefs, "a"));
        assert!(toggle_hidden(&all, &mut prefs, "id"));
        assert_eq!(prefs.hidden, Vec::<String>::new());
    }

    #[test]
    fn pin_toggles_and_width_is_clamped() {
        let mut prefs = CollectionView::default();
        toggle_pin(&mut prefs, "a");
        assert_eq!(prefs.pinned, ["id", "a"]);
        toggle_pin(&mut prefs, "id");
        assert_eq!(prefs.pinned, ["a"]);
        set_width(&mut prefs, "a", 5.);
        assert_eq!(prefs.widths["a"], MIN_WIDTH);
        set_width(&mut prefs, "a", 5000.);
        assert_eq!(prefs.widths["a"], MAX_WIDTH);
    }
}
