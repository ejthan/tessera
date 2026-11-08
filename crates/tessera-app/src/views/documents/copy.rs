// SPDX-License-Identifier: GPL-3.0-or-later

//! Copying the selected cell, row or rows, and the curl command of a row or the search.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{ClipboardItem, Context, Window};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::{Client, Error, Method, SearchRoute, document_path, search_route};
use tessera_core::documents::ID_FILTER_BYTES;
use tessera_core::documents::columns::sort_key;
use tessera_core::documents::export::{to_csv, to_jsonl};
use tessera_core::documents::ids::id_batches;
use tessera_core::documents::page::cell_full;

use super::DocumentsView;
use crate::actions::{CopyCell, CopyCsv, CopyCurl, CopyJsonl, CopyRowJson};
use crate::connection::error_text;
use crate::documents::{DocumentsBackend, DocumentsBackendGlobal};

/// What "Copy selection" writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFormat {
    /// One compact JSON document per line.
    Jsonl,
    /// The visible columns, in table order.
    Csv,
}

/// A document as the inspector and the clipboard show it.
pub fn pretty(document: &Value) -> String {
    serde_json::to_string_pretty(document).unwrap_or_default()
}

/// The copy actions, for the toolbar's dropdown and the table's context menu. `has_rows` is
/// false when nothing is ticked or selected, which leaves the selection formats nothing to copy.
pub fn entries(menu: PopupMenu, has_rows: bool) -> PopupMenu {
    menu.menu(t!("documents.copy.cell").to_string(), Box::new(CopyCell))
        .menu(t!("documents.copy.row").to_string(), Box::new(CopyRowJson))
        .menu_with_disabled(
            t!("documents.copy.jsonl").to_string(),
            Box::new(CopyJsonl),
            !has_rows,
        )
        .menu_with_disabled(
            t!("documents.copy.csv").to_string(),
            Box::new(CopyCsv),
            !has_rows,
        )
        .menu(t!("documents.copy.curl").to_string(), Box::new(CopyCurl))
}

/// The ticked ids, or the selected cell's row when none is ticked.
fn target_ids(view: &DocumentsView, cx: &gpui_kit::App) -> Vec<String> {
    let ids = view.table.read(cx).delegate().selection.ids();
    if ids.is_empty() {
        view.selected_id(cx).into_iter().collect()
    } else {
        ids
    }
}

/// Whether "Copy selection" has rows to copy.
pub fn has_rows(view: &DocumentsView, cx: &gpui_kit::App) -> bool {
    !target_ids(view, cx).is_empty()
}

/// The documents of `ids` as JSONL texts, each of one or more lines: the export bodies as
/// they came, so splitting them is left to the background. One id takes the document endpoint; several take
/// `id_batches`, whose `single` ids (and the one alone) also go through it. `.` and `..` have
/// no endpoint, so they keep the filter `id_batches` gives them.
async fn fetch_lines(
    backend: &dyn DocumentsBackend,
    client: Client,
    name: String,
    ids: &[String],
) -> Result<Vec<String>, Error> {
    let (filters, single) = match ids {
        [id] if !matches!(id.as_str(), "." | "..") => (Vec::new(), ids.to_vec()),
        _ => {
            let batches = id_batches(ids, ID_FILTER_BYTES);
            (batches.filters, batches.single)
        }
    };
    let mut texts = Vec::new();
    for filter in filters {
        texts.push(backend.export(client.clone(), name.clone(), filter).await?);
    }
    for id in single {
        let document = backend.document(client.clone(), name.clone(), id).await?;
        texts.push(to_jsonl(&[document]));
    }
    Ok(texts)
}

/// The non-empty lines of `texts`, as JSONL or as CSV over `columns`.
fn render(texts: &[String], format: CopyFormat, columns: &[String]) -> Result<String, String> {
    let lines: Vec<String> = texts
        .iter()
        .flat_map(|t| t.lines().filter(|l| !l.is_empty()).map(String::from))
        .collect();
    match format {
        CopyFormat::Jsonl => Ok(lines.join("\n")),
        CopyFormat::Csv => csv_text(columns, &lines).map_err(|e| e.to_string()),
    }
}

/// CSV of JSONL `lines` over `columns`.
fn csv_text(columns: &[String], lines: &[String]) -> Result<String, serde_json::Error> {
    let docs = lines
        .iter()
        .map(|l| serde_json::from_str::<Map<String, Value>>(l))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(to_csv(columns, &docs))
}

/// Copies the ticked rows, or the selected cell's row, as JSONL or CSV. Pages lack the vector
/// fields, so the documents come from the server again.
pub fn copy_selection(
    view: &mut DocumentsView,
    format: CopyFormat,
    window: &mut Window,
    cx: &mut Context<DocumentsView>,
) {
    let ids = target_ids(view, cx);
    let connection = view.connection.clone();
    let Some(connected) = &connection.read(cx).connected else {
        return;
    };
    if ids.is_empty() {
        return;
    }
    let client = connected.client.clone();
    let name = view.name.clone();
    let columns: Vec<String> = view
        .table
        .read(cx)
        .delegate()
        .columns
        .iter()
        .map(|c| c.key.clone())
        .collect();
    let backend = cx.global::<DocumentsBackendGlobal>().0.clone();
    view.copy_task = Some(cx.spawn_in(window, async move |this, cx| {
        let texts = fetch_lines(&*backend, client, name, &ids).await;
        let text = match texts {
            Ok(texts) => Ok(cx
                .background_executor()
                .spawn(async move { render(&texts, format, &columns) })
                .await),
            Err(e) => Err(e),
        };
        this.update(cx, |view, cx| {
            let failure = match text {
                Ok(Ok(text)) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    view.copied(cx);
                    return;
                }
                Ok(Err(text)) => text,
                Err(e) => error_text(&e, &view.connection.read(cx).profile),
            };
            view.copy_error = Some(failure.into());
            cx.notify();
        })
        .ok();
    }));
}

/// Copies a curl command for the selected row's document, or for the current search when no
/// cell is selected. The key stays `$TYPESENSE_API_KEY`.
pub fn copy_curl(view: &mut DocumentsView, cx: &mut Context<DocumentsView>) {
    let connection = view.connection.clone();
    let Some(connected) = &connection.read(cx).connected else {
        return;
    };
    let node = connected.client.base_url().clone();
    // (method, path, query, body): a long filter goes by POST /multi_search, as the client sends it.
    let result = match view.selected_id(cx) {
        Some(id) => document_path(&view.name, &id).map(|p| (Method::GET, p, None, None)),
        None => {
            let rows = view.table.read(cx).delegate();
            let per_page = rows.per_page.to_string();
            let sort = rows.sort.as_ref().map(|(k, asc)| sort_key(k, *asc));
            let mut pairs = vec![
                ("q", "*"),
                ("per_page", per_page.as_str()),
                ("enable_curations", "false"),
            ];
            if let Some(filter) = rows.filter.as_deref() {
                pairs.push(("filter_by", filter));
            }
            if let Some(sort) = sort.as_deref() {
                pairs.push(("sort_by", sort));
            }
            let owned: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            search_route(&view.name, &owned).map(|route| match route {
                SearchRoute::Get { path, query } => (Method::GET, path, Some(query), None),
                SearchRoute::Post { query, body } => (
                    Method::POST,
                    "multi_search".to_string(),
                    query,
                    Some(body.to_string()),
                ),
            })
        }
    };
    match result {
        Ok((method, path, query, body)) => {
            let text = tessera_core::curl::request(
                &node,
                &method,
                &format!("/{path}"),
                query.as_deref(),
                body.as_deref(),
            );
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            view.copied(cx);
        }
        // `.` and `..` have no document URL.
        Err(e) => {
            view.copy_error = Some(error_text(&e, &view.connection.read(cx).profile).into());
            cx.notify();
        }
    }
}

/// Copies the selected cell's whole value. An unloaded row has nothing to copy.
pub fn copy_cell(view: &mut DocumentsView, cx: &mut Context<DocumentsView>) {
    let table = view.table.read(cx);
    let Some((row, col)) = table.selected_cell() else {
        return;
    };
    let rows = table.delegate();
    let (Some(row), Some(column)) = (rows.row(row), rows.column_at(col)) else {
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
