// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the request log in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use secrecy::{ExposeSecret as _, SecretString};
use tessera_app::connection::{ConnectorGlobal, tokio_connector};
use tessera_app::runtime::Tokio;
use tessera_app::state::AppState;
use tessera_app::views::root::RootView;
use tessera_client::{Body, FULL_BODY_MAX, Method, Origin, RequestEvent};
use tessera_core::log::{BodyPart, FULL_BODY_BUDGET, Filter, LogEntry};
use tessera_core::profile::{Node, Profile, Protocol};
use tessera_core::secrets::{MemoryStore, SecretStore};
use tessera_core::store::ConnectionsFile;
use uuid::Uuid;

use common::*;

#[cfg(target_os = "macos")]
const NEXT_CHUNK: &str = "alt-cmd-]";
#[cfg(not(target_os = "macos"))]
const NEXT_CHUNK: &str = "ctrl-alt-]";
#[cfg(target_os = "macos")]
const SHOW_FULL_BODY: &str = "cmd-shift-b";
#[cfg(not(target_os = "macos"))]
const SHOW_FULL_BODY: &str = "ctrl-shift-b";

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn event(id: u64, method: Method, path: &str, status: Option<u16>, origin: Origin) -> RequestEvent {
    RequestEvent {
        id,
        started_at: SystemTime::UNIX_EPOCH,
        node: "http://localhost:8108".parse().unwrap(),
        method,
        path: path.into(),
        query: None,
        request_headers: vec![],
        key_prefix: String::new(),
        request_body: None,
        response_body: None,
        status,
        duration: Duration::from_millis(12),
        size: Some(2048),
        origin,
        error: status.is_none().then(|| "connection refused".to_string()),
    }
}

fn entry(event: RequestEvent) -> LogEntry {
    LogEntry {
        connection: Uuid::nil(),
        connection_name: "dev".into(),
        event,
    }
}

fn send(cx: &mut TestAppContext, state: &Entity<AppState>, entries: Vec<LogEntry>) {
    let tx = state.read_with(cx, |s, _| s.log_sender());
    for e in entries {
        tx.send(e).unwrap();
    }
}

/// A window with the request log open.
fn open_log(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Root>,
    Entity<AppState>,
    tokio::runtime::Runtime,
) {
    let (handle, state, rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    toggle_log(cx, handle);
    (handle, state, rt)
}

fn toggle_log(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(TOGGLE_LOG, cx);
    })
    .unwrap();
}

#[cfg(target_os = "macos")]
const SHOW_LAST_FAILURE: &str = "alt-cmd-r";
#[cfg(not(target_os = "macos"))]
const SHOW_LAST_FAILURE: &str = "ctrl-alt-r";

fn row(id: u64) -> String {
    format!("log.row.{id}")
}

fn row_label(cx: &mut TestAppContext, handle: WindowHandle<Root>, id: u64) -> String {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.find(row(id)).label().unwrap_or_default().to_string()
    })
    .unwrap()
}

#[gpui_kit::test]
async fn events_reach_the_log(cx: &mut TestAppContext) {
    let (_handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    send(
        cx,
        &state,
        (0..2)
            .map(|id| entry(event(id, Method::GET, "/health", Some(200), Origin::User)))
            .collect(),
    );
    cx.run_until_parked();
    let n = state.read_with(cx, |s, _| s.log().entries(&Filter::default()).len());
    assert_eq!(n, 2);
}

#[gpui_kit::test]
async fn the_app_log_keeps_5000_user_and_500_polling_entries(cx: &mut TestAppContext) {
    let (_handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    let events = (0..5001)
        .map(|id| event(id, Method::GET, "/u", Some(200), Origin::User))
        .chain((5001..5502).map(|id| event(id, Method::GET, "/p", Some(200), Origin::Polling)));
    send(cx, &state, events.map(entry).collect());
    cx.run_until_parked();
    let (user, polling) = state.read_with(cx, |s, _| {
        let all = s.log().entries(&Filter {
            show_polling: true,
            ..Default::default()
        });
        let polling = all.iter().filter(|e| e.event.origin == Origin::Polling);
        (all.len() - polling.clone().count(), polling.count())
    });
    assert_eq!((user, polling), (5000, 500));
}

#[gpui_kit::test]
async fn connect_hook_logs_with_profile_id_and_name(cx: &mut TestAppContext) {
    let (staging, _, _) = two_profiles();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(staging.id, &SecretString::from("k")).unwrap();
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, _, hook| {
            hook(&event(1, Method::GET, "/health", Some(200), Origin::User));
            let info = server("30.2", true);
            Box::pin(async move { Ok(info) })
        })));
    });
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets);
    state.update(cx, |s, cx| s.connect(staging.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        let entries = s.log().entries(&Filter::default());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].connection, staging.id);
        assert_eq!(&*entries[0].connection_name, "staging-eu");
    });
}

#[gpui_kit::test]
async fn request_appears_in_log(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    send(
        cx,
        &state,
        vec![entry(event(
            7,
            Method::POST,
            "/collections",
            Some(201),
            Origin::User,
        ))],
    );
    wait_for_element(cx, handle, row(7), true).await;
    let label = row_label(cx, handle, 7);
    for part in [
        "POST",
        "/collections",
        "201",
        "12 ms",
        "2.0 KB",
        "localhost:8108",
    ] {
        assert!(label.contains(part), "{part:?} missing in {label:?}");
    }
}

#[gpui_kit::test]
async fn polling_hidden_until_switch_on(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    send(
        cx,
        &state,
        vec![
            entry(event(1, Method::GET, "/health", Some(200), Origin::User)),
            entry(event(
                2,
                Method::GET,
                "/metrics.json",
                Some(200),
                Origin::Polling,
            )),
        ],
    );
    wait_for_element(cx, handle, row(1), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(
            window.try_find(row(2)).is_none(),
            "polling hidden by default"
        );
        assert_eq!(window.find("log.filter.polling").checked(), Some(false));
        window.click("log.filter.polling", cx);
    })
    .unwrap();
    wait_for_element(cx, handle, row(2), true).await;
}

#[gpui_kit::test]
async fn errors_filter_shows_failed_polling(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    send(
        cx,
        &state,
        vec![
            entry(event(
                1,
                Method::GET,
                "/metrics.json",
                None,
                Origin::Polling,
            )),
            entry(event(2, Method::GET, "/health", Some(200), Origin::User)),
        ],
    );
    wait_for_element(cx, handle, row(2), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.try_find(row(1)).is_none());
        window.click("log.filter.errors", cx);
    })
    .unwrap();
    wait_for_element(cx, handle, row(1), true).await;
    wait_for_element(cx, handle, row(2), false).await;
    assert!(row_label(cx, handle, 1).contains("failed"));
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("log.filter.all", cx)
    })
    .unwrap();
    wait_for_element(cx, handle, row(2), true).await;
}

#[gpui_kit::test]
async fn detail_shows_redacted_body(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let mut e = event(3, Method::GET, "/health", Some(200), Origin::User);
    e.key_prefix = "abcd".into();
    e.response_body = Some(Body {
        text: r#"{"api_key":"sk"}"#.into(),
        original_len: 3_565_158,
        full: None,
    });
    send(cx, &state, vec![entry(e)]);
    wait_for_element(cx, handle, row(3), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.click(row(3), cx))
        .unwrap();
    wait_for_label(cx, handle, "log.detail".into(), "Response body").await;
    let detail = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .find("log.detail")
                .label()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap();
    assert!(!detail.contains("sk"), "{detail}");
    assert!(detail.contains("••••"), "{detail}");
    assert!(detail.contains("X-TYPESENSE-API-KEY: abcd••••"), "{detail}");
    assert!(detail.contains("showing 16 B of 3.4 MB"), "{detail}");
}

#[gpui_kit::test]
async fn detail_says_when_the_body_was_not_kept(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    // `event` has no response body but a size, like a document page.
    send(
        cx,
        &state,
        vec![entry(event(
            5,
            Method::GET,
            "/collections/c/documents/search",
            Some(200),
            Origin::User,
        ))],
    );
    wait_for_element(cx, handle, row(5), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.click(row(5), cx))
        .unwrap();
    wait_for_label(
        cx,
        handle,
        "log.detail".into(),
        "Response body not kept for document pages",
    )
    .await;
}

#[gpui_kit::test]
async fn import_entry_says_bodies_were_not_kept(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    send(
        cx,
        &state,
        vec![entry(event(
            6,
            Method::POST,
            "/collections/c/documents/import",
            Some(200),
            Origin::User,
        ))],
    );
    wait_for_element(cx, handle, row(6), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.click(row(6), cx))
        .unwrap();
    wait_for_label(
        cx,
        handle,
        "log.detail".into(),
        "Bodies of import batches are not kept",
    )
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("log.copy_curl").disabled(), Some(true));
        window.press(COPY_CURL, cx);
    })
    .unwrap();
    assert!(cx.read_from_clipboard().and_then(|c| c.text()).is_none());
}

/// An entry whose response was cut at 16 bytes, with `full` as its whole text.
fn cut_entry(id: u64, full: Option<&str>) -> LogEntry {
    let mut e = event(id, Method::GET, "/keys", Some(200), Origin::User);
    e.response_body = Some(Body {
        text: r#"{"value":"sk-sec"#.into(),
        original_len: 1 << 21,
        full: full.map(Into::into),
    });
    entry(e)
}

fn select_row(cx: &mut TestAppContext, handle: WindowHandle<Root>, id: u64) {
    cx.update_window(handle.into(), |_, window, cx| window.click(row(id), cx))
        .unwrap();
}

fn pressed_full_body(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(SHOW_FULL_BODY, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn show_full_body_opens_the_view_masked(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let full = r#"{"value":"sk-secret-tail","n":1}"#;
    send(cx, &state, vec![cut_entry(7, Some(full))]);
    wait_for_element(cx, handle, row(7), true).await;
    select_row(cx, handle, 7);
    wait_for_element(cx, handle, "log.detail.full.response".to_string(), true).await;
    pressed_full_body(cx, handle);
    wait_for_element(cx, handle, "log.full.title".to_string(), true).await;
    let text = handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let log = view.read(cx).log_panel().read(cx);
            let body = log.full_view().unwrap();
            body.read(cx).text(cx).unwrap_or_default()
        })
        .unwrap();
    assert!(text.contains("\"n\": 1"), "{text}");
    assert!(!text.contains("sk-secret-tail"), "{text}");
    assert!(text.contains("••••"), "{text}");

    cx.update_window(handle.into(), |_, window, cx| window.close_dialog(cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    let held = handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            view.read(cx).log_panel().read(cx).full_view().is_some()
        })
        .unwrap();
    assert!(!held, "closing the dialog frees the text");
}

#[gpui_kit::test]
async fn full_view_pages_to_the_end_of_the_body(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let line = "plain line of a long body\n";
    let full = format!("{}the last line", line.repeat((3 << 20) / line.len()));
    send(cx, &state, vec![cut_entry(7, Some(&full))]);
    wait_for_element(cx, handle, row(7), true).await;
    select_row(cx, handle, 7);
    wait_for_element(cx, handle, "log.detail.full.response".to_string(), true).await;
    pressed_full_body(cx, handle);
    wait_for_label(cx, handle, "body.chunk".into(), "Chunk 1 of 7").await;
    for _ in 0..6 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press(NEXT_CHUNK, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    wait_for_label(cx, handle, "body.chunk".into(), "Chunk 7 of 7").await;
    let text = handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let body = view.read(cx).log_panel().read(cx).full_view().unwrap();
            body.read(cx).text(cx).unwrap_or_default()
        })
        .unwrap();
    assert!(text.ends_with("the last line"), "{}", text.len());
}

#[gpui_kit::test]
async fn too_large_a_body_says_so_and_not_that_it_went(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let mut e = cut_entry(7, None);
    e.event.response_body.as_mut().unwrap().original_len = FULL_BODY_MAX + 1;
    send(cx, &state, vec![e]);
    wait_for_element(cx, handle, row(7), true).await;
    select_row(cx, handle, 7);
    wait_for_label(cx, handle, "log.detail.full_too_large".into(), "32 MiB").await;
    wait_for_element(cx, handle, "log.detail.full_gone".to_string(), false).await;
}

#[gpui_kit::test]
async fn evicted_body_says_so(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let big = "x".repeat(FULL_BODY_BUDGET);
    send(cx, &state, vec![cut_entry(7, Some("short"))]);
    send(cx, &state, vec![cut_entry(8, Some(&big))]);
    wait_for_element(cx, handle, row(8), true).await;
    let log = state.read_with(cx, |s, _| s.log().full_body(7, BodyPart::Response));
    assert!(log.is_none());
    select_row(cx, handle, 7);
    wait_for_label(cx, handle, "log.detail.full_gone".into(), "last 64 MiB").await;
    pressed_full_body(cx, handle);
    wait_for_element(cx, handle, "log.full.title".to_string(), false).await;
}

#[gpui_kit::test]
async fn uncut_body_has_no_button(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let mut e = event(9, Method::GET, "/health", Some(200), Origin::User);
    e.response_body = Some(Body {
        text: "{}".into(),
        original_len: 2,
        full: None,
    });
    send(cx, &state, vec![entry(e)]);
    wait_for_element(cx, handle, row(9), true).await;
    select_row(cx, handle, 9);
    wait_for_label(cx, handle, "log.detail".into(), "Response body").await;
    wait_for_element(cx, handle, "log.detail.full.response".to_string(), false).await;
    wait_for_element(cx, handle, "log.detail.full_gone".to_string(), false).await;
}

#[gpui_kit::test]
async fn copy_as_curl_uses_placeholder(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let mut e = event(4, Method::POST, "/keys", Some(201), Origin::User);
    e.key_prefix = "abcd".into();
    e.request_body = Some(Body {
        text: r#"{"description":"x","value":"sk-secret"}"#.into(),
        original_len: 40,
        full: None,
    });
    send(cx, &state, vec![entry(e)]);
    wait_for_element(cx, handle, row(4), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        // No dead control: nothing to copy before a row is selected.
        assert_eq!(window.find("log.copy_curl").disabled(), Some(true));
        window.click(row(4), cx);
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_ne!(window.find("log.copy_curl").disabled(), Some(true));
        window.press(COPY_CURL, cx);
    })
    .unwrap();
    let text = cx
        .read_from_clipboard()
        .and_then(|c| c.text())
        .expect("clipboard text");
    assert!(text.contains("$TYPESENSE_API_KEY"), "{text}");
    assert!(text.contains("/keys"), "{text}");
    assert!(!text.contains("sk-secret"), "{text}");
}

#[gpui_kit::test]
async fn connection_column_only_with_two_connections(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets);
    toggle_log(cx, handle);
    state.update(cx, |s, cx| s.connect(staging.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    let mut e = entry(event(5, Method::GET, "/health", Some(200), Origin::User));
    e.connection = staging.id;
    e.connection_name = "staging-eu".into();
    send(cx, &state, vec![e]);
    wait_for_element(cx, handle, row(5), true).await;
    assert!(!row_label(cx, handle, 5).contains("staging-eu"));
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.try_find("log.filter.connections").is_none());
    })
    .unwrap();

    state.update(cx, |s, cx| s.connect(prod.id, None, cx));
    wait_for_element(cx, handle, "log.filter.connections".into(), true).await;
    assert!(row_label(cx, handle, 5).contains("staging-eu"));

    state.update(cx, |s, cx| s.disconnect(prod.id, cx));
    wait_for_element(cx, handle, "log.filter.connections".into(), false).await;
    assert!(!row_label(cx, handle, 5).contains("staging-eu"));
}

/// A failed GET of `profile`, which the Writes filter hides.
fn failure(id: u64, profile: &Profile) -> LogEntry {
    let mut e = entry(event(id, Method::GET, "/nope", Some(404), Origin::User));
    e.connection = profile.id;
    e.connection_name = profile.name.as_str().into();
    e
}

/// Two profiles, both connected, the first shown; the log is closed.
async fn connected_pair(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Root>,
    Entity<AppState>,
    Profile,
    Profile,
    tokio::runtime::Runtime,
) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, state, rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets);
    state.update(cx, |s, cx| s.connect(prod.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
    state.update(cx, |s, cx| s.connect(staging.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    (handle, state, staging, prod, rt)
}

#[gpui_kit::test]
async fn last_failure_shows_for_its_own_connection_alone(cx: &mut TestAppContext) {
    let (handle, state, staging, prod, _rt) = connected_pair(cx).await;
    let shown = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find("status.last_failure").is_some()
        })
        .unwrap()
    };
    assert!(!shown(cx), "nothing failed yet");
    send(cx, &state, vec![failure(1, &prod)]);
    cx.run_until_parked();
    assert_eq!(
        state.read_with(cx, |s, _| s.log().latest_failed(prod.id)),
        Some(1)
    );
    assert!(!shown(cx), "the other connection's failure stays out");
    send(cx, &state, vec![failure(2, &staging)]);
    wait_for_element(cx, handle, "status.last_failure".into(), true).await;
}

#[gpui_kit::test]
async fn the_chord_finds_a_failure_the_filter_hid(cx: &mut TestAppContext) {
    let (handle, state, staging, _prod, _rt) = connected_pair(cx).await;
    toggle_log(cx, handle);
    send(cx, &state, vec![failure(1, &staging)]);
    wait_for_element(cx, handle, row(1), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("log.filter.writes", cx);
    })
    .unwrap();
    wait_for_element(cx, handle, row(1), false).await;
    toggle_log(cx, handle);
    wait_for_element(cx, handle, "log.filter.writes".into(), false).await;

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(SHOW_LAST_FAILURE, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, row(1), true).await;
    wait_for_label(cx, handle, "log.detail".into(), "/nope").await;
    wait_for_label(cx, handle, "log.detail".into(), "404").await;
}

#[gpui_kit::test]
async fn the_chord_without_a_failure_does_nothing(cx: &mut TestAppContext) {
    let (handle, _state, _staging, _prod, _rt) = connected_pair(cx).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(SHOW_LAST_FAILURE, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("log.filter.writes").is_none(), "log closed");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn last_failure_shows_its_shortcut(cx: &mut TestAppContext) {
    let (handle, state, staging, _prod, _rt) = connected_pair(cx).await;
    send(cx, &state, vec![failure(1, &staging)]);
    wait_for_element(cx, handle, "status.last_failure".into(), true).await;
    let button = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("status.last_failure").bounds()
        })
        .unwrap();
    let kbd = gpui_kit::VisualTestContext::from_window(handle.into(), cx)
        .debug_bounds(key("kbd:alt-cmd-r", "kbd:ctrl-alt-r"))
        .expect("a Kbd for the shortcut is drawn");
    assert!(button.contains(&kbd.center()), "{kbd:?} not in {button:?}");
}

/// Everything traced at any level, on this thread and on the threads of the runtime it returns.
/// The capture of this thread ends with the guard.
fn traced_runtime() -> (
    Capture,
    tracing::dispatcher::DefaultGuard,
    tokio::runtime::Runtime,
) {
    let traced = Capture::default();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer({
                let traced = traced.clone();
                move || traced.clone()
            })
            .finish(),
    );
    let guard = tracing::dispatcher::set_default(&dispatch);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .on_thread_start(move || {
            // The guard would end the capture when dropped; the threads end with the runtime.
            std::mem::forget(tracing::dispatcher::set_default(&dispatch));
        })
        .build()
        .unwrap();
    (traced, guard, rt)
}

/// Spec §9 "Secrets": a whole session through the real client never puts the key into the
/// log, the clipboard or `connections.json`.
#[gpui_kit::test]
async fn full_secret_session_scan(cx: &mut TestAppContext) {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "sk-live-test-1234";
    let (traced, _trace_guard, rt) = traced_runtime();
    cx.update(|cx| cx.set_global(Tokio(rt.handle().clone())));
    cx.executor().allow_parking();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        for (route, status, body) in [
            ("/health", 200, r#"{"ok":true}"#),
            ("/debug", 200, r#"{"state":1,"version":"30.2"}"#),
            ("/collections", 200, "[]"),
            ("/keys", 200, r#"{"keys":[]}"#),
            (
                "/metrics.json",
                200,
                r#"{"system_cpu_active_percentage":"1"}"#,
            ),
        ] {
            Mock::given(path(route))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .mount(&server)
                .await;
        }
        server
    });
    let addr = server.address();
    let profile = Profile::new(
        "session",
        vec![Node {
            protocol: Protocol::Http,
            host: addr.ip().to_string(),
            port: addr.port(),
            path: String::new(),
        }],
    );
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(profile.id, &SecretString::from(KEY)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("connections.json");
    let file = ConnectionsFile {
        profiles: vec![profile.clone()],
        ..Default::default()
    };
    let (handle, state) = open_state_at(cx, Ok(file), Some(file_path.clone()), secrets);
    cx.update(|cx| cx.set_global(ConnectorGlobal(tokio_connector(rt.handle().clone()))));
    toggle_log(cx, handle);

    state.update(cx, |s, cx| s.connect(profile.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "session").await;
    // Polling traffic through the connected client.
    let client = state.read_with(cx, |s, cx| {
        let conn = s.connection(profile.id, cx).unwrap();
        conn.read(cx).connected.as_ref().unwrap().client.polling()
    });
    rt.block_on(async { client.metrics().await.unwrap() });
    cx.wait_for(handle.into(), Duration::from_millis(10_000), |_, cx| {
        state
            .read(cx)
            .log()
            .entries(&Filter {
                show_polling: true,
                ..Default::default()
            })
            .len()
            == 5
    })
    .await;
    cx.wait_for(handle.into(), Duration::from_millis(10_000), {
        let file_path = file_path.clone();
        move |_, _| file_path.exists()
    })
    .await;

    let (debug, entries) = state.read_with(cx, |s, _| {
        let entries = s.log().entries(&Filter {
            show_polling: true,
            ..Default::default()
        });
        let ids = entries.iter().map(|e| (e.event.id, e.event.path.clone()));
        (format!("{entries:?}"), ids.collect::<Vec<_>>())
    });
    assert!(!debug.contains(KEY), "key in log entries");
    assert!(debug.contains("sk-l"), "the redacted prefix is kept");

    cx.update_window(handle.into(), |_, window, cx| {
        window.click("log.filter.polling", cx)
    })
    .unwrap();
    for (id, path) in entries {
        wait_for_element(cx, handle, row(id), true).await;
        assert!(!row_label(cx, handle, id).contains(KEY), "key in row {id}");
        cx.update_window(handle.into(), |_, window, cx| window.click(row(id), cx))
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.press(COPY_CURL, cx))
            .unwrap();
        let clip = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
        assert!(
            clip.contains("$TYPESENSE_API_KEY") && clip.contains(&path),
            "{clip}"
        );
        assert!(!clip.contains(KEY), "key on the clipboard");
        let request_line = format!("GET http://{addr}{path}");
        cx.wait_for(
            handle.into(),
            Duration::from_millis(10_000),
            move |window, _| {
                window
                    .try_find("log.detail")
                    .and_then(|d| d.label().map(|l| l.contains(&request_line)))
                    .unwrap_or(false)
            },
        )
        .await;
        let detail = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window
                    .find("log.detail")
                    .label()
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap();
        assert!(!detail.contains(KEY), "key in the detail of {path}");
    }

    let traced = traced.text();
    assert!(
        traced.contains("/debug"),
        "the capture saw the client: {traced}"
    );
    assert!(!traced.contains(KEY), "key in tracing output");
    let saved = std::fs::read_to_string(&file_path).unwrap();
    assert!(!saved.contains(KEY), "key in connections.json");
}

/// D77: a provider key typed into the AI form reaches the server and nothing else, also when the
/// provider's error echoes it: not the log, tracing, a file, the secret store or the form.
#[gpui_kit::test]
async fn provider_key_session_scan(cx: &mut TestAppContext) {
    use serde_json::json;
    use tessera_app::admin::{AdminBackendGlobal, tokio_backend};
    use tessera_app::state::ResourceKind;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "sk-provider-test-5678";
    let (traced, _trace_guard, rt) = traced_runtime();
    cx.update(|cx| cx.set_global(Tokio(rt.handle().clone())));
    cx.executor().allow_parking();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        let echo = json!({"message": format!("Incorrect API key provided: {KEY}.")});
        Mock::given(method("POST"))
            .and(path("/nl_search_models"))
            .and(body_string_contains(KEY))
            .respond_with(ResponseTemplate::new(400).set_body_json(echo))
            .expect(1)
            .mount(&server)
            .await;
        for (route, body) in [
            ("/health", json!({"ok": true})),
            ("/debug", json!({"state": 1, "version": "30.2"})),
            ("/collections", json!([])),
            ("/keys", json!({"keys": []})),
            (
                "/metrics.json",
                json!({"system_cpu_active_percentage": "1"}),
            ),
            ("/conversations/models", json!([])),
            ("/nl_search_models", json!([])),
        ] {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        server
    });
    let addr = server.address();
    let profile = Profile::new(
        "session",
        vec![Node {
            protocol: Protocol::Http,
            host: addr.ip().to_string(),
            port: addr.port(),
            path: String::new(),
        }],
    );
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(profile.id, &SecretString::from("k")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("connections.json");
    let file = ConnectionsFile {
        profiles: vec![profile.clone()],
        ..Default::default()
    };
    let (handle, state) = open_state_at(cx, Ok(file), Some(file_path.clone()), secrets.clone());
    let tall = gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1800.));
    cx.simulate_window_resize(handle.into(), tall);
    fake_backend(cx, Some((vec![], vec![])));
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(tokio_connector(rt.handle().clone())));
        cx.set_global(AdminBackendGlobal(tokio_backend(rt.handle().clone())));
    });
    state.update(cx, |s, cx| s.connect(profile.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "session").await;
    state.update(cx, |s, cx| {
        s.open_resource(profile.id, ResourceKind::Ai, cx);
    });
    wait_for_element(cx, handle, "ai.conversation.reload".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("ai.view.nl", cx)
    })
    .unwrap();
    wait_for_element(cx, handle, "ai.nl.new".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.click("ai.nl.new", cx))
        .unwrap();
    wait_for_element(cx, handle, "ai.form.key".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "ai.form.model", "openai/gpt-4.1", cx);
        fill(window, "ai.form.max_bytes", "1000", cx);
        fill(window, "ai.form.key", KEY, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "ai.save".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.click("ai.save", cx))
        .unwrap();
    wait_for_element(cx, handle, "ai.error".into(), true).await;
    let shown = cx
        .update_window(handle.into(), |_, window, _| {
            let error = window.find("ai.error");
            error.label().unwrap_or_default().to_string()
        })
        .unwrap();
    assert!(shown.contains("Incorrect API key"), "{shown}");
    assert!(!shown.contains(KEY), "key in the error");

    cx.update(|cx| state.update(cx, |s, cx| s.save(cx).detach()));
    cx.wait_for(handle.into(), Duration::from_millis(10_000), {
        let file_path = file_path.clone();
        move |_, _| file_path.exists()
    })
    .await;
    cx.wait_for(handle.into(), Duration::from_millis(10_000), |_, cx| {
        let log = state.read(cx).log();
        let entries = log.entries(&Filter::default());
        entries.iter().any(|e| e.event.method == Method::POST)
    })
    .await;
    let (debug, kept) = state.read_with(cx, |s, _| {
        let entries = s.log().entries(&Filter::default());
        let post = entries.iter().find(|e| e.event.method == Method::POST);
        let kept =
            post.is_some_and(|e| e.event.request_body.is_some() || e.event.response_body.is_some());
        (format!("{entries:?}"), kept)
    });
    assert!(!kept, "a body in the log");
    assert!(debug.contains("nl_search_models"), "{debug}");
    assert!(!debug.contains(KEY), "key in log entries");
    let traced = traced.text();
    assert!(traced.contains("nl_search_models"), "{traced}");
    assert!(!traced.contains(KEY), "key in tracing output");
    let saved = std::fs::read_to_string(&file_path).unwrap();
    assert!(!saved.contains(KEY), "key in connections.json");
    let stored = secrets.get(profile.id).unwrap();
    assert!(stored.is_some_and(|k| k.expose_secret() == "k"));
    rt.block_on(server.verify());
}

#[gpui_kit::test]
fn copy_as_curl_shows_its_shortcut(cx: &mut TestAppContext) {
    let (handle, _state, _rt) = open_log(cx);
    let button = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("log.copy_curl").bounds()
        })
        .unwrap();
    let kbd = gpui_kit::VisualTestContext::from_window(handle.into(), cx)
        .debug_bounds(key("kbd:cmd-shift-c", "kbd:ctrl-shift-c"))
        .expect("a Kbd for the shortcut is drawn");
    assert!(button.contains(&kbd.center()), "{kbd:?} not in {button:?}");
}

#[gpui_kit::test]
async fn query_key_is_masked_in_row(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let mut e = event(8, Method::GET, "/health", Some(200), Origin::User);
    e.query = Some("x-typesense-api-key=sk-query".into());
    send(cx, &state, vec![entry(e)]);
    wait_for_element(cx, handle, row(8), true).await;
    let label = row_label(cx, handle, 8);
    assert!(
        label.contains("/health?x-typesense-api-key=••••"),
        "{label}"
    );
    assert!(!label.contains("sk-query"), "{label}");
}

#[gpui_kit::test]
async fn copy_as_curl_does_nothing_while_the_log_is_hidden(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    send(
        cx,
        &state,
        vec![entry(event(
            9,
            Method::GET,
            "/health",
            Some(200),
            Origin::User,
        ))],
    );
    wait_for_element(cx, handle, row(9), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.click(row(9), cx))
        .unwrap();
    toggle_log(cx, handle);
    wait_for_element(cx, handle, "dock.bottom".into(), false).await;
    cx.update_window(handle.into(), |_, window, cx| window.press(COPY_CURL, cx))
        .unwrap();
    assert!(cx.read_from_clipboard().is_none());
    toggle_log(cx, handle);
    wait_for_element(cx, handle, "dock.bottom".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| window.press(COPY_CURL, cx))
        .unwrap();
    assert!(
        cx.read_from_clipboard().is_some(),
        "the selection survives hiding"
    );
}

#[gpui_kit::test]
async fn the_palette_offers_the_last_failure_only_when_there_is_one(cx: &mut TestAppContext) {
    let (handle, state, staging, _prod, _rt) = connected_pair(cx).await;
    // The palette lists the available actions.
    let offered = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .available_actions(cx)
                .iter()
                .any(|action| action.name() == "tessera::ShowLastFailure")
        })
        .unwrap()
    };
    assert!(!offered(cx), "nothing failed yet");
    send(cx, &state, vec![failure(1, &staging)]);
    wait_for_element(cx, handle, "status.last_failure".into(), true).await;
    assert!(offered(cx));
}
