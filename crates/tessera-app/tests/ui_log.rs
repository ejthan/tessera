// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the request log in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use secrecy::SecretString;
use tessera_app::connection::{ConnectorGlobal, tokio_connector};
use tessera_app::runtime::Tokio;
use tessera_app::state::AppState;
use tessera_client::{Body, Method, Origin, RequestEvent};
use tessera_core::log::{Filter, LogEntry};
use tessera_core::profile::{Node, Profile, Protocol};
use tessera_core::secrets::{MemoryStore, SecretStore};
use tessera_core::store::ConnectionsFile;
use uuid::Uuid;

use common::*;

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
async fn copy_as_curl_uses_placeholder(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_log(cx);
    let mut e = event(4, Method::POST, "/keys", Some(201), Origin::User);
    e.key_prefix = "abcd".into();
    e.request_body = Some(Body {
        text: r#"{"description":"x","value":"sk-secret"}"#.into(),
        original_len: 40,
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

/// Spec §9 "Secrets": a whole session through the real client never puts the key into the
/// log, the clipboard or `connections.json`.
#[gpui_kit::test]
async fn full_secret_session_scan(cx: &mut TestAppContext) {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "sk-live-test-1234";
    // Everything traced at any level, on this thread and on the runtime's threads.
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
    let _trace_guard = tracing::dispatcher::set_default(&dispatch);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .on_thread_start(move || {
            // The guard would end the capture when dropped; the threads end with the runtime.
            std::mem::forget(tracing::dispatcher::set_default(&dispatch));
        })
        .build()
        .unwrap();
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
