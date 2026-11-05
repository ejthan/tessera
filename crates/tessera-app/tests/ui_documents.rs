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
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
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
    fake_backend(cx, Some((vec![collection], vec![])));
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
