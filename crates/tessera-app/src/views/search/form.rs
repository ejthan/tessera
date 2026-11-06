// SPDX-License-Identifier: GPL-3.0-or-later

//! The workbench's form: one control per parameter of `FORM_PARAMS`, over the draft's map.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, prelude::*,
    px, transparent_black,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::Collection;
use tessera_core::documents::filter::operators_for;
use tessera_core::search::facet::facet_clause;
use tessera_core::search::params::{
    FORM_PARAMS, ParamDef, ParamKind, query_fields, sortable_fields,
};
use tessera_core::search::sort::{SortRow, parse_sort, sort_text};
use tessera_widgets::{FilterField, FilterInput, FilterInputEvent};

use super::value_text;
use crate::actions::{AddCondition, CHIP, CONDITION_ROW, MoveChipLeft, MoveChipRight, RemoveChip};
use crate::connection::ConnectionState;
use crate::views::documents::{facet_fetch, filter_fields};

/// Typesense takes at most three sort criteria.
const MAX_SORT_ROWS: usize = 3;
/// The select entry of a field name that takes an expression.
const EVAL: &str = "_eval";

/// Emitted by [`SearchForm`].
pub enum FormEvent {
    /// Parameter `.0` now holds `.1`; `None` removes it.
    Set(&'static str, Option<Value>),
    /// Enter in a control.
    Submit,
}

/// The widget of a parameter whose value is one input or one select.
enum Control {
    Input(Entity<InputState>),
    Select(Entity<SelectState<Vec<String>>>),
}

/// One row of the sort rows.
struct SortControl {
    field: Entity<SelectState<Vec<String>>>,
    /// The expression of an `_eval` row.
    eval: Entity<InputState>,
    desc: bool,
    _subscriptions: [Subscription; 2],
}

/// A `query_by` chip being dragged.
#[derive(Clone)]
struct DragChip {
    index: usize,
    name: SharedString,
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .rounded_md()
            .text_sm()
            .bg(cx.theme().secondary)
            .child(self.name.clone())
    }
}

/// The form over one draft. It holds no parameters of its own: [`Self::load`] shows a map and
/// [`FormEvent::Set`] reports each edit.
pub struct SearchForm {
    controls: HashMap<&'static str, Control>,
    /// Inline errors by parameter.
    errors: HashMap<&'static str, SharedString>,
    filter: Entity<FilterInput>,
    /// The collection facet values are fetched from.
    collection: Rc<RefCell<String>>,
    fields: Vec<FilterField>,
    chips: Vec<String>,
    chip_focus: Vec<FocusHandle>,
    chip_picker: Entity<ComboboxState<Vec<String>>>,
    /// What the picker offers.
    queryable: Vec<String>,
    sortable: Vec<String>,
    sort: Vec<SortControl>,
    /// `sort_by` does not fit the rows, so it stays as it is until JSON mode changes it.
    sort_json_only: bool,
    condition_field: Entity<SelectState<Vec<String>>>,
    condition_op: Entity<SelectState<Vec<String>>>,
    condition_value: Entity<InputState>,
    condition_error: Option<SharedString>,
    more_open: bool,
    /// Parameters of the map without a control.
    only_json: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for SearchForm {}

impl SearchForm {
    /// An empty form over `connection`.
    pub fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let collection = Rc::new(RefCell::new(String::new()));
        let fetch = facet_fetch(connection, collection.clone());
        let filter = cx.new(|cx| FilterInput::new(Vec::new(), fetch, window, cx));
        let chip_picker =
            cx.new(|cx| ComboboxState::new(Vec::new(), Vec::new(), window, cx).multiple(true));
        let condition_field = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let condition_op = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let condition_value = cx.new(|cx| InputState::new(window, cx));
        let mut subscriptions = vec![
            cx.subscribe(&filter, |_, _, event: &FilterInputEvent, cx| match event {
                FilterInputEvent::Changed(text) => {
                    let value = (!text.is_empty()).then(|| text.clone().into());
                    cx.emit(FormEvent::Set("filter_by", value));
                }
                FilterInputEvent::Submitted(_) => cx.emit(FormEvent::Submit),
            }),
            cx.subscribe(
                &chip_picker,
                |this, _, event: &ComboboxEvent<Vec<String>>, cx| {
                    if let ComboboxEvent::Change(values) = event {
                        this.pick_chips(values, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &condition_field,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(field)) = event {
                        this.offer_operators(&field.clone(), window, cx);
                    }
                },
            ),
        ];
        let mut controls = HashMap::new();
        for def in FORM_PARAMS {
            let items = match def.kind {
                ParamKind::Bool => Some(&["true", "false"][..]),
                ParamKind::Choice(values) => Some(values),
                _ => None,
            };
            let control = if let Some(items) = items {
                let items = std::iter::once(t!("search.form.default").to_string())
                    .chain(items.iter().map(ToString::to_string))
                    .collect();
                let select = cx.new(|cx| SelectState::new(items, None, window, cx));
                let def = *def;
                subscriptions.push(cx.subscribe(
                    &select,
                    move |this, _, SelectEvent::Confirm(value), cx| {
                        this.select_event(def, value.as_ref(), cx);
                    },
                ));
                Control::Select(select)
            } else if matches!(def.kind, ParamKind::Filter | ParamKind::Sort)
                || def.key == "query_by"
            {
                continue;
            } else {
                let input = cx.new(|cx| InputState::new(window, cx));
                let def = *def;
                subscriptions.push(cx.subscribe(&input, move |this, input, event, cx| {
                    this.input_event(def, &input, event, cx);
                }));
                Control::Input(input)
            };
            controls.insert(def.key, control);
        }
        Self {
            controls,
            errors: HashMap::new(),
            filter,
            collection,
            fields: Vec::new(),
            chips: Vec::new(),
            chip_focus: Vec::new(),
            chip_picker,
            queryable: Vec::new(),
            sortable: Vec::new(),
            sort: Vec::new(),
            sort_json_only: false,
            condition_field,
            condition_op,
            condition_value,
            condition_error: None,
            more_open: false,
            only_json: 0,
            _subscriptions: subscriptions,
        }
    }

    fn input_event(
        &mut self,
        def: ParamDef,
        input: &Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = input.read(cx).value().trim().to_string();
                self.errors.remove(def.key);
                let value = if text.is_empty() {
                    None
                } else if def.kind == ParamKind::Int {
                    let Ok(number) = text.parse::<i64>() else {
                        let error = t!("search.form.not_a_number").to_string();
                        self.errors.insert(def.key, error.into());
                        return cx.notify();
                    };
                    Some(number.into())
                } else {
                    Some(text.into())
                };
                cx.emit(FormEvent::Set(def.key, value));
                cx.notify();
            }
            InputEvent::PressEnter { .. } => cx.emit(FormEvent::Submit),
            _ => {}
        }
    }

    fn select_event(&mut self, def: ParamDef, value: Option<&String>, cx: &mut Context<Self>) {
        let value = value
            .filter(|v| **v != t!("search.form.default").as_ref())
            .map(|v| match def.kind {
                ParamKind::Bool => (v == "true").into(),
                _ => v.as_str().into(),
            });
        cx.emit(FormEvent::Set(def.key, value));
    }

    /// Shows `params`, leaving a control alone that already says the same, so its cursor stays.
    pub fn load(
        &mut self,
        params: &Map<String, Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.errors.clear();
        self.only_json = params
            .keys()
            .filter(|k| !FORM_PARAMS.iter().any(|d| d.key == k.as_str()))
            .count();
        for (key, control) in &self.controls {
            let text = value_text(params.get(*key));
            match control {
                Control::Input(input) => {
                    if input.read(cx).value() != text {
                        input.update(cx, |i, cx| i.set_value(text, window, cx));
                    }
                }
                Control::Select(select) => {
                    let value = if text.is_empty() {
                        t!("search.form.default").to_string()
                    } else {
                        text
                    };
                    select.update(cx, |s, cx| s.set_selected_value(&value, window, cx));
                }
            }
        }
        let filter = value_text(params.get("filter_by"));
        if self.filter.read(cx).value(cx) != filter {
            self.filter
                .update(cx, |f, cx| f.set_value(&filter, window, cx));
        }
        self.chips = value_text(params.get("query_by"))
            .split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(String::from)
            .collect();
        self.sync_picker(window, cx);
        self.load_sort(&value_text(params.get("sort_by")), window, cx);
        cx.notify();
    }

    /// Takes what the collection offers: fields to query, filter and sort by. `name` is what the
    /// draft searches, which may be an alias.
    pub fn set_schema(
        &mut self,
        name: &str,
        schema: Option<&Collection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        name.clone_into(&mut self.collection.borrow_mut());
        self.fields = schema.map(filter_fields).unwrap_or_default();
        self.sortable = schema.map(sortable_fields).unwrap_or_default();
        let fields = self.fields.clone();
        self.filter.update(cx, |f, cx| f.set_fields(fields, cx));
        self.queryable = schema.map(query_fields).unwrap_or_default();
        let queryable = self.queryable.clone();
        self.chip_picker
            .update(cx, |p, cx| p.set_items(queryable, window, cx));
        self.sync_picker(window, cx);
        let names: Vec<String> = self.fields.iter().map(|f| f.name.clone()).collect();
        self.condition_field
            .update(cx, |s, cx| s.set_items(names, window, cx));
        cx.notify();
    }

    /// The text of the input of parameter `key`.
    pub fn input_text(&self, key: &str, cx: &App) -> String {
        match self.controls.get(key) {
            Some(Control::Input(input)) => input.read(cx).value().to_string(),
            _ => String::new(),
        }
    }

    /// The `filter_by` input.
    pub fn filter(&self) -> &Entity<FilterInput> {
        &self.filter
    }

    /// Puts the keyboard focus in `q`.
    pub fn focus_query(&self, window: &mut Window, cx: &mut App) {
        if let Some(Control::Input(q)) = self.controls.get("q") {
            q.update(cx, |q, cx| q.focus(window, cx));
        }
    }

    fn sync_picker(&self, window: &mut Window, cx: &mut Context<Self>) {
        let chips = self.chips.clone();
        self.chip_picker
            .update(cx, |p, cx| p.set_selected_values(&chips, window, cx));
    }

    fn write_chips(&self, cx: &mut Context<Self>) {
        let value = (!self.chips.is_empty()).then(|| self.chips.join(",").into());
        cx.emit(FormEvent::Set("query_by", value));
        cx.notify();
    }

    /// Takes the picker's selection; chips keep their order and new ones go last. A chip the
    /// picker does not offer stays.
    fn pick_chips(&mut self, values: &[String], cx: &mut Context<Self>) {
        let offered = &self.queryable;
        self.chips
            .retain(|c| values.contains(c) || !offered.contains(c));
        for value in values {
            if !self.chips.contains(value) {
                self.chips.push(value.clone());
            }
        }
        self.write_chips(cx);
    }

    fn move_chip(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from != to && from < self.chips.len() && to < self.chips.len() {
            let chip = self.chips.remove(from);
            self.chips.insert(to, chip);
            self.write_chips(cx);
        }
    }

    fn remove_chip(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.chips.len() {
            self.chips.remove(index);
            self.sync_picker(window, cx);
            self.write_chips(cx);
        }
    }

    /// Puts the keyboard focus on chip `index`.
    pub fn focus_chip(&self, index: usize, window: &mut Window, cx: &mut App) {
        if let Some(focus) = self.chip_focus.get(index) {
            focus.focus(window, cx);
        }
    }

    fn sort_row(&self, row: &SortRow, window: &mut Window, cx: &mut Context<Self>) -> SortControl {
        let inner = row.expr.strip_prefix("_eval(");
        let inner = inner.and_then(|e| e.strip_suffix(')'));
        let field = if inner.is_some() { EVAL } else { &row.expr };
        let mut items = self.sortable.clone();
        if !field.is_empty() && !items.iter().any(|i| i == field) {
            items.push(field.to_string());
        }
        let selected = items.iter().position(|i| *i == field);
        let selected = selected.map(|row| IndexPath::default().row(row));
        let select = cx.new(|cx| SelectState::new(items, selected, window, cx));
        let eval =
            cx.new(|cx| InputState::new(window, cx).default_value(inner.unwrap_or_default()));
        SortControl {
            _subscriptions: [
                cx.subscribe(&select, |this, _, _: &SelectEvent<Vec<String>>, cx| {
                    this.write_sort(cx);
                }),
                cx.subscribe(&eval, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.write_sort(cx);
                    }
                }),
            ],
            field: select,
            eval,
            desc: row.desc,
        }
    }

    fn sort_rows(&self, cx: &App) -> Vec<SortRow> {
        self.sort
            .iter()
            .filter_map(|row| {
                let field = row.field.read(cx).selected_value()?.clone();
                let expr = if field == EVAL {
                    let eval = row.eval.read(cx).value();
                    (!eval.trim().is_empty()).then(|| format!("_eval({})", eval.trim()))?
                } else {
                    field
                };
                Some(SortRow {
                    expr,
                    desc: row.desc,
                })
            })
            .collect()
    }

    fn write_sort(&self, cx: &mut Context<Self>) {
        let text = sort_text(&self.sort_rows(cx));
        let value = (!text.is_empty()).then(|| text.into());
        cx.emit(FormEvent::Set("sort_by", value));
        cx.notify();
    }

    fn load_sort(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let rows = parse_sort(text);
        self.sort_json_only = rows.is_none();
        let rows = rows.unwrap_or_default();
        if rows != self.sort_rows(cx) {
            self.sort = rows.iter().map(|r| self.sort_row(r, window, cx)).collect();
        }
    }

    /// Sets sort row `index` to `field`, as its select does.
    pub fn set_sort_field(
        &mut self,
        index: usize,
        field: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.sort.get(index) {
            let field = field.to_string();
            row.field
                .update(cx, |s, cx| s.set_selected_value(&field, window, cx));
            self.write_sort(cx);
        }
    }

    fn add_sort_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sort.len() < MAX_SORT_ROWS {
            let row = SortRow {
                expr: String::new(),
                desc: true,
            };
            let row = self.sort_row(&row, window, cx);
            self.sort.push(row);
            cx.notify();
        }
    }

    fn operators(&self, field: &str) -> Vec<String> {
        let field = self.fields.iter().find(|f| f.name == field);
        let operators = operators_for(field.map_or("string", |f| &f.r#type));
        // shortcut: a range takes two values, which the one value input cannot hold; write it
        // in the filter text.
        let operators = operators.iter().filter(|o| **o != ":[");
        operators.map(ToString::to_string).collect()
    }

    fn offer_operators(&mut self, field: &str, window: &mut Window, cx: &mut Context<Self>) {
        let operators = self.operators(field);
        let first = operators.first().cloned();
        self.condition_op.update(cx, |s, cx| {
            s.set_items(operators, window, cx);
            if let Some(first) = &first {
                s.set_selected_value(first, window, cx);
            }
        });
    }

    /// Picks the field and operator of the Add condition row, as its selects do.
    pub fn set_condition(
        &mut self,
        field: &str,
        operator: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (field, operator) = (field.to_string(), operator.to_string());
        self.condition_field
            .update(cx, |s, cx| s.set_selected_value(&field, window, cx));
        self.offer_operators(&field, window, cx);
        self.condition_op
            .update(cx, |s, cx| s.set_selected_value(&operator, window, cx));
    }

    /// Appends the row's condition to the filter; a value it cannot write stays inline.
    fn add_condition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.condition_field.read(cx).selected_value().cloned();
        let operator = self.condition_op.read(cx).selected_value().cloned();
        let (Some(field), Some(operator)) = (field, operator) else {
            return;
        };
        let field_type = self.fields.iter().find(|f| f.name == field);
        let field_type = field_type.map_or("string", |f| &f.r#type);
        let value = self.condition_value.read(cx).value().trim().to_string();
        let prefix = format!("{field}:=");
        let clause = facet_clause(&field, field_type, &[&value])
            .and_then(|c| Some(format!("{field}{operator}{}", c.strip_prefix(&prefix)?)));
        let Some(clause) = clause else {
            self.condition_error = Some(
                t!("search.condition.refused", value = value)
                    .to_string()
                    .into(),
            );
            return cx.notify();
        };
        self.condition_error = None;
        let filter = self.filter.read(cx).value(cx);
        let filter = if filter.trim().is_empty() {
            clause
        } else {
            format!("{} && {clause}", filter.trim())
        };
        self.filter
            .update(cx, |f, cx| f.set_value(&filter, window, cx));
        self.condition_value
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn chips_control(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        while self.chip_focus.len() < self.chips.len() {
            self.chip_focus.push(cx.focus_handle().tab_stop(true));
        }
        let theme = cx.theme();
        let (secondary, ring, muted) = (theme.secondary, theme.ring, theme.muted_foreground);
        let chips: Vec<_> = self
            .chips
            .iter()
            .enumerate()
            .map(|(index, chip)| {
                let name = SharedString::from(chip.clone());
                let id = format!("search.chip.{index}");
                let drag = DragChip {
                    index,
                    name: name.clone(),
                };
                h_flex()
                    .id(SharedString::from(id.clone()))
                    .test_support()
                    .aria_label(name.clone())
                    .key_context(CHIP)
                    .track_focus(&self.chip_focus[index])
                    .on_action(cx.listener(move |this, _: &MoveChipLeft, window, cx| {
                        if index > 0 {
                            this.move_chip(index, index - 1, cx);
                            this.focus_chip(index - 1, window, cx);
                        }
                    }))
                    .on_action(cx.listener(move |this, _: &MoveChipRight, window, cx| {
                        this.move_chip(index, index + 1, cx);
                        this.focus_chip((index + 1).min(this.chips.len() - 1), window, cx);
                    }))
                    .on_action(cx.listener(move |this, _: &RemoveChip, window, cx| {
                        this.remove_chip(index, window, cx);
                    }))
                    .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_drop(cx.listener(move |this, drag: &DragChip, _, cx| {
                        this.move_chip(drag.index, index, cx);
                    }))
                    .gap_1()
                    .pl_2()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(transparent_black())
                    .bg(secondary)
                    .text_sm()
                    .focus(|s| s.border_color(ring))
                    .child(name)
                    .child(
                        Button::new(SharedString::from(format!("{id}.remove")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .accessibility_label(t!("search.form.remove_field").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_chip(index, window, cx);
                            })),
                    )
            })
            .collect();
        let hint = (!self.chips.is_empty()).then(|| {
            h_flex()
                .gap_1()
                .items_center()
                .text_xs()
                .text_color(muted)
                .children(Kbd::binding_for_action(&MoveChipLeft, Some(CHIP), window))
                .children(Kbd::binding_for_action(&MoveChipRight, Some(CHIP), window))
                .child(t!("search.form.chip_move").to_string())
                .children(Kbd::binding_for_action(&RemoveChip, Some(CHIP), window))
                .child(t!("search.form.chip_remove").to_string())
        });
        let add = t!("search.form.add_field").to_string();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .id("search.param.query_by")
                    .test_support()
                    .aria_label("query_by")
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .children(chips)
                    .child(
                        Combobox::new(&self.chip_picker)
                            .menu_width(px(240.))
                            .render_trigger(move |_, _, _| {
                                Button::new("search.chip.add")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label(add.clone())
                            }),
                    ),
            )
            .children(hint)
            .into_any_element()
    }

    fn filter_control(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let danger = cx.theme().danger;
        let error = self.condition_error.clone();
        v_flex()
            .gap_1()
            .child(
                div()
                    .id("search.param.filter_by")
                    .test_support()
                    .aria_label("filter_by")
                    .child(self.filter.clone()),
            )
            .child(
                h_flex()
                    .key_context(CONDITION_ROW)
                    .on_action(cx.listener(|this, _: &AddCondition, window, cx| {
                        this.add_condition(window, cx);
                    }))
                    .gap_1()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Select::new(&self.condition_field)
                                .id("search.condition.field")
                                .small()
                                .accessibility_label(t!("search.condition.field").to_string())
                                .placeholder(t!("search.condition.field").to_string()),
                        ),
                    )
                    .child(
                        div().w(px(72.)).child(
                            Select::new(&self.condition_op)
                                .id("search.condition.operator")
                                .small()
                                .accessibility_label(t!("search.condition.operator").to_string()),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.condition_value)
                                .id("search.condition.value")
                                .small()
                                .aria_label(t!("search.condition.value").to_string()),
                        ),
                    )
                    .child(
                        Button::new("search.condition.add")
                            .outline()
                            .small()
                            .label(t!("search.condition.add").to_string())
                            .children(Kbd::binding_for_action(
                                &AddCondition,
                                Some(CONDITION_ROW),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_condition(window, cx);
                            })),
                    ),
            )
            .children(error.map(|error| {
                div()
                    .id("search.condition.error")
                    .test_support()
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(danger)
                    .child(error)
            }))
            .into_any_element()
    }

    fn sort_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.sort_json_only {
            let note = SharedString::from(t!("search.sort.json_only").to_string());
            return div()
                .id("search.sort.json_only")
                .test_support()
                .aria_label(note.clone())
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(note)
                .into_any_element();
        }
        let rows: Vec<_> = self
            .sort
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let id = |part: &str| SharedString::from(format!("search.sort.{i}.{part}"));
                let eval = row.field.read(cx).selected_value().map(String::as_str) == Some(EVAL);
                let direction = if row.desc {
                    t!("search.sort.desc")
                } else {
                    t!("search.sort.asc")
                };
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Select::new(&row.field)
                                .id(id("field"))
                                .small()
                                .accessibility_label(t!("search.sort.field").to_string()),
                        ),
                    )
                    .when(eval, |el| {
                        el.child(
                            div().flex_1().min_w_0().child(
                                Input::new(&row.eval)
                                    .id(id("eval"))
                                    .small()
                                    .aria_label(t!("search.sort.eval").to_string()),
                            ),
                        )
                    })
                    .child(
                        Button::new(id("direction"))
                            .ghost()
                            .small()
                            .label(direction.to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.sort[i].desc = !this.sort[i].desc;
                                this.write_sort(cx);
                            })),
                    )
                    .child(
                        Button::new(id("remove"))
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .accessibility_label(t!("search.sort.remove").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.sort.remove(i);
                                this.write_sort(cx);
                            })),
                    )
            })
            .collect();
        v_flex()
            .gap_1()
            .children(rows)
            .child(
                h_flex().child(
                    Button::new("search.sort.add")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("search.sort.add").to_string())
                        .disabled(self.sort.len() >= MAX_SORT_ROWS)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_sort_row(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }

    fn row(&mut self, def: &ParamDef, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let key = def.key;
        let id = SharedString::from(format!("search.param.{key}"));
        let control = match (self.controls.get(key), def.kind) {
            (Some(Control::Input(input)), _) => Input::new(input)
                .id(id.clone())
                .small()
                .aria_label(key)
                .into_any_element(),
            (Some(Control::Select(select)), _) => Select::new(select)
                .id(id.clone())
                .small()
                .accessibility_label(key)
                .into_any_element(),
            (None, ParamKind::Filter) => self.filter_control(window, cx),
            (None, ParamKind::Sort) => self.sort_control(cx),
            _ => self.chips_control(window, cx),
        };
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(key),
            )
            .child(control)
            .children(self.errors.get(key).cloned().map(|error| {
                div()
                    .id(SharedString::from(format!("{id}.error")))
                    .test_support()
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error)
            }))
            .into_any_element()
    }
}

impl Render for SearchForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (common, more): (Vec<&ParamDef>, Vec<&ParamDef>) =
            FORM_PARAMS.iter().partition(|d| d.common);
        let common: Vec<_> = common.iter().map(|d| self.row(d, window, cx)).collect();
        let more: Vec<_> = more.iter().map(|d| self.row(d, window, cx)).collect();
        let open = self.more_open;
        let only_json = (self.only_json > 0).then(|| {
            SharedString::from(t!("search.form.only_json", count = self.only_json).to_string())
        });
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("search.form")
            .size_full()
            .overflow_y_scroll()
            .gap_2()
            .p_2()
            .children(common)
            .child(
                Collapsible::new()
                    .open(open)
                    .gap_2()
                    .child(
                        h_flex().child(
                            Button::new("search.more")
                                .ghost()
                                .small()
                                .icon(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .label(t!("search.form.more").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.more_open = !this.more_open;
                                    cx.notify();
                                })),
                        ),
                    )
                    .content(v_flex().gap_2().children(more)),
            )
            .children(only_json.map(|note| {
                div()
                    .id("search.form.only_json")
                    .test_support()
                    .aria_label(note.clone())
                    .text_xs()
                    .text_color(muted)
                    .child(note)
            }))
    }
}
