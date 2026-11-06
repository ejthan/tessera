// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the stopwords view.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext, WindowHandle, base::Root};
use tessera_app::state::ResourceKind;
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::root::RootView;
use tessera_client::StopwordSet;
use tessera_core::profile::{Environment, Profile};

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    relevance: Arc<FakeRelevance>,
    _rt: tokio::runtime::Runtime,
}

fn set(id: &str, words: &[&str]) -> StopwordSet {
    StopwordSet {
        id: id.into(),
        stopwords: words.iter().map(ToString::to_string).collect(),
        locale: None,
    }
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, profile: Profile, sets: Vec<StopwordSet>) -> Self {
        fake_backend(cx, Some((vec![], vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let relevance = fake_relevance(cx);
        *relevance.stopwords.lock().unwrap() = sets;
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
        cx.update(|cx| {
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Stopwords, cx));
        });
        wait_for_element(cx, handle, "stopwords.new".into(), true).await;
        let f = Self {
            handle,
            relevance,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    async fn new(cx: &mut TestAppContext, sets: Vec<StopwordSet>) -> Self {
        Self::with(cx, Profile::new("test", vec![node()]), sets).await
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

    fn editor_text(&self, cx: &mut TestAppContext) -> String {
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
            let ResourceView::Stopwords(view) = panel.read(cx).view() else {
                panic!("not the stopwords view");
            };
            view.read(cx).editor().read(cx).text(cx)
        })
    }

    fn set_editor(&self, cx: &mut TestAppContext, text: &str) {
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
        cx.update_window(self.handle.into(), |_, window, cx| {
            let ResourceView::Stopwords(view) = panel.read(cx).view() else {
                panic!("not the stopwords view");
            };
            let editor = view.read(cx).editor().clone();
            editor.update(cx, |e, cx| e.set_text(text, window, cx));
        })
        .unwrap();
        self.settle(cx);
    }

    fn calls(&self) -> Vec<String> {
        self.relevance.calls.lock().unwrap().clone()
    }
}

#[gpui_kit::test]
async fn selecting_a_set_fills_the_editor(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the", "a"])]).await;
    f.click(cx, "stopwords.row.en");
    assert_eq!(f.editor_text(cx), "a\nthe");
}

#[gpui_kit::test]
async fn words_can_be_typed(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    let panel = f
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
    cx.update_window(f.handle.into(), |_, window, cx| {
        let ResourceView::Stopwords(view) = panel.read(cx).view() else {
            panic!("not the stopwords view");
        };
        let editor = view.read(cx).editor().clone();
        cx.update_entity(&editor, |e, cx| {
            e.set_text("", window, cx);
            e.focus(window, cx);
        });
    })
    .unwrap();
    f.press(cx, "a");
    assert_eq!(f.editor_text(cx), "a");
}

#[gpui_kit::test]
async fn save_shows_the_stored_words(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    *f.relevance.stored_words.lock().unwrap() = Some(vec!["y".into(), "x".into()]);
    f.set_editor(cx, "x\n\n  y \n");
    f.click(cx, "stopwords.save");
    assert_eq!(f.calls(), ["save_stopwords en"]);
    assert_eq!(f.editor_text(cx), "x\ny");
    assert!(!f.has(cx, "stopwords.note"));
}

#[gpui_kit::test]
async fn normalised_note_shows(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    *f.relevance.stored_words.lock().unwrap() = Some(vec!["y".into(), "x".into(), "the".into()]);
    f.set_editor(cx, "The\nthe\nx y");
    f.press(cx, SAVE);
    assert!(f.label(cx, "stopwords.note").contains("Stored 3 words"));
}

#[gpui_kit::test]
async fn empty_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    f.set_editor(cx, "  \n\n");
    f.click(cx, "stopwords.save");
    assert!(f.has(cx, "stopwords.error"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn new_refuses_existing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "en");
    f.click(cx, "resources.name.ok");
    assert!(f.label(cx, "resources.name.error").contains("en"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn new_sends_nothing_until_saved(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![]).await;
    assert!(f.has(cx, "stopwords.empty"));
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    assert_eq!(f.label(cx, "stopwords.editing"), "fresh");
    assert!(f.calls().is_empty());
    f.set_editor(cx, "word");
    f.click(cx, "stopwords.save");
    assert_eq!(f.calls(), ["save_stopwords fresh"]);
}

#[gpui_kit::test]
async fn locked_sends_nothing(cx: &mut TestAppContext) {
    let mut profile = Profile::new("test", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    assert!(f.has(cx, "stopwords.save.readonly"));
    f.click(cx, "stopwords.save");
    assert!(f.has(cx, "danger.input"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn delete_on_prod_needs_the_name(cx: &mut TestAppContext) {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    f.press(cx, DELETE);
    assert!(f.label(cx, "danger.consequence.0").contains("404"));
    f.fill(cx, "danger.input", "wrong");
    f.click(cx, "danger.ok");
    assert!(f.calls().is_empty());
    f.fill(cx, "danger.input", "en");
    f.click(cx, "danger.ok");
    assert_eq!(f.calls(), ["delete_stopwords en"]);
}

#[gpui_kit::test]
async fn save_on_prod_confirms(cx: &mut TestAppContext) {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    let f = Fixture::with(cx, profile, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    f.click(cx, "stopwords.save");
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls(), ["save_stopwords en"]);
}

#[gpui_kit::test]
async fn a_failed_save_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    *f.relevance.fail.lock().unwrap() = Some((400, "no way".into()));
    f.click(cx, "stopwords.save");
    assert!(f.label(cx, "stopwords.error").contains("no way"));
}

#[gpui_kit::test]
async fn a_new_set_stays_in_the_editor_while_the_list_changes(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    f.fill(cx, "stopwords.filter", "e");
    f.press(cx, REFRESH);
    assert_eq!(f.label(cx, "stopwords.editing"), "fresh");
}

#[gpui_kit::test]
async fn unsaved_text_asks_before_another_row_replaces_it(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"]), set("de", &["der"])]).await;
    f.click(cx, "stopwords.row.en");
    f.set_editor(cx, "the\nmine");
    f.click(cx, "stopwords.row.de");
    assert!(f.has(cx, "confirm.body"));
    assert_eq!(f.editor_text(cx), "the\nmine");
    f.click(cx, "confirm.cancel");
    assert_eq!(f.label(cx, "stopwords.editing"), "en");
    // The row went back with the editor, so Delete acts on what shows.
    f.press(cx, DELETE);
    assert!(f.label(cx, "confirm.body").contains("404"));
    f.click(cx, "confirm.cancel");
    f.click(cx, "stopwords.row.de");
    f.click(cx, "confirm.ok");
    assert_eq!(f.label(cx, "stopwords.editing"), "de");
    assert_eq!(f.editor_text(cx), "der");
}

#[gpui_kit::test]
async fn unsaved_text_asks_before_new_replaces_it(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    f.set_editor(cx, "mine");
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    assert!(f.has(cx, "confirm.body"));
    assert_eq!(f.label(cx, "stopwords.editing"), "en");
    f.click(cx, "confirm.ok");
    assert_eq!(f.label(cx, "stopwords.editing"), "fresh");
    assert_eq!(f.editor_text(cx), "");
}

#[gpui_kit::test]
async fn an_unchanged_editor_moves_on_without_asking(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the", "A"]), set("de", &["der"])]).await;
    f.click(cx, "stopwords.row.en");
    f.click(cx, "stopwords.row.de");
    assert!(!f.has(cx, "confirm.body"));
    assert_eq!(f.label(cx, "stopwords.editing"), "de");
}

#[gpui_kit::test]
async fn after_new_the_old_row_opens_and_delete_has_no_target(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.click(cx, "stopwords.row.en");
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    f.press(cx, DELETE);
    assert!(!f.has(cx, "confirm.body"));
    f.click(cx, "stopwords.row.en");
    assert_eq!(f.label(cx, "stopwords.editing"), "en");
}

#[gpui_kit::test]
async fn a_saved_new_set_becomes_the_selected_row(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"])]).await;
    f.press(cx, NEW_DOCUMENT);
    f.fill(cx, "resources.name", "fresh");
    f.click(cx, "resources.name.ok");
    f.set_editor(cx, "word");
    f.press(cx, SAVE);
    f.press(cx, DELETE);
    assert!(f.has(cx, "confirm.body"));
    f.click(cx, "confirm.ok");
    assert_eq!(
        f.calls(),
        ["save_stopwords fresh", "delete_stopwords fresh"]
    );
}

#[gpui_kit::test]
async fn a_running_save_blocks_another_and_spares_the_next_set(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, vec![set("en", &["the"]), set("de", &["der"])]).await;
    f.click(cx, "stopwords.row.en");
    let (release, hold) = tokio::sync::oneshot::channel();
    *f.relevance.hold.lock().unwrap() = Some(hold);
    f.set_editor(cx, "mine");
    f.press(cx, SAVE);
    f.press(cx, SAVE);
    assert_eq!(f.calls(), ["save_stopwords en"]);
    f.click(cx, "stopwords.row.de");
    f.click(cx, "confirm.ok");
    release.send(()).unwrap();
    assert_eq!(f.label(cx, "stopwords.editing"), "de");
    assert_eq!(f.editor_text(cx), "der");
}
