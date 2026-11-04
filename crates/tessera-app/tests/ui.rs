// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
    base::Root, px, size,
};
use tessera_app::connection::{ConnectorGlobal, ServerInfo};
use tessera_app::views::root::RootView;
use tessera_client::Error;

fn open(cx: &mut TestAppContext) -> WindowHandle<Root> {
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
            |window, cx| cx.new(|cx| RootView::new(window, cx)),
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

/// Installs a connector that answers immediately with `result` (or derives it from the URL).
fn fake_connector(
    cx: &mut TestAppContext,
    answer: impl Fn(&str) -> Result<ServerInfo, Error> + Send + Sync + 'static,
) {
    let answer = Arc::new(answer);
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |url: String, _key| {
            let result = answer(&url);
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
    fake_connector(cx, |url| {
        if url.contains("://") {
            unreachable!("test types a URL without a scheme")
        }
        Err(Error::InvalidUrl(url.to_string()))
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

fn server(version: &str, healthy: bool) -> ServerInfo {
    ServerInfo {
        url: "http://localhost:8108/".parse().unwrap(),
        version: version.into(),
        state: tessera_client::RaftState::Leader,
        healthy,
        capabilities: tessera_core::ServerCapabilities::from_version(version),
        latency: Duration::from_millis(11),
    }
}

async fn connect(cx: &mut TestAppContext, info: ServerInfo) -> WindowHandle<Root> {
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
