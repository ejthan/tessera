// SPDX-License-Identifier: GPL-3.0-or-later

//! The schema view of a collection tab (spec §7.4): header, field table, options and the
//! full response as read-only JSON, with secrets masked (§5.4).

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Stateful, Styled as _, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{Collection, Field};
use tessera_core::redact::mask_value;
use tessera_widgets::JsonEditor;

use super::collections_list::created_text;
use super::overview::grouped;
use crate::actions::{COLLECTION, Refresh};
use crate::collections::{CollectionsBackendGlobal, aliases_of};
use crate::connection::{ConnectionState, error_text};

/// The attribute columns every field table has, in the order of the spec.
pub const KNOWN_ATTRIBUTES: [&str; 14] = [
    "facet",
    "optional",
    "index",
    "sort",
    "infix",
    "locale",
    "stem",
    "store",
    "range_index",
    "reference",
    "num_dim",
    "vec_dist",
    "embed",
    "truncate_len",
];

/// How long the Copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_secs(2);
/// Row height of the field table, for sizing it to its content.
const ROW_HEIGHT: f32 = 32.;
/// The field table scrolls beyond this many rows.
const MAX_ROWS: usize = 12;
const JSON_HEIGHT: f32 = 360.;

/// The known attributes, then the ones only `fields` have, in first-seen order.
pub fn attribute_columns(fields: &[Field]) -> Vec<String> {
    let mut columns: Vec<String> = KNOWN_ATTRIBUTES.iter().map(|a| a.to_string()).collect();
    for key in fields.iter().flat_map(|f| f.attributes.keys()) {
        if !columns.contains(key) {
            columns.push(key.clone());
        }
    }
    columns
}

/// What `field` shows under `attribute`: a check mark for an enabled flag, else the value.
fn cell_text(field: &Field, attribute: &str) -> String {
    match field.attr(attribute) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(_)) | None if field.flag(attribute) => "✓".into(),
        Some(Value::Bool(_)) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The field table's data.
struct Fields {
    fields: Vec<Field>,
    attributes: Vec<String>,
}

impl TableDelegate for Fields {
    fn columns_count(&self, _: &App) -> usize {
        2 + self.attributes.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.fields.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, title, width) = match col_ix {
            0 => ("name".into(), t!("schema.col.name").to_string(), 220.),
            1 => ("type".into(), t!("schema.col.type").to_string(), 110.),
            _ => {
                let a = self.attributes[col_ix - 2].clone();
                (a.clone(), a, 100.)
            }
        };
        Column::new(key, title)
            .width(px(width))
            .resizable(false)
            .movable(false)
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        let Some(field) = self.fields.get(row_ix) else {
            return div().id(("schema.filler", row_ix));
        };
        div().id(SharedString::from(format!("schema.field.{}", field.name)))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(field) = self.fields.get(row_ix) else {
            return div().into_any_element();
        };
        match col_ix {
            0 => div()
                .truncate()
                .child(field.name.clone())
                .into_any_element(),
            1 => div()
                .truncate()
                .child(field.r#type.clone())
                .into_any_element(),
            _ => {
                let attribute = &self.attributes[col_ix - 2];
                let text = SharedString::from(cell_text(field, attribute));
                div()
                    .id(SharedString::from(format!(
                        "schema.cell.{}.{attribute}",
                        field.name
                    )))
                    .test_support()
                    .aria_label(text.clone())
                    .truncate()
                    .child(text)
                    .into_any_element()
            }
        }
    }
}

/// A loaded collection, masked once: the table and the JSON both come from it.
struct Loaded {
    collection: Collection,
    json: String,
}

fn mask(collection: &Collection) -> Result<Loaded, serde_json::Error> {
    let mut value = serde_json::to_value(collection)?;
    mask_value(&mut value);
    Ok(Loaded {
        json: serde_json::to_string_pretty(&value)?,
        collection: serde_json::from_value(value)?,
    })
}

/// The schema of one collection.
pub struct SchemaView {
    connection: Entity<ConnectionState>,
    name: String,
    loaded: Option<Loaded>,
    error: Option<SharedString>,
    loading: bool,
    table: Entity<TableState<Fields>>,
    editor: Entity<JsonEditor>,
    copied: bool,
    task: Option<Task<()>>,
    copy_reset: Option<Task<()>>,
    _observe: gpui_kit::Subscription,
}

impl SchemaView {
    /// The schema of `name` on `connection`; loads it.
    pub fn new(
        connection: Entity<ConnectionState>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table = cx.new(|cx| {
            TableState::new(
                Fields {
                    fields: Vec::new(),
                    attributes: Vec::new(),
                },
                window,
                cx,
            )
            .sortable(false)
            .col_movable(false)
            .col_resizable(false)
            .col_selectable(false)
            .row_header(false)
        });
        let editor = cx.new(|cx| JsonEditor::new("", true, window, cx));
        let mut this = Self {
            _observe: cx.observe(&connection, |_, _, cx| cx.notify()),
            connection,
            name,
            loaded: None,
            error: None,
            loading: false,
            table,
            editor,
            copied: false,
            task: None,
            copy_reset: None,
        };
        this.reload(false, window, cx);
        // The header lists aliases, which only the collection list loads.
        if this
            .connection
            .read(cx)
            .collection_cache
            .collections
            .is_none()
        {
            this.connection
                .update(cx, |c, cx| c.load_collection_list(cx));
        }
        this
    }

    /// Loads the schema again (and the aliases when `with_aliases`); the old one stays on
    /// screen until the new one arrives or the load fails.
    pub fn reload(&mut self, with_aliases: bool, window: &mut Window, cx: &mut Context<Self>) {
        let conn = self.connection.read(cx);
        let Some(connected) = &conn.connected else {
            return;
        };
        let load = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .get(connected.client.clone(), self.name.clone());
        if with_aliases {
            self.connection
                .update(cx, |c, cx| c.load_collection_list(cx));
        }
        self.loading = true;
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            this.update_in(cx, |view, window, cx| {
                view.loading = false;
                let profile = view.connection.read(cx).profile.clone();
                match result
                    .map_err(|e| error_text(&e, &profile))
                    .and_then(|c| mask(&c).map_err(|e| e.to_string()))
                {
                    Ok(loaded) => view.show(loaded, window, cx),
                    Err(e) => view.error = Some(e.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn show(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.attributes = attribute_columns(&loaded.collection.fields);
            d.fields = loaded.collection.fields.clone();
            t.refresh(cx);
            cx.notify();
        });
        self.editor
            .update(cx, |e, cx| e.set_text(&loaded.json, window, cx));
        self.loaded = Some(loaded);
    }

    /// The text of the JSON editor, for tests.
    pub fn editor_text(&self, cx: &App) -> String {
        self.editor.read(cx).text(cx)
    }

    /// Puts the masked JSON on the clipboard and says "Copied" for a moment.
    fn copy_json(&mut self, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(loaded.json.clone()));
        self.copied = true;
        cx.notify();
        self.copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |view, cx| {
                view.copied = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn header(&self, c: &Collection, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = self.connection.read(cx);
        let aliases: Vec<&str> = aliases_of(&conn.collection_cache.aliases, &c.name).collect();
        let list = |items: &[&str]| {
            if items.is_empty() {
                t!("figure.missing").to_string()
            } else {
                items.join(", ")
            }
        };
        let sets = |s: &[String]| list(&s.iter().map(String::as_str).collect::<Vec<_>>());
        let facts = [
            ("documents", grouped(c.num_documents)),
            ("created", created_text(c.created_at)),
            ("aliases", list(&aliases)),
            ("synonym_sets", sets(&c.synonym_sets)),
            ("curation_sets", sets(&c.curation_sets)),
        ];
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_4()
            .items_center()
            .child(
                div()
                    .id("schema.name")
                    .test_support()
                    .aria_label(SharedString::from(c.name.clone()))
                    .text_lg()
                    .child(c.name.clone()),
            )
            .children(facts.map(|(key, value)| {
                let text = SharedString::from(format!("{}: {value}", t!(format!("schema.{key}"))));
                div()
                    .id(SharedString::from(format!("schema.{key}")))
                    .test_support()
                    .aria_label(text.clone())
                    .text_sm()
                    .text_color(muted)
                    .child(text)
            }))
            .child(div().flex_1())
            .child(
                Button::new("schema.reload")
                    .ghost()
                    .small()
                    .label(t!("collections.reload").to_string())
                    .children(Kbd::binding_for_action(&Refresh, Some(COLLECTION), window))
                    .on_click(cx.listener(|this, _, window, cx| this.reload(true, window, cx))),
            )
    }

    fn options(&self, c: &Collection, cx: &App) -> impl IntoElement {
        let metadata = c
            .metadata
            .as_ref()
            .and_then(|m| serde_json::to_string_pretty(m).ok())
            .unwrap_or_else(|| t!("figure.missing").to_string());
        let lines = [
            ("default_sorting_field", c.default_sorting_field.clone()),
            ("token_separators", c.token_separators.join(" ")),
            ("symbols_to_index", c.symbols_to_index.join(" ")),
            ("enable_nested_fields", c.enable_nested_fields.to_string()),
            ("metadata", metadata),
        ]
        .map(|(key, value)| format!("{}: {value}", t!(format!("schema.opt.{key}"))));
        v_flex()
            .id("schema.options")
            .test_support()
            .aria_label(SharedString::from(lines.join("\n")))
            .gap_1()
            .text_sm()
            .font_family(cx.theme().mono_font_family.clone())
            .children(lines)
    }
}

impl Render for SchemaView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let notice = |id: &'static str, text: SharedString, color| {
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(color)
                .child(text)
        };
        let body = self.loaded.as_ref().map(|l| {
            let rows = l.collection.fields.len().clamp(1, MAX_ROWS);
            let (copy_label, section) = (
                if self.copied {
                    t!("welcome.copied")
                } else {
                    t!("schema.copy_json")
                },
                |key: &str| div().text_sm().text_color(muted).child(t!(key).to_string()),
            );
            v_flex()
                .gap_3()
                .child(self.header(&l.collection, window, cx))
                .child(
                    div()
                        .h(px(ROW_HEIGHT * (rows + 1) as f32 + 2.))
                        .child(DataTable::new(&self.table).bordered(true)),
                )
                .child(section("schema.options"))
                .child(self.options(&l.collection, cx))
                .child(
                    h_flex()
                        .justify_between()
                        .child(section("schema.json"))
                        .child(
                            Button::new("schema.copy_json")
                                .small()
                                .label(copy_label.to_string())
                                .on_click(cx.listener(|this, _, _, cx| this.copy_json(cx))),
                        ),
                )
                .child(
                    div()
                        .id("schema.json")
                        .test_support()
                        .h(px(JSON_HEIGHT))
                        .child(self.editor.clone()),
                )
        });
        v_flex()
            .id("schema.scroll")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .children(
                self.error
                    .clone()
                    .map(|e| notice("schema.error", e, danger)),
            )
            .children((self.loading && self.loaded.is_none()).then(|| {
                notice(
                    "schema.loading",
                    t!("schema.loading").to_string().into(),
                    muted,
                )
            }))
            .children(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_columns_append_unknown() {
        let field = |attrs: Value| Field {
            name: "f".into(),
            r#type: "string".into(),
            attributes: attrs.as_object().cloned().unwrap_or_default(),
        };
        let fields = [
            field(serde_json::json!({"zeta": 1, "facet": true})),
            field(serde_json::json!({"alpha": 2, "zeta": 3})),
        ];
        let columns = attribute_columns(&fields);
        assert_eq!(columns[..14], KNOWN_ATTRIBUTES);
        assert_eq!(columns[14..], ["zeta", "alpha"]);
    }
}
