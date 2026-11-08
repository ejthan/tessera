// SPDX-License-Identifier: GPL-3.0-or-later

//! The test panel of a set: one search twice, with and without the set. It only reads.

use std::time::Instant;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{Collection, SetKind};
use tessera_core::relevance::attachment::attached;
use tessera_core::relevance::test::{SideMeaning, TestBlocked, TestSides, test_sides};
use tessera_core::search::params::text_query_fields;

use super::super::search::hits::HitsView;
use super::{client_of, notice};
use crate::actions::{FocusQuery, RESOURCES, RunTest, ShowAttachment, TEST_PANEL};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, error_text};
use crate::search::{SearchAnswer, SearchBackendGlobal};

/// How the server words a missing attached set.
const MISSING_SET: &str = "Synonym index not found";

fn meaning_text(meaning: SideMeaning) -> String {
    match meaning {
        SideMeaning::AsConfigured => t!("test.side.as_configured"),
        SideMeaning::WithSet => t!("test.side.with_set"),
        SideMeaning::NoSynonyms => t!("test.side.no_synonyms"),
        SideMeaning::NoCurations => t!("test.side.no_curations"),
    }
    .to_string()
}

/// The panel of one [`SetsView`](super::sets::SetsView). Its tasks die with it.
pub struct TestPanel {
    kind: SetKind,
    connection: Entity<ConnectionState>,
    set: Option<String>,
    collection: Entity<SelectState<Vec<String>>>,
    list_loaded: Option<Instant>,
    /// The picked collection.
    picked: Option<String>,
    /// What the server answered for it, so that a recent attach counts.
    schema: Option<Collection>,
    reading: bool,
    read_task: Option<Task<()>>,
    read_error: Option<SharedString>,
    q: Entity<InputState>,
    query_by: Entity<InputState>,
    tags: Entity<InputState>,
    left: Entity<HitsView>,
    right: Entity<HitsView>,
    sides: Option<Result<TestSides, TestBlocked>>,
    answers: [Option<Result<SearchAnswer, SharedString>>; 2],
    tasks: [Option<Task<()>>; 2],
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 3],
}

impl TestPanel {
    /// A panel with nothing picked.
    pub(super) fn new(
        kind: SetKind,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let collection = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let placeholder = t!("test.q").to_string();
        let q = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let query_by = cx.new(|cx| InputState::new(window, cx));
        let tags = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = [
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.take_collections(window, cx);
            }),
            cx.subscribe_in(
                &collection,
                window,
                |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.pick(name.clone(), window, cx);
                    }
                },
            ),
            cx.subscribe(&tags, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_sides(cx);
                }
            }),
        ];
        let mut this = Self {
            kind,
            connection,
            set: None,
            collection,
            list_loaded: None,
            picked: None,
            schema: None,
            reading: false,
            read_task: None,
            read_error: None,
            q,
            query_by,
            tags,
            left: cx.new(HitsView::new),
            right: cx.new(HitsView::new),
            sides: None,
            answers: [None, None],
            tasks: [None, None],
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.take_collections(window, cx);
        this
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

    /// Shows `set`, keeping the collection and the text typed.
    pub(super) fn follow(
        &mut self,
        set: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if set != self.set {
            self.set = set;
            self.drop_answers();
            self.update_sides(cx);
            self.read(window, cx);
        }
    }

    /// Reads the picked collection again: the Collections tab may have changed it.
    pub(super) fn shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.q.update(cx, |q, cx| q.focus(window, cx));
        self.read(window, cx);
    }

    /// Takes `name` as the collection, with a fresh read of it.
    pub fn pick(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.picked = Some(name);
        self.schema = None;
        self.sides = None;
        self.drop_answers();
        self.read(window, cx);
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(name), Some(client)) = (self.picked.clone(), client_of(&self.connection, cx))
        else {
            return;
        };
        self.reading = true;
        self.read_error = None;
        let get = cx.global::<CollectionsBackendGlobal>().0.get(client, name);
        self.read_task = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = get.await;
            this.update_in(cx, |this, window, cx| {
                this.reading = false;
                match answer {
                    Ok(schema) => this.take_schema(schema, window, cx),
                    Err(e) => {
                        let profile = &this.connection.read(cx).profile;
                        this.read_error = Some(error_text(&e, profile).into());
                        this.schema = None;
                        this.sides = None;
                        this.drop_answers();
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// A newly picked collection fills `query_by`; reading it again keeps what was typed.
    fn take_schema(&mut self, schema: Collection, window: &mut Window, cx: &mut Context<Self>) {
        if self.schema.is_none() {
            let fields = text_query_fields(&schema).join(",");
            self.query_by
                .update(cx, |i, cx| i.set_value(fields, window, cx));
        }
        self.schema = Some(schema);
        self.update_sides(cx);
    }

    /// The sides for what is on screen. Answers to other sides mean something else.
    fn update_sides(&mut self, cx: &mut Context<Self>) {
        let sides = match (&self.set, &self.schema) {
            (Some(set), Some(schema)) => {
                let list = attached(schema, self.kind);
                Some(test_sides(
                    self.kind,
                    set,
                    list,
                    &self.tags.read(cx).value(),
                ))
            }
            _ => None,
        };
        if sides != self.sides {
            self.sides = sides;
            self.drop_answers();
        }
        cx.notify();
    }

    fn drop_answers(&mut self) {
        self.answers = [None, None];
        self.tasks = [None, None];
    }

    /// Whether Run has nothing to do now.
    pub(super) fn run_blocked(&self) -> bool {
        self.reading || !matches!(self.sides, Some(Ok(_)))
    }

    /// What `query_by` holds.
    pub fn query_by_text(&self, cx: &App) -> String {
        self.query_by.read(cx).value().to_string()
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if self.run_blocked() {
            return;
        }
        let (Some(Ok(sides)), Some(schema)) = (&self.sides, &self.schema) else {
            return;
        };
        let Some(client) = client_of(&self.connection, cx) else {
            return;
        };
        let q = self.q.read(cx).value().trim().to_string();
        let query_by = self.query_by.read(cx).value().trim().to_string();
        let mut common = vec![(
            "q".to_string(),
            if q.is_empty() { "*".to_string() } else { q },
        )];
        if !query_by.is_empty() {
            common.push(("query_by".to_string(), query_by));
        }
        common.push(("per_page".to_string(), "10".to_string()));
        common.push(("enable_analytics".to_string(), "false".to_string()));
        let backend = cx.global::<SearchBackendGlobal>().0.clone();
        let name = schema.name.clone();
        let own = [sides.left.1.clone(), sides.right.1.clone()];
        self.drop_answers();
        for (side, own) in own.into_iter().enumerate() {
            let params = common.iter().cloned().chain(own).collect();
            let tags = ("<mark>".to_string(), "</mark>".to_string());
            let search = backend.search(client.clone(), name.clone(), params, tags);
            self.tasks[side] = Some(cx.spawn(async move |this, cx| {
                let answer = search.await;
                this.update(cx, |this, cx| {
                    let hits = if side == 0 { &this.left } else { &this.right };
                    if let Ok(answer) = &answer {
                        hits.update(cx, |h, cx| h.set_result(&answer.result, cx));
                    }
                    let profile = &this.connection.read(cx).profile;
                    let answer = answer.map_err(|e| error_text(&e, profile).into());
                    this.answers[side] = Some(answer);
                    cx.notify();
                })
                .ok();
            }));
        }
        cx.notify();
    }

    /// The sentences above the sides.
    fn notes(&self) -> Vec<(&'static str, String)> {
        let mut notes = Vec::new();
        if let Some(Ok(sides)) = &self.sides {
            match sides.left.0 {
                SideMeaning::AsConfigured => {
                    notes.push(("test.note.added", t!("test.note.added").to_string()));
                }
                SideMeaning::NoSynonyms if sides.broad => {
                    let text = t!("test.note.broad_synonyms").to_string();
                    notes.push(("test.note.broad", text));
                }
                SideMeaning::NoCurations if sides.broad => {
                    let text = t!("test.note.broad_curations").to_string();
                    notes.push(("test.note.broad", text));
                }
                _ => {}
            }
        }
        let mut failed = self
            .answers
            .iter()
            .flatten()
            .filter_map(|a| a.as_ref().err());
        if failed.any(|e| e.contains(MISSING_SET)) {
            notes.push(("test.note.missing", t!("test.note.missing").to_string()));
        }
        notes
    }

    fn blocked_row(&self, window: &Window, cx: &App) -> Option<impl IntoElement + use<>> {
        let Some(Err(blocked)) = &self.sides else {
            return None;
        };
        let text = match blocked {
            TestBlocked::NotAttached => t!("test.blocked.not_attached"),
            TestBlocked::NameNotSendable => t!("test.blocked.name"),
            TestBlocked::BadTag => t!("test.blocked.tag"),
        };
        let focus = self.focus_handle.clone();
        let to_attachment = *blocked != TestBlocked::BadTag;
        Some(
            h_flex()
                .flex_none()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().child(notice(
                    "test.blocked",
                    text.to_string().into(),
                    cx.theme().danger,
                )))
                .when(to_attachment, |el| {
                    el.child(
                        Button::new("test.blocked.collections")
                            .small()
                            .label(t!("sets.tab.collections").to_string())
                            .children(Kbd::binding_for_action(
                                &ShowAttachment,
                                Some(RESOURCES),
                                window,
                            ))
                            .on_click(move |_, window, cx| {
                                focus.dispatch_action(&ShowAttachment, window, cx);
                            }),
                    )
                }),
        )
    }

    /// One side: what it means and how it went, then its hits or its error.
    fn side(&self, side: usize, cx: &App) -> impl IntoElement + use<> {
        let (prefix, hits) = if side == 0 {
            ("test.left", self.left.clone())
        } else {
            ("test.right", self.right.clone())
        };
        let sides = self.sides.as_ref().and_then(|s| s.as_ref().ok());
        let meaning = sides.map(|s| if side == 0 { s.left.0 } else { s.right.0 });
        let mut header = meaning.map(meaning_text).unwrap_or_default();
        if let Some(Ok(answer)) = &self.answers[side] {
            let stats = t!(
                "test.side.stats",
                found = answer.result.found,
                ms = answer.result.search_time_ms
            );
            header = format!("{header} · {stats}");
        }
        let header = SharedString::from(header);
        v_flex()
            .id(prefix)
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}.header")))
                    .test_support()
                    .aria_label(header.clone())
                    .flex_none()
                    .p_2()
                    .text_sm()
                    .child(header),
            )
            .child(match &self.answers[side] {
                Some(Err(error)) => notice(
                    SharedString::from(format!("{prefix}.error")),
                    error.clone(),
                    cx.theme().danger,
                )
                .into_any_element(),
                Some(Ok(_)) => div().flex_1().min_h_0().child(hits).into_any_element(),
                None => div().into_any_element(),
            })
    }
}

impl Render for TestPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted, border) = (theme.danger, theme.muted_foreground, theme.border);
        let curation = self.kind == SetKind::Curation;
        let notes = self.notes();
        let blocked = self.blocked_row(window, cx);
        v_flex()
            .key_context(TEST_PANEL)
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &RunTest, _, cx| this.run(cx)))
            .on_action(cx.listener(|this, _: &FocusQuery, window, cx| {
                this.q.update(cx, |q, cx| q.focus(window, cx));
            }))
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .items_center()
                    .child(
                        div().w(px(180.)).child(
                            Select::new(&self.collection)
                                .id("test.collection")
                                .small()
                                .accessibility_label(t!("test.collection").to_string())
                                .placeholder(t!("test.collection").to_string()),
                        ),
                    )
                    .child(
                        div().w(px(180.)).child(
                            Input::new(&self.q)
                                .small()
                                .suffix(div().children(Kbd::binding_for_action(
                                    &FocusQuery,
                                    Some(TEST_PANEL),
                                    window,
                                )))
                                .id("test.q")
                                .aria_label(t!("test.q").to_string()),
                        ),
                    )
                    .child(
                        div().w(px(180.)).child(
                            Input::new(&self.query_by)
                                .small()
                                // i18n-ok: Typesense parameter name
                                .prefix(div().text_sm().child("query_by"))
                                .id("test.query_by")
                                // i18n-ok: Typesense parameter name
                                .aria_label("query_by"),
                        ),
                    )
                    .when(curation, |el| {
                        el.child(
                            div().w(px(160.)).child(
                                Input::new(&self.tags)
                                    .small()
                                    .prefix(div().text_sm().child(t!("test.tags").to_string()))
                                    .id("test.tags")
                                    .aria_label(t!("test.tags").to_string()),
                            ),
                        )
                    })
                    .child(
                        Button::new("test.run")
                            .primary()
                            .small()
                            .label(t!("test.run").to_string())
                            .children(Kbd::binding_for_action(&RunTest, Some(TEST_PANEL), window))
                            .disabled(self.run_blocked())
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .children(
                self.read_error
                    .clone()
                    .map(|e| notice("test.read_error", e, danger)),
            )
            .children(blocked)
            .when(curation, |el| {
                el.child(notice(
                    "test.note.tags",
                    t!("test.note.tags").to_string().into(),
                    muted,
                ))
            })
            .children(
                notes
                    .into_iter()
                    .map(|(id, text)| notice(id, text.into(), muted)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(border)
                    .child(self.side(0, cx))
                    .child(div().h_full().w(px(1.)).bg(border))
                    .child(self.side(1, cx)),
            )
    }
}
