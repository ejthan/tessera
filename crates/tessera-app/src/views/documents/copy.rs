// SPDX-License-Identifier: GPL-3.0-or-later

//! Copying the selected cell or row.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{ClipboardItem, Context, Window};
use rust_i18n::t;
use serde_json::Value;
use tessera_core::documents::page::cell_full;

use super::DocumentsView;
use crate::actions::{CopyCell, CopyRowJson};
use crate::connection::error_text;
use crate::documents::DocumentsBackendGlobal;

/// A document as the inspector and the clipboard show it.
pub fn pretty(document: &Value) -> String {
    serde_json::to_string_pretty(document).unwrap_or_default()
}

/// The copy actions, for the toolbar's dropdown and the table's context menu.
pub fn entries(menu: PopupMenu) -> PopupMenu {
    menu.menu(t!("documents.copy.cell").to_string(), Box::new(CopyCell))
        .menu(t!("documents.copy.row").to_string(), Box::new(CopyRowJson))
}

/// Copies the selected cell's whole value. An unloaded row has nothing to copy.
pub fn copy_cell(view: &mut DocumentsView, cx: &mut Context<DocumentsView>) {
    let table = view.table.read(cx);
    let Some((row, col)) = table.selected_cell() else {
        return;
    };
    let rows = table.delegate();
    let (Some(row), Some(column)) = (rows.row(row), rows.columns.get(col)) else {
        return;
    };
    let text = row.doc.get(&column.key).map(cell_full).unwrap_or_default();
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    view.copied(cx);
}

/// Loads the selected cell's document and copies it: pages lack the vector fields.
pub fn copy_row(view: &mut DocumentsView, window: &mut Window, cx: &mut Context<DocumentsView>) {
    let Some(id) = view.selected_id(cx) else {
        return;
    };
    let connection = view.connection.clone();
    let Some(connected) = &connection.read(cx).connected else {
        return;
    };
    let load = cx.global::<DocumentsBackendGlobal>().0.document(
        connected.client.clone(),
        view.name.clone(),
        id,
    );
    view.copy_task = Some(cx.spawn_in(window, async move |this, cx| {
        let result = load.await;
        this.update(cx, |view, cx| match result {
            Ok(document) => {
                cx.write_to_clipboard(ClipboardItem::new_string(pretty(&document)));
                view.copied(cx);
            }
            Err(e) => {
                let text = error_text(&e, &view.connection.read(cx).profile);
                view.copy_error = Some(text.into());
                cx.notify();
            }
        })
        .ok();
    }));
}
