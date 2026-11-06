// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the test panel of a set.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use tessera_app::state::ResourceKind;
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::sets::SetsView;
use tessera_app::views::root::RootView;
use tessera_client::{Collection, ItemSet};
use tessera_core::profile::Profile;

use common::*;

/// Name, attached synonym sets, attached curation sets.
type Row<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

fn collection((name, synonym_sets, curation_sets): Row) -> Collection {
    serde_json::from_value(json!({
        "name": name,
        "fields": [
            {"name": "title", "type": "string"},
            {"name": "tags", "type": "string[]"},
            {"name": "n", "type": "int32"}
        ],
        "synonym_sets": synonym_sets,
        "curation_sets": curation_sets,
    }))
    .unwrap()
}

struct Fixture {
    handle: WindowHandle<Root>,
    data: Arc<FakeData>,
    search: Arc<FakeSearch>,
    relevance: Arc<FakeRelevance>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    async fn open(
        cx: &mut TestAppContext,
        profile: Profile,
        kind: ResourceKind,
        set: &str,
        rows: &[Row<'_>],
    ) -> Self {
        let collections = rows.iter().copied().map(collection).collect();
        let data = fake_backend(cx, Some((collections, vec![])));
        fake_documents(cx);
        let search = install_search_fake(cx);
        let relevance = fake_relevance(cx);
        relevance.sets.lock().unwrap().push(ItemSet {
            name: set.into(),
            items: vec![],
        });
        *relevance.data.lock().unwrap() = Some(data.clone());
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
        cx.update(|cx| state.update(cx, |s, cx| s.open_resource(id, kind, cx)));
        wait_for_element(cx, handle, "synonyms.new".into(), true).await;
        let f = Self {
            handle,
            data,
            search,
            relevance,
            _rt: rt,
        };
        f.settle(cx);
        f.click(cx, &format!("synonyms.row.{set}"));
        f.click(cx, "sets.tab.test");
        f
    }

    async fn synonyms(cx: &mut TestAppContext, rows: &[Row<'_>]) -> Self {
        let profile = Profile::new("test", vec![node()]);
        Self::open(cx, profile, ResourceKind::Synonyms, "s", rows).await
    }

    async fn curations(cx: &mut TestAppContext, rows: &[Row<'_>]) -> Self {
        let profile = Profile::new("test", vec![node()]);
        Self::open(cx, profile, ResourceKind::Curations, "c", rows).await
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

    /// Picks `name` as the select does.
    fn pick(&self, cx: &mut TestAppContext, name: &str) {
        let view = self.view(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            let panel = view.read(cx).test_panel().clone();
            panel.update(cx, |p, cx| p.pick(name.to_string(), window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn query_by(&self, cx: &mut TestAppContext) -> String {
        self.settle(cx);
        let view = self.view(cx);
        cx.update(|cx| view.read(cx).test_panel().read(cx).query_by_text(cx))
    }

    fn run(&self, cx: &mut TestAppContext) {
        self.press(cx, key("cmd-enter", "ctrl-enter"));
    }

    /// The parameters of each search, in order.
    fn sent(&self) -> Vec<Vec<(String, String)>> {
        self.search.calls().into_iter().map(|c| c.params).collect()
    }
}

fn key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// What every side sends for `q=novel` on the test schema.
fn common_with(q: &str, own: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut all = pairs(&[
        ("q", q),
        ("query_by", "title,tags"),
        ("per_page", "10"),
        ("enable_analytics", "false"),
    ]);
    all.extend(pairs(own));
    all
}

#[gpui_kit::test]
async fn unattached_synonym_set_is_added_on_the_right(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &[], &[])]).await;
    f.pick(cx, "books");
    f.fill(cx, "test.q", "novel");
    f.run(cx);
    assert_eq!(
        f.sent(),
        vec![
            common_with("novel", &[]),
            common_with("novel", &[("synonym_sets", "s")])
        ]
    );
    assert_eq!(f.search.calls()[0].collection, "books");
    let left = f.label(cx, "test.left.header");
    assert!(left.starts_with("As configured"), "{left}");
    assert!(left.contains("12 found") && left.contains("3 ms"), "{left}");
    let right = f.label(cx, "test.right.header");
    assert!(right.starts_with("With the set"), "{right}");
    assert!(f.has(cx, "test.note.added"));
}

#[gpui_kit::test]
async fn attached_alone_runs_the_left_side_without_synonyms(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["s"], &[])]).await;
    f.pick(cx, "books");
    f.run(cx);
    assert_eq!(
        f.sent(),
        vec![
            common_with("*", &[("enable_synonyms", "false")]),
            common_with("*", &[])
        ]
    );
    let left = f.label(cx, "test.left.header");
    assert!(left.starts_with("No synonyms"), "{left}");
    assert!(
        f.label(cx, "test.right.header")
            .starts_with("As configured")
    );
    assert!(!f.has(cx, "test.note.broad"));
    assert!(!f.has(cx, "test.note.added"));
}

#[gpui_kit::test]
async fn broad_note_shows(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["s", "t"], &[])]).await;
    f.pick(cx, "books");
    let note = f.label(cx, "test.note.broad");
    assert!(note.contains("without any synonyms"), "{note}");
    f.run(cx);
    assert_eq!(
        f.sent()[0],
        common_with("*", &[("enable_synonyms", "false")])
    );
}

#[gpui_kit::test]
async fn other_attached_sets_are_not_broad_for_an_unattached_one(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["t", "u"], &[])]).await;
    f.pick(cx, "books");
    assert!(!f.has(cx, "test.note.broad"));
    assert!(f.has(cx, "test.note.added"));
}

#[gpui_kit::test]
async fn attached_curation_set_sends_tags_on_the_right(cx: &mut TestAppContext) {
    let f = Fixture::curations(cx, &[("books", &[], &["c"])]).await;
    f.pick(cx, "books");
    assert!(f.has(cx, "test.note.tags"));
    assert!(!f.has(cx, "test.note.broad"));
    f.fill(cx, "test.tags", "vip, new");
    f.run(cx);
    assert_eq!(
        f.sent(),
        vec![
            common_with("*", &[("enable_curations", "false")]),
            common_with("*", &[("curation_tags", "vip,new")])
        ]
    );
    let left = f.label(cx, "test.left.header");
    assert!(left.starts_with("No curations"), "{left}");
}

#[gpui_kit::test]
async fn curation_set_with_others_says_so(cx: &mut TestAppContext) {
    let f = Fixture::curations(cx, &[("books", &[], &["c", "d"])]).await;
    f.pick(cx, "books");
    let note = f.label(cx, "test.note.broad");
    assert!(note.contains("without any curations"), "{note}");
}

#[gpui_kit::test]
async fn blocked_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::curations(cx, &[("books", &[], &[])]).await;
    f.pick(cx, "books");
    let note = f.label(cx, "test.blocked");
    assert!(note.contains("only when attached"), "{note}");
    f.run(cx);
    f.click(cx, "test.run");
    assert!(f.sent().is_empty());
    assert!(!f.has(cx, "test.attach"), "no attach control here");
    f.click(cx, "test.blocked.collections");
    assert!(f.has(cx, "sets.attach.filter"));
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn a_name_that_cannot_be_sent_blocks(cx: &mut TestAppContext) {
    let profile = Profile::new("test", vec![node()]);
    let rows: &[Row] = &[("books", &[], &[])];
    let f = Fixture::open(cx, profile, ResourceKind::Synonyms, "a,b", rows).await;
    f.pick(cx, "books");
    assert!(f.has(cx, "test.blocked"));
    f.run(cx);
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn a_bad_tag_blocks(cx: &mut TestAppContext) {
    let f = Fixture::curations(cx, &[("books", &[], &["c"])]).await;
    f.pick(cx, "books");
    f.fill(cx, "test.tags", "a,,b");
    assert!(f.has(cx, "test.blocked"));
    assert!(!f.has(cx, "test.blocked.collections"));
    f.run(cx);
    assert!(f.sent().is_empty());
    f.fill(cx, "test.tags", "a,b");
    assert!(!f.has(cx, "test.blocked"));
    f.run(cx);
    assert_eq!(f.sent().len(), 2);
}

#[gpui_kit::test]
async fn one_side_failing_leaves_the_other(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["s"], &[])]).await;
    *f.search.fail_with_param.lock().unwrap() = Some((
        "enable_synonyms".into(),
        (400, "Query field missing".into()),
    ));
    f.pick(cx, "books");
    f.run(cx);
    let error = f.label(cx, "test.left.error");
    assert!(error.contains("Query field missing"), "{error}");
    assert!(!f.has(cx, "test.right.error"));
    assert!(f.label(cx, "test.right.header").contains("12 found"));
}

#[gpui_kit::test]
async fn a_missing_attached_set_points_to_the_synonyms_tab(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["s"], &[])]).await;
    *f.search.fail.lock().unwrap() = Some((404, "Synonym index not found".into()));
    f.pick(cx, "books");
    assert!(!f.has(cx, "test.note.missing"));
    f.run(cx);
    assert!(f.has(cx, "test.note.missing"));
}

#[gpui_kit::test]
async fn new_run_drops_old_answers(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["s"], &[])]).await;
    f.pick(cx, "books");
    let held = f.search.hold_next();
    f.run(cx);
    assert!(!held.is_closed());
    assert!(!f.label(cx, "test.left.header").contains("found"));
    assert!(f.label(cx, "test.right.header").contains("12 found"));
    f.run(cx);
    assert!(held.is_closed(), "the old request is dropped");
    assert_eq!(f.sent().len(), 4);
    assert!(f.label(cx, "test.left.header").contains("12 found"));
}

#[gpui_kit::test]
async fn runs_on_a_locked_profile(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let rows: &[Row] = &[("books", &["s"], &[])];
    let f = Fixture::open(cx, profile, ResourceKind::Synonyms, "s", rows).await;
    f.pick(cx, "books");
    f.click(cx, "test.run");
    assert!(!f.has(cx, "danger.input"));
    assert_eq!(f.sent().len(), 2);
}

#[gpui_kit::test]
async fn no_side_writes(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &["s"], &[])]).await;
    let before = f.relevance.calls.lock().unwrap().len();
    f.pick(cx, "books");
    f.fill(cx, "test.q", "x");
    f.run(cx);
    assert_eq!(f.sent().len(), 2);
    assert_eq!(f.relevance.calls.lock().unwrap().len(), before);
    assert!(f.data.updates.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn query_by_from_schema(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &[], &[])]).await;
    assert!(f.data.gets.lock().unwrap().is_empty());
    f.pick(cx, "books");
    assert_eq!(*f.data.gets.lock().unwrap(), ["books"]);
    assert_eq!(f.query_by(cx), "title,tags");
    // Blank stays blank: an empty query_by with q=* is allowed.
    f.fill(cx, "test.query_by", "");
    f.run(cx);
    assert_eq!(
        f.sent()[0],
        pairs(&[
            ("q", "*"),
            ("per_page", "10"),
            ("enable_analytics", "false")
        ])
    );
}

#[gpui_kit::test]
async fn a_failed_read_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &[], &[])]).await;
    f.pick(cx, "nosuch");
    assert!(f.has(cx, "test.read_error"));
    f.run(cx);
    assert!(f.sent().is_empty());
}

#[gpui_kit::test]
async fn meaning_follows_a_fresh_read(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &[], &[])]).await;
    // The cache still says unattached.
    f.data.data.lock().unwrap().as_mut().unwrap().0[0].synonym_sets = vec!["s".into()];
    f.pick(cx, "books");
    f.run(cx);
    assert_eq!(
        f.sent()[0],
        common_with("*", &[("enable_synonyms", "false")])
    );
    assert!(f.label(cx, "test.left.header").starts_with("No synonyms"));
}

#[gpui_kit::test]
async fn coming_back_reads_the_collection_again(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &[], &[])]).await;
    f.pick(cx, "books");
    f.fill(cx, "test.query_by", "title");
    f.run(cx);
    assert!(f.label(cx, "test.left.header").contains("found"));
    f.click(cx, "sets.tab.collections");
    f.data.data.lock().unwrap().as_mut().unwrap().0[0].synonym_sets = vec!["s".into()];
    f.click(cx, "sets.tab.test");
    assert_eq!(f.data.gets.lock().unwrap().len(), 2);
    assert!(f.label(cx, "test.left.header").starts_with("No synonyms"));
    assert!(!f.label(cx, "test.left.header").contains("found"));
    assert_eq!(f.query_by(cx), "title", "what was typed stays");
}

#[gpui_kit::test]
async fn a_shortcut_opens_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::synonyms(cx, &[("books", &[], &[])]).await;
    f.press(cx, key("cmd-1", "ctrl-1"));
    assert!(!f.has(cx, "test.q"));
    f.press(cx, key("cmd-4", "ctrl-4"));
    assert!(f.has(cx, "test.q"));
}
