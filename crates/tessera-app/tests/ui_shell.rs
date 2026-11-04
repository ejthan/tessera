// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the window shell in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};
use tessera_core::profile::Profile;
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
