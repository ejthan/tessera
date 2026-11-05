// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the documents view of a collection tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::table::TableState;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use tessera_app::connection::ConnectionState;
use tessera_app::state::AppState;
use tessera_app::views::dock::CollectionPanel;
use tessera_app::views::documents::table::DocRows;
use tessera_client::Collection;
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
        self.act(cx, |window, cx| window.click(("icon-sort", col), cx));
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

    fn select_cell(&self, cx: &mut TestAppContext, col: usize) {
        let table = self.table(cx);
        cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, col, cx)));
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

/// Opens `collection` with `found` documents and waits for the first page.
async fn open_found(cx: &mut TestAppContext, collection: Collection, found: u64) -> Fixture {
    let docs = fake_documents(cx);
    *docs.found.lock().unwrap() = found;
    open_tab(cx, collection, docs).await
}

async fn open_tab(cx: &mut TestAppContext, collection: Collection, docs: Arc<FakeDocs>) -> Fixture {
    let name = collection.name.clone();
    let backend = fake_backend(cx, Some((vec![collection], vec![])));
    let profile = tessera_core::profile::Profile::new("test", vec![node()]);
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
    cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, 2, cx)));
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
        assert!(window.try_find(("icon-sort", 1usize)).is_none());
        assert!(window.try_find(("icon-sort", 2usize)).is_some());
    });
    let table = f.table(cx);
    cx.update(|cx| table.update(cx, |t, cx| t.set_selected_cell(0, 1, cx)));
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
        |cx: &mut TestAppContext| cx.update(|cx| f32::from(table.read(cx).col_width(1).unwrap()));
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
async fn an_empty_filter_means_no_filter(cx: &mut TestAppContext) {
    let f = open_found(cx, plain_schema(), 1_000).await;
    f.submit_filter(cx, "   ");
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
