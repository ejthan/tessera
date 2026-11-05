// SPDX-License-Identifier: GPL-3.0-or-later

//! Reusable GPUI widgets for Tessera.

pub mod confirm;
pub mod confirm_danger;
pub mod field_type_select;
pub mod json_editor;

pub use field_type_select::{FieldTypeChanged, FieldTypeSelect};
pub use json_editor::{JsonEditor, JsonEditorEvent, JsonError};

// `t!()` only works in the crate that declares the translations.
rust_i18n::i18n!("../../assets/locales", fallback = "en");
