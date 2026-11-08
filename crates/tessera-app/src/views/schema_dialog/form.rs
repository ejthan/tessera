// SPDX-License-Identifier: GPL-3.0-or-later

//! The Form tab: collection name, one row per field with "More" for the rarer attributes,
//! and the collection options (design §7.6).

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::form::Field;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::json;
use tessera_core::ai::BUILT_IN_MODELS;
use tessera_core::schema::{FieldDraft, FieldType, IssuePath, SchemaDraft, Severity, default_sort};
use tessera_widgets::completion_input::{CompletionInput, CompletionInputEvent};
use tessera_widgets::confirm;
use tessera_widgets::{FieldTypeChanged, FieldTypeSelect};

use super::SchemaDialog;
use super::sync::{self, OPTS, Opt, TEXTS, Text};
use crate::actions::{AddField, SCHEMA_LAST_FIELD};

/// Flags shown in every row: key, read, write.
type Flag = (
    &'static str,
    fn(&FieldDraft) -> bool,
    fn(&mut FieldDraft, bool),
);

const FLAGS: [Flag; 3] = [
    ("facet", |f| f.facet, |f, v| f.facet = v),
    ("optional", |f| f.optional, |f, v| f.optional = v),
    ("index", |f| f.index, |f, v| f.index = v),
];

/// Flags behind "More".
const MORE_FLAGS: [Flag; 5] = [
    ("infix", |f| f.infix, |f, v| f.infix = v),
    ("stem", |f| f.stem, |f, v| f.stem = v),
    ("store", |f| f.store, |f, v| f.store = v),
    ("range_index", |f| f.range_index, |f, v| f.range_index = v),
    (
        "async_reference",
        |f| f.async_reference,
        |f, v| f.async_reference = v,
    ),
];

/// The inputs of the collection itself.
pub struct Form {
    /// One input per [`OPTS`] entry.
    pub opts: Vec<Entity<InputState>>,
    pub rows: Vec<FieldRow>,
    pub sorting: Entity<SelectState<Vec<String>>>,
    /// The choices `sorting` holds.
    pub sortable: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl Form {
    pub fn new(draft: &SchemaDraft, window: &mut Window, cx: &mut Context<SchemaDialog>) -> Self {
        let opts: Vec<_> = OPTS
            .iter()
            .map(|o| input(sync::opt_text(draft, *o), window, cx))
            .collect();
        let sortable = sync::sortable(draft);
        let selected = sortable
            .iter()
            .position(|n| *n == draft.default_sorting_field)
            .map(|row| gpui_kit::component::IndexPath::default().row(row));
        let sorting = cx.new(|cx| SelectState::new(sortable.clone(), selected, window, cx));
        let mut subscriptions: Vec<_> = OPTS
            .iter()
            .zip(&opts)
            .map(|(which, input)| {
                let which = *which;
                cx.subscribe_in(
                    input,
                    window,
                    move |this, input, ev: &InputEvent, window, cx| {
                        if matches!(ev, InputEvent::Change) {
                            let text = input.read(cx).value().to_string();
                            this.edit(|d| sync::set_opt_text(d, which, &text), window, cx);
                        }
                    },
                )
            })
            .collect();
        subscriptions.push(cx.subscribe_in(
            &sorting,
            window,
            |this, _, SelectEvent::Confirm(name): &SelectEvent<Vec<String>>, window, cx| {
                let name = name.clone().unwrap_or_default();
                this.edit(|d| d.default_sorting_field = name, window, cx);
            },
        ));
        Self {
            opts,
            rows: draft
                .fields
                .iter()
                .map(|f| FieldRow::new(f, window, cx))
                .collect(),
            sorting,
            sortable,
            _subscriptions: subscriptions,
        }
    }

    pub fn opt(&self, which: Opt) -> &Entity<InputState> {
        &self.opts[which as usize]
    }
}

/// Remote models worth completing beside the built-in ones.
const REMOTE_MODELS: [&str; 4] = [
    "openai/text-embedding-3-small",
    "google/embedding-gecko-001",
    "gcp/embedding-gecko-001",
    "azure/text-embedding-ada-002",
];

/// The inputs of a field that is auto-embedded. The draft stays the one source: every change
/// goes through `set_embedding` into `FieldDraft.embed`.
pub struct EmbedRow {
    pub from: Entity<CompletionInput>,
    pub model: Entity<CompletionInput>,
    pub key: Entity<InputState>,
    pub url: Entity<InputState>,
    _subscriptions: [Subscription; 4],
}

impl EmbedRow {
    /// The block of `f`. The key field starts empty, whatever the draft holds.
    pub fn new(f: &FieldDraft, window: &mut Window, cx: &mut Context<SchemaDialog>) -> Self {
        let (from_text, model_text, url_text) = sync::embed_parts(f);
        let completion = |label: String, text: String, window: &mut Window, cx: &mut Context<_>| {
            cx.new(|cx| {
                let mut input = CompletionInput::new(label, window, cx);
                input.set_value(&text, window, cx);
                input
            })
        };
        let from_label = t!("schema.embed.from").to_string();
        let from = completion(from_label, from_text, window, cx);
        let model_label = t!("schema.embed.model").to_string();
        let model = completion(model_label, model_text, window, cx);
        let candidates = BUILT_IN_MODELS.iter().chain(&REMOTE_MODELS);
        let candidates: Vec<String> = candidates.map(ToString::to_string).collect();
        model.update(cx, |input, _| input.set_candidates(candidates));
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let url = input(url_text, window, cx);
        let subscriptions = [
            cx.subscribe_in(
                &from,
                window,
                |this, input, _: &CompletionInputEvent, window, cx| {
                    let i = this.embed_row(|b| &b.from == input);
                    if let Some(i) = i {
                        this.embed_changed(i, None, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &model,
                window,
                |this, input, _: &CompletionInputEvent, window, cx| {
                    let i = this.embed_row(|b| &b.model == input);
                    if let Some(i) = i {
                        this.embed_changed(i, None, window, cx);
                    }
                },
            ),
            cx.subscribe_in(&key, window, |this, input, ev: &InputEvent, window, cx| {
                let i = this.embed_row(|b| &b.key == input);
                if let (InputEvent::Change, Some(i)) = (ev, i) {
                    let text = input.read(cx).value().to_string();
                    this.embed_changed(i, Some(text), window, cx);
                }
            }),
            cx.subscribe_in(&url, window, |this, input, ev: &InputEvent, window, cx| {
                let i = this.embed_row(|b| &b.url == input);
                if let (InputEvent::Change, Some(i)) = (ev, i) {
                    this.embed_changed(i, None, window, cx);
                }
            }),
        ];
        Self {
            from,
            model,
            key,
            url,
            _subscriptions: subscriptions,
        }
    }
}

/// The controls of one field.
pub struct FieldRow {
    /// One input per [`TEXTS`] entry.
    pub inputs: Vec<Entity<InputState>>,
    pub r#type: Entity<FieldTypeSelect>,
    /// The type `r#type` shows.
    pub ty: FieldType,
    /// "More" is open.
    pub more: bool,
    /// The block under an auto-embedded field.
    pub embed: Option<EmbedRow>,
    _subscriptions: Vec<Subscription>,
}

impl FieldRow {
    pub fn new(f: &FieldDraft, window: &mut Window, cx: &mut Context<SchemaDialog>) -> Self {
        let inputs: Vec<_> = TEXTS
            .iter()
            .map(|t| input(sync::field_text(f, *t), window, cx))
            .collect();
        let select = cx.new(|cx| FieldTypeSelect::new(&f.r#type, window, cx));
        let mut subscriptions: Vec<_> = TEXTS
            .iter()
            .zip(&inputs)
            .map(|(which, input)| {
                let which = *which;
                cx.subscribe_in(
                    input,
                    window,
                    move |this, input, ev: &InputEvent, window, cx| {
                        if !matches!(ev, InputEvent::Change) {
                            return;
                        }
                        let i = this
                            .form
                            .rows
                            .iter()
                            .position(|r| &r.inputs[which as usize] == input);
                        let text = input.read(cx).value().to_string();
                        if let Some(i) = i {
                            this.edit_field(
                                i,
                                |f| sync::set_field_text(f, which, &text),
                                window,
                                cx,
                            );
                        }
                    },
                )
            })
            .collect();
        subscriptions.push(cx.subscribe_in(
            &select,
            window,
            |this, select, FieldTypeChanged(ty): &FieldTypeChanged, window, cx| {
                let Some(i) = this.form.rows.iter().position(|r| &r.r#type == select) else {
                    return;
                };
                this.form.rows[i].ty = ty.clone();
                let ty = ty.clone();
                this.edit_field(i, |f| sync::retarget(f, |f| f.r#type = ty), window, cx);
            },
        ));
        Self {
            inputs,
            r#type: select,
            ty: f.r#type.clone(),
            more: false,
            embed: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn input(&self, which: Text) -> &Entity<InputState> {
        &self.inputs[which as usize]
    }
}

fn input(text: String, window: &mut Window, cx: &mut Context<SchemaDialog>) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).default_value(text))
}

/// A line of text with a test id and the same accessibility label.
pub fn line(id: SharedString, text: SharedString, color: Hsla) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(text.clone())
        .text_sm()
        .text_color(color)
        .child(text)
}

impl SchemaDialog {
    /// The row whose block `which` picks.
    pub(super) fn embed_row(&self, which: impl Fn(&EmbedRow) -> bool) -> Option<usize> {
        let mut rows = self.form.rows.iter();
        rows.position(|r| r.embed.as_ref().is_some_and(&which))
    }

    /// What the model input of field `i` offers.
    pub fn embed_models(&self, i: usize, cx: &gpui_kit::App) -> Vec<String> {
        let block = self.form.rows.get(i).and_then(|r| r.embed.as_ref());
        block.map_or_else(Vec::new, |b| b.model.read(cx).candidates())
    }

    /// The Auto-embed checkbox of field `i`. Unchecking asks first when it would lose anything.
    fn toggle_embed(&mut self, i: usize, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on {
            let embed = json!({"from": [], "model_config": {"model_name": ""}});
            return self.edit_field(i, |f| f.embed = Some(embed), window, cx);
        }
        if !self.draft.fields.get(i).is_some_and(sync::embed_holds) {
            return self.edit_field(i, |f| f.embed = None, window, cx);
        }
        let this = cx.entity().downgrade();
        confirm::open(
            t!("schema.embed.drop_title").to_string(),
            t!("schema.embed.drop_body").to_string(),
            t!("schema.embed.drop").to_string(),
            move |window, cx| {
                this.update(cx, |d, cx| {
                    d.edit_field(i, |f| f.embed = None, window, cx);
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn render_embed(&self, i: usize, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let block = self.form.rows.get(i)?.embed.as_ref()?;
        let muted = cx.theme().muted_foreground;
        let id = |key: &str| SharedString::from(format!("schema.field.{i}.embed.{key}"));
        let model = block.model.read(cx).value(cx);
        let built_in = model.trim().starts_with("ts/");
        let completion = |key: &str, label: String, input: &Entity<CompletionInput>| {
            Field::new()
                .label(label)
                .child(div().id(id(key)).test_support().child(input.clone()))
        };
        Some(
            v_flex()
                .gap_2()
                .pl_4()
                .child(completion(
                    "from",
                    t!("schema.embed.from").to_string(),
                    &block.from,
                ))
                .child(completion(
                    "model",
                    t!("schema.embed.model").to_string(),
                    &block.model,
                ))
                .when(built_in, |el| {
                    el.child(line(
                        id("note"),
                        t!("schema.embed.model_note").to_string().into(),
                        muted,
                    ))
                })
                .when(!built_in, |el| {
                    let key_label = t!("schema.embed.key").to_string();
                    let url_label = t!("schema.embed.url").to_string();
                    el.child(
                        Field::new().label(key_label.clone()).child(
                            Input::new(&block.key)
                                .mask_toggle()
                                .id(id("key"))
                                .aria_label(key_label),
                        ),
                    )
                    .child(
                        Field::new()
                            .label(url_label.clone())
                            .child(Input::new(&block.url).id(id("url")).aria_label(url_label)),
                    )
                }),
        )
    }

    /// A field edit; ignored when field `i` is gone.
    pub(super) fn edit_field(
        &mut self,
        i: usize,
        change: impl FnOnce(&mut FieldDraft),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(
            |d| {
                if let Some(f) = d.fields.get_mut(i) {
                    change(f);
                }
            },
            window,
            cx,
        );
    }

    /// Appends an empty string field and puts the cursor in its name.
    pub(super) fn add_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked() {
            return;
        }
        let f = FieldDraft::new("", FieldType::String);
        let row = FieldRow::new(&f, window, cx);
        row.input(Text::Name)
            .update(cx, |i, cx| i.focus(window, cx));
        self.form.rows.push(row);
        self.edit(|d| d.fields.push(f), window, cx);
    }

    fn remove_field(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked() || i >= self.form.rows.len() {
            return;
        }
        self.form.rows.remove(i);
        self.focus_first(window, cx);
        self.edit(|d| drop(d.fields.remove(i)), window, cx);
    }

    /// Swaps field `i` with the one below it.
    fn swap_down(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked() || i + 1 >= self.form.rows.len() {
            return;
        }
        self.form.rows.swap(i, i + 1);
        self.edit(|d| d.fields.swap(i, i + 1), window, cx);
    }

    fn flag(&self, i: usize, flag: Flag, f: &FieldDraft, cx: &Context<Self>) -> Checkbox {
        let (key, get, set) = flag;
        Checkbox::new(SharedString::from(format!("schema.field.{i}.{key}")))
            .checked(get(f))
            .label(t!(format!("schema.field.{key}")).to_string())
            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                let on = *on;
                this.edit_field(i, |f| set(f, on), window, cx);
            }))
    }

    fn render_row(&self, i: usize, f: &FieldDraft, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.form.rows[i];
        let theme = cx.theme();
        let (danger, warning, muted) = (theme.danger, theme.warning, theme.muted_foreground);
        let id = |key: &str| SharedString::from(format!("schema.field.{i}.{key}"));
        let last = i + 1 == self.form.rows.len();
        let sort_default = default_sort(&f.r#type);
        let issues: Vec<_> = self
            .issues
            .iter()
            .filter(|x| x.path == IssuePath::Field(i))
            .collect();
        let issue_text = issues
            .iter()
            .map(|x| t!(x.key).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let num_dim_ok = sync::num_dim(&row.input(Text::NumDim).read(cx).value()).is_some();
        let can_embed = num_dim_ok && self.embed_editable(f);
        let has_block = row.embed.is_some();
        let embed = self.render_embed(i, cx);
        let text_input = |which: Text, key: &str| {
            Field::new()
                .label(t!(format!("schema.field.{key}")).to_string())
                .child(
                    Input::new(row.input(which))
                        .id(id(key))
                        .aria_label(t!(format!("schema.field.{key}")).to_string()),
                )
        };
        let more = row.more.then(|| {
            v_flex()
                .gap_2()
                .pl_4()
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .children(MORE_FLAGS.map(|flag| self.flag(i, flag, f, cx))),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().w(px(140.)).child(text_input(Text::Locale, "locale")))
                        .child(
                            div()
                                .w(px(220.))
                                .child(text_input(Text::Reference, "reference")),
                        )
                        .child(div().w(px(150.)).child(text_input(Text::NumDim, "num_dim")))
                        .child(
                            div()
                                .w(px(150.))
                                .child(text_input(Text::VecDist, "vec_dist")),
                        ),
                )
        });
        let name_label = t!("schema.field.name").to_string();
        v_flex()
            .id(id("row"))
            .gap_1()
            .py_1()
            .when(last, |el| el.key_context(SCHEMA_LAST_FIELD))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(200.)).child(
                            Input::new(row.input(Text::Name))
                                .id(id("name"))
                                .aria_label(name_label),
                        ),
                    )
                    .child(
                        div()
                            .id(id("type"))
                            .test_support()
                            .w(px(170.))
                            .child(row.r#type.clone()),
                    )
                    .children(FLAGS.map(|flag| self.flag(i, flag, f, cx)))
                    .child(
                        Checkbox::new(id("sort"))
                            .checked(f.sort.unwrap_or(sort_default))
                            .label(t!("schema.field.sort").to_string())
                            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                                let on = *on;
                                this.edit_field(
                                    i,
                                    |f| f.sort = (on != default_sort(&f.r#type)).then_some(on),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(id("more"))
                            .ghost()
                            .small()
                            .label(
                                if row.more {
                                    t!("schema.field.less")
                                } else {
                                    t!("schema.field.more")
                                }
                                .to_string(),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(row) = this.form.rows.get_mut(i) {
                                    row.more = !row.more;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(id("up"))
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .accessibility_label(t!("schema.field.up").to_string())
                            .disabled(i == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(above) = i.checked_sub(1) {
                                    this.swap_down(above, window, cx)
                                }
                            })),
                    )
                    .child(
                        Button::new(id("down"))
                            .ghost()
                            .small()
                            .icon(IconName::ArrowDown)
                            .accessibility_label(t!("schema.field.down").to_string())
                            .disabled(last)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.swap_down(i, window, cx)
                            })),
                    )
                    .child(
                        Button::new(id("remove"))
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .accessibility_label(t!("schema.field.remove").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_field(i, window, cx)
                            })),
                    ),
            )
            .when(!issues.is_empty(), |el| {
                let color = if issues.iter().any(|x| x.severity == Severity::Error) {
                    danger
                } else {
                    warning
                };
                el.child(line(id("issues"), issue_text.into(), color))
            })
            .when(!num_dim_ok, |el| {
                el.child(line(
                    id("num_dim_error"),
                    t!("schema.field.num_dim_invalid").to_string().into(),
                    danger,
                ))
            })
            .when(self.blanked.contains(&f.name), |el| {
                // Edit mode would send the mask: that blocks the review.
                let color = if self.editing() { danger } else { warning };
                el.child(line(
                    id("secret"),
                    t!("schema.field.secret_blanked").to_string().into(),
                    color,
                ))
            })
            .when(can_embed, |el| {
                el.child(
                    Checkbox::new(id("embed"))
                        .checked(f.embed.is_some())
                        .label(t!("schema.field.embed").to_string())
                        .on_click(cx.listener(move |this, on: &bool, window, cx| {
                            this.toggle_embed(i, *on, window, cx);
                        })),
                )
            })
            .children(embed)
            .when(f.embed.is_some() && !has_block, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("schema.field.secret_hint").to_string()),
                )
            })
            .children(more)
            .into_any_element()
    }

    /// The Form tab, or while the JSON does not parse, the note that jumps to the error.
    pub(super) fn render_form(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(e) = &self.json_error {
            let text = SharedString::from(match e.line {
                Some(line) => t!("schema.json_locked", line = line).to_string(),
                None => t!("schema.json_invalid", message = e.message.clone()).to_string(),
            });
            return Button::new("schema.json_locked")
                .danger()
                .outline()
                .label(text)
                .on_click(cx.listener(|this, _, window, cx| this.jump_to_error(window, cx)))
                .into_any_element();
        }
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let label = |key: &str| t!(key).to_string();
        // PATCH cannot change these (design §5.3).
        let fixed = self.editing();
        let opt_input = |which: Opt, id: &'static str, key: &str| {
            let fixed = fixed && !matches!(which, Opt::SynonymSets | Opt::Metadata);
            Input::new(self.form.opt(which))
                .id(id)
                .aria_label(label(key))
                .disabled(fixed)
        };
        let hint = |key: &str| div().text_xs().text_color(muted).child(label(key));
        let metadata_ok = sync::metadata(&self.form.opt(Opt::Metadata).read(cx).value()).is_some();
        let rows: Vec<AnyElement> = self
            .draft
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| self.render_row(i, f, cx))
            .collect();
        v_flex()
            .gap_3()
            .child(Field::new().label(label("schema.name")).child(opt_input(
                Opt::Name,
                "schema.name",
                "schema.name",
            )))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(label("schema.fields")),
            )
            .child(v_flex().gap_1().children(rows))
            .child(
                h_flex().child(
                    Button::new("schema.add_field")
                        .outline()
                        .small()
                        .icon(IconName::Plus)
                        .label(label("schema.add_field"))
                        .children(Kbd::binding_for_action(
                            &AddField,
                            Some(SCHEMA_LAST_FIELD),
                            window,
                        ))
                        .on_click(cx.listener(|this, _, window, cx| this.add_field(window, cx))),
                ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(label("schema.options")),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div().w(px(220.)).child(
                            Field::new()
                                .label(label("schema.opt.default_sorting_field"))
                                .child(
                                    Select::new(&self.form.sorting)
                                        .id("schema.sorting")
                                        .placeholder(label("schema.sorting_none"))
                                        .cleanable(true)
                                        .disabled(fixed),
                                ),
                        ),
                    )
                    .child(
                        div().w(px(180.)).child(
                            Field::new()
                                .label(label("schema.opt.token_separators"))
                                .child(opt_input(
                                    Opt::TokenSeparators,
                                    "schema.token_separators",
                                    "schema.opt.token_separators",
                                ))
                                .child(hint("schema.chars_hint")),
                        ),
                    )
                    .child(
                        div().w(px(180.)).child(
                            Field::new()
                                .label(label("schema.opt.symbols_to_index"))
                                .child(opt_input(
                                    Opt::SymbolsToIndex,
                                    "schema.symbols_to_index",
                                    "schema.opt.symbols_to_index",
                                ))
                                .child(hint("schema.chars_hint")),
                        ),
                    ),
            )
            .child(
                Field::new()
                    .label(label("schema.synonym_sets"))
                    .child(opt_input(
                        Opt::SynonymSets,
                        "schema.synonym_sets",
                        "schema.synonym_sets",
                    ))
                    .child(hint("schema.names_hint")),
            )
            .child(
                Checkbox::new("schema.nested")
                    .checked(self.draft.enable_nested_fields)
                    .disabled(fixed)
                    .label(label("schema.opt.enable_nested_fields"))
                    .on_click(cx.listener(|this, on: &bool, window, cx| {
                        let on = *on;
                        this.edit(|d| d.enable_nested_fields = on, window, cx);
                    })),
            )
            .child(
                Field::new()
                    .label(label("schema.opt.metadata"))
                    .child(opt_input(
                        Opt::Metadata,
                        "schema.metadata",
                        "schema.opt.metadata",
                    ))
                    .when(!metadata_ok, |el| {
                        el.child(line(
                            "schema.metadata_error".into(),
                            label("schema.metadata_invalid").into(),
                            danger,
                        ))
                    }),
            )
            .into_any_element()
    }
}
