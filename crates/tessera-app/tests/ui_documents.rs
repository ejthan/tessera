// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the documents view of a collection tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::component::table::TableState;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, Focusable as _, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use tessera_app::connection::ConnectionState;
use tessera_app::state::AppState;
use tessera_app::views::dock::{CollectionPanel, CollectionsPanel, OverviewPanel};
use tessera_app::views::documents::inspector::Inspector;
use tessera_app::views::documents::table::DocRows;
use tessera_app::views::root::RootView;
use tessera_client::Collection;
use tessera_core::documents::page::wanted_pages;
use tessera_widgets::FilterInput;
use tessera_widgets::filter_input::FACET_DELAY;

use common::*;

fn products(fields: serde_json::Value) -> Collection {
    serde_json::from_value(json!({"name": "products", "fields": fields})).unwrap()
}

fn plain_schema() -> Collection {
    products(json!([
        {"name": "title", "type": "string"},
        {"name": "price", "type": "float", "sort": true},
    ]))
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    conn: Entity<ConnectionState>,
    docs: Arc<FakeDocs>,
    backend: Arc<FakeData>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    /// The schema changed on the server, as a schema edit reports it.
    fn change_schema(&self, cx: &mut TestAppContext, collection: Collection) {
        *self.backend.data.lock().unwrap() = Some((vec![collection], vec![]));
        cx.update(|cx| {
            self.conn
                .update(cx, |c, cx| c.collection_changed("products".into(), cx))
        });
        self.settle(cx);
    }

    fn panel(&self, cx: &mut TestAppContext) -> Entity<CollectionPanel> {
        collection_panel(cx, self.handle, "products")
    }

    /// Removes every other tab, so the kit would refuse to close this one.
    fn leave_alone(&self, cx: &mut TestAppContext) {
        let view = self
            .handle
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<RootView>().unwrap()
            })
            .unwrap();
        self.act(cx, |window, cx| {
            let dock = view.read(cx).dock().clone();
            dock.update(cx, |area, cx| {
                let ids: Vec<_> = area
                    .layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .collect();
                for id in ids {
                    let view = area.panel(id).unwrap().view().clone();
                    if let Ok(panel) = view.clone().downcast::<OverviewPanel>() {
                        area.remove_panel(panel, window, cx);
                    } else if let Ok(panel) = view.downcast::<CollectionsPanel>() {
                        area.remove_panel(panel, window, cx);
                    }
                }
            });
        });
        self.settle(cx);
        let area = cx.read(|cx| view.read(cx).dock().clone());
        let tabs = cx.read(|cx| {
            let area = area.read(cx);
            let tree = area.layout(DockPlacement::Center).unwrap();
            tree.panels().count()
        });
        assert_eq!(tabs, 1);
    }

    fn table(&self, cx: &mut TestAppContext) -> Entity<TableState<DocRows>> {
        let panel = self.panel(cx);
        cx.update(|cx| panel.read(cx).documents().read(cx).table().clone())
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

    /// Lets the throttle fire, then draws what arrived.
    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        self.act(cx, |_, _| {});
    }

    fn scroll_to(&self, cx: &mut TestAppContext, row: usize) {
        let table = self.table(cx);
        cx.update(|cx| table.update(cx, |t, cx| t.scroll_to_row(row, cx)));
        self.act(cx, |_, _| {});
    }

    fn sort_header(&self, cx: &mut TestAppContext, col: usize) {
        // Column 0 holds the checkboxes.
        self.act(cx, |window, cx| window.click(("icon-sort", col + 1), cx));
    }

    fn profile_id(&self, cx: &mut TestAppContext) -> uuid::Uuid {
        cx.update(|cx| self.conn.read(cx).profile.id)
    }

    /// Opens the column picker.
    fn open_columns(&self, cx: &mut TestAppContext) {
        self.act(cx, |window, cx| window.click("documents.columns", cx));
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.act(cx, |window, cx| window.click(id, cx));
    }

    fn filter(&self, cx: &mut TestAppContext) -> Entity<FilterInput> {
        let panel = self.panel(cx);
        cx.update(|cx| panel.read(cx).documents().read(cx).filter().clone())
    }

    /// Puts the focus in the filter bar and types `text`.
    fn type_filter(&self, cx: &mut TestAppContext, text: &str) {
        let filter = self.filter(cx);
        self.act(cx, |window, cx| {
            filter.update(cx, |f, cx| f.focus(window, cx));
            window.input(text, cx);
        });
    }

    /// Types `text` into the filter bar and applies it.
    fn submit_filter(&self, cx: &mut TestAppContext, text: &str) {
        self.type_filter(cx, text);
        self.act(cx, |window, cx| window.press("enter", cx));
        self.settle(cx);
    }

    fn filter_text(&self, cx: &mut TestAppContext) -> String {
        let filter = self.filter(cx);
        cx.update(|cx| filter.read(cx).value(cx))
    }

    /// The `filter_by` of the last query.
    fn last_filter(&self) -> Option<String> {
        let pages = self.docs.pages.lock().unwrap();
        pages.last().unwrap().filter_by.clone()
    }

    /// Keys of the columns drawn, left to right.
    fn column_keys(&self, cx: &mut TestAppContext) -> Vec<String> {
        let table = self.table(cx);
        cx.update(|cx| {
            let columns = &table.read(cx).delegate().columns;
            columns.iter().map(|c| c.key.clone()).collect()
        })
    }

    fn inspector(&self, cx: &mut TestAppContext) -> Entity<Inspector> {
        let panel = self.panel(cx);
        cx.update(|cx| panel.read(cx).documents().read(cx).inspector().clone())
    }

    fn inspector_text(&self, cx: &mut TestAppContext) -> String {
        let inspector = self.inspector(cx);
        cx.update(|cx| inspector.read(cx).editor().read(cx).text(cx))
    }

    fn inspector_id(&self, cx: &mut TestAppContext) -> Option<String> {
        let inspector = self.inspector(cx);
        cx.update(|cx| inspector.read(cx).current_id().map(str::to_string))
    }

    /// Replaces the editor's text and lets the check after typing run.
    fn edit(&self, cx: &mut TestAppContext, text: &str) {
        let inspector = self.inspector(cx);
        let editor = cx.update(|cx| inspector.read(cx).editor().clone());
        self.act(cx, |window, cx| {
            editor.update(cx, |e, cx| {
                e.set_text(text, window, cx);
                e.focus(window, cx);
            });
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        self.settle(cx);
    }

    /// Opens the inspector on row 3, whose document is `{"id": "3", "title": "t3", …}`.
    fn open_document(&self, cx: &mut TestAppContext) {
        self.select_row(cx, 3);
        self.act(cx, |window, cx| window.press("enter", cx));
        self.settle(cx);
        let inspector = self.inspector(cx);
        let editor = cx.update(|cx| inspector.read(cx).editor().clone());
        self.act(cx, |window, cx| {
            editor.update(cx, |e, cx| e.focus(window, cx))
        });
    }

    /// Opens row 3, renames it to "new" and asks for the review.
    fn review_renamed(&self, cx: &mut TestAppContext) {
        self.open_document(cx);
        self.edit(cx, RENAMED);
        self.act(cx, |window, cx| window.press(SAVE, cx));
        self.settle(cx);
    }

    fn writes(&self) -> Vec<(String, String, serde_json::Value)> {
        self.docs.writes.lock().unwrap().clone()
    }

    fn dirty(&self, cx: &mut TestAppContext) -> bool {
        let inspector = self.inspector(cx);
        cx.update(|cx| inspector.read(cx).is_dirty(cx))
    }

    fn has(&self, cx: &mut TestAppContext, id: &'static str) -> bool {
        self.act(cx, |window, _| window.try_find(id).is_some())
    }

    fn text_of(&self, cx: &mut TestAppContext, id: &'static str) -> String {
        self.act(cx, |window, _| label(window, id))
    }

    fn review_disabled(&self, cx: &mut TestAppContext) -> bool {
        self.act(cx, |window, _| {
            window.find("inspector.review").disabled() == Some(true)
        })
    }

    /// Selects a cell of `row` and leaves the keyboard on the table.
    fn select_row(&self, cx: &mut TestAppContext, row: usize) {
        let table = self.table(cx);
        self.act(cx, |window, cx| {
            table.read(cx).focus_handle(cx).focus(window, cx);
            table.update(cx, |t, cx| t.set_selected_cell(row, 1, cx));
        });
    }

    /// Lets the wait before following a selection pass.
    fn follow(&self, cx: &mut TestAppContext) {
        cx.executor().advance_clock(Duration::from_millis(150));
        self.settle(cx);
    }

    /// Focuses Go to ID, types `id` and presses Enter.
    fn go_to(&self, cx: &mut TestAppContext, id: &str) {
        self.act(cx, |window, cx| {
            window.press(GO_TO, cx);
        });
        self.act(cx, |window, cx| {
            window.input(id, cx);
            window.press("enter", cx);
        });
        self.settle(cx);
    }

    async fn wait_for_menu(&self, cx: &mut TestAppContext) {
        cx.wait_for(
            self.handle.into(),
            Duration::from_millis(500),
            |window, _| window.try_find("popup-menu").is_some(),
        )
        .await;
    }

    /// Right-clicks element `id` and waits for the menu.
    async fn open_menu_on(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.act(cx, |window, cx| window.right_click(id, cx));
        self.wait_for_menu(cx).await;
    }

    fn clipboard(&self, cx: &mut TestAppContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|c| c.text())
    }

    fn select_cell(&self, cx: &mut TestAppContext, col: usize) {
        let table = self.table(cx);
        cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, col + 1, cx)));
    }

    /// The ticked ids.
    fn ticked(&self, cx: &mut TestAppContext) -> Vec<String> {
        let table = self.table(cx);
        cx.update(|cx| table.read(cx).delegate().selection.ids())
    }

    /// Clicks the checkbox of document `id`.
    fn tick(&self, cx: &mut TestAppContext, id: &str) {
        self.click(cx, &format!("documents.check.{id}"));
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        let key = key.to_string();
        self.act(cx, |window, cx| window.press(&key, cx));
        self.settle(cx);
    }

    /// Confirms a delete in the simple dialog.
    fn confirm_delete(&self, cx: &mut TestAppContext) {
        self.click(cx, "confirm.ok");
        self.settle(cx);
    }

    /// Lets `ms` of the bulk dialog's wait pass.
    fn wait(&self, cx: &mut TestAppContext, ms: u64) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
        self.act(cx, |_, _| {});
    }

    fn counts(&self) -> Vec<Option<String>> {
        self.docs.count_calls.lock().unwrap().clone()
    }

    /// Opens a bulk dialog with `shortcut`, types `filter` and waits for its count.
    fn bulk(&self, cx: &mut TestAppContext, shortcut: &str, filter: &str) {
        self.press(cx, shortcut);
        self.type_bulk_filter(cx, filter);
        self.wait(cx, 300);
    }

    /// Replaces the text of the focused input with `text`.
    fn replace_text(&self, cx: &mut TestAppContext, text: &str) {
        self.act(cx, |window, cx| {
            window.press(SELECT_ALL, cx);
            window.input(text, cx);
        });
    }

    fn type_bulk_filter(&self, cx: &mut TestAppContext, text: &str) {
        self.act(cx, |window, cx| window.input(text, cx));
    }

    fn type_partial(&self, cx: &mut TestAppContext, text: &str) {
        self.click(cx, "bulk.editor");
        self.replace_text(cx, text);
        self.act(cx, |_, _| {});
    }

    fn continue_disabled(&self, cx: &mut TestAppContext) -> bool {
        self.act(cx, |window, _| {
            window.find("bulk.continue").disabled() == Some(true)
        })
    }

    /// Continue, with the second count answered.
    fn bulk_continue(&self, cx: &mut TestAppContext) {
        self.click(cx, "bulk.continue");
        self.settle(cx);
    }

    /// Scrolls over the first `found` rows so their pages are loaded.
    fn load_rows(&self, cx: &mut TestAppContext, found: usize) {
        for row in (0..found).step_by(200) {
            self.scroll_to(cx, row);
            self.settle(cx);
        }
    }

    /// Closes the tab and opens the collection again.
    async fn reopen(&self, cx: &mut TestAppContext) {
        let id = self.profile_id(cx);
        let tab = format!("tab.{id}.collection.products");
        self.click(cx, &tab);
        self.act(cx, |window, cx| window.press(CLOSE_TAB, cx));
        wait_for_element(cx, self.handle, tab.clone(), false).await;
        cx.update(|cx| {
            self.state
                .update(cx, |s, cx| s.open_collection(id, "products".into(), cx))
        });
        wait_for_element(cx, self.handle, tab, true).await;
        self.settle(cx);
    }

    fn cell(&self, cx: &mut TestAppContext, id: &str) -> Option<String> {
        let id = id.to_string();
        self.act(cx, |window, _| {
            window
                .try_find(id)
                .map(|e| e.label().unwrap_or_default().to_string())
        })
    }
}

/// Row 3's document with another title.
const RENAMED: &str = r#"{"id": "3", "title": "new", "embedding": [0.25, 0.5]}"#;

/// Opens `collection` with `found` documents and waits for the first page.
async fn open_found(cx: &mut TestAppContext, collection: Collection, found: u64) -> Fixture {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = found;
    open_tab(cx, collection, docs).await
}

async fn open_tab(cx: &mut TestAppContext, collection: Collection, docs: Arc<FakeDocs>) -> Fixture {
    open_profile(cx, collection, docs, false).await
}

async fn open_profile(
    cx: &mut TestAppContext,
    collection: Collection,
    docs: Arc<FakeDocs>,
    read_only: bool,
) -> Fixture {
    let mut profile = tessera_core::profile::Profile::new("test", vec![node()]);
    profile.read_only = read_only;
    open_as(cx, collection, docs, profile).await
}

async fn open_as(
    cx: &mut TestAppContext,
    collection: Collection,
    docs: Arc<FakeDocs>,
    profile: tessera_core::profile::Profile,
) -> Fixture {
    let name = collection.name.clone();
    let backend = fake_backend(cx, Some((vec![collection], vec![])));
    let profile_id = profile.id;
    let (handle, state, rt) = {
        fake_connector(cx, |_| Ok(server("30.2", true)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        tessera_core::secrets::SecretStore::set(&*secrets, profile.id, &"k".to_string().into())
            .unwrap();
        open_app(cx, vec![profile], secrets)
    };
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    cx.update(|cx| state.update(cx, |s, cx| s.connect(profile_id, None, cx)));
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("figure.version").is_some()
    })
    .await;
    cx.update(|cx| state.update(cx, |s, cx| s.open_collection(profile_id, name, cx)));
    wait_for_element(
        cx,
        handle,
        format!("tab.{profile_id}.collection.products"),
        true,
    )
    .await;
    cx.run_until_parked();
    let conn = cx
        .update(|cx| state.read(cx).connection(profile_id, cx))
        .unwrap();
    let f = Fixture {
        handle,
        state,
        conn,
        docs,
        backend,
        _rt: rt,
    };
    f.settle(cx);
    f
}

#[gpui_kit::test]
async fn opens_on_documents_and_shows_found(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    f.act(cx, |window, _| {
        assert!(label(window, "documents.status").contains("200,000"));
        assert_eq!(label(window, "documents.cell.0.title"), "t0");
        assert!(window.try_find("schema.field.title").is_none());
    });
}

#[gpui_kit::test]
async fn empty_collection_says_so(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 0).await;
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.empty").is_some());
    });
}

#[gpui_kit::test]
async fn command_two_shows_the_schema_and_one_goes_back(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.act(cx, show_schema);
    f.act(cx, |window, _| {
        assert!(window.try_find("schema.field.title").is_some());
        assert!(window.try_find("documents.table").is_none());
    });
    f.act(cx, |window, cx| window.press(SHOW_DOCUMENTS, cx));
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.table").is_some());
        assert!(window.try_find("schema.field.title").is_none());
    });
    f.act(cx, |window, cx| window.click("collection.view.schema", cx));
    f.act(cx, |window, _| {
        assert!(window.try_find("schema.field.title").is_some());
    });
}

#[gpui_kit::test]
async fn refresh_goes_to_the_active_view(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let asked = f.docs.page_numbers().len();
    f.act(cx, |window, cx| window.press(REFRESH, cx));
    assert_eq!(f.docs.page_numbers().len(), asked + 1);
}

#[gpui_kit::test]
async fn refresh_reloads_the_pages_under_the_scroll_position(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    f.scroll_to(cx, 100_100);
    f.settle(cx);
    f.docs.pages.lock().unwrap().clear();
    f.act(cx, |window, cx| window.press(REFRESH, cx));
    f.settle(cx);
    assert_eq!(f.docs.page_numbers(), [1, 401]);
    assert_eq!(
        f.cell(cx, "documents.cell.100100.title").unwrap(),
        "t100100"
    );
}

#[gpui_kit::test]
async fn scrolling_loads_only_the_pages_it_needs(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    f.scroll_to(cx, 100_000);
    f.settle(cx);
    assert_eq!(f.docs.page_numbers(), [1, 400, 401]);
}

#[gpui_kit::test]
async fn a_jump_at_a_low_per_page_loads_every_page_it_shows(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 100).await;
    let id = f.profile_id(cx);
    let mut other = plain_schema();
    other.name = "other".into();
    *f.backend.data.lock().unwrap() = Some((vec![plain_schema(), other], vec![]));
    *f.docs.found.lock().unwrap() = 10_000;
    cx.update(|cx| {
        f.state.update(cx, |s, cx| {
            // The test window shows fewer rows on Linux and Windows; 5 per page keeps
            // more pages on screen than `MAX_INFLIGHT` on all of them.
            s.settings.per_page = 5;
            s.open_collection(id, "other".into(), cx);
        })
    });
    wait_for_element(cx, f.handle, "documents.columns".into(), true).await;
    f.settle(cx);
    let panel = collection_panel(cx, f.handle, "other");
    let table = cx.update(|cx| panel.read(cx).documents().read(cx).table().clone());
    cx.update(|cx| table.update(cx, |t, cx| t.scroll_to_row(5_008, cx)));
    f.settle(cx);
    let rows = cx.update(|cx| table.read(cx).visible_range().rows().clone());
    let wanted = wanted_pages(rows, 5, 10_000);
    assert!(wanted.len() > 4, "more than `MAX_INFLIGHT`: {wanted:?}");
    let fetched = f.docs.page_numbers();
    assert!(wanted.iter().all(|p| fetched.contains(p)), "{fetched:?}");
}

#[gpui_kit::test]
async fn long_scroll_keeps_twenty_pages(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    for jump in 1..=40 {
        f.scroll_to(cx, jump * 300);
        f.settle(cx);
    }
    let table = f.table(cx);
    assert!(cx.update(|cx| table.read(cx).delegate().cached_pages()) <= 20);
    assert!(f.docs.page_numbers().len() >= 40);
}

#[gpui_kit::test]
async fn continuous_scroll_still_fetches(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    let before = f.docs.page_numbers().len();
    for jump in 1..=10 {
        f.scroll_to(cx, jump * 5_000);
        cx.executor().advance_clock(Duration::from_millis(10));
    }
    assert!(f.docs.page_numbers().len() > before);
}

#[gpui_kit::test]
async fn late_answer_of_an_old_query_is_dropped(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    let table = f.table(cx);
    let release = f.docs.hold_next();
    let panel = f.panel(cx);
    let documents = cx.update(|cx| panel.read(cx).documents().clone());
    cx.update(|cx| documents.update(cx, |d, cx| d.reload(cx)));
    let stale = cx.update(|cx| table.read(cx).delegate().generation());
    f.sort_header(cx, 2);
    release.send(()).ok();
    f.settle(cx);
    assert_eq!(f.cell(cx, "documents.cell.999.title").unwrap(), "t999");
    // A task that survived its query: the generation stops it, not the cancellation.
    let late = f.docs.answer(&tessera_client::DocumentQuery {
        page: 1,
        per_page: 250,
        filter_by: None,
        sort_by: None,
        exclude_fields: None,
    });
    cx.update(|cx| {
        table.update(cx, |t, cx| {
            t.delegate_mut().finish(1, stale, Ok(late), cx);
        })
    });
    assert_eq!(f.cell(cx, "documents.cell.999.title").unwrap(), "t999");
    assert!(f.cell(cx, "documents.cell.0.title").is_none());
}

#[gpui_kit::test]
async fn rows_stay_until_the_new_query_answers(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    let release = f.docs.hold_next();
    f.sort_header(cx, 2);
    assert_eq!(f.cell(cx, "documents.cell.0.title").unwrap(), "t0");
    release.send(()).ok();
    f.settle(cx);
    assert!(f.cell(cx, "documents.cell.0.title").is_none());
    assert_eq!(f.cell(cx, "documents.cell.999.title").unwrap(), "t999");
}

#[gpui_kit::test]
async fn header_click_sorts(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.sort_header(cx, 2);
    assert_eq!(f.docs.last_sort().as_deref(), Some("price:desc"));
    f.sort_header(cx, 2);
    assert_eq!(f.docs.last_sort().as_deref(), Some("price:asc"));
    f.sort_header(cx, 2);
    assert_eq!(f.docs.last_sort(), None);
}

#[gpui_kit::test]
async fn sort_shortcut_uses_the_selected_cell(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    let table = f.table(cx);
    cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, 3, cx)));
    f.act(cx, |window, cx| window.press(SORT, cx));
    assert_eq!(f.docs.last_sort().as_deref(), Some("price:desc"));
    f.act(cx, |window, cx| {
        assert!(label(window, "documents.status").contains("sorted by price"));
        window.press(SORT, cx);
    });
    assert_eq!(f.docs.last_sort().as_deref(), Some("price:asc"));
    f.act(cx, |window, cx| window.press(SORT, cx));
    assert_eq!(f.docs.last_sort(), None);
}

#[gpui_kit::test]
async fn unsortable_column_has_no_icon_and_the_shortcut_does_nothing(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    let asked = f.docs.page_numbers().len();
    f.act(cx, |window, _| {
        assert!(window.try_find(("icon-sort", 2usize)).is_none());
        assert!(window.try_find(("icon-sort", 3usize)).is_some());
    });
    let table = f.table(cx);
    cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, 2, cx)));
    f.act(cx, |window, cx| window.press(SORT, cx));
    assert_eq!(f.docs.page_numbers().len(), asked);
}

#[gpui_kit::test]
async fn new_document_key_appears_as_a_column(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    *docs.extra_key.lock().unwrap() = Some("brand".into());
    let f = open_tab(cx, plain_schema(), docs).await;
    assert_eq!(f.cell(cx, "documents.cell.0.brand").unwrap(), "x");
}

#[gpui_kit::test]
async fn vector_field_is_excluded(cx: &mut TestAppContext) {
    let schema = products(json!([
        {"name": "title", "type": "string"},
        {"name": "embedding", "type": "float[]", "num_dim": 4},
    ]));
    let f = open_found(cx, schema, 10).await;
    let query = f.docs.pages.lock().unwrap()[0].clone();
    assert_eq!(query.exclude_fields.as_deref(), Some("embedding"));
}

#[gpui_kit::test]
async fn page_error_shows_in_full(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    let message = "Could not find a field named `nope` for sorting. ".repeat(4);
    *f.docs.fail.lock().unwrap() = Some((404, message.clone()));
    f.scroll_to(cx, 5_000);
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(label(window, "documents.error").contains(message.trim_end()));
    });
    // The page was not lost: the next scroll asks again and clears the error.
    *f.docs.fail.lock().unwrap() = None;
    f.scroll_to(cx, 5_100);
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.error").is_none());
    });
    assert_eq!(f.cell(cx, "documents.cell.5100.title").unwrap(), "t5100");
}

#[gpui_kit::test]
async fn failed_first_page_shows_the_error_instead_of_the_empty_state(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.fail.lock().unwrap() = Some((503, "Not Ready or Lagging".into()));
    let f = open_tab(cx, plain_schema(), docs).await;
    f.act(cx, |window, _| {
        assert!(label(window, "documents.error").contains("Not Ready or Lagging"));
        assert!(window.try_find("documents.empty").is_none());
        assert!(window.try_find("documents.status").is_none());
    });
}

#[gpui_kit::test]
async fn failed_schema_shows_on_the_documents_view_and_refresh_retries(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_tab(cx, plain_schema(), docs).await;
    // A second tab, of a collection whose schema does not load.
    let backend = fake_backend(cx, None);
    let id = cx.update(|cx| f.conn.read(cx).profile.id);
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "gone".into(), cx))
    });
    wait_for_element(cx, f.handle, format!("tab.{id}.collection.gone"), true).await;
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(!label(window, "documents.error").is_empty());
    });
    let asked = backend.gets.lock().unwrap().len();
    f.act(cx, |window, cx| window.press(REFRESH, cx));
    assert_eq!(backend.gets.lock().unwrap().len(), asked + 1);
}

#[gpui_kit::test]
async fn collection_changed_reloads(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.found.lock().unwrap() = 3;
    cx.update(|cx| {
        f.conn
            .update(cx, |c, cx| c.collection_changed("products".into(), cx))
    });
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(label(window, "documents.status").starts_with("3 of 3"));
    });
}

#[gpui_kit::test]
async fn dropping_the_sorted_field_clears_the_sort(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.sort_header(cx, 2);
    assert_eq!(f.docs.last_sort().as_deref(), Some("price:desc"));
    f.change_schema(cx, products(json!([{"name": "title", "type": "string"}])));
    assert_eq!(f.docs.last_sort(), None);
    f.act(cx, |window, _| {
        assert!(!label(window, "documents.status").contains("sorted"));
    });
    assert_eq!(f.cell(cx, "documents.cell.0.title").unwrap(), "t0");
}

#[gpui_kit::test]
async fn field_no_longer_sortable_clears_the_sort(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.sort_header(cx, 2);
    f.change_schema(
        cx,
        products(json!([
            {"name": "title", "type": "string"},
            {"name": "price", "type": "float", "sort": false},
        ])),
    );
    assert_eq!(f.docs.last_sort(), None);
}

#[gpui_kit::test]
async fn new_vector_field_is_excluded_and_gets_no_column(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    // The reload of `CollectionChanged` answers before the new schema arrives.
    *f.docs.extra_key.lock().unwrap() = Some("embedding".into());
    f.change_schema(
        cx,
        products(json!([
            {"name": "title", "type": "string"},
            {"name": "price", "type": "float", "sort": true},
            {"name": "embedding", "type": "float[]", "num_dim": 4},
        ])),
    );
    let last = f.docs.pages.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.exclude_fields.as_deref(), Some("embedding"));
    assert!(f.cell(cx, "documents.cell.0.embedding").is_none());
    let table = f.table(cx);
    let keys = cx.update(|cx| {
        let columns = &table.read(cx).delegate().columns;
        columns.iter().map(|c| c.key.clone()).collect::<Vec<_>>()
    });
    assert_eq!(keys, ["id", "title", "price"]);
}

#[gpui_kit::test]
async fn unchanged_schema_does_not_reload_twice(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let asked = f.docs.page_numbers().len();
    f.change_schema(cx, plain_schema());
    assert_eq!(f.docs.page_numbers().len(), asked + 1);
}

#[gpui_kit::test]
async fn other_collection_changing_does_not_reload(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let asked = f.docs.page_numbers().len();
    cx.update(|cx| {
        f.conn
            .update(cx, |c, cx| c.collection_changed("other".into(), cx))
    });
    f.settle(cx);
    assert_eq!(f.docs.page_numbers().len(), asked);
}

#[gpui_kit::test]
async fn shortcuts_of_the_hidden_schema_do_nothing_on_documents(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    // Focus on the tab bar, say, is outside the documents view's key context.
    let panel = f.panel(cx);
    for shortcut in [EDIT, DELETE] {
        f.act(cx, |window, cx| window.press(shortcut, cx));
        f.act(cx, |window, cx| {
            let handle = panel.read(cx).chrome_focus().clone();
            handle.focus(window, cx);
        });
        f.act(cx, |window, cx| window.press(shortcut, cx));
        f.act(cx, |window, _| {
            assert!(window.try_find("schema.mode").is_none());
            assert!(window.try_find("danger.input").is_none());
        });
    }
    // The same key on the schema view does open the dialog.
    f.act(cx, show_schema);
    f.act(cx, |window, cx| window.press(DELETE, cx));
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
}

#[gpui_kit::test]
async fn hidden_column_is_not_drawn_and_stays_hidden_after_reopen(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_columns(cx);
    f.click(cx, "documents.column.title");
    assert!(f.cell(cx, "documents.cell.0.title").is_none());
    assert_eq!(f.cell(cx, "documents.cell.0.price").unwrap(), "0");
    f.reopen(cx).await;
    assert!(f.cell(cx, "documents.cell.0.title").is_none());
    assert_eq!(f.cell(cx, "documents.cell.0.price").unwrap(), "0");
    // Shown again through the picker.
    f.open_columns(cx);
    f.click(cx, "documents.column.title");
    assert_eq!(f.cell(cx, "documents.cell.0.title").unwrap(), "t0");
}

#[gpui_kit::test]
async fn pinned_column_comes_first(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_columns(cx);
    f.click(cx, "documents.pin.price");
    assert_eq!(f.column_keys(cx), ["id", "price", "title"]);
    f.click(cx, "documents.pin.id");
    assert_eq!(f.column_keys(cx), ["price", "id", "title"]);
}

#[gpui_kit::test]
async fn pin_shortcut_uses_the_selected_cell(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_cell(cx, 2);
    f.act(cx, |window, cx| window.press(PIN, cx));
    assert_eq!(f.column_keys(cx), ["id", "price", "title"]);
    let id = f.profile_id(cx);
    let saved = cx.update(|cx| f.state.read(cx).document_view(id, "products"));
    assert_eq!(saved.pinned, ["id", "price"]);
}

#[gpui_kit::test]
async fn resized_width_survives_a_refresh(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let table = f.table(cx);
    let width =
        |cx: &mut TestAppContext| cx.update(|cx| f32::from(table.read(cx).col_width(2).unwrap()));
    let before = width(cx);
    f.select_cell(cx, 1);
    f.act(cx, |window, cx| window.press(WIDER, cx));
    assert_eq!(width(cx), before + 20.);
    f.act(cx, |window, cx| window.press(NARROWER, cx));
    f.act(cx, |window, cx| window.press(NARROWER, cx));
    assert_eq!(width(cx), before - 20.);
    f.act(cx, |window, cx| window.press(WIDER, cx));
    f.act(cx, |window, cx| window.press(WIDER, cx));
    // A page brings a new key, which redraws the columns.
    *f.docs.extra_key.lock().unwrap() = Some("brand".into());
    cx.update(|cx| {
        f.conn
            .update(cx, |c, cx| c.collection_changed("products".into(), cx))
    });
    f.settle(cx);
    assert_eq!(f.cell(cx, "documents.cell.0.brand").unwrap(), "x");
    assert_eq!(width(cx), before + 20.);
}

#[gpui_kit::test]
async fn last_visible_column_cannot_be_hidden(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_columns(cx);
    for key in ["id", "title", "price"] {
        f.click(cx, &format!("documents.column.{key}"));
    }
    assert_eq!(f.column_keys(cx), ["price"]);
    assert_eq!(f.cell(cx, "documents.cell.0.price").unwrap(), "0");
}

#[gpui_kit::test]
async fn reset_brings_back_the_default_columns(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_columns(cx);
    f.click(cx, "documents.column.title");
    f.click(cx, "documents.pin.price");
    f.click(cx, "documents.columns.reset");
    assert_eq!(f.column_keys(cx), ["id", "title", "price"]);
}

#[gpui_kit::test]
async fn preferences_are_per_collection(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_columns(cx);
    f.click(cx, "documents.column.title");
    let id = f.profile_id(cx);
    cx.update(|cx| {
        let state = f.state.read(cx);
        assert_eq!(state.document_view(id, "products").hidden, ["title"]);
        assert!(state.document_view(id, "books").hidden.is_empty());
    });
}

#[gpui_kit::test]
async fn failed_save_shows_inline(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "").unwrap();
    // The parent of the path is a file, so the write fails.
    let path = blocker.join("document_views.json");
    cx.update(|cx| f.state.update(cx, |s, _| s.views_path = Some(path)));
    f.open_columns(cx);
    f.click(cx, "documents.column.title");
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(label(window, "documents.columns_error").contains("Could not save the column"));
    });
    // The change stays in effect.
    assert!(f.cell(cx, "documents.cell.0.title").is_none());
}

#[gpui_kit::test]
async fn enter_applies_the_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    *f.docs.found.lock().unwrap() = 7;
    f.submit_filter(cx, "price:>1");
    assert_eq!(f.last_filter().as_deref(), Some("price:>1"));
    let status = f.cell(cx, "documents.status").unwrap();
    assert!(status.starts_with("7 of 7"), "{status}");
}

#[gpui_kit::test]
async fn clearing_the_filter_reloads(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    *f.docs.found.lock().unwrap() = 7;
    f.submit_filter(cx, "price:>1");
    *f.docs.found.lock().unwrap() = 1_000;
    f.click(cx, "documents.filter.clear");
    f.settle(cx);
    assert_eq!(f.last_filter(), None);
    assert_eq!(f.filter_text(cx), "");
    assert_eq!(
        f.cell(cx, "documents.filter.clear"),
        None,
        "nothing to clear"
    );
    let status = f.cell(cx, "documents.status").unwrap();
    assert!(status.starts_with("1,000 of 1,000"), "{status}");
}

#[gpui_kit::test]
async fn clear_shortcut_works_inside_the_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.submit_filter(cx, "price:>1");
    f.act(cx, |window, cx| window.press(CLEAR_FILTER, cx));
    f.settle(cx);
    assert_eq!(f.last_filter(), None);
    assert_eq!(f.filter_text(cx), "");
}

#[gpui_kit::test]
async fn an_emptied_filter_means_no_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.submit_filter(cx, "price:>1");
    let asked = f.docs.pages.lock().unwrap().len();
    f.act(cx, |window, cx| {
        window.press(SELECT_ALL, cx);
        window.press("backspace", cx);
    });
    f.submit_filter(cx, "   ");
    assert!(
        f.docs.pages.lock().unwrap().len() > asked,
        "no new query ran"
    );
    assert_eq!(f.last_filter(), None);
}

#[gpui_kit::test]
async fn bad_filter_shows_the_server_message_and_keeps_rows(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    let message = "Could not find a filter field named `nope` in the schema.";
    *f.docs.fail.lock().unwrap() = Some((400, message.into()));
    f.submit_filter(cx, "nope:1");
    let shown = f.cell(cx, "documents.filter.error").unwrap();
    assert!(shown.contains(message), "{shown}");
    assert_eq!(f.cell(cx, "documents.cell.0.title").as_deref(), Some("t0"));
    *f.docs.fail.lock().unwrap() = None;
    f.click(cx, "documents.filter.clear");
    f.settle(cx);
    assert_eq!(f.cell(cx, "documents.filter.error"), None);
}

#[gpui_kit::test]
async fn scrolling_does_not_resend_a_refused_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    *f.docs.fail.lock().unwrap() = Some((400, "bad".into()));
    f.submit_filter(cx, "nope:1");
    let asked = f.docs.pages.lock().unwrap().len();
    f.scroll_to(cx, 900);
    f.settle(cx);
    assert_eq!(f.docs.pages.lock().unwrap().len(), asked);
    assert!(f.cell(cx, "documents.filter.error").is_some());
    *f.docs.fail.lock().unwrap() = None;
    f.act(cx, |window, cx| {
        window.press(SELECT_ALL, cx);
        window.press("backspace", cx);
    });
    f.submit_filter(cx, "price:>1");
    assert_eq!(f.cell(cx, "documents.filter.error"), None);
    let pages = f.docs.page_numbers();
    assert!(
        pages[asked..].len() > 1,
        "the visible page loads: {pages:?}"
    );
}

#[gpui_kit::test]
async fn focus_shortcut_before_the_schema_does_nothing(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    let f = open_tab(cx, plain_schema(), docs).await;
    fake_backend(cx, None);
    let id = f.profile_id(cx);
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "gone".into(), cx))
    });
    wait_for_element(cx, f.handle, format!("tab.{id}.collection.gone"), true).await;
    f.settle(cx);
    let panel = collection_panel(cx, f.handle, "gone");
    let documents = cx.update(|cx| panel.read(cx).documents().clone());
    let filter = cx.update(|cx| documents.read(cx).filter().clone());
    f.act(cx, |window, cx| {
        documents.update(cx, |d, cx| d.focus_search(window, cx));
    });
    f.act(cx, |window, cx| {
        let focused = filter.read(cx).state().focus_handle(cx);
        assert!(!focused.is_focused(window));
    });
}

#[gpui_kit::test]
async fn filter_without_matches_says_so_instead_of_empty_collection(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    *f.docs.found.lock().unwrap() = 0;
    f.submit_filter(cx, "price:>1");
    assert_eq!(
        f.cell(cx, "documents.empty").as_deref(),
        Some("No documents match this filter")
    );
    assert!(f.cell(cx, "documents.filter.clear").is_some());
}

#[gpui_kit::test]
async fn focus_shortcut_moves_to_the_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.click(cx, "documents.cell.0.title");
    f.act(cx, |window, cx| {
        window.press(FIND, cx);
        window.input("ab", cx);
    });
    assert_eq!(f.filter_text(cx), "ab");
}

#[gpui_kit::test]
async fn facet_values_come_from_the_backend(cx: &mut TestAppContext) {
    let schema = products(json!([{"name": "genre", "type": "string", "facet": true}]));
    let f = open_found(cx, schema, 1_000).await;
    *f.docs.facets.lock().unwrap() = vec!["crime".into()];
    f.type_filter(cx, "genre:=cr");
    cx.executor().advance_clock(FACET_DELAY);
    f.settle(cx);
    assert_eq!(
        *f.docs.facet_calls.lock().unwrap(),
        [("products".into(), "genre".into(), "cr".into())]
    );
    let filter = f.filter(cx);
    let menu = cx.update(|cx| {
        let state = filter.read(cx).state().read(cx);
        let menu = state.completion_menu_state();
        (menu.open, menu.items[0].label.clone())
    });
    assert_eq!(menu, (true, "crime".to_string()));
}

#[gpui_kit::test]
async fn failed_facet_fetch_stays_quiet(cx: &mut TestAppContext) {
    let schema = products(json!([{"name": "genre", "type": "string", "facet": true}]));
    let f = open_found(cx, schema, 1_000).await;
    *f.docs.fail.lock().unwrap() = Some((500, "boom".into()));
    f.type_filter(cx, "genre:=cr");
    cx.executor().advance_clock(FACET_DELAY);
    f.settle(cx);
    assert_eq!(f.cell(cx, "documents.filter.error"), None);
    assert_eq!(f.cell(cx, "documents.error"), None);
}

#[gpui_kit::test]
async fn schema_change_updates_the_fields_offered(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.change_schema(cx, products(json!([{"name": "brand", "type": "string"}])));
    f.type_filter(cx, "b");
    let filter = f.filter(cx);
    let labels = cx.update(|cx| {
        let state = filter.read(cx).state().read(cx);
        let items = &state.completion_menu_state().items;
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    });
    assert_eq!(labels, ["brand"]);
}

#[gpui_kit::test]
async fn quick_filters_keep_only_the_last_answer(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    let release = f.docs.hold_next();
    f.submit_filter(cx, "price:>1");
    *f.docs.found.lock().unwrap() = 3;
    f.act(cx, |window, cx| {
        window.input("0", cx);
        window.press("enter", cx);
    });
    release.send(()).ok();
    f.settle(cx);
    assert_eq!(f.last_filter().as_deref(), Some("price:>10"));
    let status = f.cell(cx, "documents.status").unwrap();
    assert!(status.starts_with("3 of 3"), "{status}");
}

#[gpui_kit::test]
async fn enter_opens_the_inspector_with_the_full_document(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 3);
    f.act(cx, |window, cx| {
        assert!(window.try_find("documents.inspector").is_none());
        window.press("enter", cx);
    });
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.inspector").is_some());
    });
    let text = f.inspector_text(cx);
    assert!(text.contains("embedding"), "{text}");
    assert!(text.contains("\n  \"id\": \"3\""), "pretty: {text}");
    assert_eq!(f.inspector_id(cx).as_deref(), Some("3"));
}

#[gpui_kit::test]
async fn inspector_shortcut_toggles_and_shows_the_selected_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let table = f.table(cx);
    f.act(cx, |window, cx| {
        table.read(cx).focus_handle(cx).focus(window, cx);
        window.press(INSPECTOR, cx);
    });
    f.act(cx, |window, _| {
        assert_eq!(
            label(window, "documents.inspector.body"),
            "Select a row",
            "nothing selected"
        );
    });
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.inspector").is_none());
    });
    f.select_row(cx, 2);
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.settle(cx);
    assert_eq!(f.inspector_id(cx).as_deref(), Some("2"));
}

#[gpui_kit::test]
async fn closing_the_inspector_from_its_editor_leaves_the_keyboard_working(
    cx: &mut TestAppContext,
) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 2);
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    let inspector = f.inspector(cx);
    let editor = cx.update(|cx| inspector.read(cx).editor().clone());
    f.act(cx, |window, cx| {
        editor.update(cx, |e, cx| e.focus(window, cx));
    });
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.inspector").is_some());
    });
}

#[gpui_kit::test]
async fn inspector_follows_the_selected_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 0);
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    f.select_row(cx, 1);
    f.select_row(cx, 2);
    f.follow(cx);
    assert_eq!(f.inspector_id(cx).as_deref(), Some("2"));
    assert_eq!(
        *f.docs.document_calls.lock().unwrap(),
        ["0", "2"],
        "a passed row is not fetched"
    );
    // The same row again, in another column.
    let table = f.table(cx);
    cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(2, 1, cx)));
    f.follow(cx);
    assert_eq!(f.docs.document_calls.lock().unwrap().len(), 2);
}

#[gpui_kit::test]
async fn inspector_does_not_follow_while_closed(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 1);
    f.follow(cx);
    assert!(f.docs.document_calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn inspector_says_when_it_is_loading(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let release = f.docs.hold_next();
    f.select_row(cx, 1);
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert_eq!(label(window, "documents.inspector.body"), "Loading…");
    });
    release.send(()).unwrap();
    f.settle(cx);
    assert!(f.inspector_text(cx).contains("embedding"));
}

#[gpui_kit::test]
async fn failed_load_shows_in_the_inspector(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.document_fail.lock().unwrap() = Some((500, "boom".into()));
    f.select_row(cx, 1);
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(label(window, "documents.inspector.error").contains("boom"));
    });
}

#[gpui_kit::test]
async fn goto_shows_the_document(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.go_to(cx, "424242");
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.inspector").is_some());
    });
    assert!(f.inspector_text(cx).contains("424242"));
    assert_eq!(f.inspector_id(cx).as_deref(), Some("424242"));
}

#[gpui_kit::test]
async fn goto_404_shows_inline(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 1);
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    *f.docs.document_fail.lock().unwrap() = Some((404, "no such document".into()));
    f.go_to(cx, "nope");
    f.act(cx, |window, _| {
        assert!(label(window, "documents.goto.error").contains("no such document"));
        assert!(window.try_find("documents.inspector.error").is_none());
    });
    assert_eq!(
        f.inspector_id(cx).as_deref(),
        Some("1"),
        "the inspector keeps its document"
    );
    *f.docs.document_fail.lock().unwrap() = None;
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.goto.error").is_none());
    });
}

#[gpui_kit::test]
async fn copy_cell_copies_the_full_value(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let long = "x".repeat(500);
    *docs.extra_key.lock().unwrap() = Some("body".into());
    *docs.extra_value.lock().unwrap() = json!(long);
    let f = open_tab(cx, plain_schema(), docs).await;
    let col = f.column_keys(cx).iter().position(|k| k == "body").unwrap();
    let table = f.table(cx);
    f.act(cx, |window, cx| {
        table.read(cx).focus_handle(cx).focus(window, cx);
        table.update(cx, |t, cx| t.set_selected_cell(0, col + 1, cx));
    });
    f.act(cx, |window, cx| window.press(COPY_CELL, cx));
    assert_eq!(f.clipboard(cx), Some(long));
    let preview = f.cell(cx, "documents.cell.0.title");
    assert_eq!(preview.as_deref(), Some("t0"));
}

#[gpui_kit::test]
async fn copy_cell_of_an_absent_key_copies_nothing_but_succeeds(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let table = f.table(cx);
    cx.update(|cx| {
        table.update(cx, |t, _| {
            let column = t.delegate().columns[0].clone();
            t.delegate_mut().columns[0] = tessera_core::documents::columns::ColumnDef {
                key: "absent".into(),
                ..column
            };
        })
    });
    f.select_row(cx, 0);
    f.act(cx, |window, cx| window.press(COPY_CELL, cx));
    let written = cx.read_from_clipboard().expect("the clipboard is written");
    assert_eq!(written.text().unwrap_or_default(), "");
}

#[gpui_kit::test]
async fn copy_row_is_pretty_json(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 4);
    f.act(cx, |window, cx| window.press(COPY_ROW, cx));
    f.settle(cx);
    let text = f.clipboard(cx).unwrap();
    assert!(text.contains("embedding"), "{text}");
    assert!(text.contains("\n  \"id\": \"4\""), "{text}");
}

#[gpui_kit::test]
async fn failed_copy_row_shows_above_the_table(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.document_fail.lock().unwrap() = Some((500, "boom".into()));
    f.select_row(cx, 4);
    f.act(cx, |window, cx| window.press(COPY_ROW, cx));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(label(window, "documents.copy.error").contains("boom"));
    });
    assert_eq!(f.clipboard(cx), None);
    *f.docs.document_fail.lock().unwrap() = None;
    f.act(cx, |window, cx| window.press(COPY_ROW, cx));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.copy.error").is_none());
    });
}

#[gpui_kit::test]
async fn moving_the_selection_clears_a_failed_copy(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.document_fail.lock().unwrap() = Some((500, "boom".into()));
    f.select_row(cx, 4);
    f.act(cx, |window, cx| window.press(COPY_ROW, cx));
    f.settle(cx);
    f.select_row(cx, 5);
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.copy.error").is_none());
    });
}

#[gpui_kit::test]
async fn copy_on_unloaded_row_does_nothing(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 100_000).await;
    f.select_row(cx, 90_000);
    f.act(cx, |window, cx| {
        window.press(COPY_CELL, cx);
        window.press(COPY_ROW, cx);
    });
    cx.run_until_parked();
    assert_eq!(f.clipboard(cx), None);
    assert!(f.docs.document_calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn copy_menu_copies_and_says_so(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 1);
    f.click(cx, "documents.copy");
    cx.wait_for(f.handle.into(), Duration::from_millis(500), |window, _| {
        window.try_find("popup-menu").is_some()
    })
    .await;
    f.act(cx, |window, cx| {
        let mut menu = window.within("popup-menu");
        assert_eq!(menu.find(0).label(), Some("Copy cell"));
        assert_eq!(menu.find(1).label(), Some("Copy row as JSON"));
        menu.click(0, cx);
    });
    assert_eq!(f.clipboard(cx).as_deref(), Some("1"));
    f.act(cx, |window, _| {
        assert_eq!(label(window, "documents.copy"), "Copied");
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert_eq!(label(window, "documents.copy"), "Copy");
    });
}

#[gpui_kit::test]
async fn shortcuts_are_shown(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 1);
    let shown = |cx: &mut TestAppContext, chord: &str| {
        gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
            .debug_bounds(chord.to_string().leak())
            .unwrap_or_else(|| panic!("no Kbd for {chord}"))
    };
    let (toggle, goto) = f.act(cx, |window, _| {
        (
            window.find("documents.toggle_inspector").bounds(),
            window.find("documents.goto").bounds(),
        )
    });
    let i = shown(cx, key("kbd:cmd-i", "kbd:ctrl-i"));
    assert!(toggle.contains(&i.center()), "{i:?} not in {toggle:?}");
    let g = shown(cx, key("kbd:cmd-g", "kbd:ctrl-g"));
    assert!(goto.contains(&g.center()), "{g:?} not in {goto:?}");
    f.click(cx, "documents.copy");
    cx.wait_for(f.handle.into(), Duration::from_millis(500), |window, _| {
        window.try_find("popup-menu").is_some()
    })
    .await;
    f.act(cx, |_, _| {});
    shown(cx, key("kbd:cmd-c", "kbd:ctrl-c"));
    shown(cx, key("kbd:alt-cmd-c", "kbd:ctrl-alt-c"));
}

#[gpui_kit::test]
async fn right_click_copies_the_clicked_cell(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 0);
    f.open_menu_on(cx, "documents.cell.2.title").await;
    let table = f.table(cx);
    let selected = cx.update(|cx| table.read(cx).selected_cell());
    assert_eq!(selected.map(|(row, _)| row), Some(2));
    f.act(cx, |window, cx| {
        let mut menu = window.within("popup-menu");
        assert_eq!(menu.find(0).label(), Some("Copy cell"));
        menu.click(0, cx);
    });
    assert_eq!(f.clipboard(cx).as_deref(), Some("t2"));
    f.settle(cx);
    f.act(cx, |window, _| {
        assert!(window.try_find("popup-menu").is_none());
    });
    // The keyboard is back on the table.
    f.act(cx, |window, cx| window.press(COPY_ROW, cx));
    f.settle(cx);
    assert!(f.clipboard(cx).unwrap().contains("\"id\": \"2\""));
}

#[gpui_kit::test]
async fn right_click_copies_the_clicked_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 0);
    f.open_menu_on(cx, "documents.cell.3.title").await;
    f.act(cx, |window, cx| window.within("popup-menu").click(1, cx));
    f.settle(cx);
    assert!(f.clipboard(cx).unwrap().contains("\"id\": \"3\""));
}

#[gpui_kit::test]
async fn right_click_menu_opens_at_the_pointer_and_escape_closes_it(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let cell = f.act(cx, |window, _| {
        window.find("documents.cell.4.title").bounds()
    });
    f.open_menu_on(cx, "documents.cell.4.title").await;
    f.act(cx, |window, cx| {
        let menu = window.find("popup-menu").bounds();
        assert!(cell.contains(&menu.origin), "{menu:?} not at {cell:?}");
        window.press("escape", cx);
    });
    f.act(cx, |window, _| {
        assert!(window.try_find("popup-menu").is_none());
    });
}

#[gpui_kit::test]
async fn second_right_click_moves_the_menu_to_its_cell(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    // A row above: the open menu covers the ones below its click.
    f.open_menu_on(cx, "documents.cell.8.title").await;
    f.open_menu_on(cx, "documents.cell.2.title").await;
    f.settle(cx);
    f.act(cx, |window, cx| window.within("popup-menu").click(0, cx));
    assert_eq!(f.clipboard(cx).as_deref(), Some("t2"));
}

#[gpui_kit::test]
async fn right_click_beside_the_last_column_copies_that_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 0);
    let (cell, table) = f.act(cx, |window, _| {
        (
            window.find("documents.cell.5.title").bounds(),
            window.find("documents.table").bounds(),
        )
    });
    let beside = gpui_kit::point(table.right() - gpui_kit::px(10.), cell.center().y);
    let mut visual = gpui_kit::VisualTestContext::from_window(f.handle.into(), cx);
    visual.simulate_mouse_down(
        beside,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::none(),
    );
    f.wait_for_menu(cx).await;
    f.act(cx, |window, cx| window.within("popup-menu").click(0, cx));
    assert_eq!(f.clipboard(cx).as_deref(), Some("5"));
}

#[gpui_kit::test]
async fn empty_collection_links_the_docs_and_the_seed_hint(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 0).await;
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.docs").is_some());
        assert!(label(window, "documents.empty").contains("cargo xtask seed"));
    });
}

#[gpui_kit::test]
async fn edit_then_review_shows_the_diff_and_patch_body(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    assert!(
        f.text_of(cx, "review.target")
            .contains("/products/documents/3")
    );
    let change = f.text_of(cx, "review.diff.title");
    assert!(change.contains("t3") && change.contains("new"), "{change}");
    let body: serde_json::Value = serde_json::from_str(&f.text_of(cx, "review.body")).unwrap();
    assert_eq!(body, json!({"title": "new"}));
    assert!(!f.has(cx, "review.consequence"));
}

#[gpui_kit::test]
async fn apply_sends_the_merge_patch(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    f.act(cx, |window, cx| window.press("tab", cx));
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    let writes = f.writes();
    assert_eq!(
        writes,
        [("patch".into(), "3".into(), json!({"title": "new"}))]
    );
    wait_for_element(cx, f.handle, "review.apply".into(), false).await;
    assert!(f.inspector_text(cx).contains("\"new\""));
    assert!(!f.dirty(cx), "the response is the new base");
}

#[gpui_kit::test]
async fn saving_reloads_the_rows_without_a_count_change(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    let pages = f.docs.pages.lock().unwrap().len();
    f.click(cx, "review.apply");
    f.settle(cx);
    assert!(f.docs.pages.lock().unwrap().len() > pages);
}

#[gpui_kit::test]
async fn upsert_sends_the_whole_document(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.act(cx, |window, cx| window.press(SAVE_MODE, cx));
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(f.has(cx, "review.consequence"));
    f.click(cx, "review.apply");
    f.settle(cx);
    let (kind, id, body) = f.writes().pop().unwrap();
    assert_eq!((kind.as_str(), id.as_str()), ("upsert", "3"));
    assert_eq!(
        body,
        serde_json::from_str::<serde_json::Value>(RENAMED).unwrap()
    );
}

#[gpui_kit::test]
async fn save_mode_buttons_change_only_the_mode(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "inspector.save_mode.upsert");
    assert!(f.dirty(cx));
    f.click(cx, "inspector.save_mode.patch");
    f.click(cx, "inspector.review");
    f.settle(cx);
    assert!(!f.has(cx, "review.consequence"));
}

#[gpui_kit::test]
async fn review_focus_starts_on_back(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    wait_for_element(cx, f.handle, "review.back".into(), false).await;
    assert!(f.writes().is_empty());
}

#[gpui_kit::test]
async fn enter_on_the_focused_apply_button_applies(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    f.act(cx, |window, cx| window.press("tab", cx));
    f.act(cx, |window, cx| window.press("enter", cx));
    f.settle(cx);
    assert_eq!(f.writes().len(), 1);
}

#[gpui_kit::test]
async fn escape_returns_without_sending(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    f.act(cx, |window, cx| window.press("escape", cx));
    wait_for_element(cx, f.handle, "review.back".into(), false).await;
    assert!(f.writes().is_empty());
    assert!(f.dirty(cx), "the edit is still there");
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(
        f.has(cx, "review.back"),
        "the keyboard is back in the editor"
    );
}

#[gpui_kit::test]
async fn server_error_stays_in_the_review(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.write_fail.lock().unwrap() = Some((400, "bad field".into()));
    f.review_renamed(cx);
    f.click(cx, "review.apply");
    f.settle(cx);
    assert!(f.text_of(cx, "review.error").contains("bad field"));
    assert!(f.has(cx, "review.apply"));
    assert!(f.dirty(cx));
}

#[gpui_kit::test]
async fn apply_disables_itself_and_escape_waits_while_running(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    let hold = f.docs.hold_next();
    f.click(cx, "review.apply");
    f.act(cx, |window, cx| window.press("escape", cx));
    f.act(cx, |window, _| {
        assert_eq!(window.find("review.apply").disabled(), Some(true));
    });
    f.click(cx, "review.apply");
    assert_eq!(f.writes().len(), 1, "one click, one write");
    hold.send(()).unwrap();
    wait_for_element(cx, f.handle, "review.apply".into(), false).await;
}

#[gpui_kit::test]
async fn unchanged_document_disables_review(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    assert!(f.review_disabled(cx));
    assert!(f.has(cx, "inspector.review.reason"));
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(!f.has(cx, "review.back"));
}

#[gpui_kit::test]
async fn changed_id_disables_review(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(
        cx,
        r#"{"id": "4", "title": "t3", "embedding": [0.25, 0.5]}"#,
    );
    assert!(f.review_disabled(cx));
    assert!(
        f.text_of(cx, "inspector.review.reason")
            .contains("New document")
    );
}

#[gpui_kit::test]
async fn parse_error_disables_review(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, "{");
    assert!(f.review_disabled(cx));
    assert!(f.dirty(cx));
    f.edit(cx, "[1]");
    assert!(f.review_disabled(cx));
    assert!(f.has(cx, "inspector.review.reason"));
}

#[gpui_kit::test]
async fn review_right_after_typing_sees_the_last_text(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    let inspector = f.inspector(cx);
    let editor = cx.update(|cx| inspector.read(cx).editor().clone());
    f.act(cx, |window, cx| {
        editor.update(cx, |e, cx| e.set_text(RENAMED, window, cx));
        window.press(SAVE, cx);
    });
    f.settle(cx);
    assert!(f.has(cx, "review.back"), "no wait for the debounce");
}

#[gpui_kit::test]
async fn stale_document_warns_and_diffs_against_fresh(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    *f.docs.stored.lock().unwrap() =
        Some(json!({"id": "3", "title": "theirs", "embedding": [0.25, 0.5]}));
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(f.has(cx, "review.stale"));
    assert!(f.text_of(cx, "review.diff.title").contains("theirs"));
}

#[gpui_kit::test]
async fn review_of_a_deleted_document_shows_the_error_inline(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    *f.docs.document_fail.lock().unwrap() = Some((404, "gone".into()));
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(f.text_of(cx, "inspector.save_error").contains("gone"));
    assert!(!f.has(cx, "review.back"));
}

#[gpui_kit::test]
async fn locked_profile_opens_unlock_and_sends_nothing(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    f.open_document(cx);
    assert!(f.has(cx, "inspector.review.readonly"));
    f.edit(cx, RENAMED);
    let fetched = f.docs.document_calls.lock().unwrap().len();
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(f.has(cx, "danger.input"));
    assert_eq!(f.docs.document_calls.lock().unwrap().len(), fetched);
    assert!(f.writes().is_empty());
    f.click(cx, "danger.input");
    f.act(cx, |window, cx| window.input("test", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    wait_for_element(cx, f.handle, "review.back".into(), true).await;
    assert!(f.writes().is_empty());
}

#[gpui_kit::test]
async fn read_only_profile_keeps_the_editor_editable(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    f.open_document(cx);
    f.act(cx, |window, cx| window.input("x", cx));
    assert!(f.inspector_text(cx).contains('x'));
}

#[gpui_kit::test]
async fn wrong_type_is_marked_but_does_not_block(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(
        cx,
        r#"{"id": "3", "title": 5, "price": 1.0, "embedding": [0.25, 0.5]}"#,
    );
    let issues = f.text_of(cx, "inspector.issues");
    assert!(issues.contains("title"), "{issues}");
    assert!(!f.review_disabled(cx));
    let inspector = f.inspector(cx);
    let marks = cx.update(|cx| inspector.read(cx).editor().read(cx).mark_count(cx));
    assert!(marks >= 1);
}

#[gpui_kit::test]
async fn dirty_inspector_does_not_follow(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    assert!(f.has(cx, "inspector.dirty"));
    f.select_row(cx, 5);
    f.follow(cx);
    assert_eq!(f.inspector_id(cx).as_deref(), Some("3"));
    assert!(f.inspector_text(cx).contains("\"new\""));
}

#[gpui_kit::test]
async fn closing_dirty_asks(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.settle(cx);
    f.act(cx, |window, cx| window.press("enter", cx));
    wait_for_element(cx, f.handle, "confirm.ok".into(), false).await;
    // The kit gives the focus back once the dialog's exit has played.
    cx.executor().advance_clock(Duration::from_secs(1));
    f.settle(cx);
    assert!(f.has(cx, "documents.inspector"), "focus was on Cancel");
    assert!(f.dirty(cx));

    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.settle(cx);
    f.click(cx, "confirm.ok");
    wait_for_element(cx, f.handle, "confirm.ok".into(), false).await;
    f.settle(cx);
    assert!(!f.has(cx, "documents.inspector"));
    assert!(!f.dirty(cx));
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.settle(cx);
    assert!(f.has(cx, "documents.inspector"), "the keyboard still works");
}

#[gpui_kit::test]
async fn go_to_while_dirty_asks_first(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.go_to(cx, "7");
    f.settle(cx);
    assert!(f.has(cx, "confirm.ok"));
    assert!(f.inspector_text(cx).contains("\"new\""));
    f.click(cx, "confirm.ok");
    f.settle(cx);
    assert_eq!(f.inspector_id(cx).as_deref(), Some("7"));
}

#[gpui_kit::test]
async fn revert_restores(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    let original = f.inspector_text(cx);
    f.edit(cx, RENAMED);
    f.act(cx, |window, cx| window.press(REVERT, cx));
    f.settle(cx);
    assert_eq!(f.inspector_text(cx), original);
    assert!(!f.has(cx, "inspector.dirty"));
    assert!(!f.has(cx, "inspector.revert"), "nothing left to revert");
}

#[gpui_kit::test]
async fn new_document_uses_the_template(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.act(cx, |window, cx| window.press(NEW_DOCUMENT, cx));
    f.settle(cx);
    let text = f.inspector_text(cx);
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc, json!({"title": "", "price": 0.0}));
    assert!(!f.has(cx, "inspector.save_mode.patch"));
    assert!(!f.review_disabled(cx), "the template can be saved as it is");
    assert_eq!(f.text_of(cx, "documents.inspector.heading"), "New document");
}

#[gpui_kit::test]
async fn new_document_button_opens_the_draft(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.click(cx, "documents.new");
    f.settle(cx);
    assert!(f.inspector_text(cx).contains("title"));
}

#[gpui_kit::test]
async fn create_409_keeps_the_draft_inline(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.write_fail.lock().unwrap() = Some((409, "already exists".into()));
    f.act(cx, |window, cx| window.press(NEW_DOCUMENT, cx));
    f.settle(cx);
    f.edit(cx, r#"{"id": "1", "title": "x", "price": 1.0}"#);
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(
        f.text_of(cx, "inspector.save_error")
            .contains("already exists")
    );
    assert!(f.inspector_text(cx).contains("\"x\""));
    assert!(f.has(cx, "inspector.review"));
    f.act(cx, |window, _| {
        assert_eq!(label(window, "documents.inspector.heading"), "New document");
    });
}

#[gpui_kit::test]
async fn create_reloads_the_count(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.act(cx, |window, cx| window.press(NEW_DOCUMENT, cx));
    f.settle(cx);
    f.edit(cx, r#"{"title": "x", "price": 1.0}"#);
    f.click(cx, "inspector.review");
    f.settle(cx);
    assert_eq!(f.writes()[0].0, "create");
    assert!(!f.has(cx, "review.back"), "no review for a create");
    assert!(f.text_of(cx, "documents.status").starts_with("11 of"));
    assert_eq!(f.inspector_id(cx).as_deref(), Some("new"));
    assert!(
        f.has(cx, "inspector.save_mode.patch"),
        "now an existing document"
    );
    assert!(!f.dirty(cx));
}

#[gpui_kit::test]
async fn create_on_a_locked_profile_asks_first(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    f.act(cx, |window, cx| window.press(NEW_DOCUMENT, cx));
    f.settle(cx);
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(f.has(cx, "danger.input"));
    assert!(f.writes().is_empty());
}

#[gpui_kit::test]
async fn vector_survives_a_patch_edit(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    f.click(cx, "review.apply");
    f.settle(cx);
    let (_, _, body) = f.writes().pop().unwrap();
    assert!(body.get("embedding").is_none(), "{body}");
}

#[gpui_kit::test]
async fn second_press_of_the_review_chord_does_not_apply(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.review_renamed(cx);
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(f.writes().is_empty());
    assert!(f.has(cx, "review.back"));
}

#[gpui_kit::test]
async fn go_to_a_missing_id_keeps_the_draft_without_asking(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    *f.docs.document_fail.lock().unwrap() = Some((404, "no such id".into()));
    f.go_to(cx, "7");
    assert!(!f.has(cx, "confirm.ok"));
    assert!(f.text_of(cx, "documents.goto.error").contains("no such id"));
    assert!(f.inspector_text(cx).contains("\"new\""));
    assert_eq!(f.inspector_id(cx).as_deref(), Some("3"));
}

#[gpui_kit::test]
async fn back_from_a_stale_review_keeps_the_fresh_document_as_base(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    *f.docs.stored.lock().unwrap() =
        Some(json!({"id": "3", "title": "theirs", "embedding": [0.25, 0.5]}));
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    f.act(cx, |window, cx| window.press("escape", cx));
    wait_for_element(cx, f.handle, "review.back".into(), false).await;
    cx.executor().advance_clock(Duration::from_secs(1));
    f.settle(cx);
    f.act(cx, |window, cx| window.press(REVERT, cx));
    f.settle(cx);
    assert!(f.inspector_text(cx).contains("theirs"));
    assert!(!f.dirty(cx));
}

#[gpui_kit::test]
async fn draft_equal_to_the_fresh_document_opens_no_review(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    *f.docs.stored.lock().unwrap() = Some(serde_json::from_str(RENAMED).unwrap());
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    assert!(!f.has(cx, "review.back"));
    assert!(f.text_of(cx, "inspector.save_error").contains("already"));
    assert!(!f.dirty(cx));
}

#[gpui_kit::test]
async fn nothing_drops_a_create_in_flight(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.act(cx, |window, cx| window.press(NEW_DOCUMENT, cx));
    f.settle(cx);
    f.edit(cx, r#"{"title": "x", "price": 1.0}"#);
    let hold = f.docs.hold_next();
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    f.act(cx, |window, cx| window.press(REVERT, cx));
    f.act(cx, |window, cx| window.press(INSPECTOR, cx));
    f.act(cx, |window, cx| window.press(CLOSE_TAB, cx));
    f.settle(cx);
    assert!(!f.has(cx, "confirm.ok"), "no discard while it runs");
    hold.send(()).unwrap();
    f.settle(cx);
    assert_eq!(f.inspector_id(cx).as_deref(), Some("new"));
    assert!(f.text_of(cx, "documents.status").starts_with("11 of"));
}

#[gpui_kit::test]
async fn revert_during_a_load_leaves_it_running(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    let hold = f.docs.hold_next();
    f.select_row(cx, 5);
    f.follow(cx);
    let inspector = f.inspector(cx);
    f.act(cx, |window, cx| {
        inspector.update(cx, |i, cx| i.revert(window, cx));
    });
    hold.send(()).unwrap();
    f.settle(cx);
    assert_eq!(f.inspector_id(cx).as_deref(), Some("5"));
    assert!(f.inspector_text(cx).contains("t5"));
}

#[gpui_kit::test]
async fn close_tab_while_dirty_asks(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    let id = f.profile_id(cx);
    let tab = format!("tab.{id}.collection.products");
    f.act(cx, |window, cx| window.press(CLOSE_TAB, cx));
    f.settle(cx);
    assert!(f.has(cx, "confirm.ok"));
    assert!(f.dirty(cx));
    f.click(cx, "confirm.ok");
    wait_for_element(cx, f.handle, tab, false).await;
}

#[gpui_kit::test]
async fn close_tab_while_dirty_asks_on_a_lone_tab(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.leave_alone(cx);
    f.open_document(cx);
    f.edit(cx, RENAMED);
    let id = f.profile_id(cx);
    let tab = format!("tab.{id}.collection.products");
    f.act(cx, |window, cx| window.press(CLOSE_TAB, cx));
    f.settle(cx);
    assert!(f.has(cx, "confirm.ok"));
    f.click(cx, "confirm.ok");
    wait_for_element(cx, f.handle, tab, false).await;
}

#[gpui_kit::test]
async fn a_clean_lone_tab_closes(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.leave_alone(cx);
    let id = f.profile_id(cx);
    f.press(cx, CLOSE_TAB);
    let tab = format!("tab.{id}.collection.products");
    wait_for_element(cx, f.handle, tab, false).await;
}

#[gpui_kit::test]
async fn a_lone_tab_waits_for_a_running_delete(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.leave_alone(cx);
    f.tick(cx, "1");
    f.press(cx, DELETE);
    let hold = f.docs.hold_next();
    f.confirm_delete(cx);
    f.press(cx, CLOSE_TAB);
    let id = f.profile_id(cx);
    let tab = format!("tab.{id}.collection.products");
    assert!(f.act(cx, |window, _| window.try_find(tab.clone()).is_some()));
    hold.send(()).unwrap();
    f.settle(cx);
}

#[gpui_kit::test]
async fn unsaved_changes_veto_the_kits_close(cx: &mut TestAppContext) {
    use gpui_kit::component::dock::BasePanel as _;
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    let panel = f.panel(cx);
    cx.update(|cx| assert!(panel.read(cx).closable(cx)));
    f.edit(cx, RENAMED);
    cx.update(|cx| assert!(!panel.read(cx).closable(cx)));
    f.act(cx, |window, cx| window.press(REVERT, cx));
    f.settle(cx);
    cx.update(|cx| assert!(panel.read(cx).closable(cx)));
}

#[gpui_kit::test]
async fn checkbox_toggles_and_does_not_select_the_cell(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let table = f.table(cx);
    f.tick(cx, "3");
    assert_eq!(f.ticked(cx), ["3"]);
    assert_eq!(cx.update(|cx| table.read(cx).selected_cell()), None);
    assert!(f.text_of(cx, "documents.selected").starts_with('1'));
    f.tick(cx, "3");
    assert!(f.ticked(cx).is_empty());
    assert!(!f.has(cx, "documents.selected"));
}

#[gpui_kit::test]
async fn shift_click_selects_the_range(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "5");
    f.act(cx, |window, cx| {
        window.click_with_modifiers("documents.check.2", gpui_kit::Modifiers::shift(), cx)
    });
    assert_eq!(f.ticked(cx), ["2", "3", "4", "5"]);
}

#[gpui_kit::test]
async fn range_over_unloaded_rows_says_so(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 100_000).await;
    f.tick(cx, "0");
    f.scroll_to(cx, 50_000);
    f.settle(cx);
    let table = f.table(cx);
    f.act(cx, |window, cx| {
        table.read(cx).focus_handle(cx).focus(window, cx);
        window.click_with_modifiers("documents.check.50000", gpui_kit::Modifiers::shift(), cx)
    });
    assert!(f.has(cx, "documents.selection_unloaded"));
    assert!(f.ticked(cx).len() < 1_000);
    assert!(f.ticked(cx).contains(&"50000".to_string()));
}

#[gpui_kit::test]
async fn space_toggles_the_selected_cell_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 4);
    f.press(cx, "space");
    assert_eq!(f.ticked(cx), ["4"]);
    f.press(cx, "space");
    assert!(f.ticked(cx).is_empty());
    f.press(cx, "space");
    f.select_row(cx, 6);
    f.press(cx, "shift-space");
    assert_eq!(f.ticked(cx), ["4", "5", "6"]);
}

#[gpui_kit::test]
async fn space_stays_text_in_the_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 4);
    f.type_filter(cx, "a");
    f.press(cx, "space");
    assert!(f.ticked(cx).is_empty());
}

#[gpui_kit::test]
async fn command_a_selects_loaded_rows(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 900).await;
    f.select_row(cx, 0);
    f.press(cx, SELECT_ALL);
    let table = f.table(cx);
    let pages = cx.update(|cx| table.read(cx).delegate().cached_pages());
    assert!(pages < 4, "{pages}");
    assert_eq!(f.ticked(cx).len(), pages * 250);
}

#[gpui_kit::test]
async fn escape_on_the_table_clears_the_selection(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 4);
    f.press(cx, "space");
    f.press(cx, "escape");
    assert_eq!(f.ticked(cx), ["4"], "the kit clears the cell first");
    f.press(cx, "escape");
    assert!(f.ticked(cx).is_empty());
}

#[gpui_kit::test]
async fn selection_cap_stops_at_one_thousand_with_a_note(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 2_000).await;
    f.load_rows(cx, 2_000);
    f.select_row(cx, 1_999);
    f.press(cx, SELECT_ALL);
    assert_eq!(f.ticked(cx).len(), 1_000);
    assert!(f.text_of(cx, "documents.selection_limit").contains("1000"));
    let before = f.ticked(cx);
    let outside = (0..2_000)
        .map(|n| n.to_string())
        .find(|id| !before.contains(id))
        .unwrap();
    let row: usize = outside.parse().unwrap();
    f.scroll_to(cx, row);
    f.settle(cx);
    f.tick(cx, &outside);
    assert_eq!(f.ticked(cx), before, "adds nothing more");
    let back = before.last().unwrap().clone();
    f.scroll_to(cx, back.parse().unwrap());
    f.settle(cx);
    f.tick(cx, &back);
    assert_eq!(f.ticked(cx).len(), 999);
    assert!(!f.has(cx, "documents.selection_limit"));
}

#[gpui_kit::test]
async fn selection_survives_scrolling_out_of_the_cache(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 200_000).await;
    f.tick(cx, "3");
    for jump in 1..=40 {
        f.scroll_to(cx, jump * 300);
        f.settle(cx);
    }
    assert_eq!(f.ticked(cx), ["3"]);
    f.select_row(cx, 12_000);
    f.settle(cx);
    f.press(cx, DELETE);
    assert!(f.text_of(cx, "confirm.body").contains("3"));
    assert!(f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn new_filter_clears_the_selection(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "3");
    f.submit_filter(cx, "price:>1");
    assert!(f.ticked(cx).is_empty());
    f.tick(cx, "3");
    f.sort_header(cx, 2);
    f.settle(cx);
    assert!(f.ticked(cx).is_empty());
    f.tick(cx, "3");
    f.press(cx, CLEAR_FILTER);
    assert!(f.ticked(cx).is_empty());
}

#[gpui_kit::test]
async fn ticks_made_while_a_new_filter_runs_do_not_survive_it(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let hold = f.docs.hold_next();
    f.submit_filter(cx, "price:>1");
    f.tick(cx, "3");
    assert_eq!(f.ticked(cx), ["3"]);
    hold.send(()).unwrap();
    f.settle(cx);
    assert!(f.ticked(cx).is_empty());
}

#[gpui_kit::test]
async fn refresh_keeps_the_selection(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "3");
    f.select_row(cx, 3);
    f.press(cx, REFRESH);
    assert_eq!(f.ticked(cx), ["3"]);
}

#[gpui_kit::test]
async fn delete_key_in_documents_does_not_open_collection_delete(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    assert!(!f.has(cx, "danger.input"));
    let body = f.text_of(cx, "confirm.body");
    assert!(body.contains("document 3"), "{body}");
    f.click(cx, "confirm.cancel");
    f.settle(cx);
    assert!(f.writes().is_empty());
    assert_eq!(f.backend.deletes.lock().unwrap().len(), 0);
}

#[gpui_kit::test]
async fn nothing_to_delete_disables_the_button(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    assert!(f.act(cx, |window, _| {
        window.find("documents.delete").disabled() == Some(true)
    }));
    f.press(cx, DELETE);
    assert!(!f.has(cx, "confirm.ok"));
    f.tick(cx, "3");
    assert!(f.act(cx, |window, _| {
        window.find("documents.delete").disabled() != Some(true)
    }));
    f.click(cx, "documents.delete");
    f.settle(cx);
    assert!(f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn delete_button_shows_its_shortcut(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let button = f.act(cx, |window, _| window.find("documents.delete").bounds());
    let chord = key("kbd:cmd-backspace", "kbd:ctrl-backspace");
    let kbd = gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
        .debug_bounds(chord)
        .unwrap();
    assert!(button.contains(&kbd.center()));
}

#[gpui_kit::test]
async fn single_delete_uses_the_document_endpoint(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    let writes = f.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        (writes[0].0.as_str(), writes[0].1.as_str()),
        ("delete", "3")
    );
}

#[gpui_kit::test]
async fn single_delete_of_a_gone_document_counts_as_done(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.write_fail.lock().unwrap() = Some((404, "no such document".into()));
    f.tick(cx, "3");
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    assert!(!f.has(cx, "documents.delete.error"));
    assert!(f.ticked(cx).is_empty());
}

#[gpui_kit::test]
async fn multi_delete_builds_one_id_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "1");
    f.tick(cx, "2");
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    let writes = f.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, "delete_by_filter");
    assert_eq!(writes[0].1, "id:[`1`,`2`]");
    assert!(f.ticked(cx).is_empty());
}

/// Ticks `ticks` documents with `confirm_above` of 5 and presses Delete.
async fn delete_with_confirm_above_5(cx: &mut TestAppContext, ticks: u32) -> Fixture {
    let f = open_found(cx, plain_schema(), 10).await;
    let settings = tessera_core::settings::Settings {
        confirm_above: 5,
        ..Default::default()
    };
    f.act(cx, |window, cx| {
        tessera_app::settings::apply(&settings, window, cx)
    });
    for id in 1..=ticks {
        f.tick(cx, &id.to_string());
    }
    f.press(cx, DELETE);
    f
}

#[gpui_kit::test]
async fn confirm_above_of_5_leaves_a_delete_of_5_plain(cx: &mut TestAppContext) {
    let f = delete_with_confirm_above_5(cx, 5).await;
    assert!(f.has(cx, "confirm.ok"));
    assert!(!f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn confirm_above_of_5_makes_a_delete_of_6_typed(cx: &mut TestAppContext) {
    let f = delete_with_confirm_above_5(cx, 6).await;
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
}

#[gpui_kit::test]
async fn a_new_documents_view_pages_by_the_setting(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 100).await;
    let id = f.profile_id(cx);
    let mut other = plain_schema();
    other.name = "other".into();
    *f.backend.data.lock().unwrap() = Some((vec![plain_schema(), other], vec![]));
    cx.update(|cx| {
        f.state.update(cx, |s, cx| {
            s.settings.per_page = 20;
            s.open_collection(id, "other".into(), cx);
        })
    });
    wait_for_element(cx, f.handle, "documents.columns".into(), true).await;
    f.settle(cx);
    assert_eq!(f.docs.pages.lock().unwrap().last().unwrap().per_page, 20);
}

#[gpui_kit::test]
async fn long_selection_is_batched(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 1_000;
    *docs.ids.lock().unwrap() = (0..1_000).map(|n| format!("{n:032}")).collect();
    let f = open_tab(cx, plain_schema(), docs).await;
    f.load_rows(cx, 1_000);
    f.select_row(cx, 999);
    f.press(cx, SELECT_ALL);
    assert_eq!(f.ticked(cx).len(), 1_000);
    f.press(cx, DELETE);
    assert!(f.has(cx, "confirm.ok"), "a thousand is still medium danger");
    f.confirm_delete(cx);
    let writes = f.writes();
    assert!(writes.len() > 1);
    assert!(writes.iter().all(|w| w.0 == "delete_by_filter"));
    let ids: usize = writes.iter().map(|w| w.1.matches('`').count() / 2).sum();
    assert_eq!(ids, 1_000);
}

#[gpui_kit::test]
async fn backtick_id_goes_alone(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 3;
    *docs.ids.lock().unwrap() = ["a", "x`y", "b"].map(String::from).to_vec();
    let f = open_tab(cx, plain_schema(), docs).await;
    f.select_row(cx, 0);
    f.press(cx, SELECT_ALL);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    let writes = f.writes();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].1, "id:[`a`,`b`]");
    assert_eq!(
        (writes[1].0.as_str(), writes[1].1.as_str()),
        ("delete", "x`y")
    );
}

#[gpui_kit::test]
async fn failed_backtick_id_is_reported(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 3;
    *docs.ids.lock().unwrap() = ["a", "x`y", "b"].map(String::from).to_vec();
    let f = open_tab(cx, plain_schema(), docs).await;
    *f.docs.write_fail.lock().unwrap() = Some((500, "boom".into()));
    *f.docs.pass_deletes.lock().unwrap() = 1;
    f.select_row(cx, 0);
    f.press(cx, SELECT_ALL);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    let text = f.text_of(cx, "documents.delete.error");
    assert!(
        text.contains("Deleted 2 of 3") && text.contains("boom"),
        "{text}"
    );
    assert!(f.ticked(cx).contains(&"x`y".to_string()));
}

#[gpui_kit::test]
async fn partial_failure_reports_and_reloads(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 1_000;
    *docs.ids.lock().unwrap() = (0..1_000).map(|n| format!("{n:032}")).collect();
    let f = open_tab(cx, plain_schema(), docs).await;
    f.load_rows(cx, 1_000);
    f.select_row(cx, 999);
    f.press(cx, SELECT_ALL);
    *f.docs.write_fail.lock().unwrap() = Some((503, "not ready".into()));
    *f.docs.pass_deletes.lock().unwrap() = 1;
    f.press(cx, DELETE);
    let asked = f.docs.pages.lock().unwrap().len();
    f.confirm_delete(cx);
    assert_eq!(f.writes().len(), 2, "stops at the first failure");
    let text = f.text_of(cx, "documents.delete.error");
    assert!(
        text.contains("of 1,000") && text.contains("not ready"),
        "{text}"
    );
    assert!(f.docs.pages.lock().unwrap().len() > asked);
    assert_eq!(f.ticked(cx).len(), 1_000);
}

async fn open_prod(cx: &mut TestAppContext, found: u64) -> Fixture {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = found;
    let mut profile = tessera_core::profile::Profile::new("test", vec![node()]);
    profile.environment = tessera_core::profile::Environment::Prod;
    open_as(cx, plain_schema(), docs, profile).await
}

#[gpui_kit::test]
async fn prod_needs_the_collection_name(cx: &mut TestAppContext) {
    let f = open_prod(cx, 10).await;
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    assert!(f.has(cx, "danger.prod"));
    assert!(!f.has(cx, "confirm.ok"));
    f.act(cx, |window, _| {
        assert_eq!(window.find("danger.input").focused(), Some(true));
        assert!(label(window, "danger.consequence.0").contains("document 3"));
    });
    f.act(cx, |window, cx| window.input("products", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    wait_for_element(cx, f.handle, "danger.ok".into(), false).await;
    assert_eq!(f.writes()[0].0, "delete");
}

#[gpui_kit::test]
async fn failure_on_prod_stays_in_the_dialog(cx: &mut TestAppContext) {
    let f = open_prod(cx, 10).await;
    *f.docs.write_fail.lock().unwrap() = Some((500, "boom".into()));
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    f.act(cx, |window, cx| window.input("products", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    let text = f.text_of(cx, "danger.error");
    assert!(
        text.contains("Deleted 0 of 1") && text.contains("boom"),
        "{text}"
    );
}

#[gpui_kit::test]
async fn locked_profile_opens_unlock_first(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    assert!(f.has(cx, "documents.delete.readonly"));
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    assert!(f.has(cx, "danger.input"));
    assert!(!f.has(cx, "confirm.ok"));
    f.click(cx, "danger.input");
    f.act(cx, |window, cx| window.input("test", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    wait_for_element(cx, f.handle, "confirm.ok".into(), true).await;
    assert!(f.writes().is_empty());
    f.confirm_delete(cx);
    assert_eq!(f.writes().len(), 1);
}

#[gpui_kit::test]
async fn read_only_sends_nothing(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    f.tick(cx, "3");
    f.click(cx, "documents.delete");
    f.settle(cx);
    assert!(f.has(cx, "danger.input"));
    f.press(cx, "escape");
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    assert!(f.writes().is_empty());
    assert_eq!(f.ticked(cx), ["3"]);
}

#[gpui_kit::test]
async fn deleted_row_closes_the_inspector(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    assert!(f.has(cx, "documents.inspector"));
    f.click(cx, "documents.delete");
    f.settle(cx);
    f.confirm_delete(cx);
    assert!(!f.has(cx, "documents.inspector"));
}

#[gpui_kit::test]
async fn inspector_on_another_row_stays_open(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.tick(cx, "5");
    f.click(cx, "documents.delete");
    f.settle(cx);
    f.confirm_delete(cx);
    assert!(f.has(cx, "documents.inspector"));
    assert_eq!(f.inspector_id(cx).as_deref(), Some("3"));
}

#[gpui_kit::test]
async fn draft_of_a_doomed_row_is_named_and_goes_only_with_the_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.delete");
    f.settle(cx);
    let body = f.text_of(cx, "confirm.body");
    assert!(
        body.contains("document 3") && body.contains("unsaved"),
        "{body}"
    );
    f.click(cx, "confirm.cancel");
    wait_for_element(cx, f.handle, "confirm.ok".into(), false).await;
    cx.executor().advance_clock(Duration::from_secs(1));
    f.settle(cx);
    assert!(f.dirty(cx), "cancelling changes nothing");
    assert!(f.inspector_text(cx).contains("\"new\""));

    f.click(cx, "documents.delete");
    f.settle(cx);
    f.confirm_delete(cx);
    assert_eq!(f.writes()[0].0, "delete");
    // One question: a second one would keep it from going.
    wait_for_element(cx, f.handle, "confirm.ok".into(), false).await;
    assert!(!f.has(cx, "documents.inspector"));
    assert!(!f.dirty(cx));
}

#[gpui_kit::test]
async fn failed_delete_keeps_the_draft(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.write_fail.lock().unwrap() = Some((500, "boom".into()));
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.delete");
    f.settle(cx);
    f.confirm_delete(cx);
    assert!(f.text_of(cx, "documents.delete.error").contains("boom"));
    assert!(f.dirty(cx));
    assert!(f.has(cx, "documents.inspector"));
}

#[gpui_kit::test]
async fn cancelling_the_unlock_keeps_the_draft(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 10;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.delete");
    f.settle(cx);
    assert!(f.has(cx, "danger.input"));
    f.click(cx, "danger.cancel");
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    assert!(f.dirty(cx));
    assert!(f.writes().is_empty());
}

#[gpui_kit::test]
async fn draft_on_prod_is_a_consequence(cx: &mut TestAppContext) {
    let f = open_prod(cx, 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.delete");
    f.settle(cx);
    assert!(f.text_of(cx, "danger.consequence.1").contains("unsaved"));
}

#[gpui_kit::test]
async fn busy_inspector_says_why_nothing_opens(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    let hold = f.docs.hold_next();
    f.act(cx, |window, cx| window.press(SAVE, cx));
    f.settle(cx);
    f.click(cx, "documents.delete");
    f.settle(cx);
    assert!(!f.has(cx, "confirm.ok"));
    assert!(
        f.text_of(cx, "documents.delete.error")
            .contains("inspector")
    );
    drop(hold);
}

#[gpui_kit::test]
async fn short_filter_count_reports_and_keeps_the_ticks(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.filter_short.lock().unwrap() = 1;
    f.open_document(cx);
    f.tick(cx, "3");
    f.tick(cx, "4");
    f.click(cx, "documents.delete");
    f.settle(cx);
    f.confirm_delete(cx);
    let text = f.text_of(cx, "documents.delete.error");
    assert!(text.contains("Deleted 1 of 2"), "{text}");
    assert_eq!(f.ticked(cx), ["3", "4"]);
    assert!(f.has(cx, "documents.inspector"));
}

#[gpui_kit::test]
async fn star_and_spaced_ids_never_reach_a_filter(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 4;
    *docs.ids.lock().unwrap() = ["*", "keep1 ", " lead", "plain"].map(String::from).to_vec();
    let f = open_tab(cx, plain_schema(), docs).await;
    f.select_row(cx, 0);
    f.press(cx, SELECT_ALL);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    let writes = f.writes();
    assert_eq!(writes[0].1, "id:[`plain`]");
    let single: Vec<_> = writes[1..]
        .iter()
        .map(|w| (w.0.as_str(), w.1.as_str()))
        .collect();
    assert_eq!(
        single,
        [("delete", " lead"), ("delete", "*"), ("delete", "keep1 ")]
    );
}

#[gpui_kit::test]
async fn a_lone_dot_id_goes_through_a_filter(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 2;
    *docs.ids.lock().unwrap() = ["..", "a"].map(String::from).to_vec();
    let f = open_tab(cx, plain_schema(), docs).await;
    f.select_row(cx, 0);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    let writes = f.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].1, "id:[`..`]");
    assert_eq!(writes[0].0, "delete_by_filter");
}

#[gpui_kit::test]
async fn delete_waits_for_a_new_query(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 3);
    let hold = f.docs.hold_next();
    f.submit_filter(cx, "price:>1");
    f.tick(cx, "4");
    f.press(cx, DELETE);
    assert!(!f.has(cx, "confirm.ok"));
    assert!(f.act(cx, |window, _| {
        window.find("documents.delete").disabled() == Some(true)
    }));
    hold.send(()).unwrap();
    f.settle(cx);
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    assert!(f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn delete_works_again_after_a_refused_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.fail.lock().unwrap() = Some((400, "bad filter".into()));
    f.submit_filter(cx, "nope:=1");
    *f.docs.fail.lock().unwrap() = None;
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    assert!(f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn delete_error_goes_with_the_next_query_and_masks_nothing(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    *f.docs.write_fail.lock().unwrap() = Some((500, "boom".into()));
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    assert!(f.text_of(cx, "documents.delete.error").contains("boom"));
    *f.docs.fail.lock().unwrap() = Some((500, "pages down".into()));
    f.scroll_to(cx, 900);
    f.settle(cx);
    assert!(f.text_of(cx, "documents.error").contains("pages down"));
    assert!(f.text_of(cx, "documents.delete.error").contains("boom"));

    *f.docs.fail.lock().unwrap() = None;
    f.press(cx, REFRESH);
    assert!(!f.has(cx, "documents.delete.error"), "refresh clears it");
    f.select_row(cx, 3);
    f.press(cx, DELETE);
    f.confirm_delete(cx);
    assert!(f.has(cx, "documents.delete.error"));
    f.submit_filter(cx, "price:>1");
    assert!(
        !f.has(cx, "documents.delete.error"),
        "a new query clears it"
    );
}

#[gpui_kit::test]
async fn draft_of_another_row_is_left_alone(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.tick(cx, "5");
    f.click(cx, "documents.delete");
    f.settle(cx);
    assert!(f.text_of(cx, "confirm.body").contains("document 5"));
    f.confirm_delete(cx);
    assert!(f.dirty(cx));
    assert!(f.has(cx, "documents.inspector"));
}

#[gpui_kit::test]
async fn closing_the_tab_waits_for_a_running_delete(cx: &mut TestAppContext) {
    use gpui_kit::component::dock::BasePanel as _;
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "1");
    f.tick(cx, "2");
    f.press(cx, DELETE);
    let hold = f.docs.hold_next();
    f.confirm_delete(cx);
    let panel = f.panel(cx);
    cx.update(|cx| assert!(!panel.read(cx).closable(cx)));
    f.act(cx, |window, _| {
        assert_eq!(window.find("documents.delete").disabled(), Some(true));
    });
    f.press(cx, DELETE);
    assert!(!f.has(cx, "confirm.ok"), "one delete at a time");
    f.press(cx, CLOSE_TAB);
    let id = f.profile_id(cx);
    let tab = format!("tab.{id}.collection.products");
    assert!(f.act(cx, |window, _| window.try_find(tab.clone()).is_some()));
    hold.send(()).unwrap();
    f.settle(cx);
    cx.update(|cx| assert!(panel.read(cx).closable(cx)));
    assert!(f.ticked(cx).is_empty());
}

#[gpui_kit::test]
async fn count_refreshes_after_delete(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "1");
    f.tick(cx, "2");
    f.press(cx, DELETE);
    let listed = *f.backend.lists.lock().unwrap();
    f.confirm_delete(cx);
    assert!(f.text_of(cx, "documents.status").starts_with("8 of"));
    assert!(
        *f.backend.lists.lock().unwrap() > listed,
        "the list reloads"
    );
}

#[gpui_kit::test]
async fn delete_query_starts_with_the_table_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.submit_filter(cx, "price:>1");
    f.press(cx, DELETE_QUERY);
    f.wait(cx, 300);
    assert_eq!(f.counts(), [Some("price:>1".to_string())]);
    assert!(f.text_of(cx, "bulk.count").contains("10"));
    // The focus is in the filter.
    f.type_bulk_filter(cx, " && price:<9");
    f.wait(cx, 300);
    assert_eq!(f.counts()[1].as_deref(), Some("price:>1 && price:<9"));
}

#[gpui_kit::test]
async fn bulk_buttons_open_their_dialogs(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.click(cx, "documents.delete_query");
    f.settle(cx);
    assert!(!f.has(cx, "bulk.editor"));
    f.click(cx, "bulk.cancel");
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.click(cx, "documents.update_query");
    f.settle(cx);
    assert!(f.has(cx, "bulk.editor"));
}

#[gpui_kit::test]
async fn bulk_waits_for_a_new_query(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    let hold = f.docs.hold_next();
    f.type_filter(cx, "price:>1");
    f.act(cx, |window, cx| window.press("enter", cx));
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    assert!(f.act(cx, |window, _| {
        window.find("documents.delete_query").disabled() == Some(true)
            && window.find("documents.update_query").disabled() == Some(true)
    }));
    f.press(cx, DELETE_QUERY);
    assert!(!f.has(cx, "bulk.continue"));
    drop(hold);
    f.settle(cx);
    f.press(cx, DELETE_QUERY);
    assert!(f.has(cx, "bulk.continue"));
}

#[gpui_kit::test]
async fn count_shows_after_the_debounce(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 7).await;
    f.press(cx, DELETE_QUERY);
    f.type_bulk_filter(cx, "price:>1");
    assert!(f.text_of(cx, "bulk.count").contains("Counting"));
    f.wait(cx, 200);
    assert!(f.counts().is_empty());
    f.wait(cx, 100);
    assert_eq!(f.counts().len(), 1);
    assert!(f.text_of(cx, "bulk.count").contains("Affects 7"));
}

#[gpui_kit::test]
async fn newer_filter_replaces_a_slow_count(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 7).await;
    f.press(cx, DELETE_QUERY);
    f.type_bulk_filter(cx, "price:>1");
    let hold = f.docs.hold_next();
    f.wait(cx, 300);
    *f.docs.found.lock().unwrap() = 3;
    f.type_bulk_filter(cx, "0");
    assert!(f.text_of(cx, "bulk.count").contains("Counting"));
    f.wait(cx, 300);
    assert!(f.text_of(cx, "bulk.count").contains("Affects 3"));
    let _ = hold.send(());
    f.settle(cx);
    assert!(f.text_of(cx, "bulk.count").contains("Affects 3"));
    assert_eq!(f.counts().len(), 2);
}

#[gpui_kit::test]
async fn empty_filter_disables_continue(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 7).await;
    f.press(cx, DELETE_QUERY);
    f.wait(cx, 300);
    assert!(f.continue_disabled(cx));
    assert!(f.text_of(cx, "bulk.count").contains("Truncate"));
    f.type_bulk_filter(cx, "   ");
    f.wait(cx, 300);
    assert!(f.continue_disabled(cx));
    assert!(f.counts().is_empty(), "a blank filter counts everything");
}

#[gpui_kit::test]
async fn a_filter_for_everything_never_reaches_the_server(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 7).await;
    for (shortcut, filter) in [
        (DELETE_QUERY, "*"),
        (UPDATE_QUERY, "*"),
        (UPDATE_QUERY, " * "),
        (DELETE_QUERY, "id:*"),
        (UPDATE_QUERY, "id:=[a,*]"),
        (UPDATE_QUERY, "   "),
    ] {
        f.bulk(cx, shortcut, filter);
        if shortcut == UPDATE_QUERY {
            f.type_partial(cx, r#"{"title":"x"}"#);
        }
        assert!(f.continue_disabled(cx), "{filter:?}");
        assert!(f.text_of(cx, "bulk.count").contains("Enter a filter"));
        f.act(cx, |window, cx| window.press(SAVE, cx));
        f.settle(cx);
        assert!(!f.has(cx, "confirm.ok"), "{filter:?}");
        f.click(cx, "bulk.cancel");
        wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
        // The kit gives the focus back once the dialog's exit has played.
        f.wait(cx, 1000);
    }
    assert!(f.counts().is_empty());
    assert!(f.writes().is_empty());
}

#[gpui_kit::test]
async fn zero_matches_disables_continue(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 3).await;
    *f.docs.found.lock().unwrap() = 0;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    assert!(f.continue_disabled(cx));
    assert!(f.text_of(cx, "bulk.count").contains("No documents"));
}

#[gpui_kit::test]
async fn bad_filter_shows_the_server_message(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 3).await;
    f.press(cx, DELETE_QUERY);
    *f.docs.fail.lock().unwrap() = Some((400, "Could not parse the filter query.".into()));
    f.type_bulk_filter(cx, "price:");
    f.wait(cx, 300);
    let text = f.text_of(cx, "bulk.error");
    assert!(text.contains("Could not parse the filter query."), "{text}");
    assert!(f.continue_disabled(cx));
}

#[gpui_kit::test]
async fn continue_counts_again(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    *f.docs.found.lock().unwrap() = 8;
    f.bulk_continue(cx);
    assert_eq!(f.counts().len(), 2);
    let body = f.text_of(cx, "confirm.body");
    assert!(
        body.contains("Deletes 8 documents from products matching: price:>1"),
        "{body}"
    );
}

#[gpui_kit::test]
async fn cancel_changes_nothing(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.click(cx, "confirm.cancel");
    f.settle(cx);
    assert!(f.writes().is_empty());
    assert!(
        !f.continue_disabled(cx),
        "the dialog is still there to retry"
    );
}

#[gpui_kit::test]
async fn at_one_thousand_uses_the_simple_confirm(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1000).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    assert!(f.has(cx, "confirm.ok"));
    assert!(!f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn above_one_thousand_needs_the_name(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1001).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    assert!(!f.has(cx, "confirm.ok"));
    let line = f.text_of(cx, "danger.consequence.0");
    assert!(
        line.contains("1,001") && line.contains("price:>1"),
        "{line}"
    );
    f.act(cx, |window, cx| window.input("products", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    wait_for_element(cx, f.handle, "danger.ok".into(), false).await;
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    assert_eq!(f.writes()[0].0, "delete_by_filter");
    assert_eq!(f.writes()[0].1, "price:>1");
    assert!(f.text_of(cx, "bulk.result").contains("1,001"));
}

#[gpui_kit::test]
async fn stop_waiting_still_closes_with_the_result(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1001).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.act(cx, |window, cx| window.input("products", cx));
    let hold = f.docs.hold_next();
    f.click(cx, "danger.ok");
    f.settle(cx);
    f.click(cx, "danger.stop_waiting");
    wait_for_element(cx, f.handle, "danger.ok".into(), false).await;
    assert!(f.has(cx, "bulk.continue"));
    drop(hold);
    f.settle(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    assert!(f.has(cx, "bulk.result"));
}

#[gpui_kit::test]
async fn prod_needs_the_name(cx: &mut TestAppContext) {
    let f = open_prod(cx, 2).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    assert!(f.has(cx, "danger.prod"));
    assert!(!f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn type_to_confirm_is_exact(cx: &mut TestAppContext) {
    let f = open_prod(cx, 2).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.act(cx, |window, cx| window.input("Products", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    assert!(f.writes().is_empty());
    assert!(f.has(cx, "danger.ok"));
}

#[gpui_kit::test]
async fn delete_reports_the_count_and_reloads(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.tick(cx, "3");
    let pages = f.docs.pages.lock().unwrap().len();
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.settle(cx);
    assert_eq!(f.writes()[0].1, "price:>1");
    assert!(f.text_of(cx, "bulk.result").contains("Deleted 5"));
    assert!(f.docs.pages.lock().unwrap().len() > pages);
    assert!(f.ticked(cx).is_empty());
    assert!(f.text_of(cx, "documents.status").contains('0'));
}

#[gpui_kit::test]
async fn update_sends_filter_and_partial(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, UPDATE_QUERY, "price:>1");
    f.type_partial(cx, r#"{"title":"x"}"#);
    f.bulk_continue(cx);
    let body = f.text_of(cx, "confirm.body");
    assert!(
        body.contains("5") && body.contains("price:>1") && body.contains("Sets: title"),
        "{body}"
    );
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    let writes = f.writes();
    assert_eq!(writes[0].0, "update_by_filter");
    assert_eq!(writes[0].1, "price:>1");
    assert_eq!(writes[0].2, json!({"title": "x"}));
    assert!(f.text_of(cx, "bulk.result").contains("Updated 5"));
}

#[gpui_kit::test]
async fn update_with_id_is_refused(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, UPDATE_QUERY, "price:>1");
    f.type_partial(cx, r#"{"id":"z","title":"x"}"#);
    assert!(f.continue_disabled(cx));
    assert!(f.text_of(cx, "bulk.issues").contains("`id`"));
}

#[gpui_kit::test]
async fn update_marks_only_the_keys_present(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, UPDATE_QUERY, "price:>1");
    f.type_partial(cx, r#"{"price":"x"}"#);
    let issues = f.text_of(cx, "bulk.issues");
    assert!(
        issues.contains("price") && !issues.contains("title"),
        "{issues}"
    );
    assert!(
        !f.continue_disabled(cx),
        "marks advise, §7.8 lets it through"
    );
}

// Review focus 2.
#[gpui_kit::test]
async fn update_mismatch_stays_open_with_the_warning(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    *f.docs.num_updated.lock().unwrap() = Some(0);
    f.bulk(cx, UPDATE_QUERY, "price:>1");
    f.type_partial(cx, r#"{"price":"x"}"#);
    f.bulk_continue(cx);
    f.confirm_delete(cx);
    f.settle(cx);
    assert!(f.has(cx, "bulk.continue"), "the dialog stays");
    let text = f.text_of(cx, "bulk.partial");
    assert!(text.contains("Updated 0 of 5"), "{text}");
    assert!(!f.has(cx, "bulk.result"), "not reported as a success");
}

#[gpui_kit::test]
async fn update_mismatch_on_prod_comes_back_to_the_dialog(cx: &mut TestAppContext) {
    let f = open_prod(cx, 5).await;
    *f.docs.num_updated.lock().unwrap() = Some(0);
    f.bulk(cx, UPDATE_QUERY, "price:>1");
    f.type_partial(cx, r#"{"price":"x"}"#);
    f.bulk_continue(cx);
    f.act(cx, |window, cx| window.input("products", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    wait_for_element(cx, f.handle, "danger.ok".into(), false).await;
    f.settle(cx);
    assert!(f.has(cx, "bulk.continue"));
    assert!(f.text_of(cx, "bulk.partial").contains("Updated 0 of 5"));
    assert!(!f.has(cx, "bulk.result"));
}

#[gpui_kit::test]
async fn update_invalid_json_disables_continue(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, UPDATE_QUERY, "price:>1");
    assert!(f.continue_disabled(cx), "{{}} changes nothing");
    for partial in ["{", "[1]", "{}"] {
        f.type_partial(cx, partial);
        assert!(f.continue_disabled(cx), "{partial}");
    }
    f.type_partial(cx, r#"{"title":"x"}"#);
    assert!(!f.continue_disabled(cx));
}

#[gpui_kit::test]
async fn locked_profile_unlocks_first_and_sends_nothing_before(cx: &mut TestAppContext) {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = 5;
    let f = open_profile(cx, plain_schema(), docs, true).await;
    assert!(f.has(cx, "documents.delete_query.readonly"));
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    assert!(f.has(cx, "danger.input"));
    assert_eq!(f.counts().len(), 1, "no second count before the unlock");
    f.click(cx, "danger.input");
    f.act(cx, |window, cx| window.input("test", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    wait_for_element(cx, f.handle, "confirm.ok".into(), true).await;
    assert_eq!(f.counts().len(), 2);
    assert!(f.writes().is_empty());
    f.confirm_delete(cx);
    assert_eq!(f.writes().len(), 1);
}

#[gpui_kit::test]
async fn timeout_uses_the_long_text(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    *f.docs.write_fail.lock().unwrap() = Some((TIMEOUT, String::new()));
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.confirm_delete(cx);
    f.settle(cx);
    let text = f.text_of(cx, "bulk.error");
    assert!(text.contains("did not finish within 30 minutes"), "{text}");
    assert!(f.has(cx, "bulk.continue"));
}

#[gpui_kit::test]
async fn failure_on_prod_shows_in_the_danger_dialog(cx: &mut TestAppContext) {
    let f = open_prod(cx, 5).await;
    *f.docs.write_fail.lock().unwrap() = Some((500, "boom".into()));
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.act(cx, |window, cx| window.input("products", cx));
    f.click(cx, "danger.ok");
    f.settle(cx);
    assert!(f.text_of(cx, "danger.error").contains("boom"));
}

#[gpui_kit::test]
async fn dialog_buttons_show_their_shortcuts(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    let visual = |f: &Fixture, cx: &mut TestAppContext, id: &'static str, chord: &'static str| {
        let button = f.act(cx, |window, _| window.find(id).bounds());
        let kbd = gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
            .debug_bounds(chord)
            .unwrap();
        assert!(button.contains(&kbd.center()), "{id}");
    };
    visual(
        &f,
        cx,
        "documents.delete_query",
        key("kbd:cmd-shift-k", "kbd:ctrl-shift-k"),
    );
    visual(
        &f,
        cx,
        "documents.update_query",
        key("kbd:cmd-shift-u", "kbd:ctrl-shift-u"),
    );
    f.bulk(cx, DELETE_QUERY, "price:>1");
    visual(
        &f,
        cx,
        "bulk.continue",
        key("kbd:cmd-enter", "kbd:ctrl-enter"),
    );
    visual(&f, cx, "bulk.cancel", "kbd:escape");
}

#[gpui_kit::test]
async fn short_delete_says_how_many_went(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    *f.docs.filter_short.lock().unwrap() = 2;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.bulk_continue(cx);
    f.confirm_delete(cx);
    f.settle(cx);
    assert!(f.has(cx, "bulk.continue"), "the dialog stays");
    let text = f.text_of(cx, "bulk.partial");
    assert!(text.contains("Deleted 3 of 5"), "{text}");
    assert!(!f.has(cx, "bulk.result"));
}

#[gpui_kit::test]
async fn a_draft_is_named_and_goes_only_with_its_row(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.delete_query");
    f.settle(cx);
    f.type_bulk_filter(cx, "price:>1");
    f.wait(cx, 300);
    f.bulk_continue(cx);
    assert!(f.text_of(cx, "confirm.body").contains("unsaved"));
    f.click(cx, "confirm.cancel");
    wait_for_element(cx, f.handle, "confirm.ok".into(), false).await;
    f.wait(cx, 1000);
    assert!(f.dirty(cx), "cancelling changes nothing");

    f.bulk_continue(cx);
    *f.docs.document_fail.lock().unwrap() = Some((404, "gone".into()));
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.settle(cx);
    assert!(!f.has(cx, "documents.inspector"));
    assert!(!f.dirty(cx));
}

#[gpui_kit::test]
async fn a_draft_of_a_row_that_stayed_is_kept(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.delete_query");
    f.settle(cx);
    f.type_bulk_filter(cx, "price:>1");
    f.wait(cx, 300);
    f.bulk_continue(cx);
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.settle(cx);
    assert!(f.has(cx, "documents.inspector"));
    assert!(f.inspector_text(cx).contains("\"new\""));
}

#[gpui_kit::test]
async fn an_update_never_mentions_a_draft_and_keeps_it(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.open_document(cx);
    f.edit(cx, RENAMED);
    f.click(cx, "documents.update_query");
    f.settle(cx);
    f.type_bulk_filter(cx, "price:>1");
    f.wait(cx, 300);
    f.type_partial(cx, r#"{"title":"x"}"#);
    f.bulk_continue(cx);
    assert!(!f.text_of(cx, "confirm.body").contains("unsaved"));
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.settle(cx);
    assert!(f.inspector_text(cx).contains("\"new\""));
}

#[gpui_kit::test]
async fn a_clean_inspector_shows_the_stored_document_after_an_update(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.open_document(cx);
    f.click(cx, "documents.update_query");
    f.settle(cx);
    f.type_bulk_filter(cx, "price:>1");
    f.wait(cx, 300);
    f.type_partial(cx, r#"{"title":"x"}"#);
    f.bulk_continue(cx);
    *f.docs.stored.lock().unwrap() = Some(json!({"id": "3", "title": "x"}));
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.settle(cx);
    assert!(f.inspector_text(cx).contains("\"x\""));
    assert!(!f.dirty(cx));
}

#[gpui_kit::test]
async fn a_clean_inspector_closes_on_a_deleted_document(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.open_document(cx);
    f.click(cx, "documents.delete_query");
    f.settle(cx);
    f.type_bulk_filter(cx, "price:>1");
    f.wait(cx, 300);
    f.bulk_continue(cx);
    assert!(!f.text_of(cx, "confirm.body").contains("unsaved"));
    *f.docs.document_fail.lock().unwrap() = Some((404, "gone".into()));
    f.confirm_delete(cx);
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.settle(cx);
    assert!(!f.has(cx, "documents.inspector"));
}

#[gpui_kit::test]
async fn closing_the_dialog_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.press(cx, "escape");
    wait_for_element(cx, f.handle, "bulk.continue".into(), false).await;
    f.wait(cx, 1000);
    f.press(cx, UPDATE_QUERY);
    assert!(f.has(cx, "bulk.editor"));
}

#[gpui_kit::test]
async fn command_enter_continues_from_the_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 5).await;
    f.bulk(cx, DELETE_QUERY, "price:>1");
    f.press(cx, SAVE);
    assert!(f.has(cx, "confirm.ok"));
}

fn lines(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[gpui_kit::test]
async fn jsonl_copies_the_selection(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "1");
    f.tick(cx, "3");
    f.press(cx, COPY_JSONL);
    let text = f.clipboard(cx).unwrap();
    let docs = lines(&text);
    assert_eq!(
        docs.iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["1", "3"]
    );
    assert_eq!(*f.docs.export_calls.lock().unwrap(), ["id:[`1`,`3`]"]);
    assert!(f.docs.document_calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn jsonl_of_the_selected_row_when_nothing_is_ticked(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 4);
    f.press(cx, COPY_JSONL);
    let docs = lines(&f.clipboard(cx).unwrap());
    assert_eq!(docs[0]["id"], "4");
    // One id takes the document endpoint, which also carries the vector fields.
    assert!(docs[0].get("embedding").is_some());
    assert!(f.docs.export_calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn csv_uses_the_visible_columns_in_order(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.tick(cx, "1");
    f.tick(cx, "2");
    f.press(cx, COPY_CSV);
    let keys = f.column_keys(cx);
    let text = f.clipboard(cx).unwrap();
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows[0], keys.join(","));
    assert!(!rows[0].contains("__select"));
    assert_eq!(rows.len(), 3);
    let cell = |row: &str, key: &str| {
        row.split(',')
            .nth(keys.iter().position(|k| k == key).unwrap())
            .unwrap()
            .to_string()
    };
    assert_eq!(cell(rows[1], "title"), "t1");
    assert_eq!(cell(rows[2], "id"), "2");
}

#[gpui_kit::test]
async fn csv_quotes_values(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.stored.lock().unwrap() =
        Some(json!({"id": "7", "title": "say \"hi\", ok", "price": 1.5}));
    f.select_row(cx, 7);
    f.press(cx, COPY_CSV);
    let text = f.clipboard(cx).unwrap();
    assert!(text.contains("\"say \"\"hi\"\", ok\""), "{text}");
    assert!(text.contains("1.5"), "{text}");
}

#[gpui_kit::test]
async fn selection_is_batched_for_export(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 250).await;
    // Long ids, so one filter cannot hold them all.
    *f.docs.ids.lock().unwrap() = (0..250).map(|i| format!("doc-{i:0>40}")).collect();
    f.change_schema(cx, plain_schema());
    f.select_row(cx, 0);
    f.press(cx, SELECT_ALL);
    f.press(cx, COPY_JSONL);
    let calls = f.docs.export_calls.lock().unwrap().clone();
    assert!(calls.len() >= 2, "{}", calls.len());
    assert!(calls.iter().all(|c| c.len() < 7_500 * 2));
    assert_eq!(lines(&f.clipboard(cx).unwrap()).len(), 250);
}

#[gpui_kit::test]
async fn backtick_id_uses_the_document_endpoint(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.ids.lock().unwrap() = vec!["a`b".into(), "1".into(), "x*".into()];
    f.change_schema(cx, plain_schema());
    f.tick(cx, "a`b");
    f.tick(cx, "x*");
    f.tick(cx, "1");
    f.press(cx, COPY_JSONL);
    assert_eq!(*f.docs.export_calls.lock().unwrap(), ["id:[`1`]"]);
    assert_eq!(*f.docs.document_calls.lock().unwrap(), ["a`b", "x*"]);
    assert_eq!(lines(&f.clipboard(cx).unwrap()).len(), 3);
}

#[gpui_kit::test]
async fn dot_ids_are_exported_by_their_own_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.ids.lock().unwrap() = vec![".".into(), "..".into()];
    f.change_schema(cx, plain_schema());
    f.tick(cx, ".");
    f.press(cx, COPY_JSONL);
    assert_eq!(*f.docs.export_calls.lock().unwrap(), ["id:[`.`]"]);
    assert!(f.docs.document_calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn copy_error_shows_inline(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.export_fail.lock().unwrap() = Some((500, "boom".into()));
    f.tick(cx, "1");
    f.tick(cx, "2");
    f.press(cx, COPY_JSONL);
    f.act(cx, |window, _| {
        assert!(label(window, "documents.copy.error").contains("boom"));
    });
    assert_eq!(f.clipboard(cx), None);
    *f.docs.export_fail.lock().unwrap() = None;
    f.press(cx, COPY_JSONL);
    f.act(cx, |window, _| {
        assert!(window.try_find("documents.copy.error").is_none());
    });
}

#[gpui_kit::test]
async fn curl_for_a_row_has_the_placeholder_key(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 2);
    f.press(cx, COPY_DOC_CURL);
    let text = f.clipboard(cx).unwrap();
    assert!(text.starts_with("curl "), "{text}");
    assert!(
        text.contains("/collections/products/documents/2'"),
        "{text}"
    );
    assert!(
        text.contains("X-TYPESENSE-API-KEY: $TYPESENSE_API_KEY"),
        "{text}"
    );
    assert!(!text.contains("-X"), "{text}");
}

#[gpui_kit::test]
async fn curl_for_a_dot_id_is_refused_inline(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    *f.docs.ids.lock().unwrap() = vec!["..".into()];
    f.change_schema(cx, plain_schema());
    f.select_row(cx, 0);
    f.press(cx, COPY_DOC_CURL);
    assert_eq!(f.clipboard(cx), None);
    f.act(cx, |window, _| {
        assert!(!label(window, "documents.copy.error").is_empty());
    });
}

#[gpui_kit::test]
async fn curl_without_a_row_repeats_the_search(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.submit_filter(cx, "price:>1 && title:=a");
    f.press(cx, COPY_DOC_CURL);
    let text = f.clipboard(cx).unwrap();
    assert!(
        text.contains("/collections/products/documents/search?"),
        "{text}"
    );
    for part in [
        "q=%2A",
        "per_page=250",
        "enable_curations=false",
        "filter_by=price%3A%3E1%20%26%26%20title%3A%3Da",
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert!(!text.contains("sort_by"), "{text}");
}

#[gpui_kit::test]
async fn selection_entries_are_disabled_without_rows(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.click(cx, "documents.copy");
    f.wait_for_menu(cx).await;
    f.act(cx, |window, cx| {
        let mut menu = window.within("popup-menu");
        assert_eq!(menu.find(2).label(), Some("Copy selection as JSONL"));
        assert_eq!(menu.find(4).label(), Some("Copy as curl"));
        // A disabled entry does not dismiss the menu.
        menu.click(2, cx);
    });
    f.act(cx, |window, _| {
        assert!(window.try_find("popup-menu").is_some())
    });
    f.act(cx, |window, cx| window.within("popup-menu").click(4, cx));
    f.act(cx, |window, _| {
        assert!(window.try_find("popup-menu").is_none())
    });
    assert!(f.clipboard(cx).unwrap().contains("documents/search"));
}

#[gpui_kit::test]
async fn copy_menu_lists_the_new_shortcuts(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 10).await;
    f.select_row(cx, 1);
    f.click(cx, "documents.copy");
    f.wait_for_menu(cx).await;
    f.act(cx, |_, _| {});
    for chord in [
        key("kbd:alt-cmd-j", "kbd:ctrl-alt-j"),
        key("kbd:alt-cmd-s", "kbd:ctrl-alt-s"),
        key("kbd:alt-cmd-u", "kbd:ctrl-alt-u"),
    ] {
        gpui_kit::VisualTestContext::from_window(f.handle.into(), cx)
            .debug_bounds(chord)
            .unwrap_or_else(|| panic!("no Kbd for {chord}"));
    }
}
