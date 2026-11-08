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
use tessera_client::{Collection, Preset};
use tessera_core::profile::{Environment, Profile};
use tessera_core::search::history::HistoryEntry;
use tessera_core::search::params::SearchDraft;
use tessera_core::search::results::SearchResult;
use tessera_core::search::saved::SavedQuery;
use tessera_widgets::Shown;

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
    backend: Arc<FakeData>,
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
        let backend = fake_backend(cx, Some((vec![books()], vec![])));
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
            backend,
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

    /// The `q` of each search in the history, newest first.
    fn history(&self, cx: &mut TestAppContext) -> Vec<String> {
        let id = self.id;
        cx.update(|cx| {
            let state = self.state.read(cx);
            let items = state.history.searches(id, "");
            let entries = items.iter().map(|item| match &item.entry {
                HistoryEntry::Search { searches, .. } => searches[0].params["q"].to_string(),
                HistoryEntry::Raw { .. } => unreachable!(),
            });
            entries.collect()
        })
    }

    fn saved(&self, cx: &mut TestAppContext) -> Vec<SavedQuery> {
        cx.update(|cx| self.state.read(cx).saved.queries.clone())
    }

    fn add_saved(&self, cx: &mut TestAppContext, name: &str, collection: &str, tags: &[&str]) {
        let query = SavedQuery {
            id: uuid::Uuid::new_v4(),
            profile: self.id,
            name: name.into(),
            tags: tags.iter().map(ToString::to_string).collect(),
            union: false,
            searches: vec![SearchDraft::new(collection)],
        };
        cx.update(|cx| {
            self.state
                .update(cx, |s, cx| s.save_query(query, cx))
                .detach();
        });
        self.settle(cx);
    }

    /// Saves the draft through the dialog; the name may ask to replace.
    fn save_as(&self, cx: &mut TestAppContext, name: &str, tags: &str) {
        self.press(cx, SAVE_QUERY);
        self.fill(cx, "search.save.name", name);
        self.fill(cx, "search.save.tags", tags);
        self.click(cx, "search.save.ok");
    }

    fn set_presets(&self, presets: &[(&str, Value)]) {
        *self.search.presets.lock().unwrap() = presets
            .iter()
            .map(|(name, value)| Preset {
                name: (*name).to_string(),
                value: value.clone(),
            })
            .collect();
    }

    fn saved_presets(&self) -> Vec<(String, Value)> {
        self.search.saved_presets.lock().unwrap().clone()
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
    assert!(f.has(cx, "search.run.readonly"), "Run shows the lock");

    f.run(cx);
    assert!(f.has(cx, "danger.input"), "the unlock dialog asks first");
    assert!(f.search.calls().is_empty());
}

#[gpui_kit::test]
async fn a_read_only_profile_may_run_a_plain_search(cx: &mut TestAppContext) {
    let mut profile = Profile::new("locked", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    assert!(!f.has(cx, "search.run.readonly"));
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

/// Two hits, a mark on the first, and facets with a value no filter can hold.
fn results_body() -> String {
    json!({
        "found": 25, "out_of": 100, "page": 1, "search_time_ms": 3,
        "hits": [
            {"document": {"id": "d1", "title": "Dune"},
             "highlight": {"title": {"snippet": "<mark>Du</mark>ne", "matched_tokens": ["Du"]}},
             "text_match": 578, "text_match_info": {"score": "578", "tokens_matched": 1}},
            {"document": {"id": "d2", "title": "Emma"}, "curated": true}
        ],
        "facet_counts": [{"field_name": "genre", "sampled": true, "counts": [
            {"value": "crime", "count": 3},
            {"value": "drama", "count": 2},
            {"value": "a`b", "count": 1}
        ]}]
    })
    .to_string()
}

impl Fixture {
    /// Runs a search answered with `body`.
    fn run_with(&self, cx: &mut TestAppContext, body: String) {
        *self.search.body.lock().unwrap() = body;
        self.run(cx);
    }

    fn detail_text(&self, cx: &mut TestAppContext) -> Option<String> {
        let workbench = self.workbench(cx);
        cx.update(|cx| {
            let hits = workbench.read(cx).hits().read(cx);
            Some(hits.detail()?.read(cx).text(cx))
        })
    }

    fn filter_text(&self, cx: &mut TestAppContext) -> String {
        let workbench = self.workbench(cx);
        cx.update(|cx| {
            let filter = workbench.read(cx).form().read(cx).filter().read(cx);
            filter.value(cx)
        })
    }

    fn last_call(&self) -> SearchCall {
        self.search.calls().last().unwrap().clone()
    }
}

#[gpui_kit::test]
async fn hits_show_marks_and_score(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run_with(cx, results_body());
    let row = f.text_of(cx, "search.hit.0");
    assert!(row.contains("d1"), "{row}");
    assert!(row.contains("Dune"), "{row}");
    assert!(!row.contains("mark"), "{row}");
    assert!(row.contains("578"), "{row}");
    let row = f.text_of(cx, "search.hit.1");
    assert!(row.contains("Curated"), "{row}");
}

#[gpui_kit::test]
async fn selecting_a_hit_shows_document_and_match_info(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run_with(cx, results_body());
    assert_eq!(f.detail_text(cx), None);

    f.click(cx, "search.hit.0");
    assert!(f.detail_text(cx).unwrap().contains("\"Dune\""));
    assert!(
        f.text_of(cx, "search.match_info")
            .contains("tokens_matched")
    );

    f.press(cx, "down");
    assert!(f.detail_text(cx).unwrap().contains("\"Emma\""));
    assert!(!f.has(cx, "search.match_info"));
    f.press(cx, "down");
    assert!(
        f.detail_text(cx).unwrap().contains("\"Emma\""),
        "stops at the end"
    );
    f.press(cx, "up");
    assert!(f.detail_text(cx).unwrap().contains("\"Dune\""));

    f.run(cx);
    assert_eq!(f.detail_text(cx), None, "a new result starts unselected");
}

#[gpui_kit::test]
async fn facet_click_writes_the_filter_and_runs(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.run_with(cx, results_body());
    assert!(f.has(cx, "search.facet.genre.sampled"));
    f.click(cx, "search.facet.genre.crime");
    assert_eq!(f.params(cx)["filter_by"], "genre:=`crime`");
    assert_eq!(f.filter_text(cx), "genre:=`crime`");
    assert_eq!(f.last_call().param("filter_by"), Some("genre:=`crime`"));
    assert_eq!(f.search.calls().len(), 2);
    assert_eq!(
        f.act(cx, |window, _| window
            .find("search.facet.genre.crime")
            .checked()),
        Some(true)
    );

    f.click(cx, "search.facet.genre.crime");
    assert!(
        !f.params(cx).contains_key("filter_by"),
        "the key goes when empty"
    );
    assert_eq!(f.filter_text(cx), "");
    assert_eq!(f.last_call().param("filter_by"), None);
}

#[gpui_kit::test]
async fn second_value_makes_a_list(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run_with(cx, results_body());
    f.click(cx, "search.facet.genre.crime");
    f.click(cx, "search.facet.genre.drama");
    assert_eq!(f.params(cx)["filter_by"], "genre:=[`crime`,`drama`]");
}

#[gpui_kit::test]
async fn a_facet_click_goes_back_to_the_first_page(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run_with(cx, results_body());
    f.click(cx, "search.page.next");
    assert_eq!(f.params(cx)["page"], 2);
    f.click(cx, "search.facet.genre.crime");
    assert!(!f.params(cx).contains_key("page"));
}

#[gpui_kit::test]
async fn a_facet_click_shows_in_json_mode(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run_with(cx, results_body());
    f.press(cx, SHOW_JSON);
    f.click(cx, "search.facet.genre.crime");
    let json: Value = serde_json::from_str(&f.json_text(cx)).unwrap();
    assert_eq!(json["filter_by"], "genre:=`crime`");
}

#[gpui_kit::test]
async fn unwritable_value_is_disabled(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run_with(cx, results_body());
    assert!(f.disabled(cx, "search.facet.genre.a`b"));
    assert!(f.has(cx, "search.facet.genre.a`b.note"));
    let calls = f.search.calls().len();
    f.click(cx, "search.facet.genre.a`b");
    assert!(!f.params(cx).contains_key("filter_by"));
    assert_eq!(f.search.calls().len(), calls);
}

#[gpui_kit::test]
async fn groups_draw_headings(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let body = json!({
        "found": 2, "found_docs": 9, "out_of": 100, "page": 1, "search_time_ms": 3,
        "grouped_hits": [
            {"group_key": ["crime"], "found": 7, "hits": [{"document": {"id": "a"}}]},
            {"group_key": ["drama"], "found": 2, "hits": [{"document": {"id": "b"}}]}
        ]
    });
    f.run_with(cx, body.to_string());
    let heading = f.text_of(cx, "search.group.0");
    assert!(
        heading.contains("crime") && heading.contains('7'),
        "{heading}"
    );
    assert!(f.text_of(cx, "search.hit.1").contains('a'));
    assert!(f.text_of(cx, "search.group.2").contains("drama"));
    assert!(f.text_of(cx, "search.status").contains('9'));

    // Headings are not selectable: Down steps over them.
    f.click(cx, "search.hit.1");
    f.press(cx, "down");
    assert!(f.detail_text(cx).unwrap().contains("\"b\""));
}

#[gpui_kit::test]
async fn next_page_writes_page_and_runs(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.run_with(cx, results_body());
    assert!(f.disabled(cx, "search.page.previous"));
    f.click(cx, "search.page.next");
    assert_eq!(f.params(cx)["page"], 2);
    assert_eq!(f.last_call().param("page"), Some("2"));
    assert_eq!(f.search.calls().len(), 2);
    let workbench = f.workbench(cx);
    let page = cx.update(|cx| workbench.read(cx).form().read(cx).input_text("page", cx));
    assert_eq!(page, "2");

    *f.search.body.lock().unwrap() = results_body().replace("\"page\":1", "\"page\":2");
    f.press(cx, NEXT_PAGE);
    assert_eq!(f.last_call().param("page"), Some("3"));
}

#[gpui_kit::test]
async fn next_is_disabled_on_the_last_page(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "*", "per_page": 25}"#);
    f.press(cx, SHOW_FORM);
    f.run_with(cx, results_body());
    assert!(f.disabled(cx, "search.page.next"));
    let calls = f.search.calls().len();
    f.press(cx, NEXT_PAGE);
    assert_eq!(f.search.calls().len(), calls);
}

#[gpui_kit::test]
async fn no_hits_says_what_was_searched(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.fill(cx, "search.param.q", "zzz");
    f.run_with(cx, search_body(0));
    let text = f.text_of(cx, "search.no_hits");
    assert!(text.contains("No documents match"), "{text}");
    assert!(text.contains("zzz"), "{text}");
}

/// Opens a draft of `collection` with `query_by` set.
fn open_query_by(f: &Fixture, cx: &mut TestAppContext, collection: &str, field: &str) {
    let mut draft = SearchDraft::new(collection);
    draft.params.insert("query_by".into(), field.into());
    let id = f.id;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_search(id, Some(draft), cx))
    });
    f.settle(cx);
}

#[gpui_kit::test]
async fn instant_waits_while_the_schema_is_unknown(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    open_query_by(&f, cx, "not_listed", "embedding");
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 300);
    assert!(f.search.calls().is_empty());
    assert!(f.text_of(cx, "search.instant.paused").contains("schema"));
    f.run(cx);
    assert_eq!(f.search.calls().len(), 1, "Run still runs");
}

#[gpui_kit::test]
async fn the_timer_rechecks_what_arrived_during_the_wait(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    open_query_by(&f, cx, "books", "title");
    f.fill(cx, "search.param.q", "dune");
    let embedded: Collection = serde_json::from_value(json!({"name": "books", "fields": [
        {"name": "title", "type": "float[]", "num_dim": 3,
         "embed": {"from": ["x"], "model_config": {"model_name": "openai/m"}}},
    ]}))
    .unwrap();
    *f.backend.data.lock().unwrap() = Some((vec![embedded], vec![]));
    let conn = cx
        .update(|cx| f.state.read(cx).connection(f.id, cx))
        .unwrap();
    cx.update(|cx| conn.update(cx, |c, cx| c.invalidate_collections(cx)));
    f.settle(cx);
    f.wait(cx, 300);
    assert!(f.search.calls().is_empty());
}

#[gpui_kit::test]
async fn bad_json_outlives_a_trip_through_the_form(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    f.set_json(cx, "{ typed");
    f.press(cx, SHOW_FORM);
    assert!(f.disabled(cx, "search.run"), "Run stays blocked");
    assert!(f.has(cx, "search.json.error"));
    f.press(cx, SHOW_JSON);
    assert_eq!(f.json_text(cx), "{ typed");
    assert!(f.disabled(cx, "search.run"));

    f.set_json(cx, r#"{"q": "ok"}"#);
    assert!(!f.disabled(cx, "search.run"));
    assert_eq!(f.params(cx)["q"], "ok");
}

#[gpui_kit::test]
async fn bad_json_disables_facets_and_paging(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.run_with(cx, results_body());
    f.click(cx, "search.page.next");
    f.press(cx, SHOW_JSON);
    f.set_json(cx, "{ typed");
    assert!(f.disabled(cx, "search.page.next"));
    assert!(f.disabled(cx, "search.page.previous"));
    assert!(f.disabled(cx, "search.facet.genre.crime"));
}

#[gpui_kit::test]
async fn an_edit_of_another_key_goes_back_to_the_first_page(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.page", "5");
    assert_eq!(f.params(cx)["page"], 5);
    f.fill(cx, "search.param.q", "narrow");
    assert!(!f.params(cx).contains_key("page"));
    let workbench = f.workbench(cx);
    let page = cx.update(|cx| workbench.read(cx).form().read(cx).input_text("page", cx));
    assert_eq!(page, "");
}

#[gpui_kit::test]
async fn a_page_past_the_end_is_not_a_miss_and_previous_recovers(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "*", "page": 5}"#);
    f.press(cx, SHOW_FORM);
    f.run_with(cx, search_body(3));
    assert!(!f.has(cx, "search.no_hits"));
    assert!(f.text_of(cx, "search.past_end").contains('3'));
    assert!(!f.disabled(cx, "search.page.previous"));
    f.click(cx, "search.page.previous");
    assert_eq!(f.params(cx)["page"], 1);
}

#[gpui_kit::test]
async fn raw_is_filled_only_when_it_shows(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.run(cx);
    let workbench = f.workbench(cx);
    let raw = cx.update(|cx| workbench.read(cx).raw().clone());
    assert_eq!(cx.update(|cx| raw.read(cx).shown()), Shown::Empty);
    f.press(cx, SHOW_RAW);
    assert_eq!(cx.update(|cx| raw.read(cx).shown()), Shown::Highlighted);
    *f.search.body.lock().unwrap() = search_body(40);
    f.run(cx);
    let text = cx.update(|cx| raw.read(cx).text(cx)).unwrap();
    assert!(
        text.contains("\"found\": 40"),
        "a shown tab follows the answers"
    );
}

#[gpui_kit::test]
async fn load_drops_the_running_read(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    let held = f.search.hold_next();
    f.run(cx);
    open_query_by(&f, cx, "books", "title");
    assert!(held.is_closed());
    assert_eq!(f.result(cx), None);
}

#[gpui_kit::test]
async fn removing_the_last_chip_keeps_the_keyboard_in_the_workbench(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "*", "query_by": "title"}"#);
    f.press(cx, SHOW_FORM);
    let workbench = f.workbench(cx);
    let form = cx.update(|cx| workbench.read(cx).form().clone());
    f.act(cx, |window, cx| {
        form.update(cx, |form, cx| form.focus_chip(0, window, cx))
    });
    f.press(cx, "delete");
    assert!(!f.params(cx).contains_key("query_by"));
    f.press(cx, SHOW_JSON);
    assert!(f.has(cx, "json.editor"), "workbench shortcuts still arrive");
}

impl Fixture {
    /// Two searches of `books`, the second with `q` set, and no instant runs.
    fn two_searches(&self, cx: &mut TestAppContext) {
        self.instant_off(cx);
        self.press(cx, ADD_SEARCH);
        self.fill(cx, "search.param.q", "dune");
    }

    fn load(&self, cx: &mut TestAppContext, drafts: Vec<SearchDraft>, union: bool) {
        let workbench = self.workbench(cx);
        self.act(cx, |window, cx| {
            workbench.update(cx, |w, cx| w.load(drafts, union, window, cx));
        });
        self.settle(cx);
    }

    fn drafts(&self, cx: &mut TestAppContext) -> Vec<SearchDraft> {
        let workbench = self.workbench(cx);
        cx.update(|cx| workbench.read(cx).drafts().to_vec())
    }

    fn active(&self, cx: &mut TestAppContext) -> usize {
        let workbench = self.workbench(cx);
        cx.update(|cx| workbench.read(cx).active())
    }
}

fn federated_body() -> String {
    format!(
        r#"{{"results":[{},{{"code":404,"error":"Not found: `nope`"}}]}}"#,
        search_body(7)
    )
}

fn draft_with(params: Value) -> SearchDraft {
    let mut draft = SearchDraft::new("books");
    draft.params.extend(params.as_object().unwrap().clone());
    draft
}

#[gpui_kit::test]
async fn add_copies_the_draft_up_to_50(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.fill(cx, "search.param.q", "dune");
    assert!(!f.has(cx, "search.add.limit"));
    for _ in 0..49 {
        f.press(cx, ADD_SEARCH);
    }
    let drafts = f.drafts(cx);
    assert_eq!(drafts.len(), 50);
    assert!(drafts.iter().all(|d| d.params["q"] == "dune"));
    assert_eq!(f.active(cx), 49);
    assert!(f.disabled(cx, "search.add"));
    assert!(f.has(cx, "search.add.limit"));
    f.press(cx, ADD_SEARCH);
    assert_eq!(f.drafts(cx).len(), 50);
}

#[gpui_kit::test]
async fn one_search_uses_search_two_use_multi(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.run(cx);
    assert_eq!(
        (f.search.calls().len(), f.search.multi_calls().len()),
        (1, 0)
    );
    f.press(cx, ADD_SEARCH);
    f.run(cx);
    assert_eq!(
        (f.search.calls().len(), f.search.multi_calls().len()),
        (1, 1)
    );
    let request = f.search.multi_calls().remove(0);
    assert!(!request.union);
    assert_eq!(request.searches.len(), 2);
    assert_eq!(request.searches[1]["collection"], "books");
    assert_eq!(request.searches[1]["enable_analytics"], json!(false));
    f.press(cx, REMOVE_SEARCH);
    f.run(cx);
    assert_eq!(
        (f.search.calls().len(), f.search.multi_calls().len()),
        (2, 1)
    );
    assert!(f.disabled(cx, "search.remove"));
}

#[gpui_kit::test]
async fn federated_error_shows_on_its_tab_only(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    *f.search.multi_body.lock().unwrap() = Some(federated_body());
    f.run(cx);
    // Add moved to the second search, whose answer is the failure.
    let text = f.text_of(cx, "search.item_error");
    assert!(
        text.contains("404") && text.contains("Not found: `nope`"),
        "{text}"
    );
    assert!(!f.has(cx, "search.status"));
    assert!(!f.has(cx, "search.error"));
    f.click(cx, "search.result_tab.0");
    assert!(!f.has(cx, "search.item_error"));
    assert!(f.text_of(cx, "search.status").contains('7'));
    assert_eq!(f.result(cx).unwrap().found, 7);
}

#[gpui_kit::test]
async fn union_sends_page_in_the_request_not_the_searches(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.load(
        cx,
        vec![
            draft_with(json!({"page": 3, "per_page": 5})),
            draft_with(json!({"page": 9, "per_page": 7})),
        ],
        true,
    );
    f.run(cx);
    let request = f.search.multi_calls().remove(0);
    assert!(request.union);
    assert_eq!((request.page, request.per_page), (Some(3), Some(5)));
    for search in &request.searches {
        assert!(!search.contains_key("page") && !search.contains_key("per_page"));
    }
}

#[gpui_kit::test]
async fn union_locks_paging_on_the_other_tabs(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    // An input exposes no disabled flag to tests; the note comes from the same condition.
    assert!(!f.has(cx, "search.param.page.union"));
    f.press(cx, TOGGLE_UNION);
    assert!(f.has(cx, "search.param.page.union"));
    assert!(f.has(cx, "search.param.per_page.union"));
    f.press(cx, PREVIOUS_SEARCH);
    assert!(!f.has(cx, "search.param.page.union"));
}

#[gpui_kit::test]
async fn union_status_says_before_duplicates(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    f.press(cx, TOGGLE_UNION);
    *f.search.multi_body.lock().unwrap() = Some(
        r#"{"found":2,"out_of":9,"page":1,"search_time_ms":1,"facet_counts":[
            {"field_name":"genre","counts":[{"value":"crime","count":1}]}],"hits":[
            {"document":{"id":"1"},"collection":"books","search_index":0}]}"#
            .to_string(),
    );
    f.run(cx);
    assert!(f.text_of(cx, "search.status").contains("before duplicates"));
    assert!(f.text_of(cx, "search.hit.0").contains("books"));
    assert!(!f.has(cx, "search.facets"));
    assert!(!f.has(cx, "search.result_tab.0"));
}

#[gpui_kit::test]
async fn union_needs_two_searches(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    assert!(f.disabled(cx, "search.union"));
    f.press(cx, TOGGLE_UNION);
    let workbench = f.workbench(cx);
    assert!(!cx.update(|cx| workbench.read(cx).union()));
    f.press(cx, ADD_SEARCH);
    assert!(!f.disabled(cx, "search.union"));
    f.press(cx, TOGGLE_UNION);
    assert!(cx.update(|cx| workbench.read(cx).union()));
    f.press(cx, REMOVE_SEARCH);
    assert!(
        !cx.update(|cx| workbench.read(cx).union()),
        "one search left"
    );
}

#[gpui_kit::test]
async fn union_error_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    f.press(cx, TOGGLE_UNION);
    *f.search.fail.lock().unwrap() = Some((400, "sort_by must match".to_string()));
    f.run(cx);
    assert!(f.text_of(cx, "search.error").contains("sort_by must match"));
}

#[gpui_kit::test]
async fn bad_key_on_second_tab_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    let bad = draft_with(json!({"Bad-Key": 1}));
    f.load(cx, vec![SearchDraft::new("books"), bad], false);
    f.run(cx);
    assert!(f.search.multi_calls().is_empty() && f.search.calls().is_empty());
    assert!(f.text_of(cx, "search.tab.1.error").contains("Bad-Key"));
    assert!(!f.has(cx, "search.tab.0.error"));
}

#[gpui_kit::test]
async fn instant_runs_all_searches_after_a_change(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, ADD_SEARCH);
    f.wait(cx, 200);
    let calls = f.search.multi_calls().len();
    assert_eq!(calls, 1, "adding a search is a change");
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 200);
    let requests = f.search.multi_calls();
    assert_eq!(requests.len(), calls + 1);
    assert_eq!(requests[calls].searches[1]["q"], "dune");
}

#[gpui_kit::test]
async fn a_conversation_on_another_tab_pauses_instant(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.load(
        cx,
        vec![
            draft_with(json!({"conversation": true})),
            SearchDraft::new("books"),
        ],
        false,
    );
    f.press(cx, NEXT_SEARCH);
    assert!(f.has(cx, "search.instant.paused"));
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 300);
    assert!(f.search.multi_calls().is_empty());
}

#[gpui_kit::test]
async fn tabs_switch_the_form_and_json_text_stays(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    f.press(cx, PREVIOUS_SEARCH);
    assert_eq!(f.active(cx), 0);
    let workbench = f.workbench(cx);
    let q = |cx: &mut TestAppContext| {
        cx.update(|cx| workbench.read(cx).form().read(cx).input_text("q", cx))
    };
    assert_eq!(q(cx), "*");
    f.press(cx, NEXT_SEARCH);
    assert_eq!(q(cx), "dune");
    f.press(cx, SHOW_JSON);
    f.set_json(cx, "{bad");
    f.press(cx, PREVIOUS_SEARCH);
    assert_eq!(f.active(cx), 1, "the text would land on the other search");
    assert!(f.disabled(cx, "search.remove"));
    assert_eq!(f.drafts(cx).len(), 2);
}

#[gpui_kit::test]
async fn facets_act_on_the_result_tab_shown(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    let facets = r#"{"found":1,"out_of":9,"page":1,"search_time_ms":1,"hits":[],"facet_counts":[
        {"field_name":"genre","counts":[{"value":"crime","count":1}]}]}"#;
    *f.search.multi_body.lock().unwrap() = Some(format!(r#"{{"results":[{facets},{facets}]}}"#));
    f.run(cx);
    f.click(cx, "search.result_tab.0");
    assert_eq!(f.active(cx), 1);
    f.click(cx, "search.facet.genre.crime");
    let drafts = f.drafts(cx);
    assert_eq!(drafts[0].params["filter_by"], "genre:=`crime`");
    assert!(!drafts[1].params.contains_key("filter_by"));
    assert_eq!(f.search.multi_calls().len(), 2);
}

#[gpui_kit::test]
async fn removing_a_search_drops_its_answer_and_the_running_read(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.two_searches(cx);
    *f.search.multi_body.lock().unwrap() = Some(federated_body());
    f.run(cx);
    f.press(cx, REMOVE_SEARCH);
    assert!(!f.has(cx, "search.item_error"));
    assert_eq!(f.result(cx).unwrap().found, 7);
    assert!(!f.has(cx, "search.result_tab.0"));
    f.press(cx, ADD_SEARCH);
    let held = f.search.hold_next();
    f.run(cx);
    f.press(cx, REMOVE_SEARCH);
    assert!(held.is_closed());
}

#[gpui_kit::test]
async fn instant_run_records_only_after_settling(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 150);
    assert_eq!(f.search.calls().last().unwrap().param("q"), Some("dune"));
    f.wait(cx, 1999);
    assert!(f.history(cx).is_empty());
    f.wait(cx, 1);
    assert_eq!(f.history(cx), [r#""dune""#]);

    // A newer run replaces the wait, so what was only passing through stays out.
    f.fill(cx, "search.param.q", "arrakis");
    f.wait(cx, 150);
    f.wait(cx, 1000);
    f.fill(cx, "search.param.q", "caladan");
    f.wait(cx, 150);
    f.wait(cx, 1999);
    assert_eq!(f.history(cx).len(), 1);
    f.wait(cx, 1);
    assert_eq!(f.history(cx), [r#""caladan""#, r#""dune""#]);
}

#[gpui_kit::test]
async fn explicit_run_records_at_once(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.fill(cx, "search.param.q", "dune");
    f.run(cx);
    assert_eq!(f.history(cx), [r#""dune""#]);
}

#[gpui_kit::test]
async fn a_slow_instant_answer_records_when_it_arrives(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let held = f.search.hold_next();
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 150);
    f.wait(cx, 2500);
    assert!(f.history(cx).is_empty());
    held.send(()).unwrap();
    f.wait(cx, 1);
    assert_eq!(f.history(cx), [r#""dune""#]);
}

#[gpui_kit::test]
async fn failed_run_records_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    *f.search.fail.lock().unwrap() = Some((400, "no".into()));
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 150);
    f.wait(cx, 3000);
    f.run(cx);
    f.wait(cx, 3000);
    assert!(f.has(cx, "search.error"));
    assert!(f.history(cx).is_empty());
}

#[gpui_kit::test]
async fn history_never_holds_a_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    f.set_json(
        cx,
        r#"{"q": "dune", "api_key": "topsecret", "scoped_key": "topsecret"}"#,
    );
    f.run(cx);
    assert_eq!(f.history(cx), [r#""dune""#]);
    let line = cx.update(|cx| format!("{:?}", f.state.read(cx).history));
    assert!(!line.contains("topsecret"), "{line}");
}

#[gpui_kit::test]
async fn history_load_runs_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.fill(cx, "search.param.q", "dune");
    f.run(cx);
    f.fill(cx, "search.param.q", "other");
    f.wait(cx, 150);
    let calls = f.search.calls().len();

    f.press(cx, OPEN_HISTORY);
    assert!(f.text_of(cx, "search.history.row.0").contains("dune"));
    f.press(cx, "enter");
    f.wait(cx, 1000);
    assert!(!f.has(cx, "search.history.row.0"));
    assert_eq!(f.params(cx)["q"], "dune");
    assert_eq!(f.search.calls().len(), calls);
}

#[gpui_kit::test]
async fn escape_closes_a_list_and_the_arrows_pick_a_row(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    for q in ["one", "two"] {
        f.fill(cx, "search.param.q", q);
        f.run(cx);
    }
    f.press(cx, OPEN_HISTORY);
    f.press(cx, "escape");
    assert!(!f.has(cx, "search.history.row.0"));

    f.press(cx, OPEN_HISTORY);
    f.press(cx, "down");
    f.press(cx, "enter");
    assert_eq!(f.params(cx)["q"], "one");
}

#[gpui_kit::test]
async fn clear_history_asks(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.run(cx);
    f.press(cx, OPEN_HISTORY);
    f.click(cx, "search.history.clear");
    assert!(f.has(cx, "confirm.body"));
    f.click(cx, "confirm.cancel");
    assert_eq!(f.history(cx).len(), 1);

    f.press(cx, OPEN_HISTORY);
    f.click(cx, "search.history.clear");
    f.click(cx, "confirm.ok");
    assert!(f.history(cx).is_empty());
}

#[gpui_kit::test]
async fn saved_filters_by_collection_and_tag(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.add_saved(cx, "alpha", "books", &["fiction"]);
    f.add_saved(cx, "beta", "books", &["science"]);
    f.add_saved(cx, "gamma", "films", &[]);

    f.press(cx, OPEN_SAVED);
    assert!(f.text_of(cx, "search.saved.row.0").contains("fiction"));
    assert!(f.has(cx, "search.saved.row.1"));
    assert!(!f.has(cx, "search.saved.row.2"));

    f.click(cx, "search.saved.all");
    assert!(f.text_of(cx, "search.saved.row.2").contains("gamma"));

    f.fill(cx, "search.queries.filter", "SCIEN");
    assert!(f.text_of(cx, "search.saved.row.0").contains("beta"));
    assert!(!f.has(cx, "search.saved.row.1"));
}

#[gpui_kit::test]
async fn saved_load_replaces_the_drafts_and_runs_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.add_saved(cx, "alpha", "books", &[]);
    f.fill(cx, "search.param.q", "dune");
    f.wait(cx, 150);
    let calls = f.search.calls().len();
    f.press(cx, OPEN_SAVED);
    f.press(cx, "enter");
    f.wait(cx, 1000);
    assert_eq!(f.params(cx)["q"], "*");
    assert_eq!(f.search.calls().len(), calls);
}

#[gpui_kit::test]
async fn delete_saved_asks(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.add_saved(cx, "alpha", "books", &[]);
    f.press(cx, OPEN_SAVED);
    f.press(cx, DELETE);
    assert!(f.has(cx, "confirm.body"));
    f.click(cx, "confirm.cancel");
    assert_eq!(f.saved(cx).len(), 1);

    f.press(cx, OPEN_SAVED);
    f.click(cx, "search.saved.delete");
    f.click(cx, "confirm.ok");
    assert!(f.saved(cx).is_empty());
}

#[gpui_kit::test]
async fn save_trims_the_name_and_splits_the_tags(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.save_as(cx, "  mine ", " a, ,b ,");
    let saved = f.saved(cx);
    assert_eq!(saved[0].name, "mine");
    assert_eq!(saved[0].tags, ["a", "b"]);
    assert_eq!(saved[0].profile, f.id);
}

#[gpui_kit::test]
async fn save_needs_a_name_and_keeps_no_key(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, SHOW_JSON);
    f.set_json(cx, r#"{"q": "dune", "api_key": "topsecret"}"#);
    f.press(cx, SAVE_QUERY);
    f.click(cx, "search.save.ok");
    assert!(f.has(cx, "search.save.error"));
    f.fill(cx, "search.save.name", "mine");
    f.click(cx, "search.save.ok");
    let saved = f.saved(cx);
    assert!(!saved[0].searches[0].params.contains_key("api_key"));
}

#[gpui_kit::test]
async fn save_replaces_after_asking(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.save_as(cx, "mine", "");
    let id = f.saved(cx)[0].id;
    f.fill(cx, "search.param.q", "dune");

    f.save_as(cx, "mine", "");
    assert!(f.has(cx, "confirm.body"));
    f.click(cx, "confirm.cancel");
    assert_eq!(f.saved(cx)[0].searches[0].params["q"], "*");

    f.save_as(cx, "mine", "");
    f.click(cx, "confirm.ok");
    let saved = f.saved(cx);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, id);
    assert_eq!(saved[0].searches[0].params["q"], "dune");
}

#[gpui_kit::test]
async fn saved_error_shows_in_the_list_and_disables_save(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    cx.update(|cx| {
        f.state
            .update(cx, |s, _| s.saved_error = Some("the file is bad".into()));
    });
    f.settle(cx);
    assert!(f.disabled(cx, "search.save"));
    f.press(cx, SAVE_QUERY);
    assert!(!f.has(cx, "search.save.name"));
    f.press(cx, OPEN_SAVED);
    assert_eq!(f.text_of(cx, "search.saved.error"), "the file is bad");
}

#[gpui_kit::test]
async fn preset_with_searches_makes_drafts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.set_presets(&[(
        "two",
        json!({"searches": [
            {"collection": "books", "q": "a", "per_page": 3},
            {"collection": "films", "q": "b"},
        ]}),
    )]);
    f.press(cx, OPEN_PRESETS);
    f.settle(cx);
    f.press(cx, "enter");
    let drafts = f.drafts(cx);
    assert_eq!(drafts.len(), 2);
    assert_eq!(drafts[0].collection, "books");
    assert_eq!(drafts[0].params["per_page"], 3);
    assert!(!drafts[0].params.contains_key("collection"));
    assert_eq!(drafts[1].collection, "films");
    assert!(f.search.calls().is_empty());
}

#[gpui_kit::test]
async fn preset_of_parameters_stays_on_the_current_collection(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.set_presets(&[("one", json!({"q": "a"}))]);
    f.press(cx, OPEN_PRESETS);
    f.click(cx, "search.preset.row.0");
    let drafts = f.drafts(cx);
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].collection, "books");
    assert_eq!(drafts[0].params["q"], "a");
}

#[gpui_kit::test]
async fn preset_not_an_object_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.set_presets(&[
        ("text", json!("nope")),
        ("bad", json!({"searches": [{"Bad-Key": 1}]})),
    ]);
    f.press(cx, OPEN_PRESETS);
    f.press(cx, "enter");
    assert!(f.has(cx, "search.preset.error"));
    assert!(f.text_of(cx, "search.preset.error").contains("object"));
    f.press(cx, "down");
    f.press(cx, "enter");
    assert!(f.text_of(cx, "search.preset.error").contains("Bad-Key"));
    assert_eq!(f.drafts(cx).len(), 1);
    assert_eq!(f.params(cx)["q"], "*");
}

#[gpui_kit::test]
async fn presets_error_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    *f.search.fail.lock().unwrap() = Some((401, "no way".into()));
    f.press(cx, OPEN_PRESETS);
    assert!(f.text_of(cx, "search.preset.error").contains("no way"));
    assert!(f.disabled(cx, "search.preset.save"));
}

#[gpui_kit::test]
async fn preset_save_sends_the_search_without_its_collection(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.fill(cx, "search.param.q", "dune");
    f.press(cx, OPEN_PRESETS);
    f.click(cx, "search.preset.save");
    f.fill(cx, "search.save.name", " mine ");
    f.click(cx, "search.save.ok");
    let saved = f.saved_presets();
    assert_eq!(saved[0].0, "mine");
    assert_eq!(saved[0].1["q"], "dune");
    assert!(saved[0].1.get("collection").is_none());

    f.press(cx, ADD_SEARCH);
    f.press(cx, OPEN_PRESETS);
    f.click(cx, "search.preset.save");
    f.fill(cx, "search.save.name", "two");
    f.click(cx, "search.save.ok");
    let searches = f.saved_presets()[1].1["searches"].clone();
    assert_eq!(searches[1]["collection"], "books");
}

#[gpui_kit::test]
async fn preset_save_failure_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, OPEN_PRESETS);
    f.click(cx, "search.preset.save");
    *f.search.fail.lock().unwrap() = Some((400, "refused".into()));
    f.fill(cx, "search.save.name", "mine");
    f.click(cx, "search.save.ok");
    assert!(f.text_of(cx, "search.queries.error").contains("refused"));
}

#[gpui_kit::test]
async fn preset_save_on_locked_profile_unlocks_first(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    f.press(cx, OPEN_PRESETS);
    assert!(f.has(cx, "search.preset.save.readonly"));
    f.click(cx, "search.preset.save");
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    assert!(!f.has(cx, "search.save.name"));
    f.act(cx, |window, cx| {
        window.click("danger.input".to_string(), cx);
        window.input("test", cx);
        window.render_frame(cx);
        window.click("danger.ok".to_string(), cx);
    });
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    f.settle(cx);
    f.fill(cx, "search.save.name", "mine");
    f.click(cx, "search.save.ok");
    assert_eq!(f.saved_presets().len(), 1);
}

#[gpui_kit::test]
async fn preset_replace_cancel_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.set_presets(&[("mine", json!({"q": "old"}))]);
    f.press(cx, OPEN_PRESETS);
    f.click(cx, "search.preset.save");
    f.fill(cx, "search.save.name", "mine");
    f.click(cx, "search.save.ok");
    assert!(f.has(cx, "confirm.body"));
    f.click(cx, "confirm.cancel");
    assert!(f.saved_presets().is_empty());

    f.press(cx, OPEN_PRESETS);
    f.click(cx, "search.preset.save");
    f.fill(cx, "search.save.name", "mine");
    f.click(cx, "search.save.ok");
    f.click(cx, "confirm.ok");
    assert_eq!(f.saved_presets().len(), 1);
}

#[gpui_kit::test]
async fn export_menu_copies_with_the_key_placeholder(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.fill(cx, "search.param.q", "dune");

    for (entry, expected) in [
        (0, "$TYPESENSE_API_KEY"),
        (1, "process.env.TYPESENSE_API_KEY"),
        (2, r#"os.environ["TYPESENSE_API_KEY"]"#),
        (3, r#"std::env::var("TYPESENSE_API_KEY")?"#),
    ] {
        f.press(cx, OPEN_EXPORT);
        f.act(cx, |window, cx| {
            window.within("popup-menu").click(entry, cx)
        });
        f.settle(cx);
        let code = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
        assert!(code.contains(expected), "{code}");
        assert!(
            code.contains("q=dune") || code.contains(r#""q": "dune""#),
            "{code}"
        );
        // The fake's key is `k`.
        assert!(!code.contains(": k\""), "{code}");
    }
    assert_eq!(f.text_of(cx, "search.export.copied"), "Copied");
    f.wait(cx, 2000);
    assert!(!f.has(cx, "search.export.copied"));

    f.press(cx, OPEN_EXPORT);
    f.act(cx, |window, cx| window.within("popup-menu").click(4, cx));
    f.settle(cx);
    let url = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
    assert!(
        url.starts_with("http://localhost:8108/collections/books/"),
        "{url}"
    );
    assert!(!url.to_lowercase().contains("api-key"), "{url}");
}

#[gpui_kit::test]
async fn copy_curl_needs_no_menu(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.press(cx, COPY_DOC_CURL);
    let code = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
    assert!(code.starts_with("curl "), "{code}");
    assert!(f.has(cx, "search.export.copied"));
}

#[gpui_kit::test]
async fn a_post_route_disables_the_raw_url(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, ADD_SEARCH);
    f.press(cx, OPEN_EXPORT);
    let label = f.act(cx, |window, cx| {
        let mut menu = window.within("popup-menu");
        let label = menu.find(4).label().map(|l| l.to_string());
        menu.click(0, cx);
        label
    });
    assert_eq!(label.unwrap(), "Copy the URL: this search is sent as POST");
}

#[gpui_kit::test]
async fn a_change_on_another_tab_sends_a_union_to_the_first_page(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.load(
        cx,
        vec![draft_with(json!({"page": 3})), draft_with(json!({}))],
        true,
    );
    f.press(cx, NEXT_SEARCH);
    assert_eq!(f.active(cx), 1);
    f.fill(cx, "search.param.q", "dune");
    assert!(!f.drafts(cx)[0].params.contains_key("page"));
    f.run(cx);
    assert_eq!(f.search.multi_calls().remove(0).page, None);
}

#[gpui_kit::test]
async fn a_union_page_that_is_no_number_shows_an_error(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.load(
        cx,
        vec![draft_with(json!({"page": "two"})), draft_with(json!({}))],
        true,
    );
    f.run(cx);
    assert!(f.text_of(cx, "search.error").contains("`page`"));
    assert!(f.search.multi_calls().is_empty());
}

#[gpui_kit::test]
async fn a_union_ends_at_a_short_page_whatever_found_says(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.load(
        cx,
        vec![
            draft_with(json!({"page": 2, "per_page": 2})),
            draft_with(json!({})),
        ],
        true,
    );
    *f.search.multi_body.lock().unwrap() = Some(
        r#"{"found":9,"out_of":9,"page":2,"search_time_ms":1,"hits":[
            {"document":{"id":"1"},"collection":"books","search_index":0}]}"#
            .to_string(),
    );
    f.run(cx);
    assert!(f.text_of(cx, "search.status").contains("approximate"));
    assert!(f.disabled(cx, "search.page.next"));
    assert!(!f.disabled(cx, "search.page.previous"));

    *f.search.multi_body.lock().unwrap() = Some(search_body(9));
    f.run(cx);
    assert!(f.has(cx, "search.end"));
    assert!(!f.has(cx, "search.past_end"));
}

#[gpui_kit::test]
async fn an_export_that_cannot_run_says_why(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.instant_off(cx);
    f.press(cx, SHOW_JSON);
    f.set_json(cx, "{not json");
    f.press(cx, COPY_DOC_CURL);
    assert!(f.has(cx, "search.export.error"));
    assert!(cx.read_from_clipboard().is_none());
    f.set_json(cx, r#"{"q": "dune"}"#);
    f.press(cx, COPY_DOC_CURL);
    assert!(!f.has(cx, "search.export.error"));
    assert!(f.has(cx, "search.export.copied"));
}
