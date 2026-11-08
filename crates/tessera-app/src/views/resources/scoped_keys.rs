// SPDX-License-Identifier: GPL-3.0-or-later

//! Scoped keys: made and read here, offline, and tried against the server. Nothing in this panel
//! is stored.

use std::time::{Duration, Instant};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*, px,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rust_i18n::t;
use secrecy::SecretString;
use serde_json::Value;
use tessera_client::{ApiKey, Error};
use tessera_core::keys::scoped::{
    DecodeError, DecodedKey, ParentProblem, ScopedError, decode, generate,
};
use tessera_core::keys::{ParentNote, parent_note};
use tessera_core::search::results::parse_result;
use tessera_widgets::json_editor::{JsonEditor, JsonEditorEvent};

use super::super::search::hits::HitsView;
use super::{client_of, notice};
use crate::actions::{RunTest, TEST_PANEL};
use crate::admin::AdminBackendGlobal;
use crate::connection::{ConnectionState, error_text};

/// How long the Copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_secs(2);
/// The hits of a test.
const PER_PAGE: &str = "20";
/// A key the parameters start with lives this long.
const FIRST_LIFETIME: i64 = 86_400;

fn scoped_error_text(error: &ScopedError) -> String {
    match error {
        ScopedError::NotAnObject => t!("scoped.error.not_an_object"),
        ScopedError::Parent(ParentProblem::Short) => t!("scoped.error.parent_short"),
        ScopedError::Parent(ParentProblem::NotAscii) => t!("scoped.error.parent_not_ascii"),
        ScopedError::Number => t!("scoped.error.number"),
        ScopedError::IndexKey => t!("scoped.error.index_key"),
        ScopedError::ArrayItem(name) => t!("scoped.error.array_item", name = name),
        ScopedError::ExpiresAt => t!("scoped.error.expires_at"),
    }
    .to_string()
}

fn decode_error_text(error: DecodeError) -> String {
    match error {
        DecodeError::Base64 => t!("scoped.decode.base64"),
        DecodeError::TooShort => t!("scoped.decode.too_short"),
        DecodeError::NotUtf8 => t!("scoped.decode.not_utf8"),
        DecodeError::NotJson => t!("scoped.decode.not_json"),
    }
    .to_string()
}

fn date(at: i64) -> Option<String> {
    let at = Timestamp::from_second(at).ok()?;
    let local = at.to_zoned(TimeZone::system());
    Some(local.strftime("%Y-%m-%d %H:%M").to_string())
}

/// What a finished test shows.
enum TestAnswer {
    /// `found` of `out_of` documents.
    Visible(u64, u64),
    /// Why it failed, and whether the parent is the likely reason.
    Failed(SharedString, bool),
}

/// The right side's second view. It lives as long as its tab, so switching keeps its text.
pub struct ScopedKeysView {
    connection: Entity<ConnectionState>,
    keys: Vec<ApiKey>,
    parent: Entity<InputState>,
    masked: bool,
    params: Entity<JsonEditor>,
    generated: Entity<InputState>,
    generate_error: Option<SharedString>,
    decode_input: Entity<TextareaState>,
    decoded: Option<Result<DecodedKey, DecodeError>>,
    decoded_params: Entity<JsonEditor>,
    collection: Entity<SelectState<Vec<String>>>,
    list_loaded: Option<Instant>,
    picked: Option<String>,
    hits: Entity<HitsView>,
    answer: Option<TestAnswer>,
    test_task: Option<Task<()>>,
    copied: bool,
    copy_reset: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 5],
}

impl ScopedKeysView {
    /// An empty panel, whose parameters expire a day from now.
    pub(super) fn new(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let first = format!(
            "{{\n  \"filter_by\": \"\",\n  \"expires_at\": {}\n}}",
            Timestamp::now().as_second() + FIRST_LIFETIME
        );
        let parent = cx.new(|cx| InputState::new(window, cx).masked(true));
        let params = cx.new(|cx| JsonEditor::new(&first, false, window, cx));
        let decode_input = cx.new(|cx| TextareaState::new(window, cx).rows(3));
        let collection =
            cx.new(|cx| SelectState::new(Vec::new(), None, window, cx).searchable(true));
        let subscriptions = [
            cx.subscribe_in(
                &parent,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.regenerate(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &params,
                window,
                |this, _, _: &JsonEditorEvent, window, cx| this.regenerate(window, cx),
            ),
            cx.subscribe_in(
                &decode_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.redecode(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &collection,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, _, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.pick(name.clone(), cx);
                    }
                },
            ),
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.take_collections(window, cx);
            }),
        ];
        let mut this = Self {
            connection,
            keys: Vec::new(),
            parent,
            masked: true,
            params,
            generated: cx.new(|cx| InputState::new(window, cx)),
            generate_error: None,
            decode_input,
            decoded: None,
            decoded_params: cx.new(|cx| JsonEditor::new("", true, window, cx)),
            collection,
            list_loaded: None,
            picked: None,
            hits: cx.new(HitsView::new),
            answer: None,
            test_task: None,
            copied: false,
            copy_reset: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.take_collections(window, cx);
        this
    }

    /// The key list the parent is looked up in.
    pub(super) fn set_keys(&mut self, keys: Vec<ApiKey>, cx: &mut Context<Self>) {
        self.keys = keys;
        cx.notify();
    }

    /// What the test sends, and what Generate shows.
    pub fn generated(&self, cx: &App) -> String {
        self.generated.read(cx).value().to_string()
    }

    /// The parameters' editor.
    pub fn params(&self) -> &Entity<JsonEditor> {
        &self.params
    }

    fn take_collections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cache = &self.connection.read(cx).collection_cache;
        // The connection also changes with every poll.
        if cache.loaded_at.is_none() || cache.loaded_at == self.list_loaded {
            return;
        }
        self.list_loaded = cache.loaded_at;
        let collections = cache.collections.iter().flatten();
        let names: Vec<String> = collections.map(|c| c.name.clone()).collect();
        self.collection
            .update(cx, |s, cx| s.set_items(names, window, cx));
        if let Some(name) = self.picked.clone() {
            self.collection
                .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
        }
        cx.notify();
    }

    /// Takes `name` as the collection of the test.
    pub fn pick(&mut self, name: String, cx: &mut Context<Self>) {
        self.picked = Some(name);
        self.answer = None;
        self.test_task = None;
        cx.notify();
    }

    /// Generates from what is typed. Runs here, without the server.
    fn regenerate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parent = self.parent.read(cx).value();
        let text = self.params.read(cx).text(cx);
        let made = match serde_json::from_str::<Value>(&text) {
            _ if parent.is_empty() => Ok(String::new()),
            Ok(params) => generate(&parent, &params).map_err(|e| scoped_error_text(&e)),
            Err(e) => Err(t!("scoped.bad_json", detail = e).to_string()),
        };
        let (key, error) = match made {
            Ok(key) => (key, None),
            Err(text) => (String::new(), Some(text.into())),
        };
        self.generate_error = error;
        self.generated
            .update(cx, |g, cx| g.set_value(key, window, cx));
        cx.notify();
    }

    /// Reads the pasted key. It is not logged, and verifying needs the parent input.
    fn redecode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.decode_input.read(cx).value();
        self.decoded = (!text.trim().is_empty()).then(|| decode(&text));
        let shown = match &self.decoded {
            Some(Ok(key)) => serde_json::to_string_pretty(&key.params).unwrap_or_default(),
            _ => String::new(),
        };
        self.decoded_params
            .update(cx, |e, cx| e.set_text(&shown, window, cx));
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.masked = !self.masked;
        self.parent.update(cx, |i, cx| i.toggle_masked(window, cx));
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.generated(cx)));
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

    /// The key Run sends: the pasted one, else the generated one.
    fn test_key(&self, cx: &App) -> Option<String> {
        let pasted = self.decode_input.read(cx).value().trim().to_string();
        let key = if pasted.is_empty() {
            self.generated(cx)
        } else {
            pasted
        };
        (!key.is_empty()).then_some(key)
    }

    fn run_blocked(&self, cx: &App) -> bool {
        self.picked.is_none() || self.test_key(cx).is_none()
    }

    /// A new run replaces the one before. It reads, so it runs on a read-only profile.
    fn run(&mut self, cx: &mut Context<Self>) {
        let (Some(name), Some(key), Some(client)) = (
            self.picked.clone(),
            self.test_key(cx),
            client_of(&self.connection, cx),
        ) else {
            return;
        };
        let params = [
            ("q", "*"),
            ("per_page", PER_PAGE),
            ("enable_analytics", "false"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let search = cx.global::<AdminBackendGlobal>().0.scoped_search(
            client,
            SecretString::from(key),
            name,
            params.to_vec(),
        );
        self.answer = None;
        self.test_task = Some(cx.spawn(async move |this, cx| {
            let answer = search.await;
            this.update(cx, |this, cx| {
                this.answer = Some(this.take_answer(answer, cx));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn take_answer(&mut self, answer: Result<String, Error>, cx: &mut Context<Self>) -> TestAnswer {
        let parsed = answer
            .and_then(|text| parse_result(&text, ("<mark>", "</mark>")).map_err(Error::Decode));
        match parsed {
            Ok(result) => {
                self.hits.update(cx, |h, cx| h.set_result(&result, cx));
                TestAnswer::Visible(result.found, result.out_of)
            }
            Err(e) => {
                let forbidden = matches!(e, Error::Http { status: 401, .. });
                let profile = &self.connection.read(cx).profile;
                TestAnswer::Failed(error_text(&e, profile).into(), forbidden)
            }
        }
    }

    fn note(&self, cx: &App) -> Option<String> {
        let parent = self.parent.read(cx).value();
        if parent.is_empty() {
            return None;
        }
        match parent_note(&self.keys, &parent, Timestamp::now()) {
            ParentNote::SearchOnly => None,
            ParentNote::NotSearchOnly(id) => Some(t!("scoped.note.not_search_only", id = id)),
            ParentNote::Expired(id) => Some(t!("scoped.note.expired", id = id)),
            ParentNote::Unknown => Some(t!("scoped.note.unknown")),
        }
        .map(|text| text.to_string())
    }

    fn generate_section(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, warning) = (theme.danger, theme.warning);
        let show = if self.masked {
            t!("connect.key.show")
        } else {
            t!("connect.key.hide")
        };
        let copy = if self.copied {
            t!("keys.copied")
        } else {
            t!("keys.copy")
        };
        let empty = self.generated(cx).is_empty();
        v_flex()
            .gap_2()
            .child(div().text_sm().child(t!("scoped.parent").to_string()))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.parent)
                                .id("scoped.parent")
                                .aria_label(t!("scoped.parent").to_string()),
                        ),
                    )
                    .child(
                        Button::new("scoped.parent_toggle")
                            .ghost()
                            .icon(if self.masked {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            })
                            .accessibility_label(show.to_string())
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    ),
            )
            .children(
                self.note(cx)
                    .map(|text| notice("scoped.note", text.into(), warning)),
            )
            .child(div().text_sm().child(t!("scoped.params").to_string()))
            .child(div().h(px(120.)).child(self.params.clone()))
            .children(
                self.generate_error
                    .clone()
                    .map(|e| notice("scoped.error", e, danger)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.generated)
                                .id("scoped.key")
                                .aria_label(t!("scoped.key").to_string())
                                .disabled(true),
                        ),
                    )
                    .child(
                        Button::new("scoped.copy")
                            .small()
                            .label(copy.to_string())
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                    ),
            )
    }

    fn decode_section(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let lines: Vec<(&'static str, String)> = match &self.decoded {
            Some(Ok(key)) => {
                let parent = self.parent.read(cx).value();
                let expires = key.expires_at.and_then(date);
                let signature = if parent.is_empty() {
                    t!("scoped.not_verified")
                } else if key.verify(&parent) {
                    t!("scoped.signature_valid")
                } else {
                    t!("scoped.signature_invalid")
                };
                vec![
                    (
                        "scoped.decoded.prefix",
                        t!("scoped.prefix", prefix = key.prefix).to_string(),
                    ),
                    (
                        "scoped.decoded.expires",
                        expires.unwrap_or_else(|| t!("scoped.no_expiry").to_string()),
                    ),
                    ("scoped.signature", signature.to_string()),
                ]
            }
            _ => Vec::new(),
        };
        let failed = match &self.decoded {
            Some(Err(e)) => Some(decode_error_text(*e)),
            _ => None,
        };
        let shown = matches!(self.decoded, Some(Ok(_)));
        v_flex()
            .gap_2()
            .child(div().text_sm().child(t!("scoped.decode").to_string()))
            .child(
                div().id("scoped.decode").test_support().child(
                    Textarea::new(&self.decode_input)
                        .aria_label(t!("scoped.decode").to_string())
                        .h(px(64.)),
                ),
            )
            .children(failed.map(|e| notice("scoped.decode_error", e.into(), danger)))
            .children(
                lines
                    .into_iter()
                    .map(|(id, text)| notice(id, text.into(), muted)),
            )
            .when(shown, |el| {
                el.child(div().h(px(100.)).child(self.decoded_params.clone()))
            })
    }

    fn test_section(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let (line, hint, show_hits) = match &self.answer {
            Some(TestAnswer::Visible(found, out_of)) => {
                let text = t!("scoped.visible", found = found, out_of = out_of).to_string();
                (
                    Some(notice("scoped.visible", text.into(), muted)),
                    None,
                    true,
                )
            }
            Some(TestAnswer::Failed(text, forbidden)) => (
                Some(notice("scoped.test_error", text.clone(), danger)),
                forbidden.then(|| {
                    notice(
                        "scoped.forbidden_hint",
                        t!("scoped.forbidden_hint").to_string().into(),
                        danger,
                    )
                }),
                false,
            ),
            None => (None, None, false),
        };
        v_flex()
            .gap_2()
            .child(div().text_sm().child(t!("scoped.test").to_string()))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(220.)).child(
                            Select::new(&self.collection)
                                .id("scoped.collection")
                                .small()
                                .accessibility_label(t!("test.collection").to_string())
                                .placeholder(t!("test.collection").to_string()),
                        ),
                    )
                    .child(
                        Button::new("scoped.run")
                            .primary()
                            .small()
                            .label(t!("test.run").to_string())
                            .children(Kbd::binding_for_action(&RunTest, Some(TEST_PANEL), window))
                            .disabled(self.run_blocked(cx))
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .children(line)
            .children(hint)
            .when(show_hits, |el| {
                el.child(div().h(px(220.)).child(self.hits.clone()))
            })
    }
}

impl Render for ScopedKeysView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let generate = self.generate_section(cx);
        let decode = self.decode_section(cx);
        let test = self.test_section(window, cx);
        div()
            .id("scoped.panel")
            .key_context(TEST_PANEL)
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .on_action(cx.listener(|this, _: &RunTest, _, cx| this.run(cx)))
            .child(
                v_flex()
                    .gap_4()
                    .p_2()
                    .child(generate)
                    .child(decode)
                    .child(test),
            )
    }
}
