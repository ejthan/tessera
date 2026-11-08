// SPDX-License-Identifier: GPL-3.0-or-later

//! Form ↔ JSON (design §7.6). The JSON text is the source of truth: every form edit rewrites
//! it, every JSON edit that parses refills the form, and a guard count keeps the rewrite from
//! refilling the form the user is typing in. Only one tab shows at a time, so a rewrite never
//! moves a visible cursor; that is the §13 fallback, and it holds by construction.

use gpui_kit::component::input::InputState;
use gpui_kit::{Context, Entity, Window};
use serde_json::{Map, Value};
use tessera_core::ai::set_embedding;
use tessera_core::schema::{FieldDraft, FieldType, SchemaDraft};
use tessera_core::transfer::infer::InferNote;

use super::SchemaDialog;
use super::form::{EmbedRow, FieldRow};

/// A text input of a field row; the index into [`FieldRow::inputs`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Text {
    Name,
    Locale,
    Reference,
    NumDim,
    VecDist,
}

/// The row inputs in [`FieldRow::inputs`] order.
pub const TEXTS: [Text; 5] = [
    Text::Name,
    Text::Locale,
    Text::Reference,
    Text::NumDim,
    Text::VecDist,
];

/// A text input of the collection; the index into `Form::opts`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opt {
    Name,
    TokenSeparators,
    SymbolsToIndex,
    SynonymSets,
    Metadata,
}

/// The collection inputs in `Form::opts` order.
pub const OPTS: [Opt; 5] = [
    Opt::Name,
    Opt::TokenSeparators,
    Opt::SymbolsToIndex,
    Opt::SynonymSets,
    Opt::Metadata,
];

/// The JSON tab's text for `draft`. A credential reads as the mask: the send takes it from the
/// draft ([`SchemaDraft::restore_secrets`]).
pub fn pretty(draft: &SchemaDraft) -> String {
    serde_json::to_string_pretty(&draft.to_masked_json()).unwrap_or_default()
}

/// The source fields, model name and endpoint of `f`'s `embed`, as the inputs show them.
pub fn embed_parts(f: &FieldDraft) -> (String, String, String) {
    let embed = f.embed.as_ref();
    let from = embed.and_then(|e| e["from"].as_array());
    let from: Vec<&str> = from
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let config = |key: &str| {
        let value = embed.and_then(|e| e["model_config"][key].as_str());
        value.unwrap_or_default().to_string()
    };
    (from.join(", "), config("model_name"), config("url"))
}

/// The names in the source fields input.
pub fn embed_from(text: &str) -> Vec<String> {
    let names = text.split(',').map(str::trim).filter(|s| !s.is_empty());
    names.map(str::to_string).collect()
}

/// Whether unchecking "Auto-embed" would lose anything.
pub fn embed_holds(f: &FieldDraft) -> bool {
    let (from, model, url) = embed_parts(f);
    let key = f.embed.as_ref().map(|e| &e["model_config"]["api_key"]);
    !from.is_empty()
        || !model.is_empty()
        || !url.is_empty()
        || key.and_then(Value::as_str).is_some_and(|k| !k.is_empty())
}

/// What input `which` of a row shows for `f`.
pub fn field_text(f: &FieldDraft, which: Text) -> String {
    match which {
        Text::Name => f.name.clone(),
        Text::Locale => f.locale.clone(),
        Text::Reference => f.reference.clone(),
        Text::NumDim => f.num_dim.map(|n| n.to_string()).unwrap_or_default(),
        Text::VecDist => f.vec_dist.clone().unwrap_or_default(),
    }
}

/// Puts the text of input `which` into `f`. Invalid vector dimensions clear `num_dim`; the
/// row shows why ([`num_dim`]).
pub fn set_field_text(f: &mut FieldDraft, which: Text, text: &str) {
    match which {
        Text::Name => retarget(f, |f| f.name = text.to_string()),
        Text::Locale => f.locale = text.to_string(),
        Text::Reference => f.reference = text.to_string(),
        Text::NumDim => f.num_dim = num_dim(text).flatten(),
        Text::VecDist => f.vec_dist = (!text.is_empty()).then(|| text.to_string()),
    }
}

/// What input `which` of the collection shows for `d`.
pub fn opt_text(d: &SchemaDraft, which: Opt) -> String {
    match which {
        Opt::Name => d.name.clone(),
        Opt::TokenSeparators => d.token_separators.concat(),
        Opt::SymbolsToIndex => d.symbols_to_index.concat(),
        Opt::SynonymSets => d.synonym_sets.join(", "),
        Opt::Metadata => d
            .metadata
            .as_ref()
            .map(|m| Value::Object(m.clone()).to_string())
            .unwrap_or_default(),
    }
}

/// Puts the text of input `which` into `d`. Metadata that is no JSON object leaves the draft
/// as it was; the form shows why ([`metadata`]).
pub fn set_opt_text(d: &mut SchemaDraft, which: Opt, text: &str) {
    match which {
        Opt::Name => d.name = text.to_string(),
        Opt::TokenSeparators => d.token_separators = chars(text),
        Opt::SymbolsToIndex => d.symbols_to_index = chars(text),
        Opt::SynonymSets => {
            d.synonym_sets = text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        Opt::Metadata => {
            if let Some(m) = metadata(text) {
                d.metadata = m;
            }
        }
    }
}

/// One entry per character, as `token_separators` and `symbols_to_index` take them.
fn chars(text: &str) -> Vec<String> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(String::from)
        .collect()
}

/// `Some(None)` for an empty text, `None` when it is not a positive whole number.
pub fn num_dim(text: &str) -> Option<Option<u32>> {
    let text = text.trim();
    if text.is_empty() {
        return Some(None);
    }
    text.parse().ok().filter(|n| *n > 0).map(Some)
}

/// `Some(None)` for an empty text, `None` when it is not a JSON object.
pub fn metadata(text: &str) -> Option<Option<Map<String, Value>>> {
    if text.trim().is_empty() {
        return Some(None);
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(m)) => Some(Some(m)),
        _ => None,
    }
}

/// Applies `change`; an `optional` left at its default follows a new name or type (`auto`
/// and names with `*` are optional by default).
pub fn retarget(f: &mut FieldDraft, change: impl FnOnce(&mut FieldDraft)) {
    let default = |f: &FieldDraft| FieldDraft::new(f.name.clone(), f.r#type.clone()).optional;
    let was_default = f.optional == default(f);
    change(f);
    if was_default {
        f.optional = default(f);
    }
}

/// The fields a default sorting field may name (design §5.2).
pub fn sortable(d: &SchemaDraft) -> Vec<String> {
    d.fields
        .iter()
        .filter(|f| {
            !f.name.is_empty()
                && (matches!(
                    f.r#type,
                    FieldType::Int32 | FieldType::Int64 | FieldType::Float
                ) || (f.r#type == FieldType::String && f.sort == Some(true)))
        })
        .map(|f| f.name.clone())
        .collect()
}

/// Sets `input` to `text` only when it differs, so an unchanged input keeps its cursor.
fn show(input: &Entity<InputState>, text: String, window: &mut Window, cx: &mut gpui_kit::App) {
    if input.read(cx).value() != text {
        input.update(cx, |i, cx| i.set_value(text, window, cx));
    }
}

impl SchemaDialog {
    /// A form edit: changes the draft, rewrites the JSON and validates again. Refused while
    /// the JSON does not parse (the form is read-only then).
    pub(super) fn edit(
        &mut self,
        change: impl FnOnce(&mut SchemaDraft),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.locked() {
            return;
        }
        change(&mut self.draft);
        // `set_text` emits `Changed` once, after this update; `json_changed` skips it. A count,
        // because several edits can queue their rewrites before the first event arrives.
        self.syncing += 1;
        let text = pretty(&self.draft);
        self.json.update(cx, |e, cx| e.set_text(&text, window, cx));
        self.refresh(window, cx);
    }

    /// Replaces the draft as a whole: the JSON follows and the form is refilled. Refused
    /// while the JSON does not parse, like any form edit.
    pub(super) fn replace_draft(
        &mut self,
        draft: SchemaDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.locked() {
            return;
        }
        self.draft = draft;
        // `set_text` always emits `Changed`, which `json_changed` skips.
        self.syncing += 1;
        let text = pretty(&self.draft);
        self.json.update(cx, |e, cx| e.set_text(&text, window, cx));
        self.fill_form(window, cx);
        self.refresh(window, cx);
    }

    /// The JSON text changed: refills the form, unless the change is the form's own rewrite.
    pub(super) fn json_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.syncing > 0 {
            self.syncing -= 1;
            return;
        }
        match SchemaDraft::from_json(&self.json.read(cx).text(cx)) {
            Ok(mut draft) => {
                draft.restore_secrets(&self.draft);
                self.draft = draft;
                self.json_error = None;
                self.fill_form(window, cx);
            }
            Err(e) => self.json_error = Some(e),
        }
        self.refresh(window, cx);
    }

    /// Validation (and in edit mode the diff checks) and the sorting field choices for the
    /// current draft.
    pub(super) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.check_draft();
        // Use geopoint means nothing once the field is gone or already one.
        let draft = &self.draft;
        self.infer_notes.retain(|note| match note {
            InferNote::MaybeGeopoint(field) => draft
                .fields
                .iter()
                .any(|f| f.name == *field && f.r#type != FieldType::Geopoint),
            _ => true,
        });
        let sortable = sortable(&self.draft);
        if sortable != self.form.sortable {
            self.form.sortable = sortable.clone();
            self.form
                .sorting
                .update(cx, |s, cx| s.set_items(sortable, window, cx));
        }
        let field = self.draft.default_sorting_field.clone();
        let shown = self.form.sorting.read(cx).selected_value().cloned();
        if shown.unwrap_or_default() != field {
            self.form
                .sorting
                .update(cx, |s, cx| s.set_selected_value(&field, window, cx));
        }
        self.sync_embed_rows(window, cx);
        cx.notify();
    }

    /// Whether the form may edit `f`'s embedding: a stored one is the server's to keep.
    pub(super) fn embed_editable(&self, f: &FieldDraft) -> bool {
        f.r#type == FieldType::FloatArray && !self.stored_field(&f.name)
    }

    /// The blocks follow the draft: those of embedded fields exist, and show what it holds.
    /// Each input is left alone while it means the same, so typing keeps its cursor.
    fn sync_embed_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sources: Vec<String> = self
            .draft
            .fields
            .iter()
            .filter(|f| matches!(f.r#type, FieldType::String | FieldType::StringArray))
            .filter(|f| !f.name.is_empty())
            .map(|f| f.name.clone())
            .collect();
        for i in 0..self.form.rows.len().min(self.draft.fields.len()) {
            let f = self.draft.fields[i].clone();
            if f.embed.is_none() || !self.embed_editable(&f) {
                self.form.rows[i].embed = None;
                continue;
            }
            if self.form.rows[i].embed.is_none() {
                self.form.rows[i].embed = Some(EmbedRow::new(&f, window, cx));
            }
            let row = &self.form.rows[i];
            show(row.input(Text::NumDim), String::new(), window, cx);
            let Some(block) = &row.embed else {
                continue;
            };
            let (from, model, url) = embed_parts(&f);
            block
                .from
                .update(cx, |input, _| input.set_candidates(sources.clone()));
            if embed_from(&block.from.read(cx).value(cx)) != embed_from(&from) {
                block
                    .from
                    .update(cx, |input, cx| input.set_value(&from, window, cx));
            }
            if block.model.read(cx).value(cx).trim() != model {
                block
                    .model
                    .update(cx, |input, cx| input.set_value(&model, window, cx));
            }
            if block.url.read(cx).value().trim() != url {
                show(&block.url, url, window, cx);
            }
        }
    }

    /// An input of field `i`'s block changed. `key` is the key field's text, when that one did.
    /// A built-in model takes neither key nor endpoint, so hidden inputs send nothing.
    pub(super) fn embed_changed(
        &mut self,
        i: usize,
        key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(block) = self.form.rows.get(i).and_then(|r| r.embed.as_ref()) else {
            return;
        };
        let from = embed_from(&block.from.read(cx).value(cx));
        let model = block.model.read(cx).value(cx).trim().to_string();
        let mut url = block.url.read(cx).value().trim().to_string();
        let mut key = key;
        if model.starts_with("ts/") {
            show(&block.key, String::new(), window, cx);
            show(&block.url, String::new(), window, cx);
            url.clear();
            key = Some(String::new());
        }
        let Some(f) = self.draft.fields.get(i) else {
            return;
        };
        let mut changed = f.clone();
        set_embedding(&mut changed, &from, &model, key.as_deref(), &url);
        // What the JSON tab just put here must not rewrite the text being typed there.
        if changed != *f {
            self.edit_field(i, |f| *f = changed, window, cx);
        }
    }

    /// Shows the draft in the form. Rows are rebuilt when the number of fields changed.
    fn fill_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (which, input) in OPTS.iter().zip(&self.form.opts) {
            show(input, opt_text(&self.draft, *which), window, cx);
        }
        if self.form.rows.len() != self.draft.fields.len() {
            self.form.rows = self
                .draft
                .fields
                .iter()
                .map(|f| FieldRow::new(f, window, cx))
                .collect();
            return;
        }
        for (row, f) in self.form.rows.iter_mut().zip(&self.draft.fields) {
            for (which, input) in TEXTS.iter().zip(&row.inputs) {
                show(input, field_text(f, *which), window, cx);
            }
            if row.ty != f.r#type {
                row.ty = f.r#type.clone();
                row.r#type.update(cx, |s, cx| s.set(&f.r#type, window, cx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_round_trip_and_reject_bad_input() {
        let mut d = SchemaDraft::default();
        set_opt_text(&mut d, Opt::TokenSeparators, "- /");
        assert_eq!(d.token_separators, ["-", "/"]);
        assert_eq!(opt_text(&d, Opt::TokenSeparators), "-/");
        set_opt_text(&mut d, Opt::SynonymSets, " a, ,b ");
        assert_eq!(d.synonym_sets, ["a", "b"]);
        set_opt_text(&mut d, Opt::Metadata, r#"{"k": 1}"#);
        assert_eq!(opt_text(&d, Opt::Metadata), r#"{"k":1}"#);
        // Not an object: the draft keeps the last good metadata.
        set_opt_text(&mut d, Opt::Metadata, "[1]");
        assert_eq!(opt_text(&d, Opt::Metadata), r#"{"k":1}"#);
        assert_eq!(metadata("[1]"), None);
        set_opt_text(&mut d, Opt::Metadata, " ");
        assert_eq!(d.metadata, None);

        assert_eq!(num_dim(""), Some(None));
        assert_eq!(num_dim("384"), Some(Some(384)));
        assert_eq!(num_dim("0"), None);
        assert_eq!(num_dim("12x"), None);
        let mut f = FieldDraft::new("v", FieldType::FloatArray);
        set_field_text(&mut f, Text::NumDim, "12x");
        assert_eq!(f.num_dim, None);
        set_field_text(&mut f, Text::VecDist, "cosine");
        assert_eq!(field_text(&f, Text::VecDist), "cosine");
    }

    #[test]
    fn optional_at_its_default_follows_name_and_type() {
        let mut f = FieldDraft::new("tag", FieldType::String);
        set_field_text(&mut f, Text::Name, "tag*");
        assert!(f.optional, "a name with * is optional by default");
        retarget(&mut f, |f| f.r#type = FieldType::Auto);
        set_field_text(&mut f, Text::Name, "tag");
        assert!(f.optional, "auto is optional by default");
        // Set by the user: kept.
        let mut f = FieldDraft::new("t", FieldType::String);
        f.optional = true;
        retarget(&mut f, |f| f.r#type = FieldType::Int32);
        assert!(f.optional);
    }

    #[test]
    fn sortable_fields_follow_validation() {
        let d = SchemaDraft::from_json(
            r#"{"name":"c","fields":[{"name":"n","type":"int32"},{"name":"s","type":"string"},
            {"name":"t","type":"string","sort":true},{"name":"","type":"float"}]}"#,
        )
        .unwrap();
        assert_eq!(sortable(&d), ["n", "t"]);
    }
}
