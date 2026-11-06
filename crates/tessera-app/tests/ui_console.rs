// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the raw console.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::state::AppState;
use tessera_app::views::console::ConsoleView;
use tessera_app::views::dock::ConsolePanel;
use tessera_app::views::root::RootView;
use tessera_client::{Method, RawBody};
use tessera_core::profile::{Environment, Profile};
use tessera_core::search::history::{HistoryEntry, raw_entry};

use common::*;

#[cfg(target_os = "macos")]
const SEND: &str = "cmd-enter";
#[cfg(not(target_os = "macos"))]
const SEND: &str = "ctrl-enter";
#[cfg(target_os = "macos")]
const OPEN_HISTORY: &str = "cmd-shift-h";
#[cfg(not(target_os = "macos"))]
const OPEN_HISTORY: &str = "ctrl-shift-h";
#[cfg(target_os = "macos")]
const ADD_ROW: &str = "cmd-shift-a";
#[cfg(not(target_os = "macos"))]
const ADD_ROW: &str = "ctrl-shift-a";
#[cfg(target_os = "macos")]
const REVEAL: &str = "alt-cmd-k";
#[cfg(not(target_os = "macos"))]
const REVEAL: &str = "ctrl-alt-k";
#[cfg(target_os = "macos")]
const FAVORITE: &str = "cmd-d";
#[cfg(not(target_os = "macos"))]
const FAVORITE: &str = "ctrl-d";
#[cfg(target_os = "macos")]
const COPY_CURL: &str = "alt-cmd-u";
#[cfg(not(target_os = "macos"))]
const COPY_CURL: &str = "ctrl-alt-u";
#[cfg(target_os = "macos")]
const OPEN_CONSOLE: &str = "cmd-shift-y";
#[cfg(not(target_os = "macos"))]
const OPEN_CONSOLE: &str = "ctrl-shift-y";

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

    /// Connects `profile` and opens its console tab.
    async fn with(cx: &mut TestAppContext, profile: Profile) -> Self {
        fake_backend(cx, Some((vec![], vec![])));
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
        let f = Self {
            handle,
            state,
            search,
            id,
            _rt: rt,
        };
        cx.update(|cx| f.state.update(cx, |s, cx| s.open_console(id, cx)));
        wait_for_element(cx, handle, format!("tab.{id}.console"), true).await;
        f.settle(cx);
        f
    }

    fn panels(&self, cx: &mut TestAppContext) -> Vec<Entity<ConsolePanel>> {
        self.handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<ConsolePanel>().ok())
                    .collect()
            })
            .unwrap()
    }

    fn view(&self, cx: &mut TestAppContext) -> Entity<ConsoleView> {
        let panel = self.panels(cx).remove(0);
        cx.update(|cx| panel.read(cx).view().clone())
    }

    fn close_tab(&self, cx: &mut TestAppContext) {
        let panel = self.panels(cx).remove(0);
        self.handle
            .update(cx, |root, window, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let dock = view.read(cx).dock().clone();
                dock.update(cx, |area, cx| area.remove_panel(panel, window, cx));
            })
            .unwrap();
        self.settle(cx);
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

    /// Fills the form: `method`, `path` and a body.
    fn form(&self, cx: &mut TestAppContext, method: &str, path: &str, body: &str) {
        let view = self.view(cx);
        self.act(cx, |window, cx| {
            view.update(cx, |v, cx| {
                v.set_method(method, window, cx);
                v.set_path(path, window, cx);
                v.set_body(body, window, cx);
            });
        });
        self.settle(cx);
    }

    /// Fills the form and presses Send.
    fn send(&self, cx: &mut TestAppContext, method: &str, path: &str, body: &str) {
        self.form(cx, method, path, body);
        self.click(cx, "console.send");
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

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.act(cx, |window, cx| fill(window, id, text, cx));
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

    fn calls(&self) -> Vec<tessera_client::RawRequest> {
        self.search.raw_calls.lock().unwrap().clone()
    }

    fn answer(&self, status: u16, body: &str) {
        *self.search.raw_answer.lock().unwrap() = (status, body.to_string());
    }

    fn body_text(&self, cx: &mut TestAppContext) -> String {
        let view = self.view(cx);
        cx.update(|cx| {
            let body = view.read(cx).response_view().clone();
            body.read(cx).text(cx).unwrap_or_default()
        })
    }
}

fn prod() -> Profile {
    let mut profile = Profile::new("live", vec![node()]);
    profile.environment = Environment::Prod;
    profile
}

#[gpui_kit::test]
async fn read_only_profile_unlocks_before_post(cx: &mut TestAppContext) {
    let mut profile = Profile::new("ro", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    f.form(cx, "POST", "collections", "{}");
    assert!(f.has(cx, "console.send.readonly"));
    f.click(cx, "console.send");
    assert!(f.has(cx, "danger.input"), "the unlock dialog asks first");
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn get_runs_without_unlock(cx: &mut TestAppContext) {
    let mut profile = Profile::new("ro", vec![node()]);
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    f.send(cx, "GET", "collections", "");
    assert!(!f.has(cx, "danger.input"));
    assert!(!f.has(cx, "console.send.readonly"));
    assert_eq!(f.calls()[0].method, Method::GET);
    assert_eq!(f.calls()[0].path, "collections");
}

#[gpui_kit::test]
async fn invalid_path_disables_send(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.form(cx, "GET", "a b", "");
    assert!(f.disabled(cx, "console.send"));
    assert!(f.has(cx, "console.path_error"));
    f.form(cx, "GET", "/collections", "");
    assert!(!f.disabled(cx, "console.send"));
    assert!(!f.has(cx, "console.path_error"));
}

#[gpui_kit::test]
async fn foreign_path_never_reaches_the_backend(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    for path in [
        "//evil.example/x",
        "http://evil.example/",
        "http:x",
        "\\\\evil",
        "/\\evil.example",
        "%2e%2e/keys",
        "collections/..%2f..%2fkeys",
        "collections/%252e%252e/keys",
    ] {
        f.form(cx, "GET", path, "");
        f.press(cx, SEND);
        assert!(f.has(cx, "console.path_error"), "{path}");
    }
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn delete_collection_needs_the_name(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.send(cx, "DELETE", "collections/books", "");
    assert!(f.has(cx, "danger.input"));
    assert!(
        f.text_of(cx, "danger.consequence.0")
            .contains("DELETE /collections/books")
    );
    assert!(f.text_of(cx, "danger.consequence.1").contains("test"));
    assert!(f.calls().is_empty());
    f.fill(cx, "danger.input", "book");
    assert!(f.disabled(cx, "danger.ok"));
    f.fill(cx, "danger.input", "books");
    assert!(!f.disabled(cx, "danger.ok"));
    f.click(cx, "danger.ok");
    assert_eq!(f.calls().len(), 1);
    assert!(!f.has(cx, "danger.input"));
}

#[gpui_kit::test]
async fn prod_post_asks(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, prod()).await;
    f.send(cx, "POST", "collections", "{}");
    assert!(f.has(cx, "confirm.body"));
    assert!(f.calls().is_empty());
    f.click(cx, "confirm.ok");
    assert_eq!(f.calls().len(), 1);
}

#[gpui_kit::test]
async fn dev_post_does_not_ask(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.send(cx, "POST", "collections", r#"{"name":"a"}"#);
    assert!(!f.has(cx, "confirm.body"));
    assert_eq!(f.calls().len(), 1);
    assert_eq!(
        f.calls()[0].body,
        Some(RawBody::Json(r#"{"name":"a"}"#.into()))
    );
}

#[gpui_kit::test]
async fn cancel_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, prod()).await;
    f.send(cx, "POST", "collections", "{}");
    f.click(cx, "confirm.cancel");
    assert!(!f.has(cx, "confirm.body"));
    f.send(cx, "DELETE", "collections/books", "");
    f.click(cx, "danger.cancel");
    assert!(!f.has(cx, "danger.input"));
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn write_survives_closing_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let gate = f.search.hold_next();
    f.send(cx, "POST", "collections", "{}");
    assert!(f.disabled(cx, "console.send"), "Send stays busy");
    assert_eq!(f.search.raw_done.load(Ordering::SeqCst), 0);

    f.close_tab(cx);
    assert!(f.panels(cx).is_empty());
    assert!(!gate.is_closed(), "closing the tab dropped the write");
    gate.send(()).unwrap();
    f.settle(cx);
    assert_eq!(f.search.raw_done.load(Ordering::SeqCst), 1);
    let raw = cx.update(|cx| f.state.read(cx).history.raw(f.id, "").len());
    assert_eq!(raw, 1, "the request reaches the history");
}

#[gpui_kit::test]
async fn stop_waiting_lets_go_of_a_write(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let gate = f.search.hold_next();
    f.send(cx, "POST", "collections", "{}");
    f.click(cx, "console.stop");
    assert!(!f.disabled(cx, "console.send"));
    assert!(!gate.is_closed());
    gate.send(()).unwrap();
    f.settle(cx);
    assert_eq!(f.search.raw_done.load(Ordering::SeqCst), 1);
    assert!(!f.has(cx, "console.status"), "its answer is let go");
}

#[gpui_kit::test]
async fn read_is_cancelled_by_a_new_send(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let first = f.search.hold_next();
    f.send(cx, "GET", "collections", "");
    assert!(!first.is_closed());
    f.send(cx, "GET", "health", "");
    assert!(first.is_closed(), "the first read was dropped");
    assert_eq!(f.calls().len(), 2);
    assert_eq!(f.search.raw_done.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
async fn key_param_in_table_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.form(cx, "GET", "collections", "");
    f.press(cx, ADD_ROW);
    f.fill(cx, "console.query.0.key", "X-TYPESENSE-API-KEY");
    f.fill(cx, "console.query.0.value", "secret");
    f.press(cx, SEND);
    assert!(f.has(cx, "console.query_error"));
    assert!(f.calls().is_empty());
    f.fill(cx, "console.query.0.key", "limit");
    f.fill(cx, "console.query.0.value", "1 2");
    f.press(cx, SEND);
    assert_eq!(f.calls()[0].query.as_deref(), Some("limit=1%202"));
}

#[gpui_kit::test]
async fn response_is_masked_until_reveal(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.answer(201, r#"{"id":1,"value":"the-only-copy"}"#);
    f.send(cx, "POST", "keys", "{}");
    assert!(f.text_of(cx, "console.status").contains("201"));
    assert!(!f.body_text(cx).contains("the-only-copy"));
    f.press(cx, REVEAL);
    assert!(f.body_text(cx).contains("the-only-copy"));
}

#[gpui_kit::test]
async fn reveal_resets_on_send(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.answer(200, r#"{"value":"the-only-copy"}"#);
    f.send(cx, "GET", "keys", "");
    f.press(cx, REVEAL);
    let gate = f.search.hold_next();
    f.send(cx, "GET", "keys", "");
    assert!(
        !f.body_text(cx).contains("the-only-copy"),
        "before the answer"
    );
    gate.send(()).unwrap();
    f.settle(cx);
    assert!(!f.body_text(cx).contains("the-only-copy"));
}

#[gpui_kit::test]
async fn status_of_a_failure_shows_with_the_body(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.answer(404, r#"{"message":"Not Found"}"#);
    f.send(cx, "GET", "nope", "");
    assert!(f.text_of(cx, "console.status").contains("404"));
    assert!(f.body_text(cx).contains("Not Found"));
    assert!(!f.has(cx, "console.error"));
}

#[gpui_kit::test]
async fn transport_error_shows_inline(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    *f.search.fail.lock().unwrap() = Some((500, "boom".into()));
    f.send(cx, "GET", "health", "");
    assert!(f.text_of(cx, "console.error").contains("boom"));
    let raw = cx.update(|cx| f.state.read(cx).history.raw(f.id, "").len());
    assert_eq!(raw, 0, "it did not reach the server");
}

#[gpui_kit::test]
async fn import_failure_shows_a_warning_on_200(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.answer(
        200,
        "{\"success\":true}\n{\"success\":false,\"error\":\"x\"}",
    );
    f.send(cx, "POST", "collections/c/documents/import", "{}\n{}");
    let note = f.text_of(cx, "console.outcome");
    assert!(note.contains('1') && note.contains('2'), "{note}");
}

fn load(f: &Fixture, cx: &mut TestAppContext, entry: HistoryEntry) {
    let view = f.view(cx);
    f.act(cx, |window, cx| {
        view.update(cx, |v, cx| v.load(&entry, window, cx));
    });
}

#[gpui_kit::test]
async fn history_load_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.send(cx, "POST", "collections", r#"{"name":"a"}"#);
    f.form(cx, "GET", "health", "");
    assert_eq!(f.calls().len(), 1);
    f.press(cx, OPEN_HISTORY);
    assert!(
        f.text_of(cx, "console.history.row.0")
            .contains("POST collections")
    );
    f.click(cx, "console.history.row.0");
    assert_eq!(f.calls().len(), 1, "loading sends nothing");
    let view = f.view(cx);
    let (method, path) = cx.update(|cx| {
        let v = view.read(cx);
        (v.method(cx), v.path_text().to_string())
    });
    assert_eq!((method, path.as_str()), (Method::POST, "collections"));
}

#[gpui_kit::test]
async fn masked_body_is_refused(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let body = r#"{"api_key":"secret","q":"a"}"#;
    load(&f, cx, raw_entry("POST", "multi_search", None, Some(body)));
    f.press(cx, SEND);
    assert!(f.has(cx, "console.error"));
    assert!(f.calls().is_empty());
    let jsonl = "{\"api_key\":\"secret\"}\n{\"n\":1}";
    load(
        &f,
        cx,
        raw_entry("POST", "collections/c/documents/import", None, Some(jsonl)),
    );
    f.press(cx, SEND);
    assert!(f.has(cx, "console.error"), "masked text that is not JSON");
    assert!(f.calls().is_empty());
}

#[gpui_kit::test]
async fn favorites_survive_and_list_first(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.send(cx, "GET", "aliases", "");
    f.press(cx, FAVORITE);
    f.send(cx, "GET", "health", "");
    let favorites = cx.update(|cx| f.state.read(cx).saved.favorites.len());
    assert_eq!(favorites, 1);
    f.press(cx, OPEN_HISTORY);
    let first = f.text_of(cx, "console.history.row.0");
    assert!(first.contains("aliases") && first.contains('★'), "{first}");
    assert!(f.text_of(cx, "console.history.row.1").contains("health"));
    f.click(cx, "console.history.row.0");
    f.press(cx, FAVORITE);
    let favorites = cx.update(|cx| f.state.read(cx).saved.favorites.len());
    assert_eq!(favorites, 0, "the shortcut toggles");
}

#[gpui_kit::test]
async fn curl_uses_the_placeholder(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.form(cx, "POST", "/multi_search", r#"{"api_key":"secret"}"#);
    f.press(cx, COPY_CURL);
    let text = cx
        .read_from_clipboard()
        .and_then(|c| c.text())
        .unwrap_or_default();
    assert!(text.contains("$TYPESENSE_API_KEY"), "{text}");
    assert!(text.contains("/multi_search"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert!(!text.contains("--data-binary @name"));
}

#[gpui_kit::test]
async fn tab_carries_the_connection_badge(cx: &mut TestAppContext) {
    let mut profile = prod();
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    let tab = format!("tab.{}.console", f.id);
    assert!(f.has(cx, &tab));
    assert!(f.has(cx, &format!("{tab}.readonly")));
}

#[gpui_kit::test]
async fn sidebar_row_and_shortcut_open_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.close_tab(cx);
    f.press(cx, OPEN_CONSOLE);
    wait_for_element(cx, f.handle, format!("tab.{}.console", f.id), true).await;
    f.close_tab(cx);
    f.click(cx, &format!("sidebar.{}.console", f.id));
    wait_for_element(cx, f.handle, format!("tab.{}.console", f.id), true).await;
    assert_eq!(f.panels(cx).len(), 1);
}

#[gpui_kit::test]
async fn path_menu_offers_routes_and_enter_sends(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let view = f.view(cx);
    let path = cx.update(|cx| view.read(cx).path_input().clone());
    f.act(cx, |window, cx| {
        path.update(cx, |p, cx| p.focus(window, cx));
        window.input("/ke", cx);
    });
    f.settle(cx);
    let items = cx.update(|cx| {
        let state = path.read(cx).state().read(cx);
        let items = &state.completion_menu_state().items;
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    });
    assert_eq!(items, ["/keys", "/keys/{keyId}"]);

    f.act(cx, |window, cx| {
        path.update(cx, |p, cx| p.set_value("", window, cx));
        path.update(cx, |p, cx| p.focus(window, cx));
        window.input("health", cx);
    });
    f.settle(cx);
    f.press(cx, "enter");
    assert_eq!(f.calls().len(), 1, "no menu stands in the way of Enter");
    assert_eq!(f.calls()[0].path, "health");
}

#[gpui_kit::test]
async fn a_body_file_goes_out_as_text(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("lines.jsonl");
    std::fs::write(&good, "{\"n\":1}\n{\"n\":2}").unwrap();
    let bad = dir.path().join("binary.bin");
    std::fs::write(&bad, [0xFF, 0xFE]).unwrap();
    f.form(cx, "POST", "collections/c/documents/import", "");

    let view = f.view(cx);
    f.act(cx, |window, cx| {
        view.update(cx, |v, cx| v.attach(bad, window, cx));
    });
    cx.executor().allow_parking();
    wait_for_element(cx, f.handle, "console.error".to_string(), true).await;
    f.act(cx, |window, cx| {
        view.update(cx, |v, cx| v.attach(good, window, cx));
    });
    wait_for_element(cx, f.handle, "console.file".to_string(), true).await;
    f.click(cx, "console.send");
    assert_eq!(
        f.calls()[0].body,
        Some(RawBody::Text("{\"n\":1}\n{\"n\":2}".into()))
    );
}
