// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the window shell in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use gpui_kit::base::{Placement, Root};
use gpui_kit::component::dock::{DockArea, DockPlacement, InsertTarget, NodeId, PaneRef, PanelId};
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, WindowHandle, px};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tessera_app::actions::{Refresh, ToggleJobs, ToggleKeyMask, ToggleRequestLog, apply_keymap};
use tessera_app::state::AppState;
use tessera_app::views::dock::OverviewPanel;
use tessera_app::views::root::RootView;
use tessera_core::keymap::UserBinding;
use tessera_core::layout::BottomTab;
use tessera_core::profile::{Environment, Profile};
use tessera_core::store::{ConnectionsFile, StoreError};

use common::*;

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

#[gpui_kit::test]
async fn connect_success_shows_version_in_status_bar(cx: &mut TestAppContext) {
    let (handle, _rt) = connect(cx, server("30.2", true)).await;
    cx.update_window(handle.into(), |_, window, _| {
        let status = label(window, "status.text");
        // The latency of the first poll (3 ms in the fake), not the connect's 11 ms.
        for part in ["Connected", "localhost:8108", "v30.2", "Leader", "3 ms"] {
            assert!(status.contains(part), "{part:?} missing in {status:?}");
        }
        assert!(label(window, "figure.version").contains("30.2"));
        assert!(label(window, "node.0").contains("Leader"));
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
    rust_i18n::set_locale("en");
    fake_poll(cx, |_| {
        let mut sample = ok_sample();
        sample.health = Ok(tessera_client::Health { ok: false });
        sample
    });
    let (handle, _rt) = connect(cx, server("30.2", false)).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(label(window, "node.0.error").contains("not ready"));
    })
    .unwrap();
}

/// The id of `profile`'s overview tab title.
fn tab(profile: &Profile) -> String {
    format!("tab.{}.overview", profile.id)
}

/// Clicks `profile`'s sidebar entry and waits until its overview tab is active.
async fn open_tab(cx: &mut TestAppContext, handle: WindowHandle<Root>, profile: &Profile) {
    let id = entry(profile);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, tab(profile), true).await;
    wait_for_label(cx, handle, "status.text".into(), name_of(profile)).await;
}

/// The name of one of the two profiles of `two_profiles`.
fn name_of(profile: &Profile) -> &'static str {
    if profile.environment == Environment::Prod {
        "prod-ch"
    } else {
        "staging-eu"
    }
}

/// Whether the prod band and frame are drawn; they always go together.
fn prod_frame(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) -> bool {
    window.render_frame(cx);
    let frame = window.try_find("window.prod_frame").is_some();
    let band = window.try_find("window.prod_band").is_some();
    assert_eq!(frame, band, "band and frame go together");
    frame
}

#[gpui_kit::test]
async fn connecting_opens_overview_tab(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("welcome.add_connection").visible());
    })
    .unwrap();
    open_tab(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(tab(&staging)).visible());
        assert_eq!(window.find(tab(&staging)).label(), Some("staging-eu"));
        assert!(window.find("figure.version").visible());
        assert!(window.try_find("welcome.add_connection").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn reopening_focuses_existing_tab(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    open_tab(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find_all(tab(&staging)).len(), 1);
        assert_eq!(window.find_all(tab(&prod)).len(), 1);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn disconnect_closes_tabs(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod], secrets);
    open_tab(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&staging), cx);
        window.press(DISCONNECT, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, tab(&staging), false).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("welcome.add_connection").visible());
        assert_eq!(window.find("status.text").label(), Some("Not connected"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn prod_frame_follows_active_tab(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!prod_frame(window, cx), "no tab");
    })
    .unwrap();
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(prod_frame(window, cx), "prod tab active");
        assert_eq!(window.find("titlebar.env").label(), Some("PROD"));
        window.click(tab(&staging), cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!prod_frame(window, cx), "staging tab active");
        assert_eq!(window.find("titlebar.env").label(), Some("STAGING"));
        window.click(tab(&prod), cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(prod_frame(window, cx), "prod tab active again");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn cmd_j_toggles_bottom_dock(cx: &mut TestAppContext) {
    let handle = open(cx);
    let open = |cx: &mut TestAppContext, press: Option<&str>| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            if let Some(key) = press {
                window.press(key, cx);
                window.render_frame(cx);
            }
            window.try_find("dock.bottom").is_some()
        })
        .unwrap()
    };
    assert!(!open(cx, None), "closed at start");
    assert!(open(cx, Some(TOGGLE_LOG)));
    assert!(!open(cx, Some(TOGGLE_LOG)));
    // The status bar button does the same.
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("status.toggle_log", cx)
    })
    .unwrap();
    wait_for_element(cx, handle, "dock.bottom".into(), true).await;
}

#[gpui_kit::test]
async fn sidebar_overview_focuses_tab(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    let child = |p: &Profile| format!("{}.overview", entry(p));
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find(child(&staging)).label(), Some("Overview"));
        focus(window, &child(&staging), cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(child(&prod), cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find_all(tab(&staging)).len(), 1);
        assert_eq!(window.find_all(tab(&prod)).len(), 1);
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

#[gpui_kit::test]
async fn closing_a_tab_keeps_the_connection(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    // Clicking the tab focuses it, so the close shortcut reaches its tab group.
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(tab(&staging), cx)
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| window.press(CLOSE_TAB, cx))
        .unwrap();
    wait_for_element(cx, handle, tab(&staging), false).await;
    wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
    state.read_with(cx, |s, cx| {
        let conn = s.connection(staging.id, cx).expect("still open");
        assert!(conn.read(cx).connected.is_some());
    });
}

fn open_with_state(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<AppState>) {
    let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
    open_state(cx, Ok(ConnectionsFile::default()), secrets)
}

/// A root built on `state`, as a new start builds it.
fn rebuilt_dock(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    state: &Entity<AppState>,
) -> Entity<DockArea> {
    let state = state.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        let root = cx.new(|cx| RootView::new(state, true, window, cx));
        root.read(cx).dock().clone()
    })
    .unwrap()
}

fn is_at(area: &Entity<DockArea>, cx: &mut TestAppContext, left: f32, bottom: f32) -> bool {
    cx.read(|cx| {
        let area = area.read(cx);
        area.dock_size(DockPlacement::Left) == Some(px(left))
            && area.dock_size(DockPlacement::Bottom) == Some(px(bottom))
    })
}

#[gpui_kit::test]
async fn sizes_and_the_bottom_tab_survive_a_rebuilt_root(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout").join("main.json");
    let (handle, state) = open_with_state(cx);
    cx.update(|cx| state.update(cx, |s, _| s.load_layout(Some(path.clone()))));
    let area = dock(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_action(Box::new(ToggleJobs), cx);
        area.update(cx, |area, cx| {
            area.set_dock_size(DockPlacement::Left, px(300.), window, cx);
            area.set_dock_size(DockPlacement::Bottom, px(320.), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!path.exists(), "the save waits a moment");
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let saved = tessera_core::layout::load(&path);
    assert_eq!(saved.left_width, 300.);
    assert_eq!(saved.bottom_height, 320.);
    assert!(saved.bottom_open);
    assert_eq!(saved.bottom_tab, BottomTab::Jobs);

    let rebuilt = rebuilt_dock(cx, handle, &state);
    assert!(is_at(&rebuilt, cx, 300., 320.));
    cx.read(|cx| assert!(rebuilt.read(cx).is_dock_open(DockPlacement::Bottom)));
}

#[gpui_kit::test]
async fn a_newer_change_drops_the_pending_save(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.json");
    let (handle, state) = open_with_state(cx);
    cx.update(|cx| state.update(cx, |s, _| s.load_layout(Some(path.clone()))));
    let area = dock(cx, handle);
    for width in [300., 310.] {
        cx.update_window(handle.into(), |_, window, cx| {
            area.update(cx, |area, cx| {
                area.set_dock_size(DockPlacement::Left, px(width), window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
    }
    assert!(!path.exists());
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(tessera_core::layout::load(&path).left_width, 310.);
}

#[gpui_kit::test]
async fn a_bad_layout_file_starts_with_the_defaults(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.json");
    std::fs::write(&path, "not json").unwrap();
    let (handle, state) = open_with_state(cx);
    cx.update(|cx| state.update(cx, |s, _| s.load_layout(Some(path))));
    let rebuilt = rebuilt_dock(cx, handle, &state);
    assert!(is_at(&rebuilt, cx, 240., 240.));
    cx.read(|cx| assert!(!rebuilt.read(cx).is_dock_open(DockPlacement::Bottom)));
}

#[gpui_kit::test]
fn welcome_offers_each_way_in(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for id in [
            "welcome.add_connection",
            "welcome.import",
            "welcome.copy",
            "welcome.connect_local",
        ] {
            assert!(window.find(id).visible(), "{id}");
        }
    })
    .unwrap();
}

#[gpui_kit::test]
async fn an_empty_log_and_empty_jobs_say_what_comes(cx: &mut TestAppContext) {
    let handle = open(cx);
    for action in [true, false] {
        cx.update_window(handle.into(), |_, window, cx| {
            if action {
                window.dispatch_action(Box::new(ToggleRequestLog), cx);
            } else {
                window.dispatch_action(Box::new(ToggleJobs), cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        let id = if action { "log.empty" } else { "jobs.docs" };
        wait_for_element(cx, handle, id.into(), true).await;
    }
}

/// The dock area of the window.
fn dock(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Entity<DockArea> {
    handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            view.read(cx).dock().clone()
        })
        .unwrap()
}

/// The panel id of `profile`'s overview tab and the tab group holding it.
fn tab_panel(area: &DockArea, profile: &Profile, cx: &gpui_kit::App) -> (PanelId, NodeId) {
    let center = area.layout(DockPlacement::Center).unwrap();
    let id = center
        .panels()
        .find(|id| {
            let view = area.panel(*id).unwrap().view().downcast::<OverviewPanel>();
            view.is_ok_and(|v| v.read(cx).connection().read(cx).profile.id == profile.id)
        })
        .unwrap();
    (id, center.find_panel_node(id).unwrap())
}

/// Clicks `profile`'s "Overview" row in the sidebar and waits until its tab is active.
async fn click_overview(cx: &mut TestAppContext, handle: WindowHandle<Root>, profile: &Profile) {
    let id = format!("{}.overview", entry(profile));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), name_of(profile)).await;
}

#[gpui_kit::test]
async fn sidebar_activates_a_tab_in_another_split_group(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    let area = dock(cx, handle);
    // Drag prod into its own group right of staging: both tabs are on screen.
    handle
        .update(cx, |_, window, cx| {
            let (id, node) = tab_panel(area.read(cx), &prod, cx);
            let target = InsertTarget::Split {
                node,
                placement: Placement::Right,
                size: None,
            };
            area.update(cx, |area, cx| area.move_panel(id, target, window, cx));
        })
        .unwrap();
    click_overview(cx, handle, &prod).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(prod_frame(window, cx), "prod active");
    })
    .unwrap();
    click_overview(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!prod_frame(window, cx), "staging active");
        assert!(window.find(tab(&prod)).visible(), "prod still on screen");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn a_tab_moved_into_a_dock_stays_tracked(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    let area = dock(cx, handle);
    // Drag prod into the closed bottom dock.
    handle
        .update(cx, |_, window, cx| {
            let (id, _) = tab_panel(area.read(cx), &prod, cx);
            let mut bottom = None;
            let tree = area.read(cx).layout(DockPlacement::Bottom).unwrap();
            tree.root().walk(&mut |node| {
                if matches!(node.kind(), PaneRef::Tabs { .. }) {
                    bottom.get_or_insert(node.id());
                }
            });
            let target = InsertTarget::Tabs {
                node: bottom.unwrap(),
                ix: None,
                activate: true,
            };
            area.update(cx, |area, cx| area.move_panel(id, target, window, cx));
        })
        .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!prod_frame(window, cx), "prod is off screen");
    })
    .unwrap();
    // The sidebar opens the dock the tab is in.
    click_overview(cx, handle, &prod).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(prod_frame(window, cx), "prod tab in the bottom dock");
        assert!(window.find(tab(&prod)).visible());
        window.press(TOGGLE_LOG, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!prod_frame(window, cx), "the log took the dock");
    })
    .unwrap();
    click_overview(cx, handle, &prod).await;
    wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(prod_frame(window, cx), "prod tab selected again");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn disconnecting_the_active_tab_shows_the_other(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&prod), cx);
        window.press(DISCONNECT, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, tab(&prod), false).await;
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!prod_frame(window, cx));
        assert_eq!(
            window.find("titlebar.connection").label(),
            Some("staging-eu")
        );
        assert_eq!(window.find("titlebar.env").label(), Some("STAGING"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn deleting_a_connected_profile_closes_its_tab(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod], secrets);
    open_tab(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&staging), cx);
        window.press(DELETE, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "confirm.ok".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("confirm.ok", cx)
    })
    .unwrap();
    wait_for_element(cx, handle, tab(&staging), false).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("welcome.add_connection").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn reopening_a_closed_tab_gives_one_tab(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (handle, _rt) = open_profiles(cx, vec![staging.clone(), prod.clone()], secrets);
    open_tab(cx, handle, &staging).await;
    open_tab(cx, handle, &prod).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(tab(&staging), cx)
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| window.press(CLOSE_TAB, cx))
        .unwrap();
    wait_for_element(cx, handle, tab(&staging), false).await;
    open_tab(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find_all(tab(&staging)).len(), 1);
    })
    .unwrap();
}

#[gpui_kit::test]
fn request_log_toggle_shows_its_shortcut(cx: &mut TestAppContext) {
    let handle = open(cx);
    let button = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("status.toggle_log").bounds()
        })
        .unwrap();
    let kbd = VisualTestContext::from_window(handle.into(), cx)
        .debug_bounds(key("kbd:cmd-j", "kbd:ctrl-j"))
        .expect("a Kbd for the shortcut is drawn");
    assert!(button.contains(&kbd.center()), "{kbd:?} not in {button:?}");
}

fn user(key: &str, action: Option<&str>, context: Option<&str>) -> UserBinding {
    UserBinding {
        key: key.into(),
        action: action.map(Into::into),
        context: context.map(Into::into),
    }
}

fn draw(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
}

/// Whether `action` ran when `chord` was pressed, counted by a global listener.
fn counts_refresh(cx: &mut TestAppContext) -> std::rc::Rc<std::cell::Cell<u32>> {
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = count.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &Refresh, _| seen.set(seen.get() + 1));
    });
    count
}

#[gpui_kit::test]
fn unbind_stops_the_chord(cx: &mut TestAppContext) {
    let handle = open(cx);
    let chord = key("cmd-k", "ctrl-k");
    let errors = cx.update(|cx| apply_keymap(&[user(chord, None, None)], cx));
    assert!(errors.is_empty(), "{errors:?}");
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| window.press(chord, cx))
        .unwrap();
    cx.run_until_parked();
    let opened = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find("palette").is_some()
        })
        .unwrap();
    assert!(!opened);
}

#[gpui_kit::test]
fn bad_entries_are_reported_and_others_apply(cx: &mut TestAppContext) {
    let handle = open(cx);
    let count = counts_refresh(cx);
    let errors = cx.update(|cx| {
        apply_keymap(
            &[
                user("cmd-shift-1", Some("tessera::NoSuchAction"), None),
                user("not-a-modifier-x-y", Some("tessera::Refresh"), None),
                user("", Some("tessera::Refresh"), None),
                user("cmd-shift-2", Some("tessera::Refresh"), Some("a &&")),
                user("f6", Some("tessera::Refresh"), None),
            ],
            cx,
        )
    });
    assert_eq!(errors.len(), 4, "{errors:?}");
    assert!(errors[0].contains("tessera::NoSuchAction"), "{errors:?}");
    assert!(errors[3].contains("a &&"), "{errors:?}");
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| window.press("f6", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(count.get(), 1);
}

#[gpui_kit::test]
fn escape_stays_the_way_out_of_a_dialog(cx: &mut TestAppContext) {
    let handle = open(cx);
    let errors = cx.update(|cx| {
        apply_keymap(
            &[
                user("escape", None, None),
                user("escape", None, Some("Dialog")),
                user("escape", Some("tessera::ToggleRequestLog"), None),
                user("escape x", Some("tessera::ToggleRequestLog"), None),
                user("shift-escape", Some("tessera::ToggleRequestLog"), None),
            ],
            cx,
        )
    });
    assert_eq!(errors.len(), 4, "{errors:?}");
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(NEW_CONNECTION, cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(dialog_open(cx, handle));
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!dialog_open(cx, handle));
}

fn dialog_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::component::WindowExt::has_active_dialog(window, cx)
    })
    .unwrap()
}

#[gpui_kit::test]
fn keymap_errors_show_in_the_window(cx: &mut TestAppContext) {
    let handle = open(cx);
    draw(cx, handle);
    let root = handle
        .update(cx, |root, _, _| {
            root.view().clone().downcast::<RootView>().unwrap()
        })
        .unwrap();
    let errors = cx.update_window(handle.into(), |_, window, _| {
        window.try_find("keymap.errors").is_some()
    });
    assert_eq!(errors.ok(), Some(false));
    cx.update(|cx| {
        root.update(cx, |view, cx| {
            view.set_keymap_errors(vec!["line 3: nope".into(), "line 9: no".into()], cx)
        })
    });
    let text = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .find("keymap.errors")
                .label()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap();
    assert!(text.contains("line 3: nope"), "{text}");
    assert!(text.contains("line 9: no"), "{text}");
}

/// A window connected to `name` on `environment`, on its overview tab.
async fn on_overview(
    cx: &mut TestAppContext,
    name: &str,
    environment: Environment,
) -> (
    WindowHandle<Root>,
    Entity<AppState>,
    Profile,
    tokio::runtime::Runtime,
) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let mut profile = Profile::new(name, vec![node()]);
    profile.environment = environment;
    let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
    let (handle, state, rt) = open_app(cx, vec![profile.clone()], secrets);
    state.update(cx, |s, cx| {
        s.connect(profile.id, Some("k".to_string().into()), cx)
    });
    wait_for_element(cx, handle, "titlebar.breadcrumb".into(), true).await;
    (handle, state, profile, rt)
}

fn crumb_path(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> String {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        label(window, "titlebar.breadcrumb")
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_welcome_tab_has_no_breadcrumb(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("titlebar.breadcrumb").is_none());
        assert!(window.find("titlebar.search").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn crumbs_name_the_connection_the_section_and_the_collection(cx: &mut TestAppContext) {
    let (handle, state, profile, _rt) = on_overview(cx, "staging", Environment::Staging).await;
    assert_eq!(crumb_path(cx, handle), "staging / Overview");
    state.update(cx, |s, cx| {
        s.open_collection(profile.id, "books".into(), cx)
    });
    wait_for_label(cx, handle, "titlebar.breadcrumb".into(), "books").await;
    assert_eq!(crumb_path(cx, handle), "staging / Collections / books");
}

#[gpui_kit::test]
async fn the_connection_crumb_leads_to_the_overview(cx: &mut TestAppContext) {
    let (handle, state, profile, _rt) = on_overview(cx, "staging", Environment::Staging).await;
    state.update(cx, |s, cx| {
        s.open_collection(profile.id, "books".into(), cx)
    });
    wait_for_label(cx, handle, "titlebar.breadcrumb".into(), "books").await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The first crumb starts at the left edge; the kit gives it no id of its own.
        let bounds = window.find("titlebar.breadcrumb").bounds();
        let at = bounds.origin + gpui_kit::point(px(4.), bounds.size.height / 2.);
        window.pointer_move(at, None, cx);
        window.pointer_down(at, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "titlebar.breadcrumb".into(), "Overview").await;
}

#[gpui_kit::test]
fn the_search_field_opens_the_palette(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("titlebar.search", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("palette").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_search_field_shows_its_shortcut(cx: &mut TestAppContext) {
    let handle = open(cx);
    let button = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("titlebar.search").bounds()
        })
        .unwrap();
    let kbd = VisualTestContext::from_window(handle.into(), cx)
        .debug_bounds(key("kbd:cmd-k", "kbd:ctrl-k"))
        .expect("a Kbd for the shortcut is drawn");
    assert!(button.contains(&kbd.center()), "{kbd:?} not in {button:?}");
}

#[gpui_kit::test]
async fn the_prod_badge_stays_at_the_smallest_window(cx: &mut TestAppContext) {
    let name = "a very long name of a production cluster ".repeat(4);
    let (handle, _state, _profile, _rt) = on_overview(cx, &name, Environment::Prod).await;
    let width = px(960.);
    cx.simulate_window_resize(handle.into(), gpui_kit::size(width, px(600.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let badge = window.find("titlebar.env").bounds();
        assert!(badge.size.width > px(0.), "{badge:?}");
        assert!(badge.origin.x + badge.size.width < width, "{badge:?}");
        let search = window.find("titlebar.search").bounds();
        assert!(
            badge.origin.x + badge.size.width <= search.origin.x,
            "{badge:?} {search:?}"
        );
        assert!(window.find("titlebar.settings").bounds().origin.x < width);
    })
    .unwrap();
}

#[gpui_kit::test]
fn connect_to_it_fills_the_form_and_connects_to_nothing(cx: &mut TestAppContext) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    fake_connector(cx, move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(server("30.2", true))
    });
    let (handle, state) = open_with_state(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("welcome.connect_local", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("connect.name").value(), Some("local"));
        assert_eq!(
            window.find("connect.node.0.host").value(),
            Some("localhost")
        );
        assert_eq!(window.find("connect.node.0.port").value(), Some("8108"));
        // The field is masked, so its value shows only with the mask off.
        window.dispatch_action(Box::new(ToggleKeyMask), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("connect.key").value(), Some(""));
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("connect.name").is_none(), "dialog closed");
    })
    .unwrap();
    assert!(cx.read(|cx| state.read(cx).profiles.is_empty()));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[gpui_kit::test]
fn the_welcome_screen_shows_the_palette_and_settings_chords(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let line = label(window, "welcome.next");
        for chord in [key("⌘K", "Ctrl+K"), key("⌘,", "Ctrl+,")] {
            assert!(line.contains(chord), "{chord:?} missing in {line:?}");
        }
    })
    .unwrap();
}
