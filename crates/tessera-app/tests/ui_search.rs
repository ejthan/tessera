// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the search tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::state::AppState;
use tessera_app::views::dock::SearchPanel;
use tessera_app::views::root::RootView;
use tessera_app::views::search::SearchWorkbench;
use tessera_client::Collection;
use tessera_core::profile::{Environment, Profile};
use tessera_core::search::params::SearchDraft;
use tessera_core::search::results::SearchResult;

use common::*;

fn books() -> Collection {
    serde_json::from_value(json!({"name": "books", "fields": [
        {"name": "title", "type": "string"},
        {"name": "genre", "type": "string", "facet": true},
        {"name": "year", "type": "int32", "sort": true, "facet": true},
    ]}))
    .unwrap()
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    search: Arc<FakeSearch>,
    id: uuid::Uuid,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn new(cx: &mut TestAppContext) -> Self {
        Self::with(cx, Profile::new("test", vec![node()])).await
    }

    /// Connects `profile`, with `books` on the server, and opens its search tab.
    async fn with(cx: &mut TestAppContext, profile: Profile) -> Self {
        let f = Self::connected(cx, profile).await;
        f.open_tab(cx);
        wait_for_element(cx, f.handle, format!("tab.{}.search", f.id), true).await;
        f.settle(cx);
        f
    }

    async fn connected(cx: &mut TestAppContext, profile: Profile) -> Self {
        fake_backend(cx, Some((vec![books()], vec![])));
        fake_documents(cx);
        let search = install_search_fake(cx);
        let id = profile.id;
        fake_connector(cx, |_| Ok(server("30.2", true)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        tessera_core::secrets::SecretStore::set(&*secrets, id, &"k".to_string().into()).unwrap();
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        Self {
            handle,
            state,
            search,
            id,
            _rt: rt,
        }
    }

    fn open_tab(&self, cx: &mut TestAppContext) {
        cx.update(|cx| {
            self.state
                .update(cx, |s, cx| s.open_search(self.id, None, cx))
        });
    }

    fn panels(&self, cx: &mut TestAppContext) -> Vec<Entity<SearchPanel>> {
        self.handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<SearchPanel>().ok())
                    .collect()
            })
            .unwrap()
    }

    fn workbench(&self, cx: &mut TestAppContext) -> Entity<SearchWorkbench> {
        let panel = self.panels(cx).remove(0);
        cx.update(|cx| panel.read(cx).workbench().clone())
    }

    fn act<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
    ) -> R {
        let out = cx
            .update_window(self.handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let out = f(window, cx);
                window.render_frame(cx);
                out
            })
            .unwrap();
        cx.run_until_parked();
        out
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        self.act(cx, |_, _| {});
    }

    /// Lets `ms` pass, then draws what arrived.
    fn wait(&self, cx: &mut TestAppContext, ms: u64) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle(cx);
    }

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.act(cx, |window, cx| fill(window, id, text, cx));
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        let key = key.to_string();
        self.act(cx, |window, cx| window.press(&key, cx));
        self.settle(cx);
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.act(cx, |window, cx| window.click(id, cx));
        self.settle(cx);
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.try_find(id).is_some())
    }

    fn text_of(&self, cx: &mut TestAppContext, id: &str) -> String {
        let id = id.to_string();
        self.act(cx, |window, _| {
            window.find(id).label().unwrap_or_default().to_string()
        })
    }

    fn disabled(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.find(id).disabled() == Some(true))
    }

    fn params(&self, cx: &mut TestAppContext) -> serde_json::Map<String, Value> {
        let workbench = self.workbench(cx);
        cx.update(|cx| workbench.read(cx).drafts()[0].params.clone())
    }

    fn result(&self, cx: &mut TestAppContext) -> Option<SearchResult> {
        let workbench = self.workbench(cx);
        cx.update(|cx| workbench.read(cx).result().cloned())
    }

    /// Runs the search with the button.
    fn run(&self, cx: &mut TestAppContext) {
        self.click(cx, "search.run");
    }

    fn set_json(&self, cx: &mut TestAppContext, text: &str) {
        let workbench = self.workbench(cx);
        let editor = cx.update(|cx| workbench.read(cx).json().clone());
        let text = text.to_string();
        self.act(cx, |window, cx| {
            editor.update(cx, |e, cx| e.set_text(&text, window, cx));
        });
        self.settle(cx);
    }

    fn json_text(&self, cx: &mut TestAppContext) -> String {
        let workbench = self.workbench(cx);
        cx.update(|cx| {
            let editor = workbench.read(cx).json().read(cx);
            editor.text(cx)
        })
    }

    fn instant_off(&self, cx: &mut TestAppContext) {
        self.press(cx, TOGGLE_INSTANT);
    }
}

fn prod_profile() -> Profile {
    let mut profile = Profile::new("prod", vec![node()]);
    profile.environment = Environment::Prod;
    profile
}

#[gpui_kit::test]
async fn sidebar_opens_one_search_tab_per_connection(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx, Profile::new("test", vec![node()])).await;
    let link = format!("sidebar.{}.search", f.id);
    f.settle(cx);
    f.click(cx, &link);
    f.settle(cx);
    assert_eq!(f.panels(cx).len(), 1);
    f.click(cx, &link);
    assert_eq!(f.panels(cx).len(), 1);
}

#[gpui_kit::test]
async fn the_shortcut_opens_the_tab_of_the_selected_connection(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx, Profile::new("test", vec![node()])).await;
    f.press(cx, OPEN_SEARCH);
    assert_eq!(f.panels(cx).len(), 1);
}

#[gpui_kit::test]
async fn tab_shows_environment_badge(cx: &mut TestAppContext) {
    let mut profile = prod_profile();
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    let tab = format!("tab.{}.search", f.id);
    assert!(f.has(cx, &tab));
    assert!(f.has(cx, &format!("{tab}.readonly")));
}

#[gpui_kit::test]
async fn the_first_collection_is_the_default(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let workbench = f.workbench(cx);
    let collection = cx.update(|cx| workbench.read(cx).drafts()[0].collection.clone());
    assert_eq!(collection, "books");
}

#[gpui_kit::test]
async fn form_and_json_edit_the_same_map(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.q", "dune");
    f.fill(cx, "search.param.per_page", "5");
    assert_eq!(f.params(cx)["q"], "dune");
    assert_eq!(f.params(cx)["per_page"], 5);

    f.press(cx, SHOW_JSON);
    let json: Value = serde_json::from_str(&f.json_text(cx)).unwrap();
    assert_eq!(json["q"], "dune");
    assert_eq!(json["per_page"], 5);

    f.set_json(cx, r#"{"q": "emma", "enable_analytics": false}"#);
    assert_eq!(f.params(cx)["q"], "emma");
    f.press(cx, SHOW_FORM);
    let workbench = f.workbench(cx);
    let (q, page) = cx.update(|cx| {
        let form = workbench.read(cx).form().read(cx);
        (form.input_text("q", cx), form.input_text("per_page", cx))
    });
    assert_eq!(q, "emma");
    assert_eq!(page, "");
}

#[gpui_kit::test]
async fn an_emptied_control_removes_its_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.per_page", "5");
    f.fill(cx, "search.param.per_page", "");
    assert!(!f.params(cx).contains_key("per_page"));
}

#[gpui_kit::test]
async fn a_bad_number_stays_inline_and_keeps_the_map(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.per_page", "5");
    f.fill(cx, "search.param.per_page", "5x");
    assert_eq!(f.params(cx)["per_page"], 5);
    assert!(f.has(cx, "search.param.per_page.error"));
}

#[gpui_kit::test]
async fn bad_json_keeps_the_map_and_disables_run(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    assert!(!f.disabled(cx, "search.run"));
    let before = f.params(cx);

    f.set_json(cx, "{ nope");
    assert_eq!(f.params(cx), before);
    assert!(f.disabled(cx, "search.run"));

    f.set_json(cx, r#"{"Bad-Key": 1}"#);
    assert_eq!(f.params(cx), before);
    assert!(f.disabled(cx, "search.run"));
    assert!(f.text_of(cx, "search.json.error").contains("Bad-Key"));
    f.press(cx, SAVE);
    assert!(f.search.calls().is_empty());

    f.set_json(cx, r#"{"q": "ok"}"#);
    assert!(!f.disabled(cx, "search.run"));
    assert_eq!(f.params(cx)["q"], "ok");
}

#[gpui_kit::test]
async fn unknown_key_survives_a_form_edit(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "*", "x_custom": 1, "y_custom": "a"}"#);
    f.press(cx, SHOW_FORM);
    assert!(f.text_of(cx, "search.form.only_json").contains('2'));
    f.fill(cx, "search.param.q", "dune");
    assert_eq!(f.params(cx)["x_custom"], 1);
    assert_eq!(f.params(cx)["y_custom"], "a");
}

#[gpui_kit::test]
async fn new_draft_sends_analytics_false(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run(cx);
    let calls = f.search.calls();
    assert_eq!(calls[0].collection, "books");
    assert_eq!(calls[0].param("q"), Some("*"));
    assert_eq!(calls[0].param("enable_analytics"), Some("false"));
    assert_eq!(f.result(cx).unwrap().found, 12);
    assert!(f.text_of(cx, "search.status").contains("12"));
}

#[gpui_kit::test]
async fn raw_shows_the_response_text(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run(cx);
    f.press(cx, SHOW_RAW);
    let workbench = f.workbench(cx);
    let text = cx.update(|cx| workbench.read(cx).raw().read(cx).text(cx));
    assert!(text.unwrap().contains("\"found\": 12"));
}

#[gpui_kit::test]
async fn stale_answer_is_dropped(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    *f.search.body.lock().unwrap() = search_body(1);
    let first = f.search.hold_next();
    f.run(cx);
    assert!(!first.is_closed());

    *f.search.body.lock().unwrap() = search_body(2);
    f.run(cx);
    // The newer query cancelled the older one's request.
    assert!(first.is_closed());
    first.send(()).ok();
    f.settle(cx);
    assert_eq!(f.search.calls().len(), 2);
    assert_eq!(f.result(cx).unwrap().found, 2);
}

#[gpui_kit::test]
async fn error_shows_inline_with_old_result_dimmed(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run(cx);
    *f.search.fail.lock().unwrap() = Some((400, "Field `x` not found".into()));
    f.run(cx);
    assert!(
        f.text_of(cx, "search.error")
            .contains("Field `x` not found")
    );
    assert_eq!(f.result(cx).unwrap().found, 12);
    assert!(f.has(cx, "search.result.dimmed"));

    *f.search.fail.lock().unwrap() = None;
    f.run(cx);
    assert!(!f.has(cx, "search.error"));
    assert!(!f.has(cx, "search.result.dimmed"));
}

#[gpui_kit::test]
async fn instant_runs_once_after_150_ms(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    assert!(f.search.calls().is_empty());
    for q in ["d", "du", "dun", "dune"] {
        f.fill(cx, "search.param.q", q);
        f.wait(cx, 100);
    }
    assert!(f.search.calls().is_empty(), "still within the debounce");
    f.wait(cx, 60);
    let calls = f.search.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].param("q"), Some("dune"));
}

#[gpui_kit::test]
async fn a_newer_instant_query_cancels_the_running_one(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let first = f.search.hold_next();
    f.fill(cx, "search.param.q", "a");
    f.wait(cx, 150);
    assert_eq!(f.search.calls().len(), 1);
    f.fill(cx, "search.param.q", "ab");
    f.wait(cx, 150);
    assert!(first.is_closed());
    assert_eq!(f.search.calls().len(), 2);
}

#[gpui_kit::test]
async fn run_replaces_a_waiting_instant_run(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.q", "dune");
    f.run(cx);
    f.wait(cx, 300);
    assert_eq!(f.search.calls().len(), 1);
}

#[gpui_kit::test]
async fn instant_waits_for_good_json(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "a"}"#);
    f.set_json(cx, "{ nope");
    f.wait(cx, 300);
    assert!(f.search.calls().is_empty());
}

#[gpui_kit::test]
async fn opening_json_starts_no_search(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    f.press(cx, SHOW_FORM);
    f.wait(cx, 300);
    assert!(f.search.calls().is_empty());
}

#[gpui_kit::test]
async fn instant_is_off_on_prod(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, prod_profile()).await;
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 300);
    assert!(f.search.calls().is_empty());
    assert_eq!(
        f.act(cx, |window, _| window.find("search.instant").checked()),
        Some(false)
    );
    f.press(cx, TOGGLE_INSTANT);
    f.fill(cx, "search.param.q", "emma");
    f.wait(cx, 300);
    assert_eq!(f.search.calls().len(), 1);
}

#[gpui_kit::test]
async fn analytics_shows_a_notice_and_does_not_pause(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    assert!(!f.has(cx, "search.instant.analytics"));
    f.set_json(cx, r#"{"q": "a"}"#);
    assert!(f.has(cx, "search.instant.analytics"));
    f.wait(cx, 300);
    assert_eq!(f.search.calls().len(), 1);
}

#[gpui_kit::test]
async fn conversation_pauses_instant_and_needs_unlock(cx: &mut TestAppContext) {
    let mut profile = Profile::new("locked", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    f.press(cx, SHOW_JSON);
    f.set_json(
        cx,
        r#"{"q": "a", "enable_analytics": false, "conversation": true}"#,
    );
    f.wait(cx, 300);
    assert!(f.search.calls().is_empty());
    assert!(f.has(cx, "search.instant.paused"));

    f.run(cx);
    assert!(f.has(cx, "danger.input"), "the unlock dialog asks first");
    assert!(f.search.calls().is_empty());
}

#[gpui_kit::test]
async fn a_read_only_profile_may_run_a_plain_search(cx: &mut TestAppContext) {
    let mut profile = Profile::new("locked", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    f.run(cx);
    assert_eq!(f.search.calls().len(), 1);
}

#[gpui_kit::test]
async fn run_is_shortcut_and_shown_on_the_button(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    // In JSON mode, because the Add condition row shows the same chord.
    f.press(cx, SHOW_JSON);
    let button = f.act(cx, |window, _| window.find("search.run").bounds());
    let kbd = gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
        .debug_bounds(key("kbd:cmd-enter", "kbd:ctrl-enter"))
        .expect("a Kbd for Run is drawn");
    assert!(button.contains(&kbd.center()), "{kbd:?} not in {button:?}");
    f.press(cx, SAVE);
    assert_eq!(f.search.calls().len(), 1);
    f.press(cx, SHOW_FORM);
    f.act(cx, |window, cx| window.click("search.param.q", cx));
    f.press(cx, SAVE);
    assert_eq!(f.search.calls().len(), 2);
}

#[gpui_kit::test]
async fn find_focuses_the_query(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, FIND);
    f.press(cx, SELECT_ALL);
    f.act(cx, |window, cx| window.input("dune", cx));
    assert_eq!(f.params(cx)["q"], "dune");
}

#[gpui_kit::test]
async fn load_replaces_the_drafts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let mut draft = SearchDraft::new("books");
    draft.params.insert("q".into(), "emma".into());
    let id = f.id;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_search(id, Some(draft), cx))
    });
    f.settle(cx);
    assert_eq!(f.panels(cx).len(), 1);
    assert_eq!(f.params(cx)["q"], "emma");
    let workbench = f.workbench(cx);
    let q = cx.update(|cx| workbench.read(cx).form().read(cx).input_text("q", cx));
    assert_eq!(q, "emma");
}

#[gpui_kit::test]
async fn a_chip_moves_and_goes_by_keyboard(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "*", "query_by": "title,genre"}"#);
    f.press(cx, SHOW_FORM);
    let workbench = f.workbench(cx);
    let form = cx.update(|cx| workbench.read(cx).form().clone());
    f.act(cx, |window, cx| {
        form.update(cx, |form, cx| form.focus_chip(1, window, cx))
    });
    f.press(cx, "alt-left");
    assert_eq!(f.params(cx)["query_by"], "genre,title");
    f.press(cx, "delete");
    assert_eq!(f.params(cx)["query_by"], "title");
}

#[gpui_kit::test]
async fn add_condition_appends_to_the_filter(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    let workbench = f.workbench(cx);
    let form = cx.update(|cx| workbench.read(cx).form().clone());
    f.act(cx, |window, cx| {
        form.update(cx, |form, cx| {
            form.set_condition("year", ":>", window, cx);
        })
    });
    f.fill(cx, "search.condition.value", "1999");
    f.press(cx, SAVE);
    assert_eq!(f.params(cx)["filter_by"], "year:>1999");
    assert!(f.search.calls().is_empty(), "⌘↩ in the row adds, not runs");

    f.act(cx, |window, cx| {
        form.update(cx, |form, cx| {
            form.set_condition("genre", ":=", window, cx);
        })
    });
    f.fill(cx, "search.condition.value", "cr`ime");
    f.press(cx, SAVE);
    assert_eq!(f.params(cx)["filter_by"], "year:>1999");
    assert!(f.has(cx, "search.condition.error"));

    f.fill(cx, "search.condition.value", "crime");
    f.press(cx, SAVE);
    assert_eq!(f.params(cx)["filter_by"], "year:>1999 && genre:=`crime`");
    assert!(!f.has(cx, "search.condition.error"));
}

#[gpui_kit::test]
async fn sort_rows_write_sort_by(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.click(cx, "search.sort.add");
    let workbench = f.workbench(cx);
    let form = cx.update(|cx| workbench.read(cx).form().clone());
    f.act(cx, |window, cx| {
        form.update(cx, |form, cx| form.set_sort_field(0, "year", window, cx))
    });
    assert_eq!(f.params(cx)["sort_by"], "year:desc");
    f.click(cx, "search.sort.0.direction");
    assert_eq!(f.params(cx)["sort_by"], "year:asc");
    f.click(cx, "search.sort.0.remove");
    assert!(!f.params(cx).contains_key("sort_by"));
}

#[gpui_kit::test]
async fn a_sort_with_expressions_survives_the_round_trip(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    let sort = "_eval(genre:crime):desc,year:asc";
    f.set_json(cx, &json!({"q": "*", "sort_by": sort}).to_string());
    f.press(cx, SHOW_FORM);
    assert_eq!(f.params(cx)["sort_by"], sort);
    f.fill(cx, "search.sort.0.eval", "genre:drama");
    assert_eq!(f.params(cx)["sort_by"], "_eval(genre:drama):desc,year:asc");
}

#[gpui_kit::test]
async fn a_sort_the_rows_cannot_hold_is_left_to_json(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "*", "sort_by": "a:asc,b:asc,c:asc,d:asc"}"#);
    f.press(cx, SHOW_FORM);
    assert!(f.has(cx, "search.sort.json_only"));
    assert_eq!(f.params(cx)["sort_by"], "a:asc,b:asc,c:asc,d:asc");
}
