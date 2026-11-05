// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the collections tab and its sidebar entries, through a fake backend.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::state::{AppState, OpenCollection};
use tessera_app::views::dock::CollectionsPanel;
use tessera_app::views::root::RootView;
use tessera_client::{Alias, Collection};
use tessera_core::profile::Profile;
use uuid::Uuid;

use common::*;

const TOGGLE_FAVORITE: &str = key("cmd-d", "ctrl-d");
const FIND: &str = key("cmd-f", "ctrl-f");

fn collection(name: &str, documents: u64) -> Collection {
    Collection {
        name: name.into(),
        num_documents: documents,
        created_at: 1_700_000_000,
        ..Default::default()
    }
}

fn data() -> (Vec<Collection>, Vec<Alias>) {
    let alias = Alias {
        name: "books_current".into(),
        collection_name: "books".into(),
    };
    (
        vec![collection("books", 10_000), collection("authors", 42)],
        vec![alias],
    )
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    profile: Profile,
    backend: Arc<FakeData>,
    opened: Arc<Mutex<Vec<(Uuid, String)>>>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    fn tab(&self) -> String {
        format!("tab.{}.collections", self.profile.id)
    }

    fn sidebar(&self, what: &str) -> String {
        format!("sidebar.{}.{what}", self.profile.id)
    }

    async fn click(&self, cx: &mut TestAppContext, id: String) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    /// Opens the collections tab through the sidebar.
    async fn open_collections(&self, cx: &mut TestAppContext) {
        self.click(cx, self.sidebar("collections")).await;
        wait_for_element(cx, self.handle, self.tab(), true).await;
        cx.run_until_parked();
    }
}

/// A connected local profile with its overview open and `data` on the fake backend.
async fn connected(
    cx: &mut TestAppContext,
    data: Option<(Vec<Collection>, Vec<Alias>)>,
) -> Fixture {
    let backend = fake_backend(cx, data);
    fake_connector(cx, |p| Ok(server_for(p)));
    let (staging, _prod, secrets) = two_profiles();
    let (handle, state, rt) = open_app(cx, vec![staging.clone()], secrets);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let record = opened.clone();
    cx.update(|cx| {
        cx.subscribe(&state, move |_, event: &OpenCollection, _| {
            record.lock().unwrap().push((event.0, event.1.clone()));
        })
        .detach();
    });
    let fixture = Fixture {
        handle,
        state,
        profile: staging,
        backend,
        opened,
        _rt: rt,
    };
    fixture.click(cx, entry(&fixture.profile)).await;
    wait_for_element(
        cx,
        handle,
        format!("tab.{}.overview", fixture.profile.id),
        true,
    )
    .await;
    wait_for_element(cx, handle, fixture.sidebar("collections"), true).await;
    fixture
}

fn y(window: &mut gpui_kit::Window, id: &str) -> f32 {
    f32::from(window.find(id.to_string()).bounds().origin.y)
}

/// Runs `step` in the window, then lets the effects it queued (input events, state
/// notifications) run and draws a frame, so the next step sees their result.
fn act(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    step: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        step(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
async fn list_shows_counts_and_aliases(cx: &mut TestAppContext) {
    let f = connected(cx, Some(data())).await;
    f.open_collections(cx).await;
    act(cx, f.handle, |window, _| {
        let books = label(window, "collections.row.books");
        for part in ["books", "10,000", "books_current"] {
            assert!(books.contains(part), "{part:?} missing in {books:?}");
        }
        assert!(label(window, "collections.row.authors").contains("42"));
        assert_eq!(label(window, "collections.col.name"), "Name");
    });
    assert_eq!(*f.backend.lists.lock().unwrap(), 1);
}

#[gpui_kit::test]
async fn search_filters_by_name_and_alias(cx: &mut TestAppContext) {
    let f = connected(cx, Some(data())).await;
    f.open_collections(cx).await;
    act(cx, f.handle, |window, cx| {
        window.press(FIND, cx);
        window.input("current", cx);
    });
    act(cx, f.handle, |window, _| {
        assert!(window.try_find("collections.row.books").is_some());
        assert!(window.try_find("collections.row.authors").is_none());
    });
}

#[gpui_kit::test]
async fn favorites_come_first_and_show_in_sidebar(cx: &mut TestAppContext) {
    let f = connected(cx, Some(data())).await;
    f.open_collections(cx).await;
    // Name A to Z by default; one more click on the header reverses it.
    act(cx, f.handle, |window, cx| {
        assert!(y(window, "collections.row.authors") < y(window, "collections.row.books"));
        window.click("collections.col.name".to_string(), cx);
    });
    act(cx, f.handle, |window, cx| {
        assert!(y(window, "collections.row.books") < y(window, "collections.row.authors"));
        window.click("collections.row.authors".to_string(), cx);
    });
    act(cx, f.handle, |window, cx| window.press(TOGGLE_FAVORITE, cx));
    let sidebar = format!("sidebar.{}.collection.authors", f.profile.id);
    act(cx, f.handle, |window, _| {
        assert!(y(window, "collections.row.authors") < y(window, "collections.row.books"));
        assert!(window.try_find(sidebar).is_some());
    });
    let favorites = f
        .state
        .read_with(cx, |s, _| s.profiles[0].favorite_collections.clone());
    assert_eq!(favorites, ["authors"]);
}

#[gpui_kit::test]
async fn enter_opens_collection_tab(cx: &mut TestAppContext) {
    let f = connected(cx, Some(data())).await;
    f.open_collections(cx).await;
    act(cx, f.handle, |window, cx| {
        window.click("collections.row.books".to_string(), cx);
    });
    act(cx, f.handle, |window, cx| window.press("enter", cx));
    assert_eq!(*f.opened.lock().unwrap(), [(f.profile.id, "books".into())]);
}

#[gpui_kit::test]
async fn enter_on_a_header_sorts_instead(cx: &mut TestAppContext) {
    let f = connected(cx, Some(data())).await;
    f.open_collections(cx).await;
    // The click focuses the header and sorts largest first; Enter on it flips the order.
    act(cx, f.handle, |window, cx| {
        window.click("collections.col.documents".to_string(), cx);
    });
    act(cx, f.handle, |window, _| {
        assert!(y(window, "collections.row.books") < y(window, "collections.row.authors"));
    });
    act(cx, f.handle, |window, cx| window.press("enter", cx));
    act(cx, f.handle, |window, _| {
        assert!(y(window, "collections.row.authors") < y(window, "collections.row.books"));
    });
    assert!(f.opened.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn empty_cluster_shows_explanation(cx: &mut TestAppContext) {
    let f = connected(cx, Some((Vec::new(), Vec::new()))).await;
    f.open_collections(cx).await;
    act(cx, f.handle, |window, _| {
        let text = label(window, "collections.empty");
        assert!(text.contains("cargo xtask seed"), "{text:?}");
        assert!(window.try_find("collections.docs").is_some());
        assert!(window.try_find("collections.row.books").is_none());
    });
}

#[gpui_kit::test]
async fn load_error_shows_inline_in_full(cx: &mut TestAppContext) {
    let f = connected(cx, None).await;
    f.open_collections(cx).await;
    let expected = f.state.read_with(cx, |s, cx| {
        let conn = s.connection(f.profile.id, cx).unwrap();
        conn.read(cx).collection_cache.error.clone().unwrap()
    });
    act(cx, f.handle, |window, _| {
        assert!(!expected.is_empty());
        assert_eq!(label(window, "collections.error"), expected.to_string());
    });
}

#[gpui_kit::test]
async fn opening_twice_focuses_the_existing_tab(cx: &mut TestAppContext) {
    let f = connected(cx, Some(data())).await;
    f.open_collections(cx).await;
    f.click(cx, format!("{}.overview", entry(&f.profile))).await;
    f.click(cx, f.sidebar("collections")).await;
    act(cx, f.handle, |window, _| {
        assert!(window.find("collections.search").visible());
    });
    let panels = f
        .handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let area = view.read(cx).dock().read(cx);
            area.layout(DockPlacement::Center)
                .unwrap()
                .panels()
                .filter(|id| {
                    area.panel(*id)
                        .unwrap()
                        .view()
                        .downcast::<CollectionsPanel>()
                        .is_ok()
                })
                .count()
        })
        .unwrap();
    assert_eq!(panels, 1);
    assert_eq!(*f.backend.lists.lock().unwrap(), 1);
}
