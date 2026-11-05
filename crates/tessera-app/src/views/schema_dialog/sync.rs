// SPDX-License-Identifier: GPL-3.0-or-later

//! Form ↔ JSON (design §7.6). The JSON text is the source of truth: every form edit rewrites
//! it, every JSON edit that parses refills the form, and a guard count keeps the rewrite from
//! refilling the form the user is typing in. Only one tab shows at a time, so a rewrite never
//! moves a visible cursor; that is the §13 fallback, and it holds by construction.

use gpui_kit::component::input::InputState;
use gpui_kit::{Context, Entity, Window};
use serde_json::{Map, Value};
use tessera_core::schema::{FieldDraft, FieldType, SchemaDraft};

use super::SchemaDialog;
use super::form::FieldRow;

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

/// The JSON tab's text for `draft`.
pub fn pretty(draft: &SchemaDraft) -> String {
    serde_json::to_string_pretty(&draft.to_json()).unwrap_or_default()
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

    /// The JSON text changed: refills the form, unless the change is the form's own rewrite.
    pub(super) fn json_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.syncing > 0 {
            self.syncing -= 1;
            return;
        }
        match SchemaDraft::from_json(&self.json.read(cx).text(cx)) {
            Ok(draft) => {
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
        cx.notify();
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
