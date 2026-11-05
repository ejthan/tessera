// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the filter input in a headless GPUI window.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Entity, Point, Task, TestAppContext, Window, WindowBounds,
    WindowHandle, WindowOptions, base::Root, px, size,
};
use tessera_widgets::filter_input::FACET_DELAY;
use tessera_widgets::{FacetFetch, FilterField, FilterInput, FilterInputEvent};

fn field(name: &str, r#type: &str, facet: bool) -> FilterField {
    FilterField {
        name: name.into(),
        r#type: r#type.into(),
        facet,
    }
}

struct Wrapper(Entity<FilterInput>);

impl gpui_kit::Render for Wrapper {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.0.clone()
    }
}

type Calls = Rc<RefCell<Vec<(String, String)>>>;

struct Fixture {
    window: WindowHandle<Root>,
    input: Entity<FilterInput>,
    calls: Calls,
    events: Rc<RefCell<Vec<FilterInputEvent>>>,
}

impl Fixture {
    fn open(cx: &mut TestAppContext) -> Self {
        let calls = Calls::default();
        let seen = calls.clone();
        let fetch: FacetFetch = Rc::new(move |field, prefix, _: &mut App| {
            seen.borrow_mut().push((field, prefix.clone()));
            let values = ["crime", "crimea", "science fiction", "`odd"];
            let values = values.iter().filter(|v| v.starts_with(&prefix));
            Task::ready(values.map(ToString::to_string).collect())
        });
        let fields = vec![
            field("title", "string", false),
            field("genre", "string", true),
            field("year", "int32", true),
            field("active", "bool", true),
        ];
        let (window, input) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::component::set_locale("en");
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(600.), px(400.)),
            };
            let mut input = None;
            let (window, _) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let i = cx.new(|cx| FilterInput::new(fields, fetch, window, cx));
                    input = Some(i.clone());
                    cx.new(|_| Wrapper(i))
                },
            )
            .unwrap();
            (window.downcast::<Root>().unwrap(), input.unwrap())
        });
        let events: Rc<RefCell<Vec<FilterInputEvent>>> = Rc::default();
        let log = events.clone();
        cx.update(|cx| {
            cx.subscribe(&input, move |_, event: &FilterInputEvent, _| {
                log.borrow_mut().push(event.clone());
            })
            .detach();
        });
        let f = Self {
            window,
            input,
            calls,
            events,
        };
        f.act(cx, |window, cx| {
            f.input.update(cx, |i, cx| i.focus(window, cx));
        });
        f
    }

    fn act(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window, &mut App)) {
        cx.update_window(self.window.into(), |_, window, cx| {
            window.render_frame(cx);
            f(window, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        // The menu takes its items while drawing.
        cx.update_window(self.window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        self.act(cx, |window, cx| window.input(text, cx));
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        self.act(cx, |window, cx| window.press(key, cx));
    }

    /// Lets `delay` pass and draws what it brought.
    fn wait(&self, cx: &mut TestAppContext, delay: Duration) {
        cx.executor().advance_clock(delay);
        self.act(cx, |_, _| {});
    }

    /// Labels of the open completion menu.
    fn menu(&self, cx: &mut TestAppContext) -> Vec<String> {
        cx.update(|cx| {
            let state = self.input.read(cx).state().read(cx);
            let menu = state.completion_menu_state();
            let items = menu.items.iter().map(|i| i.label.clone());
            items.filter(|_| menu.open).collect()
        })
    }

    fn value(&self, cx: &mut TestAppContext) -> String {
        cx.update(|cx| self.input.read(cx).value(cx))
    }

    fn set_value(&self, cx: &mut TestAppContext, text: &str) {
        self.act(cx, |window, cx| {
            self.input.update(cx, |i, cx| i.set_value(text, window, cx));
        });
    }

    fn submitted(&self) -> Vec<String> {
        let events = self.events.borrow();
        events
            .iter()
            .filter_map(|e| match e {
                FilterInputEvent::Submitted(text) => Some(text.clone()),
                FilterInputEvent::Changed(_) => None,
            })
            .collect()
    }
}

#[gpui_kit::test]
async fn typing_offers_fields(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "t");
    assert_eq!(f.menu(cx), ["title"]);
}

#[gpui_kit::test]
async fn after_field_offers_operators(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "year:");
    assert_eq!(f.menu(cx), [":=", ":>", ":>=", ":<", ":<=", ":!=", ":["]);
    f.type_text(cx, ">");
    assert_eq!(f.menu(cx), [":>", ":>="]);
}

#[gpui_kit::test]
async fn a_finished_condition_opens_no_menu_so_enter_submits(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "title:a ");
    assert!(f.menu(cx).is_empty());
    f.press(cx, "enter");
    assert_eq!(f.value(cx), "title:a ");
    assert_eq!(f.submitted(), ["title:a"]);
}

#[gpui_kit::test]
async fn joins_are_typed_and_the_next_field_is_offered(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "title:a &");
    assert!(f.menu(cx).is_empty());
    f.type_text(cx, "& g");
    assert_eq!(f.menu(cx), ["genre"]);
}

#[gpui_kit::test]
async fn a_blank_text_opens_no_menu_and_enter_submits_it(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "t");
    f.press(cx, "backspace");
    assert!(f.menu(cx).is_empty());
    f.press(cx, "enter");
    assert_eq!(f.value(cx), "");
    assert_eq!(f.submitted(), [""]);
    f.type_text(cx, "  ");
    assert!(f.menu(cx).is_empty());
}

#[gpui_kit::test]
async fn a_value_equal_to_its_only_suggestion_needs_one_enter(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=crimea");
    f.wait(cx, FACET_DELAY);
    assert!(f.menu(cx).is_empty());
    f.press(cx, "enter");
    assert_eq!(f.submitted(), ["genre:=crimea"]);
}

#[gpui_kit::test]
async fn a_value_with_longer_suggestions_keeps_the_menu(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=crime");
    f.wait(cx, FACET_DELAY);
    assert_eq!(f.menu(cx), ["crime", "crimea"]);
}

#[gpui_kit::test]
async fn a_finished_bool_opens_no_menu(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "active:=true");
    assert!(f.menu(cx).is_empty());
}

#[gpui_kit::test]
async fn an_open_quote_keeps_the_menu_so_a_pick_closes_it(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=`science fiction");
    f.wait(cx, FACET_DELAY);
    assert_eq!(f.menu(cx), ["science fiction"]);
}

#[gpui_kit::test]
async fn a_value_that_cannot_be_quoted_is_not_offered(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre: ");
    f.wait(cx, FACET_DELAY);
    assert_eq!(f.menu(cx), ["crime", "crimea", "science fiction"]);
}

#[gpui_kit::test]
async fn enter_cancels_a_pending_fetch(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=cr");
    f.press(cx, "enter");
    assert_eq!(f.submitted(), ["genre:=cr"]);
    f.wait(cx, FACET_DELAY);
    assert!(f.calls.borrow().is_empty());
    assert!(f.menu(cx).is_empty());
}

#[gpui_kit::test]
async fn secondary_enter_submits_and_closes_an_open_menu(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "tit");
    let key = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    f.press(cx, key);
    assert_eq!(f.submitted(), ["tit"]);
    assert!(f.menu(cx).is_empty());
}

#[gpui_kit::test]
async fn facet_value_waits_then_fetches(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=cr");
    f.wait(cx, FACET_DELAY - Duration::from_millis(1));
    assert!(f.calls.borrow().is_empty());
    f.wait(cx, Duration::from_millis(1));
    assert_eq!(*f.calls.borrow(), [("genre".to_string(), "cr".to_string())]);
    assert_eq!(f.menu(cx), ["crime", "crimea"]);
}

#[gpui_kit::test]
async fn quick_typing_fetches_only_the_last_prefix(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=c");
    f.wait(cx, Duration::from_millis(100));
    f.type_text(cx, "r");
    f.wait(cx, FACET_DELAY);
    assert_eq!(*f.calls.borrow(), [("genre".to_string(), "cr".to_string())]);
}

#[gpui_kit::test]
async fn a_dropped_input_does_not_fetch(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=cr");
    cx.update_window(f.window.into(), |_, window, _| window.remove_window())
        .unwrap();
    let Fixture { input, calls, .. } = f;
    drop(input);
    cx.update(|_| {});
    cx.run_until_parked();
    cx.executor().advance_clock(FACET_DELAY);
    cx.run_until_parked();
    assert!(calls.borrow().is_empty());
}

#[gpui_kit::test]
async fn bool_offers_true_false_without_fetching(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "active:=");
    f.type_text(cx, "t");
    assert_eq!(f.menu(cx), ["true"]);
    f.press(cx, "backspace");
    f.type_text(cx, " ");
    assert_eq!(f.menu(cx), ["true", "false"]);
    f.wait(cx, FACET_DELAY);
    assert!(f.calls.borrow().is_empty());
}

#[gpui_kit::test]
async fn plain_field_value_offers_nothing(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "title:=a");
    f.wait(cx, FACET_DELAY);
    assert!(f.menu(cx).is_empty());
    assert!(f.calls.borrow().is_empty());
}

#[gpui_kit::test]
async fn enter_with_open_menu_picks_and_does_not_submit(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "tit");
    f.press(cx, "enter");
    assert_eq!(f.value(cx), "title");
    assert!(f.menu(cx).is_empty());
    assert!(f.submitted().is_empty());
    let changed = FilterInputEvent::Changed("title".into());
    assert_eq!(f.events.borrow().last(), Some(&changed));
}

#[gpui_kit::test]
async fn enter_with_closed_menu_submits(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.set_value(cx, "  genre:=crime ");
    f.press(cx, "enter");
    assert_eq!(f.submitted(), ["genre:=crime"]);
    assert_eq!(f.value(cx), "  genre:=crime ");
}

#[gpui_kit::test]
async fn escape_closes_the_menu_and_enter_then_submits(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "tit");
    f.press(cx, "escape");
    assert!(f.menu(cx).is_empty());
    f.press(cx, "enter");
    assert_eq!(f.submitted(), ["tit"]);
}

#[gpui_kit::test]
async fn pick_replaces_only_the_prefix(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "year:>2000 && gen");
    assert_eq!(f.menu(cx), ["genre"]);
    f.press(cx, "enter");
    assert_eq!(f.value(cx), "year:>2000 && genre");
}

#[gpui_kit::test]
async fn pick_quotes_a_value_and_replaces_its_opening_backtick(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "genre:=`sci");
    f.wait(cx, FACET_DELAY);
    f.press(cx, "enter");
    assert_eq!(f.value(cx), "genre:=`science fiction`");
}

#[gpui_kit::test]
async fn pick_counts_offsets_in_multibyte_text(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "title:=mü && ge");
    f.press(cx, "enter");
    assert_eq!(f.value(cx), "title:=mü && genre");
}

#[gpui_kit::test]
async fn newline_becomes_space(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "title:=a");
    f.press(cx, "home");
    f.press(cx, "right");
    f.press(cx, "shift-enter");
    assert_eq!(f.value(cx), "t itle:=a");
    assert!(f.submitted().is_empty());
    f.type_text(cx, "x");
    assert_eq!(f.value(cx), "t xitle:=a", "the cursor stays where it was");
}

#[gpui_kit::test]
async fn tab_leaves_the_text_alone(cx: &mut TestAppContext) {
    let f = Fixture::open(cx);
    f.type_text(cx, "a");
    f.press(cx, "tab");
    assert_eq!(f.value(cx), "a");
}
