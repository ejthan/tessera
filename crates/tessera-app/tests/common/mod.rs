// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers shared by the UI test binaries.

// Each test binary uses only some of the helpers.
#![allow(dead_code)]
// Helpers unwrap; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, base::Root, px, size,
};
use secrecy::ExposeSecret as _;
use tessera_app::connection::{Connected, ConnectorGlobal, ServerInfo};
use tessera_app::poller::{CollectionsFetcherGlobal, NodeSample, PollFetcherGlobal, SampleFuture};
use tessera_app::runtime::Tokio;
use tessera_app::state::AppState;
use tessera_app::views::root::RootView;
use tessera_client::{
    Client, ClientOptions, CollectionSummary, Debug, Error, Health, KeyAccess, Metrics, NodeClient,
    Origin, Stats,
};
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use tessera_core::secrets::{MemoryStore, SecretStore};
use tessera_core::store::{ConnectionsFile, StoreError};

pub fn open(cx: &mut TestAppContext) -> WindowHandle<Root> {
    open_with(cx, Ok(ConnectionsFile::default()))
}

pub fn open_with(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
) -> WindowHandle<Root> {
    open_state(cx, loaded, Arc::new(MemoryStore::default())).0
}

pub fn open_state(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
    secrets: Arc<dyn SecretStore>,
) -> (WindowHandle<Root>, Entity<AppState>) {
    open_state_at(cx, loaded, None, secrets)
}

/// Like [`open_state`], with profiles saved to `path`.
pub fn open_state_at(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
    path: Option<std::path::PathBuf>,
    secrets: Arc<dyn SecretStore>,
) -> (WindowHandle<Root>, Entity<AppState>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        tessera_app::actions::bind_keys(cx);
        install_poll_fakes(cx);
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(800.)),
        };
        let state = cx.new(|cx| {
            let mut state = AppState::new(loaded, path, secrets);
            state.spawn_log_drain(cx);
            state
        });
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
pub fn tokio(cx: &mut TestAppContext) -> tokio::runtime::Runtime {
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
pub fn open_app(
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

/// A node sample where every request succeeded.
pub fn ok_sample() -> NodeSample {
    NodeSample {
        health: Ok(Health { ok: true }),
        debug: Ok(Debug {
            version: "30.2".into(),
            state: tessera_client::RaftState::Leader,
        }),
        metrics: Ok(Metrics {
            cpu_percent: Some(12.5),
            memory_used: Some(1),
            memory_total: Some(4),
            disk_used: Some(1),
            disk_total: Some(2),
            net_received: Some(0),
            net_sent: Some(0),
            typesense_memory_active: Some(1),
            raw: Default::default(),
        }),
        stats: Ok(Stats {
            latency_ms: Default::default(),
            requests_per_second: Default::default(),
            total_requests_per_second: None,
            raw: Default::default(),
        }),
        latency: Duration::from_millis(3),
    }
}

/// Unless the test installed its own, a node fetcher that answers with [`ok_sample`] and a
/// collections loader that answers with no collections.
fn install_poll_fakes(cx: &mut gpui_kit::App) {
    if !cx.has_global::<PollFetcherGlobal>() {
        cx.set_global(PollFetcherGlobal(Arc::new(|_| {
            Box::pin(async { ok_sample() })
        })));
    }
    if !cx.has_global::<CollectionsFetcherGlobal>() {
        cx.set_global(CollectionsFetcherGlobal(Arc::new(|_| {
            Box::pin(async { Ok(Vec::new()) })
        })));
    }
}

/// Installs a node fetcher that answers with `answer` and records every node URL it polls.
pub fn fake_poll(
    cx: &mut TestAppContext,
    answer: impl Fn(&NodeClient) -> NodeSample + Send + Sync + 'static,
) -> Arc<Mutex<Vec<String>>> {
    fake_poll_async(cx, move |node| {
        let sample = answer(node);
        Box::pin(async move { sample })
    })
}

/// Like [`fake_poll`], with an answer that may take time or never come.
pub fn fake_poll_async(
    cx: &mut TestAppContext,
    answer: impl Fn(&NodeClient) -> SampleFuture + Send + Sync + 'static,
) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(PollFetcherGlobal(Arc::new(move |node: NodeClient| {
            record.lock().unwrap().push(node.url().to_string());
            answer(&node)
        })));
    });
    seen
}

/// Installs a collections loader that records the origin of each client it loads through.
pub fn fake_collections(cx: &mut TestAppContext) -> Arc<Mutex<Vec<Origin>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(CollectionsFetcherGlobal(Arc::new(move |client: Client| {
            record.lock().unwrap().push(client.origin());
            Box::pin(async {
                Ok(vec![CollectionSummary {
                    name: "books".into(),
                    num_documents: 7,
                }])
            })
        })));
    });
    seen
}

/// A connection to the nodes of `profile`, as the fake connector answers it.
pub fn server_for(profile: &Profile) -> Connected {
    let (nodes, options) = tessera_app::connection::client_options(profile).unwrap();
    let nodes: Vec<&str> = nodes.iter().map(String::as_str).collect();
    let mut connected = server("30.2", true);
    connected.client = Client::new(&nodes, "k".to_string().into(), options).unwrap();
    connected.info.url = connected.client.base_url().clone();
    connected
}

pub const fn key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}
pub const SELECT_ALL: &str = key("cmd-a", "ctrl-a");
pub const DISCONNECT: &str = key("cmd-shift-d", "ctrl-shift-d");
pub const NEW_CONNECTION: &str = key("cmd-n", "ctrl-n");
pub const EDIT: &str = key("cmd-e", "ctrl-e");
pub const TEST: &str = key("cmd-t", "ctrl-t");
pub const SAVE: &str = key("cmd-enter", "ctrl-enter");
pub const TOGGLE_KEY: &str = key("alt-cmd-k", "ctrl-alt-k");
pub const DELETE: &str = key("cmd-backspace", "ctrl-backspace");
pub const TOGGLE_LOG: &str = key("cmd-j", "ctrl-j");
pub const CLOSE_TAB: &str = key("cmd-w", "ctrl-w");
pub const COPY_CURL: &str = key("cmd-shift-c", "ctrl-shift-c");

/// Installs a connector that answers immediately with `result` (or derives it from the profile).
pub fn fake_connector(
    cx: &mut TestAppContext,
    answer: impl Fn(&Profile) -> Result<Connected, Error> + Send + Sync + 'static,
) {
    let answer = Arc::new(answer);
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(
            move |profile: Profile, _key, _hook| {
                let result = answer(&profile);
                Box::pin(async move { result })
            },
        )));
    });
}

/// A connector that records the keys it was called with and answers with `info`.
pub fn recording_connector(cx: &mut TestAppContext, info: Connected) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, key, _hook| {
            record.lock().unwrap().push(key.expose_secret().to_string());
            let info = info.clone();
            Box::pin(async move { Ok(info) })
        })));
    });
    seen
}

pub async fn open_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(NEW_CONNECTION, cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
}

pub async fn wait_for_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window
            .try_find("connect.name")
            .is_some_and(|name| name.visible())
    })
    .await;
}

pub async fn wait_for_close(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.name").is_none()
    })
    .await;
}

/// Replaces the text of input `id`.
pub fn fill(window: &mut gpui_kit::Window, id: &str, text: &str, cx: &mut gpui_kit::App) {
    window.click(id.to_string(), cx);
    window.press(SELECT_ALL, cx);
    window.press("backspace", cx);
    window.input(text, cx);
}

pub fn server(version: &str, healthy: bool) -> Connected {
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

pub async fn connect(
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

pub fn label(window: &mut gpui_kit::Window, id: &'static str) -> String {
    window.find(id).label().unwrap_or_default().to_string()
}

pub fn node() -> Node {
    Node {
        protocol: Protocol::Http,
        host: "localhost".into(),
        port: 8108,
        path: String::new(),
    }
}

/// A staging and a read-only prod profile, both with a stored key.
pub fn two_profiles() -> (Profile, Profile, Arc<MemoryStore>) {
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
pub fn open_profiles(
    cx: &mut TestAppContext,
    profiles: Vec<Profile>,
    secrets: Arc<MemoryStore>,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    let (handle, _, rt) = open_app(cx, profiles, secrets);
    (handle, rt)
}

pub fn entry(profile: &Profile) -> String {
    format!("sidebar.profile.{}", profile.id)
}

/// Presses Tab until `id` has keyboard focus.
pub fn focus(window: &mut gpui_kit::Window, id: &str, cx: &mut gpui_kit::App) {
    window.render_frame(cx);
    for _ in 0..20 {
        if window.find(id.to_string()).focused() == Some(true) {
            return;
        }
        window.press("tab", cx);
    }
    panic!("Tab never focused {id}");
}

pub async fn wait_for_label(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    id: String,
    part: &'static str,
) {
    cx.wait_for(
        handle.into(),
        Duration::from_millis(10_000),
        move |window, _| {
            window
                .try_find(id.clone())
                .and_then(|e| e.label().map(|l| l.contains(part)))
                .unwrap_or(false)
        },
    )
    .await;
}

/// Opens the entry's context menu and clicks its first item, which must read `label`.
pub async fn menu_first_item(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entry: String,
    label: &'static str,
) {
    menu_item(cx, handle, entry, 0, label).await;
}

/// Opens the entry's context menu and clicks item `index`, which must read `label`.
pub async fn menu_item(
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

/// Waits until element `id` is (`true`) or is not (`false`) in the window.
pub async fn wait_for_element(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    id: String,
    present: bool,
) {
    cx.wait_for(
        handle.into(),
        Duration::from_millis(1000),
        move |window, _| window.try_find(id.clone()).is_some() == present,
    )
    .await;
}
