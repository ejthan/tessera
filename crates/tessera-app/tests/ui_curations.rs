// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the curation sets view and its item dialog.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::DateTime;
use gpui_kit::component::date_picker::DateTime as PickedTime;
use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, Focusable as _, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::state::ResourceKind;
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::curation_dialog::CurationItemDialog;
use tessera_app::views::resources::sets::SetsView;
use tessera_app::views::root::RootView;
use tessera_client::{Collection, ItemSet};
use tessera_core::profile::Profile;
use tessera_core::relevance::curations::CurationDraft;

use common::*;

fn collection(name: &str, attached: &[&str]) -> Collection {
    serde_json::from_value(json!({
        "name": name,
        "fields": [
            {"name": "title", "type": "string"},
            {"name": "price", "type": "int32", "sort": true},
            {"name": "embedding", "type": "float[]", "num_dim": 3}
        ],
        "curation_sets": attached,
    }))
    .unwrap()
}

fn rule() -> Value {
    json!({"id": "a", "rule": {"query": "chair", "match": "exact"}, "excludes": [{"id": "9"}]})
}

struct Fixture {
    handle: WindowHandle<Root>,
    search: Arc<FakeSearch>,
    relevance: Arc<FakeRelevance>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn open(
        cx: &mut TestAppContext,
        profile: Profile,
        version: &'static str,
        items: Vec<Value>,
    ) -> Self {
        let collections = vec![collection("other", &[]), collection("shop", &["c"])];
        let data = fake_backend(cx, Some((collections, vec![])));
        fake_documents(cx);
        let search = install_search_fake(cx);
        let relevance = fake_relevance(cx);
        relevance.sets.lock().unwrap().push(ItemSet {
            name: "c".into(),
            items,
        });
        *relevance.data.lock().unwrap() = Some(data);
        let id = profile.id;
        fake_connector(cx, move |_| Ok(server(version, true)));
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
        cx.update(|cx| state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Curations, cx)));
        wait_for_element(cx, handle, "curations.new".into(), true).await;
        let f = Self {
            handle,
            search,
            relevance,
            _rt: rt,
        };
        f.settle(cx);
        f.click(cx, "curations.row.c");
        f
    }

    async fn new(cx: &mut TestAppContext, items: Vec<Value>) -> Self {
        Self::open(cx, Profile::new("test", vec![node()]), "30.2", items).await
    }

    /// The dialog on a new item.
    async fn adding(cx: &mut TestAppContext, items: Vec<Value>) -> Self {
        let f = Self::new(cx, items).await;
        f.click(cx, "sets.items.add");
        f
    }

    /// The dialog on stored `item`, whose id is `a`.
    async fn editing(cx: &mut TestAppContext, item: Value) -> Self {
        let f = Self::new(cx, vec![item]).await;
        f.click(cx, "curations.items.row.a");
        f.click(cx, "sets.items.edit");
        f
    }

    fn settle(&self, cx: &mut TestAppContext) {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            window.try_find(id.to_string()).is_some()
        })
        .unwrap()
    }

    fn label(&self, cx: &mut TestAppContext, id: &str) -> String {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            let found = window.find(id.to_string());
            found.label().unwrap_or_default().to_string()
        })
        .unwrap()
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.click(id.to_string(), cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            fill(window, id, text, cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| window.press(keys, cx))
            .unwrap();
        self.settle(cx);
    }

    fn view(&self, cx: &mut TestAppContext) -> Entity<SetsView> {
        let panel = self
            .handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<ResourcePanel>().ok())
                    .next()
                    .unwrap()
            })
            .unwrap();
        cx.update(|cx| {
            let ResourceView::Sets(view) = panel.read(cx).view() else {
                panic!("not the sets view");
            };
            view.clone()
        })
    }

    fn dialog(&self, cx: &mut TestAppContext) -> Entity<CurationItemDialog> {
        let view = self.view(cx);
        cx.update(|cx| view.read(cx).curation_dialog().unwrap().clone())
    }

    fn draft(&self, cx: &mut TestAppContext) -> CurationDraft {
        self.settle(cx);
        let dialog = self.dialog(cx);
        cx.update(|cx| dialog.read(cx).draft(cx).unwrap())
    }

    fn set_json(&self, cx: &mut TestAppContext, text: &str) {
        let dialog = self.dialog(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            let json = dialog.read(cx).json().clone();
            json.update(cx, |e, cx| e.set_text(text, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn json(&self, cx: &mut TestAppContext) -> String {
        let dialog = self.dialog(cx);
        cx.update(|cx| dialog.read(cx).json().read(cx).text(cx))
    }

    /// Whether Save would do anything now, by what it does on a click.
    fn save_blocked(&self, cx: &mut TestAppContext) -> bool {
        self.settle(cx);
        let dialog = self.dialog(cx);
        cx.update(|cx| dialog.read(cx).save_blocked(cx))
    }

    /// Fills the fields a valid new item needs.
    fn fill_valid(&self, cx: &mut TestAppContext) {
        self.fill(cx, "curations.id", "n");
        self.fill(cx, "curations.query", "chair");
    }

    fn ids(&self, cx: &mut TestAppContext) -> Vec<(String, i64)> {
        let includes = self.draft(cx).includes.into_iter();
        includes.map(|i| (i.id, i.position)).collect()
    }

    /// Adds documents by the manual input.
    fn pin(&self, cx: &mut TestAppContext, ids: &[&str]) {
        for id in ids {
            self.fill(cx, "curations.picker.manual", id);
            self.click(cx, "curations.picker.add");
        }
    }

    /// The debounce of the picker passes.
    fn wait_for_picker(&self, cx: &mut TestAppContext) {
        self.settle(cx);
        cx.executor().advance_clock(Duration::from_millis(200));
        self.settle(cx);
    }

    fn sent(&self) -> Vec<Value> {
        self.relevance.sent_items.lock().unwrap().clone()
    }

    fn calls(&self) -> Vec<String> {
        self.relevance.calls.lock().unwrap().clone()
    }
}

fn key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

fn form_key() -> &'static str {
    key("cmd-1", "ctrl-1")
}

fn json_key() -> &'static str {
    key("cmd-2", "ctrl-2")
}

fn save_key() -> &'static str {
    key("cmd-enter", "ctrl-enter")
}

fn stamp(text: &str) -> i64 {
    DateTime::parse_from_rfc3339(text).unwrap().timestamp()
}

#[gpui_kit::test]
async fn items_table_shows_trigger_counts_and_state(cx: &mut TestAppContext) {
    let items = vec![
        json!({"id": "a", "rule": {"query": "chair", "match": "contains"},
               "includes": [{"id": "1", "position": 1}], "stop_processing": false}),
        json!({"id": "b", "rule": {"tags": ["vip", "new"]}, "excludes": [{"id": "9"}],
               "effective_to_ts": 1}),
        json!({"id": "c", "rule": {"tags": ["x"]}, "excludes": [{"id": "9"}],
               "effective_from_ts": 99_999_999_999_i64}),
    ];
    let f = Fixture::new(cx, items).await;
    assert_eq!(f.label(cx, "curations.items.trigger.a"), "chair (contains)");
    assert_eq!(f.label(cx, "curations.items.includes.a"), "1");
    assert_eq!(f.label(cx, "curations.items.excludes.a"), "0");
    assert_eq!(f.label(cx, "curations.items.active.a"), "Active");
    assert_eq!(f.label(cx, "curations.items.stops.a"), "No");
    assert_eq!(f.label(cx, "curations.items.trigger.b"), "vip, new");
    assert_eq!(f.label(cx, "curations.items.active.b"), "Expired");
    assert_eq!(f.label(cx, "curations.items.active.c"), "Scheduled");
    assert_eq!(f.label(cx, "curations.items.stops.c"), "Yes");
}

#[gpui_kit::test]
async fn form_to_json_keeps_extra(cx: &mut TestAppContext) {
    let mut item = rule();
    item["diversity"] = json!({"similarity_equation": "1.0"});
    item["rule"]["later"] = json!(7);
    item["metadata"] = json!({"banner": "x"});
    let f = Fixture::editing(cx, item).await;
    f.fill(cx, "curations.replace_query", "seat");
    f.press(cx, json_key());
    let value: Value = serde_json::from_str(&f.json(cx)).unwrap();
    assert_eq!(value["diversity"]["similarity_equation"], "1.0");
    assert_eq!(value["rule"]["later"], 7);
    assert_eq!(value["metadata"]["banner"], "x");
    assert_eq!(value["replace_query"], "seat");
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["diversity"]["similarity_equation"], "1.0");
    assert_eq!(f.sent()[0]["rule"]["later"], 7);
}

#[gpui_kit::test]
async fn json_to_form_keeps_unknown(cx: &mut TestAppContext) {
    let f = Fixture::editing(cx, rule()).await;
    f.press(cx, json_key());
    f.set_json(
        cx,
        r#"{"id":"a","rule":{"query":"desk","match":"contains","later":1},"excludes":[{"id":"9","note":2}],"future":true}"#,
    );
    f.press(cx, form_key());
    let draft = f.draft(cx);
    assert_eq!(draft.rule.query, "desk");
    assert_eq!(draft.rule.r#match, "contains");
    assert_eq!(draft.rule.extra["later"], 1);
    assert_eq!(draft.extra["future"], true);
    assert_eq!(draft.excludes[0].extra["note"], 2);
    let dialog = f.dialog(cx);
    let raw = cx.update(|cx| dialog.read(cx).extra().read(cx).text(cx));
    assert!(
        raw.contains("rule.later") && raw.contains("future"),
        "{raw}"
    );
}

#[gpui_kit::test]
async fn bad_json_blocks_save_and_keeps_text(cx: &mut TestAppContext) {
    let f = Fixture::editing(cx, rule()).await;
    f.press(cx, json_key());
    f.set_json(cx, "{ nope");
    assert!(f.has(cx, "curations.unreadable"));
    assert!(f.save_blocked(cx));
    f.press(cx, form_key());
    assert_eq!(f.json(cx), "{ nope");
    assert!(f.has(cx, "curations.unreadable"), "the tab stays");
    f.press(cx, save_key());
    assert!(f.sent().is_empty());
    // Valid JSON of the wrong shape is no draft either.
    f.set_json(cx, r#"{"id": 3}"#);
    assert!(f.save_blocked(cx));
}

#[gpui_kit::test]
async fn new_rule_defaults(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    let draft = f.draft(cx);
    assert!(draft.stop_processing);
    assert!(draft.remove_matched_tokens);
    assert!(!draft.filter_curated_hits);
    assert!(f.save_blocked(cx), "no id, no trigger");
    f.fill_valid(cx);
    assert_eq!(f.draft(cx).rule.r#match, "exact");
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["stop_processing"], true);
    assert_eq!(f.sent()[0]["remove_matched_tokens"], true);
    assert_eq!(
        f.sent()[0]["rule"],
        json!({"query": "chair", "match": "exact"})
    );
}

#[gpui_kit::test]
async fn a_rule_of_tags_sends_no_match(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    f.fill(cx, "curations.id", "n");
    f.fill(cx, "curations.tags", "vip, new");
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["rule"], json!({"tags": ["vip", "new"]}));

    // Clearing the query of a stored rule takes its match along: the server refuses one alone.
    let mut item = rule();
    item["rule"]["tags"] = json!(["vip"]);
    let f = Fixture::editing(cx, item).await;
    f.fill(cx, "curations.query", "");
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["rule"], json!({"tags": ["vip"]}));
}

#[gpui_kit::test]
async fn a_tag_with_a_comma_survives_the_form(cx: &mut TestAppContext) {
    let mut item = rule();
    item["rule"]["tags"] = json!(["a,b", "c"]);
    let f = Fixture::editing(cx, item).await;
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["rule"]["tags"], json!(["a,b", "c"]));
}

#[gpui_kit::test]
async fn a_stamp_the_picker_cannot_hold_stays_in_the_raw_block(cx: &mut TestAppContext) {
    let mut item = rule();
    item["effective_from_ts"] = json!(i64::MAX);
    let f = Fixture::editing(cx, item).await;
    let dialog = f.dialog(cx);
    let raw = cx.update(|cx| dialog.read(cx).extra().read(cx).text(cx));
    assert!(raw.contains(&i64::MAX.to_string()), "{raw}");
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["effective_from_ts"], json!(i64::MAX));
}

#[gpui_kit::test]
async fn a_picked_date_replaces_a_stamp_in_the_raw_block(cx: &mut TestAppContext) {
    let mut item = rule();
    item["effective_from_ts"] = json!(i64::MAX);
    let f = Fixture::editing(cx, item).await;
    let dialog = f.dialog(cx);
    let at = stamp("2026-03-01T08:30:00Z");
    let picked = DateTime::from_timestamp(at, 0).unwrap().naive_utc();
    cx.update_window(f.handle.into(), |_, window, cx| {
        let state = dialog.read(cx).dates()[0].clone();
        state.update(cx, |s, cx| {
            s.set_date_time(PickedTime::Single(Some(picked)), window, cx);
        });
    })
    .unwrap();
    f.press(cx, save_key());
    assert_eq!(f.sent()[0]["effective_from_ts"], json!(at));
}

#[gpui_kit::test]
async fn a_new_pin_takes_the_place_after_the_last(cx: &mut TestAppContext) {
    let mut item = rule();
    item["includes"] = json!([{"id": "1", "position": 1}, {"id": "2", "position": 2}]);
    let f = Fixture::editing(cx, item).await;
    f.click(cx, "curations.include.0.remove");
    f.pin(cx, &["3"]);
    let positions: Vec<i64> = f.draft(cx).includes.iter().map(|i| i.position).collect();
    assert_eq!(positions, [2, 3]);
}

#[gpui_kit::test]
async fn two_pins_on_one_place_block_the_save(cx: &mut TestAppContext) {
    let mut item = rule();
    item["includes"] = json!([{"id": "3", "position": 1}, {"id": "4", "position": 1}]);
    let f = Fixture::editing(cx, item).await;
    assert!(f.label(cx, "sets.item.issue.0").contains("position 1"));
    f.press(cx, save_key());
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn a_stored_position_of_zero_does_not_block_a_json_save(cx: &mut TestAppContext) {
    let mut item = rule();
    item["includes"] = json!([{"id": "3", "position": 0}]);
    let f = Fixture::new(cx, vec![item]).await;
    f.click(cx, "sets.tab.json");
    f.click(cx, "sets.json.save");
    assert_eq!(f.calls(), ["save_set c 1"]);
}

#[gpui_kit::test]
async fn remove_tokens_follows_the_rule_until_touched(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    assert!(f.draft(cx).remove_matched_tokens);
    f.pin(cx, &["1"]);
    assert!(!f.draft(cx).remove_matched_tokens, "the server's rule");
    f.click(cx, "curations.include.0.remove");
    assert!(f.draft(cx).remove_matched_tokens);
    f.click(cx, "curations.remove_tokens");
    assert!(!f.draft(cx).remove_matched_tokens);
    f.click(cx, "curations.remove_tokens");
    f.pin(cx, &["1"]);
    assert!(f.draft(cx).remove_matched_tokens, "touched, it stays");
}

#[gpui_kit::test]
async fn a_stored_flag_is_not_changed_by_a_new_include(cx: &mut TestAppContext) {
    let mut item = rule();
    item["remove_matched_tokens"] = json!(true);
    let f = Fixture::editing(cx, item).await;
    f.pin(cx, &["1"]);
    assert!(f.draft(cx).remove_matched_tokens);
}

#[gpui_kit::test]
async fn include_moves_by_key(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    f.pin(cx, &["1", "2", "3"]);
    let dialog = f.dialog(cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        let focus = dialog.read(cx).include_focus(2).unwrap().clone();
        focus.focus(window, cx);
    })
    .unwrap();
    f.press(cx, "alt-up");
    f.press(cx, "alt-up");
    assert_eq!(
        f.ids(cx),
        [
            ("3".to_string(), 1),
            ("1".to_string(), 2),
            ("2".to_string(), 3)
        ]
    );
    f.press(cx, "alt-down");
    assert_eq!(
        f.ids(cx),
        [
            ("1".to_string(), 1),
            ("3".to_string(), 2),
            ("2".to_string(), 3)
        ]
    );
}

#[gpui_kit::test]
async fn include_moves_by_drag(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    f.pin(cx, &["1", "2", "3"]);
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.drag_to("curations.include.0", "curations.include.2", cx);
    })
    .unwrap();
    assert_eq!(
        f.ids(cx),
        [
            ("2".to_string(), 1),
            ("3".to_string(), 2),
            ("1".to_string(), 3)
        ]
    );
}

#[gpui_kit::test]
async fn positions_renumber(cx: &mut TestAppContext) {
    let mut item = rule();
    item["includes"] = json!([{"id": "1", "position": 4}, {"id": "2", "position": 9}]);
    let f = Fixture::editing(cx, item).await;
    assert_eq!(f.ids(cx), [("1".to_string(), 4), ("2".to_string(), 9)]);
    f.click(cx, "curations.include.1.up");
    assert_eq!(f.ids(cx), [("2".to_string(), 1), ("1".to_string(), 2)]);
    // A typed position stays, and order follows it.
    f.fill(cx, "curations.include.0.position", "5");
    assert_eq!(f.ids(cx), [("1".to_string(), 2), ("2".to_string(), 5)]);
    f.fill(cx, "curations.include.1.position", "");
    assert!(f.has(cx, "sets.item.issue.0"), "an empty position blocks");
    assert!(f.save_blocked(cx));
}

#[gpui_kit::test]
async fn include_extras_survive_a_move(cx: &mut TestAppContext) {
    let mut item = rule();
    item["includes"] = json!([{"id": "1", "position": 1, "x": 1}, {"id": "2", "position": 2}]);
    let f = Fixture::editing(cx, item).await;
    f.click(cx, "curations.include.0.down");
    assert_eq!(f.draft(cx).includes[1].extra["x"], 1);
}

#[gpui_kit::test]
async fn picker_adds_pin_and_hide(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    *f.search.body.lock().unwrap() = r#"{"found":2,"out_of":2,"page":1,"search_time_ms":1,"hits":[
        {"document":{"id":"7","title":"Lamp"},"highlight":{}},
        {"document":{"id":"8","title":"Stool"},"highlight":{}}]}"#
        .to_string();
    f.fill(cx, "curations.picker.q", "la");
    f.wait_for_picker(cx);
    f.click(cx, "curations.hit.0.pin");
    f.click(cx, "curations.hit.1.hide");
    f.click(cx, "curations.hit.0.pin");
    let draft = f.draft(cx);
    assert_eq!(draft.includes.len(), 1, "a document is pinned once");
    assert_eq!(
        (draft.includes[0].id.as_str(), draft.includes[0].position),
        ("7", 1)
    );
    assert_eq!(draft.excludes[0].id, "8");
}

#[gpui_kit::test]
async fn picker_sends_enable_curations_false(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    f.fill(cx, "curations.picker.q", "a");
    f.fill(cx, "curations.picker.q", "ab");
    assert!(f.search.calls().is_empty(), "the debounce holds it");
    f.wait_for_picker(cx);
    let calls = f.search.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].collection, "shop", "attached first");
    assert_eq!(calls[0].param("q"), Some("ab"));
    assert_eq!(calls[0].param("query_by"), Some("title"));
    assert_eq!(calls[0].param("per_page"), Some("20"));
    assert_eq!(calls[0].param("enable_curations"), Some("false"));
    assert_eq!(calls[0].param("enable_analytics"), Some("false"));
    assert_eq!(calls[0].param("exclude_fields"), Some("embedding"));
}

#[gpui_kit::test]
async fn picker_error_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    *f.search.fail.lock().unwrap() = Some((400, "Bad query".into()));
    f.fill(cx, "curations.picker.q", "a");
    f.wait_for_picker(cx);
    assert!(f.label(cx, "curations.picker.error").contains("Bad query"));
}

#[gpui_kit::test]
async fn old_hits_go_with_an_error_and_a_new_collection(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![]).await;
    *f.search.body.lock().unwrap() = r#"{"found":1,"out_of":1,"page":1,"search_time_ms":1,"hits":[
        {"document":{"id":"7","title":"Lamp"},"highlights":[]}]}"#
        .to_string();
    let search = |q: &str, cx: &mut TestAppContext| {
        f.fill(cx, "curations.picker.q", q);
        f.wait_for_picker(cx);
    };
    search("a", cx);
    assert!(f.has(cx, "curations.hit.0.pin"));
    *f.search.fail.lock().unwrap() = Some((400, "Bad query".into()));
    search("ab", cx);
    assert!(!f.has(cx, "curations.hit.0.pin"));
    *f.search.fail.lock().unwrap() = None;
    search("a", cx);
    assert!(f.has(cx, "curations.hit.0.pin"));
    let dialog = f.dialog(cx);
    dialog.update(cx, |d, cx| d.pick_collection("other", cx));
    assert!(!f.has(cx, "curations.hit.0.pin"), "before the new answer");
}

#[gpui_kit::test]
async fn dates_round_trip_in_utc(cx: &mut TestAppContext) {
    let (from, to) = (stamp("2026-03-01T08:30:00Z"), stamp("2026-04-01T22:15:45Z"));
    let mut item = rule();
    item["effective_from_ts"] = json!(from);
    item["effective_to_ts"] = json!(to);
    let f = Fixture::editing(cx, item).await;
    let dialog = f.dialog(cx);
    let shown = cx.update(|cx| dialog.read(cx).dates()[0].read(cx).date_time());
    let at = DateTime::from_timestamp(from, 0).unwrap().naive_utc();
    assert_eq!(shown, PickedTime::Single(Some(at)));
    let draft = f.draft(cx);
    assert_eq!(draft.effective_from_ts, Some(from));
    assert_eq!(draft.effective_to_ts, Some(to), "seconds survive the form");
    cx.update_window(f.handle.into(), |_, window, cx| {
        let state = dialog.read(cx).dates()[0].clone();
        let later = DateTime::from_timestamp(from + 3600, 0)
            .unwrap()
            .naive_utc();
        state.update(cx, |s, cx| s.set_date_time(later, window, cx));
        let state = dialog.read(cx).dates()[1].clone();
        state.update(cx, |s, cx| {
            s.set_date_time(PickedTime::Single(None), window, cx)
        });
    })
    .unwrap();
    let draft = f.draft(cx);
    assert_eq!(draft.effective_from_ts, Some(from + 3600));
    assert_eq!(draft.effective_to_ts, None);
}

#[gpui_kit::test]
async fn the_date_picker_opens_over_the_dialog(cx: &mut TestAppContext) {
    let f = Fixture::editing(cx, rule()).await;
    let dialog = f.dialog(cx);
    let state = cx.update(|cx| dialog.read(cx).dates()[0].clone());
    let picker = gpui_kit::ElementId::from(("date-picker", state.entity_id()));
    cx.update_window(f.handle.into(), |_, window, cx| window.click(picker, cx))
        .unwrap();
    assert!(f.has(cx, "time"), "the popup with its time field");
    let shown = cx
        .update_window(f.handle.into(), |_, window, _| {
            window.find("time").visible()
        })
        .unwrap();
    assert!(shown, "not clipped by the dialog");
    f.press(cx, "escape");
    assert!(!f.has(cx, "time"));
    assert!(f.has(cx, "curations.id"), "Esc closes the popup first");
}

#[gpui_kit::test]
async fn from_after_to_blocks_save(cx: &mut TestAppContext) {
    let mut item = rule();
    item["effective_from_ts"] = json!(2_000);
    item["effective_to_ts"] = json!(1_000);
    let f = Fixture::editing(cx, item).await;
    assert!(f.has(cx, "sets.item.issue.0"));
    assert!(f.save_blocked(cx));
    f.press(cx, save_key());
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn flags_hide_on_30_0(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let f = Fixture::open(cx, profile, "30.0", vec![rule()]).await;
    f.click(cx, "curations.items.row.a");
    f.click(cx, "sets.items.edit");
    assert!(!f.has(cx, "curations.rule_synonyms"));
    assert!(!f.has(cx, "curations.rule_stem"));
    assert!(!f.has(cx, "curations.flags_dropped"));
    f.press(cx, json_key());
    f.set_json(
        cx,
        r#"{"id":"a","rule":{"query":"x","match":"exact","stem":true},"excludes":[{"id":"9"}]}"#,
    );
    assert!(f.has(cx, "curations.flags_dropped"));
}

#[gpui_kit::test]
async fn flags_show_on_30_2(cx: &mut TestAppContext) {
    let f = Fixture::editing(cx, rule()).await;
    f.click(cx, "curations.rule_stem");
    assert!(f.draft(cx).rule.stem);
    assert!(f.has(cx, "curations.rule_synonyms"));
}

#[gpui_kit::test]
async fn sort_warning_names_the_collection(cx: &mut TestAppContext) {
    let f = Fixture::editing(cx, rule()).await;
    f.fill(cx, "curations.sort_by", "nosuch:desc");
    let warning = f.label(cx, "sets.item.issue.0");
    assert!(
        warning.contains("nosuch") && warning.contains("shop"),
        "{warning}"
    );
    assert!(!warning.contains("other"), "only attached collections");
    assert!(!f.save_blocked(cx), "a warning does not block");
    f.fill(cx, "curations.sort_by", "price:desc");
    assert!(!f.has(cx, "sets.item.issue.0"));
}

#[gpui_kit::test]
async fn dropped_keys_show(cx: &mut TestAppContext) {
    let f = Fixture::editing(cx, rule()).await;
    let mut stored = rule();
    stored["rule"]
        .as_object_mut()
        .unwrap()
        .remove("match")
        .unwrap();
    *f.relevance.stored_item.lock().unwrap() = Some(stored);
    f.press(cx, save_key());
    assert_eq!(f.calls().last().unwrap(), "save_item c a");
    let note = f.label(cx, "sets.note");
    assert!(note.contains("rule.match"), "{note}");
    assert!(!f.has(cx, "curations.id"), "saved, so closed");
}

#[gpui_kit::test]
async fn locked_sends_nothing(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::open(cx, profile, "30.2", vec![rule()]).await;
    f.click(cx, "sets.items.add");
    assert!(f.has(cx, "danger.input"));
    assert!(!f.has(cx, "curations.id"));
    f.press(cx, "escape");
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn a_taken_id_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::adding(cx, vec![rule()]).await;
    f.fill(cx, "curations.id", "a");
    f.fill(cx, "curations.query", "chair");
    f.press(cx, save_key());
    assert!(f.has(cx, "sets.item.error"));
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn set_json_tab_checks_curations(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![rule()]).await;
    f.click(cx, "sets.tab.json");
    let view = f.view(cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        let json = view.read(cx).json().clone();
        json.update(cx, |e, cx| {
            e.set_text(r#"{"items":[{"id":"a","rule":{}}]}"#, window, cx);
        });
    })
    .unwrap();
    f.settle(cx);
    f.click(cx, "sets.json.save");
    assert!(f.label(cx, "sets.error").contains("Row 1"));
    assert!(f.calls().iter().all(|c| !c.starts_with("save_set")));
}

#[gpui_kit::test]
async fn tab_order_reaches_every_control(cx: &mut TestAppContext) {
    let mut item = rule();
    item["includes"] = json!([{"id": "1", "position": 1}]);
    let f = Fixture::editing(cx, item).await;
    let dialog = f.dialog(cx);
    let expected = [
        "curations.id",
        "curations.query",
        "curations.match.exact",
        "curations.match.contains",
        "curations.tags",
        "curations.rule_filter",
        "curations.rule_synonyms",
        "curations.rule_stem",
        "curations.include.0",
        "curations.include.0.position",
        "curations.include.0.remove",
        "curations.exclude.0.remove",
        "curations.picker.collection",
        "curations.picker.q",
        "curations.picker.manual",
        "curations.picker.add",
        "curations.filter_by",
        "curations.sort_by",
        "curations.replace_query",
        "curations.remove_tokens",
        "curations.filter_curated",
        "curations.stop",
        "sets.item.cancel",
        "sets.item.save",
    ];
    let mut seen: Vec<&str> = Vec::new();
    let mut dates = [false, false];
    for _ in 0..60 {
        f.press(cx, "tab");
        cx.update_window(f.handle.into(), |_, window, cx| {
            for id in expected {
                let found = window.try_find(id.to_string());
                if found.is_some_and(|e| e.focused() == Some(true)) && !seen.contains(&id) {
                    seen.push(id);
                }
            }
            let states = dialog.read(cx).dates().map(|s| s.read(cx).focus_handle(cx));
            for (reached, state) in dates.iter_mut().zip(states) {
                *reached |= state.is_focused(window);
            }
        })
        .unwrap();
    }
    let missed: Vec<&&str> = expected.iter().filter(|id| !seen.contains(id)).collect();
    assert!(missed.is_empty(), "never focused: {missed:?}");
    assert_eq!(dates, [true, true]);
}
