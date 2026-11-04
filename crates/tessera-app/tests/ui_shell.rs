// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the window shell in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use gpui_kit::base::{Placement, Root};
use gpui_kit::component::dock::{DockArea, DockPlacement, InsertTarget, NodeId, PaneRef, PanelId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, WindowHandle};
use tessera_app::views::dock::OverviewPanel;
use tessera_app::views::root::RootView;
use tessera_core::profile::{Environment, Profile};
use tessera_core::store::StoreError;

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
        assert!(!prod_frame(window, cx), "dock closed");
        window.press(TOGGLE_LOG, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(prod_frame(window, cx), "dock open again");
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
