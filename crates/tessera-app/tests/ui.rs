// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, base::Root, px, size,
};
use secrecy::{ExposeSecret as _, SecretString};
use tessera_app::connection::{Connected, ConnectorGlobal, ServerInfo};
use tessera_app::runtime::Tokio;
use tessera_app::state::AppState;
use tessera_app::views::root::RootView;
use tessera_client::{Client, ClientOptions, Error, KeyAccess};
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use tessera_core::secrets::{MemoryStore, SecretError, SecretStore};
use tessera_core::store::{ConnectionsFile, StoreError};
use uuid::Uuid;

fn open(cx: &mut TestAppContext) -> WindowHandle<Root> {
    open_with(cx, Ok(ConnectionsFile::default()))
}

fn open_with(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
) -> WindowHandle<Root> {
    open_state(cx, loaded, Arc::new(MemoryStore::default())).0
}

fn open_state(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
    secrets: Arc<dyn SecretStore>,
) -> (WindowHandle<Root>, Entity<AppState>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        tessera_app::actions::bind_keys(cx);
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(800.)),
        };
        let state = cx.new(|_| AppState::new(loaded, None, secrets));
        let root_state = state.clone();
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| RootView::new(root_state, window, cx)),
        )
        .expect("open test window");
        (window.downcast::<Root>().expect("Base Root"), state)
    })
}

/// Installs a real tokio runtime: keychain and file I/O run on its blocking pool, so the
/// test scheduler must be allowed to park until it answers.
fn tokio(cx: &mut TestAppContext) -> tokio::runtime::Runtime {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let handle = rt.handle().clone();
    cx.update(|cx| cx.set_global(Tokio(handle)));
    cx.executor().allow_parking();
    rt
}

/// A window with a tokio runtime, the given profiles and secret store.
fn open_app(
    cx: &mut TestAppContext,
    profiles: Vec<Profile>,
    secrets: Arc<dyn SecretStore>,
) -> (
    WindowHandle<Root>,
    Entity<AppState>,
    tokio::runtime::Runtime,
) {
    let rt = tokio(cx);
    let file = ConnectionsFile {
        profiles,
        ..Default::default()
    };
    let (handle, state) = open_state(cx, Ok(file), secrets);
    (handle, state, rt)
}

const fn key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}
const SELECT_ALL: &str = key("cmd-a", "ctrl-a");
const DISCONNECT: &str = key("cmd-shift-d", "ctrl-shift-d");
const NEW_CONNECTION: &str = key("cmd-n", "ctrl-n");
const EDIT: &str = key("cmd-e", "ctrl-e");
const TEST: &str = key("cmd-t", "ctrl-t");
const SAVE: &str = key("cmd-enter", "ctrl-enter");
const TOGGLE_KEY: &str = key("alt-cmd-k", "ctrl-alt-k");

/// Installs a connector that answers immediately with `result` (or derives it from the profile).
fn fake_connector(
    cx: &mut TestAppContext,
    answer: impl Fn(&Profile) -> Result<Connected, Error> + Send + Sync + 'static,
) {
    let answer = Arc::new(answer);
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |profile: Profile, _key| {
            let result = answer(&profile);
            Box::pin(async move { result })
        })));
    });
}

/// A connector that records the keys it was called with and answers with `info`.
fn recording_connector(cx: &mut TestAppContext, info: Connected) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, key| {
            record.lock().unwrap().push(key.expose_secret().to_string());
            let info = info.clone();
            Box::pin(async move { Ok(info) })
        })));
    });
    seen
}

async fn open_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(NEW_CONNECTION, cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
}

async fn wait_for_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window
            .try_find("connect.name")
            .is_some_and(|name| name.visible())
    })
    .await;
}

async fn wait_for_close(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.name").is_none()
    })
    .await;
}

/// Replaces the text of input `id`.
fn fill(window: &mut gpui_kit::Window, id: &str, text: &str, cx: &mut gpui_kit::App) {
    window.click(id.to_string(), cx);
    window.press(SELECT_ALL, cx);
    window.press("backspace", cx);
    window.input(text, cx);
}

#[gpui_kit::test]
async fn new_connection_opens_dialog_with_name_focused(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("connect.name").focused(), Some(true));
        assert_eq!(
            window.find("connect.node.0.host").value(),
            Some("localhost")
        );
        assert_eq!(window.find("connect.node.0.port").value(), Some("8108"));
        assert!(
            window.try_find("connect.node.0.remove").is_none(),
            "one node stays"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn welcome_button_opens_dialog(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("welcome.add_connection", cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
}

#[gpui_kit::test]
async fn connect_error_shows_typesense_message(cx: &mut TestAppContext) {
    fake_connector(cx, |_| {
        Err(Error::Http {
            status: 503,
            message: "Not Ready or Lagging".into(),
            body: String::new(),
        })
    });
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(TEST, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "Not Ready or Lagging").await;
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.find("connect.name").visible(), "dialog stays open");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn host_with_scheme_is_rejected(cx: &mut TestAppContext) {
    fake_connector(cx, |_| {
        unreachable!("an invalid host never reaches the connector")
    });
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.node.0.host", "https://ts.example.com/", cx);
        fill(window, "connect.key", "k", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_label(
        cx,
        handle,
        "connect.node.0.error".into(),
        "without protocol or path",
    )
    .await;
    cx.run_until_parked();
    state.read_with(cx, |s, _| assert!(s.profiles.is_empty()));
}

#[gpui_kit::test]
async fn out_of_range_values_are_rejected(cx: &mut TestAppContext) {
    fake_connector(cx, |_| {
        unreachable!("invalid values never reach the connector")
    });
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.node.0.port", "70000", cx);
        window.click("connect.advanced", cx);
        window.render_frame(cx);
        fill(window, "connect.timeout", "0", cx);
        fill(window, "connect.retries", "11", cx);
        window.press(TEST, cx);
        window.render_frame(cx);
        assert!(label(window, "connect.node.0.error").contains("1 and 65535"));
        assert!(label(window, "connect.timeout.error").contains("1 and 300"));
        assert!(label(window, "connect.retries.error").contains("0 and 10"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn escape_closes_dialog(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    wait_for_close(cx, handle).await;
}

#[gpui_kit::test]
async fn key_is_masked_by_default(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("connect.key", cx);
        window.input("secret-key-42", cx);
        let key = window.find("connect.key");
        assert_eq!(key.focused(), Some(true));
        assert_ne!(key.value(), Some("secret-key-42"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn key_toggle_reachable_by_keyboard(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("connect.key", cx);
        window.input("secret-key-42", cx);
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("connect.key.toggle").focused(), Some(true));
        window.press(TOGGLE_KEY, cx);
        window.render_frame(cx);
        assert_eq!(window.find("connect.key").value(), Some("secret-key-42"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn remote_http_shows_warning(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.try_find("connect.http_warning").is_none());
        fill(window, "connect.node.0.host", "ts.example.com", cx);
        window.render_frame(cx);
        assert!(window.find("connect.http_warning").visible());
        window.click("connect.node.0.protocol", cx);
        window.render_frame(cx);
        assert!(window.try_find("connect.http_warning").is_none(), "https");
        window.click("connect.node.0.protocol", cx);
        fill(window, "connect.node.0.host", "localhost", cx);
        window.render_frame(cx);
        assert!(window.try_find("connect.http_warning").is_none(), "local");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn choosing_prod_turns_read_only_on(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("connect.read_only").checked(), Some(false));
        assert_eq!(window.find("connect.env.other").label(), Some("other"));
        assert_eq!(window.find("connect.color.teal").label(), Some("Teal"));
        window.click("connect.env.prod", cx);
        window.render_frame(cx);
        assert_eq!(window.find("connect.read_only").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn restricted_key_result(cx: &mut TestAppContext) {
    let mut info = server("30.2", true);
    info.access = KeyAccess::Restricted;
    fake_connector(cx, move |_| Ok(info.clone()));
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(TEST, cx);
    })
    .unwrap();
    let result = "connect.result".to_string();
    wait_for_label(
        cx,
        handle,
        result.clone(),
        "Restricted key: can read collections, cannot list API keys",
    )
    .await;
    cx.update_window(handle.into(), |_, window, _| {
        let text = label(window, "connect.result");
        assert!(
            text.contains("Server healthy · Typesense 30.2 · Leader"),
            "{text}"
        );
        assert!(text.contains("Tested"), "{text}");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn cancel_during_test_does_not_connect(cx: &mut TestAppContext) {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let called = Arc::new(AtomicBool::new(false));
    let (flag, call) = (dropped.clone(), called.clone());
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, _| {
            call.store(true, Ordering::SeqCst);
            let guard = Dropped(flag.clone());
            Box::pin(async move {
                let _guard = guard;
                std::future::pending().await
            })
        })));
    });
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(TEST, cx);
    })
    .unwrap();
    let running = called.clone();
    cx.wait_for(handle.into(), Duration::from_millis(500), move |_, _| {
        running.load(Ordering::SeqCst)
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    wait_for_close(cx, handle).await;
    cx.run_until_parked();
    assert!(
        dropped.load(Ordering::SeqCst),
        "the test request was dropped"
    );
    state.read_with(cx, |s, _| {
        assert!(s.connections.is_empty());
        assert!(s.profiles.is_empty());
    });
}

#[gpui_kit::test]
async fn save_creates_profile_and_stores_key(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let secrets = Arc::new(MemoryStore::default());
    let (handle, state, _rt) = open_app(cx, Vec::new(), secrets.clone());
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.name", "local-dev", cx);
        fill(window, "connect.node.0.host", "127.0.0.1", cx);
        fill(window, "connect.key", "k-new", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    wait_for_label(cx, handle, "status.text".into(), "local-dev").await;
    let profile = state.read_with(cx, |s, _| {
        assert_eq!(s.profiles.len(), 1);
        s.profiles[0].clone()
    });
    assert_eq!(profile.name, "local-dev");
    assert_eq!(profile.nodes[0].host, "127.0.0.1");
    let stored = secrets.get(profile.id).unwrap().expect("key stored");
    assert_eq!(stored.expose_secret(), "k-new");
    assert!(seen.lock().unwrap().iter().all(|k| k == "k-new"));
}

/// A store whose writes fail like a missing Secret Service.
struct FailingStore;

impl SecretStore for FailingStore {
    fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
        Err(SecretError::new("no Secret Service is running"))
    }
    fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Ok(())
    }
}

#[gpui_kit::test]
async fn keychain_failure_offers_session_only(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(FailingStore));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.name", "no-keychain", cx);
        fill(window, "connect.key", "k-session", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_label(
        cx,
        handle,
        "connect.keychain_error".into(),
        "no Secret Service is running",
    )
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("connect.session_only", cx)
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    wait_for_label(cx, handle, "status.text".into(), "no-keychain").await;
    state.read_with(cx, |s, cx| {
        assert_eq!(s.profiles.len(), 1);
        let id = s.profiles[0].id;
        assert!(s.connection(id, cx).unwrap().read(cx).connected.is_some());
    });
    assert!(seen.lock().unwrap().iter().all(|k| k == "k-session"));
}

#[gpui_kit::test]
async fn edit_with_empty_key_keeps_stored_key(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let (staging, _, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&staging), cx);
        window.press(EDIT, cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("connect.name").value(), Some("staging-eu"));
        // A masked field reports no value; an empty one reports no text either.
        assert_eq!(window.find("connect.key").value().unwrap_or(""), "");
        fill(window, "connect.name", "staging-eu-2", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    wait_for_label(cx, handle, "status.text".into(), "staging-eu-2").await;
    state.read_with(cx, |s, _| {
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.profiles[0].name, "staging-eu-2");
        assert_eq!(s.profiles[0].environment, Environment::Staging);
    });
    let stored = secrets.get(staging.id).unwrap().expect("key kept");
    assert_eq!(stored.expose_secret(), "k");
    assert!(!seen.lock().unwrap().is_empty());
    assert!(seen.lock().unwrap().iter().all(|k| k == "k"));
}

#[gpui_kit::test]
async fn missing_key_offers_edit(cx: &mut TestAppContext) {
    fake_connector(cx, |_| unreachable!("no key, no request"));
    let profile = Profile::new("keyless", vec![node()]);
    let (handle, _state, _rt) =
        open_app(cx, vec![profile.clone()], Arc::new(MemoryStore::default()));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(entry(&profile), cx);
    })
    .unwrap();
    let add_key = format!("{}.add_key", entry(&profile));
    let target = add_key.clone();
    cx.wait_for(
        handle.into(),
        Duration::from_millis(1000),
        move |window, _| window.try_find(target.clone()).is_some(),
    )
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &add_key, cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("connect.name").value(), Some("keyless"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn shell_shows_not_connected(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("status.text").label(), Some("Not connected"));
        assert_eq!(
            window.find("titlebar.connection").label(),
            Some("No connection")
        );
        assert!(window.find("sidebar").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn load_error_shows_on_welcome(cx: &mut TestAppContext) {
    let handle = open_with(cx, Err(StoreError::NewerSchema(9)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let text = label(window, "welcome.load_error");
        assert!(text.contains("schema version 9"), "{text:?}");
    })
    .unwrap();
}

fn server(version: &str, healthy: bool) -> Connected {
    let client = Client::new(
        &["http://localhost:8108"],
        "k".to_string().into(),
        ClientOptions::default(),
    )
    .unwrap();
    Connected {
        info: ServerInfo {
            url: client.base_url().clone(),
            version: version.into(),
            state: tessera_client::RaftState::Leader,
            healthy,
            capabilities: tessera_core::ServerCapabilities::from_version(version),
            latency: Duration::from_millis(11),
        },
        access: KeyAccess::Admin,
        client,
    }
}

async fn connect(
    cx: &mut TestAppContext,
    info: Connected,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    fake_connector(cx, move |_| Ok(info.clone()));
    let (handle, _, rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.name").is_none() && window.try_find("overview.version").is_some()
    })
    .await;
    (handle, rt)
}

fn label(window: &mut gpui_kit::Window, id: &'static str) -> String {
    window.find(id).label().unwrap_or_default().to_string()
}

#[gpui_kit::test]
async fn connect_success_shows_version_in_status_bar(cx: &mut TestAppContext) {
    let (handle, _rt) = connect(cx, server("30.2", true)).await;
    cx.update_window(handle.into(), |_, window, _| {
        let status = label(window, "status.text");
        for part in ["Connected", "localhost:8108", "v30.2", "Leader", "11 ms"] {
            assert!(status.contains(part), "{part:?} missing in {status:?}");
        }
        assert!(label(window, "overview.version").contains("30.2"));
        assert!(label(window, "overview.role").contains("Leader"));
        assert!(window.find("sidebar.overview").visible());
        assert!(label(window, "titlebar.connection").contains("localhost:8108"));
        assert!(window.try_find("overview.unsupported").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn old_server_shows_warning(cx: &mut TestAppContext) {
    let (handle, _rt) = connect(cx, server("29.0", true)).await;
    cx.update_window(handle.into(), |_, window, _| {
        assert!(label(window, "overview.unsupported").contains("29.0"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn unhealthy_server_shows_unhealthy(cx: &mut TestAppContext) {
    let (handle, _rt) = connect(cx, server("30.2", false)).await;
    cx.update_window(handle.into(), |_, window, _| {
        assert!(label(window, "overview.health").contains("Unhealthy"));
    })
    .unwrap();
}

fn node() -> Node {
    Node {
        protocol: Protocol::Http,
        host: "localhost".into(),
        port: 8108,
        path: String::new(),
    }
}

/// A staging and a read-only prod profile, both with a stored key.
fn two_profiles() -> (Profile, Profile, Arc<MemoryStore>) {
    let mut staging = Profile::new("staging-eu", vec![node()]);
    staging.environment = Environment::Staging;
    staging.color = ProfileColor::Amber;
    let mut prod = Profile::new("prod-ch", vec![node()]);
    prod.environment = Environment::Prod;
    prod.color = ProfileColor::Red;
    prod.read_only = true;
    let secrets = Arc::new(MemoryStore::default());
    for p in [&staging, &prod] {
        secrets.set(p.id, &"k".to_string().into()).unwrap();
    }
    (staging, prod, secrets)
}

/// Opens a window with saved `profiles`.
fn open_profiles(
    cx: &mut TestAppContext,
    profiles: Vec<Profile>,
    secrets: Arc<MemoryStore>,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    let (handle, _, rt) = open_app(cx, profiles, secrets);
    (handle, rt)
}

fn entry(profile: &Profile) -> String {
    format!("sidebar.profile.{}", profile.id)
}

/// Presses Tab until `id` has keyboard focus.
fn focus(window: &mut gpui_kit::Window, id: &str, cx: &mut gpui_kit::App) {
    window.render_frame(cx);
    for _ in 0..20 {
        if window.find(id.to_string()).focused() == Some(true) {
            return;
        }
        window.press("tab", cx);
    }
    panic!("Tab never focused {id}");
}

async fn wait_for_label(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    id: String,
    part: &'static str,
) {
    cx.wait_for(
        handle.into(),
        Duration::from_millis(1000),
        move |window, _| {
            window
                .try_find(id.clone())
                .and_then(|e| e.label().map(|l| l.contains(part)))
                .unwrap_or(false)
        },
    )
    .await;
}

#[gpui_kit::test]
fn sidebar_lists_profiles_with_badges(cx: &mut TestAppContext) {
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(entry(&staging)).visible());
        assert!(window.find(entry(&prod)).visible());
        assert!(window.find(format!("{}.readonly", entry(&prod))).visible());
        let env = |p: &Profile| format!("{}.env", entry(p));
        assert_eq!(window.find(env(&prod)).label(), Some("PROD"));
        assert_eq!(window.find(env(&staging)).label(), Some("STAGING"));
        assert!(
            window
                .try_find(format!("{}.readonly", entry(&staging)))
                .is_none()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn enter_on_profile_connects(cx: &mut TestAppContext) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    fake_connector(cx, move |p| {
        record.lock().unwrap().push(p.name.clone());
        Ok(server("30.2", true))
    });
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&staging), cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    assert_eq!(*seen.lock().unwrap(), ["staging-eu"]);
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&staging), cx);
        window.press(DISCONNECT, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "Not connected").await;
}

#[gpui_kit::test]
async fn connect_failure_shows_inline_under_entry(cx: &mut TestAppContext) {
    fake_connector(cx, |_| {
        Err(Error::Http {
            status: 401,
            message: "Forbidden - a valid `x-typesense-api-key` header must be sent.".into(),
            body: String::new(),
        })
    });
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(entry(&staging), cx);
    })
    .unwrap();
    let error = format!("{}.error", entry(&staging));
    wait_for_label(
        cx,
        handle,
        error,
        "a valid `x-typesense-api-key` header must be sent.",
    )
    .await;
}

#[gpui_kit::test]
async fn prod_selection_draws_frame(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    let frame = |cx: &mut TestAppContext, click: Option<&Profile>| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            if let Some(p) = click {
                window.click(entry(p), cx);
                window.render_frame(cx);
            }
            let frame = window.try_find("window.prod_frame").is_some();
            let band = window.try_find("window.prod_band").is_some();
            assert_eq!(frame, band, "band and frame go together");
            frame
        })
        .unwrap()
    };
    assert!(!frame(cx, None), "nothing selected");
    assert!(frame(cx, Some(&prod)), "prod selected");
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("titlebar.env").label(), Some("PROD"));
    })
    .unwrap();
    assert!(!frame(cx, Some(&staging)), "staging selected");
}

#[gpui_kit::test]
async fn timeout_text_uses_profile_timeout(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Err(Error::Timeout));
    let (mut staging, _, secrets) = two_profiles();
    staging.timeout_secs = 12;
    let (handle, _rt) = open_profiles(cx, vec![staging.clone()], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(entry(&staging), cx);
    })
    .unwrap();
    wait_for_label(
        cx,
        handle,
        format!("{}.error", entry(&staging)),
        "12 seconds",
    )
    .await;
}

/// Opens the entry's context menu and clicks its first item, which must read `label`.
async fn menu_first_item(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entry: String,
    label: &'static str,
) {
    menu_item(cx, handle, entry, 0, label).await;
}

/// Opens the entry's context menu and clicks item `index`, which must read `label`.
async fn menu_item(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entry: String,
    index: usize,
    label: &'static str,
) {
    cx.update_window(handle.into(), |_, window, cx| window.right_click(entry, cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window.try_find("popup-menu").is_some()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let mut menu = window.within("popup-menu");
        assert_eq!(menu.find(index).label(), Some(label));
        menu.click(index, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn context_menu_connects_and_disconnects(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod], secrets);
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    menu_first_item(cx, handle, entry(&staging), "Connect").await;
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    menu_first_item(cx, handle, entry(&staging), "Disconnect").await;
    wait_for_label(cx, handle, "status.text".into(), "Not connected").await;
}

#[gpui_kit::test]
async fn context_menu_edit_opens_dialog(cx: &mut TestAppContext) {
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging, prod.clone()], secrets);
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    menu_item(cx, handle, entry(&prod), 1, "Edit…").await;
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("connect.name").value(), Some("prod-ch"));
        assert_eq!(window.find("connect.read_only").checked(), Some(true));
    })
    .unwrap();
}

/// Shortcut labels come from GPUI Kit's `Kbd`, which formats them per platform.
#[test]
fn no_hardcoded_shortcut_labels() {
    const LABELS: [&str; 4] = ["Ctrl+", "Cmd+", "Strg+", "⌘"];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    let mut check = |path: &std::path::Path, text: &str| {
        for label in LABELS {
            if text.contains(label) {
                hits.push(format!("{}: {label:?} in {text:?}", path.display()));
            }
        }
    };
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // The unit tests at the end of a file may spell shortcuts out.
            let code = text
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or_default();
            for literal in string_literals(code) {
                check(&path, &literal);
            }
        }
    }
    for entry in std::fs::read_dir(root.join("../../assets/locales")).unwrap() {
        let path = entry.unwrap().path();
        check(&path, &std::fs::read_to_string(&path).unwrap());
    }
    assert!(
        hits.is_empty(),
        "hand-written shortcut labels:\n{}",
        hits.join("\n")
    );
}

/// The contents of the `"…"` literals in Rust source. Skips `'"'` char literals; raw strings
/// are read as ordinary ones, which is enough to find a label inside them.
fn string_literals(code: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = code.chars().peekable();
    let mut current: Option<String> = None;
    while let Some(c) = chars.next() {
        match (&mut current, c) {
            (None, '\'') => {
                // A char literal such as '"' or '\"' must not open a string.
                let rest: String = chars.clone().take(3).collect();
                if rest.starts_with("\"'") {
                    chars.nth(1);
                } else if rest.starts_with("\\\"'") {
                    chars.nth(2);
                }
            }
            (None, '"') => current = Some(String::new()),
            (Some(literal), '\\') => {
                literal.push(c);
                literal.extend(chars.next());
            }
            (Some(_), '"') => literals.extend(current.take()),
            (Some(literal), c) => literal.push(c),
            (None, _) => {}
        }
    }
    literals
}

#[test]
fn string_literals_are_found() {
    let code = r#"let a = "x ⌘N"; let q = '"'; let b = "Ctrl+\"N\""; // "c""#;
    assert_eq!(string_literals(code), ["x ⌘N", r#"Ctrl+\"N\""#, "c"]);
}
