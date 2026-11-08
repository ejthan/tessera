// SPDX-License-Identifier: GPL-3.0-or-later

//! Views of import and export jobs.

pub mod export_dialog;
pub mod import_dialog;
pub mod panel;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, Context, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    div, prelude::*,
};
use rust_i18n::t;
use tessera_core::transfer::infer::InferNote;

/// The sentence for a note of the inference.
fn note_text(note: &InferNote) -> String {
    match note {
        InferNote::MaybeGeopoint(field) => {
            t!("transfer.infer.note.maybe_geopoint", field = field)
        }
        InferNote::MixedTypes(field) => t!("transfer.infer.note.mixed", field = field),
        InferNote::EmptyArray(field) => t!("transfer.infer.note.empty_array", field = field),
        InferNote::OnlyNull(field) => t!("transfer.infer.note.only_null", field = field),
        InferNote::SkippedName(field) => t!("transfer.infer.note.skipped", field = field),
        InferNote::NonStringId => t!("transfer.infer.note.non_string_id"),
    }
    .to_string()
}

/// A small line of text that tests can find by `id`.
pub(super) fn note_owned(id: String, text: String, color: Hsla) -> impl IntoElement {
    let text = SharedString::from(text);
    div()
        .id(SharedString::from(id))
        .test_support()
        .aria_label(text.clone())
        .text_xs()
        .text_color(color)
        .child(text)
}

/// Like [`note_owned`], for a fixed id.
pub(super) fn note(id: &'static str, text: String, color: Hsla) -> impl IntoElement {
    note_owned(id.to_string(), text, color)
}

/// One row for each note of an inference, `transfer.infer.note.{n}`, with Use geopoint where
/// a pair of numbers may be one. `use_geopoint` runs on the view that owns `cx`.
pub(super) fn infer_notes<V: 'static>(
    notes: &[InferNote],
    cx: &Context<V>,
    use_geopoint: fn(&mut V, &str, &mut Window, &mut Context<V>),
) -> Vec<AnyElement> {
    let warning = cx.theme().warning;
    notes
        .iter()
        .enumerate()
        .map(|(n, item)| {
            let id = format!("transfer.infer.note.{n}");
            let row =
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(note_owned(id, note_text(item), warning));
            let row = match item {
                InferNote::MaybeGeopoint(field) => {
                    let field = field.clone();
                    row.child(
                        Button::new(SharedString::from(format!(
                            "transfer.infer.use_geopoint.{field}"
                        )))
                        .ghost()
                        .xsmall()
                        .label(t!("transfer.infer.use_geopoint").to_string())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                use_geopoint(this, &field, window, cx);
                            },
                        )),
                    )
                }
                _ => row,
            };
            row.into_any_element()
        })
        .collect()
}
