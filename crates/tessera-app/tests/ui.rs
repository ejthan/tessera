// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
    base::Root, px, size,
};
use tessera_app::connection::{Connected, ConnectorGlobal, ServerInfo};
use tessera_app::runtime::Tokio;
use tessera_app::state::AppState;
use tessera_app::views::root::RootView;
use tessera_client::{Client, ClientOptions, Error, KeyAccess};
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use tessera_core::secrets::{MemoryStore, SecretStore as _};
use tessera_core::store::{ConnectionsFile, StoreError};

fn open(cx: &mut TestAppContext) -> WindowHandle<Root> {
    open_with(cx, Ok(ConnectionsFile::default()))
}

fn open_with(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
) -> WindowHandle<Root> {
    open_state(cx, loaded, Arc::default())
}

fn open_state(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
    secrets: Arc<MemoryStore>,
) -> WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        tessera_app::actions::bind_keys(cx);
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(800.)),
        };
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let state = cx.new(|_| AppState::new(loaded, None, secrets));
                cx.new(|cx| RootView::new(state, window, cx))
            },
        )
        .expect("open test window");
        window.downcast::<Root>().expect("Base Root")
    })
}

const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};
const NEW_CONNECTION: &str = if cfg!(target_os = "macos") {
    "cmd-n"
} else {
    "ctrl-n"
};

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

async fn open_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(NEW_CONNECTION, cx);
    })
    .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window
            .try_find("connect.url")
            .is_some_and(|url| url.visible())
    })
    .await;
}

#[gpui_kit::test]
async fn new_connection_opens_dialog_with_url_focused(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, _| {
        let url = window.find("connect.url");
        assert_eq!(url.focused(), Some(true));
        assert_eq!(url.value(), Some("http://localhost:8108"));
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
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window.try_find("connect.url").is_some()
    })
    .await;
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
    cx.update_window(handle.into(), |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window
            .try_find("connect.error")
            .and_then(|e| e.label().map(|l| l.contains("Not Ready or Lagging")))
            .unwrap_or(false)
    })
    .await;
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.find("connect.url").visible(), "dialog stays open");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn invalid_url_shows_inline_error(cx: &mut TestAppContext) {
    fake_connector(cx, |_| {
        unreachable!("a URL without a scheme never reaches the connector")
    });
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(SELECT_ALL, cx);
        window.input("localhost:8108", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window
            .try_find("connect.error")
            .and_then(|e| e.label().map(|l| l.contains("http:// or https://")))
            .unwrap_or(false)
    })
    .await;
}

#[gpui_kit::test]
async fn escape_closes_dialog(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.url").is_none()
    })
    .await;
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

async fn connect(cx: &mut TestAppContext, info: Connected) -> WindowHandle<Root> {
    fake_connector(cx, move |_| Ok(info.clone()));
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.url").is_none() && window.try_find("overview.version").is_some()
    })
    .await;
    handle
}

fn label(window: &mut gpui_kit::Window, id: &'static str) -> String {
    window.find(id).label().unwrap_or_default().to_string()
}

#[gpui_kit::test]
async fn connect_success_shows_version_in_status_bar(cx: &mut TestAppContext) {
    let handle = connect(cx, server("30.2", true)).await;
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
    let handle = connect(cx, server("29.0", true)).await;
    cx.update_window(handle.into(), |_, window, _| {
        assert!(label(window, "overview.unsupported").contains("29.0"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn unhealthy_server_shows_unhealthy(cx: &mut TestAppContext) {
    let handle = connect(cx, server("30.2", false)).await;
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

/// Opens a window with saved `profiles`. Stored keys are read on tokio's blocking pool, so
/// this installs a runtime and lets the test scheduler park until it answers.
fn open_profiles(
    cx: &mut TestAppContext,
    profiles: Vec<Profile>,
    secrets: Arc<MemoryStore>,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let handle = rt.handle().clone();
    cx.update(|cx| cx.set_global(Tokio(handle)));
    cx.executor().allow_parking();
    let file = ConnectionsFile {
        profiles,
        ..Default::default()
    };
    (open_state(cx, Ok(file), secrets), rt)
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
            window.try_find("window.prod_frame").is_some()
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
    cx.update_window(handle.into(), |_, window, cx| window.right_click(entry, cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window.try_find("popup-menu").is_some()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let mut menu = window.within("popup-menu");
        assert_eq!(menu.find(0usize).label(), Some(label));
        menu.click(0usize, cx);
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

/// Shortcut labels come from GPUI Kit's `Kbd`, which formats them per platform.
#[test]
fn no_hardcoded_shortcut_labels() {
    fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, hits);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Unit tests sit at the end of a file and may spell shortcuts out.
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (i, line) in code.lines().enumerate() {
                if line.contains("\"Ctrl+") || line.contains("\"⌘") {
                    hits.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    }
    let mut hits = Vec::new();
    scan(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
    );
    assert!(
        hits.is_empty(),
        "hand-written shortcut labels:\n{}",
        hits.join("\n")
    );
}
