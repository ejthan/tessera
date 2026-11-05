// SPDX-License-Identifier: GPL-3.0-or-later

//! Drop-down over the Typesense field types, with a one-line description per type.

use gpui_kit::AppContext as _;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{IndexPath, h_flex, searchable_list::SearchableListItem};
use gpui_kit::{
    AnyElement, App, Context, EventEmitter, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div,
};
use rust_i18n::t;
use tessera_core::schema::FieldType;

/// Emitted when the user picks a type.
pub struct FieldTypeChanged(pub FieldType);

#[derive(Clone)]
struct TypeItem(FieldType);

/// The one-line description of a type in `locale`; empty for types outside the list.
pub fn field_type_description(ty: &FieldType, locale: &str) -> String {
    let key = match ty {
        FieldType::String => "string",
        FieldType::StringArray => "string_array",
        FieldType::StringStar => "string_star",
        FieldType::Int32 => "int32",
        FieldType::Int32Array => "int32_array",
        FieldType::Int64 => "int64",
        FieldType::Int64Array => "int64_array",
        FieldType::Float => "float",
        FieldType::FloatArray => "float_array",
        FieldType::Bool => "bool",
        FieldType::BoolArray => "bool_array",
        FieldType::Geopoint => "geopoint",
        FieldType::GeopointArray => "geopoint_array",
        FieldType::Geopolygon => "geopolygon",
        FieldType::Object => "object",
        FieldType::ObjectArray => "object_array",
        FieldType::Image => "image",
        FieldType::Auto => "auto",
        FieldType::Other(_) => return String::new(),
    };
    t!(format!("field_type.{key}"), locale = locale).to_string()
}

impl SearchableListItem for TypeItem {
    type Value = FieldType;

    fn title(&self) -> SharedString {
        self.0.as_str().to_string().into()
    }

    fn display_title(&self) -> Option<AnyElement> {
        None
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        h_flex()
            .gap_2()
            .child(div().w(gpui_kit::px(110.)).child(self.title()))
            .child(
                div()
                    .text_xs()
                    .opacity(0.7)
                    .child(field_type_description(&self.0, &rust_i18n::locale())),
            )
    }

    fn value(&self) -> &FieldType {
        &self.0
    }
}

/// A `Select` over [`FieldType::ALL`]. A type outside the list (`Other`) shows no selection;
/// the placeholder names it.
pub struct FieldTypeSelect {
    state: gpui_kit::Entity<SelectState<Vec<TypeItem>>>,
    /// The selected type when it is not in the list.
    other: Option<String>,
    _sub: Subscription,
}

fn other_name(ty: &FieldType) -> Option<String> {
    matches!(ty, FieldType::Other(_)).then(|| ty.as_str().to_string())
}

impl EventEmitter<FieldTypeChanged> for FieldTypeSelect {}

impl FieldTypeSelect {
    /// Creates the select with `selected` chosen, if it is a known type.
    pub fn new(selected: &FieldType, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let items: Vec<TypeItem> = FieldType::ALL.iter().cloned().map(TypeItem).collect();
        let ix = FieldType::ALL
            .iter()
            .position(|t| t == selected)
            .map(|row| IndexPath::default().row(row));
        let state = cx.new(|cx| SelectState::new(items, ix, window, cx));
        let _sub = cx.subscribe(&state, |_, _, ev: &SelectEvent<Vec<TypeItem>>, cx| {
            let SelectEvent::Confirm(Some(ty)) = ev else {
                return;
            };
            cx.emit(FieldTypeChanged(ty.clone()));
        });
        Self {
            state,
            other: other_name(selected),
            _sub,
        }
    }

    /// Selects `ty` without emitting [`FieldTypeChanged`].
    pub fn set(&mut self, ty: &FieldType, window: &mut Window, cx: &mut Context<Self>) {
        self.other = other_name(ty);
        self.state
            .update(cx, |s, cx| s.set_selected_value(ty, window, cx));
        cx.notify();
    }
}

impl Render for FieldTypeSelect {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // GPUI Kit's own placeholder is not translated.
        let placeholder = match &self.other {
            Some(name) => t!("field_type.other", name = name),
            None => t!("field_type.placeholder"),
        };
        Select::new(&self.state)
            .placeholder(placeholder.to_string())
            .id("field_type")
            .accessibility_label(t!("field_type.label").to_string())
    }
}
