// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the transport banner, and the audit of destructive buttons (§8.2).

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext, WindowHandle, base::Root};
use tessera_core::cli::Launch;
use tessera_core::profile::{Node, Profile, Protocol};

use common::*;

async fn connected(
    cx: &mut TestAppContext,
    profile: Profile,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    fake_backend(cx, Some((vec![], vec![])));
    fake_documents(cx);
    install_search_fake(cx);
    fake_admin(cx);
    let id = profile.id;
    fake_connector(cx, |profile| Ok(server_for(profile)));
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
    (handle, rt)
}

fn remote(protocol: Protocol) -> Profile {
    let node = Node {
        protocol,
        host: "search.example".into(),
        port: 8108,
        path: String::new(),
    };
    Profile::new("test", vec![node])
}

fn banner(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Option<String> {
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.update_window(handle.into(), |_, window, _| {
        let found = window.try_find("transport.banner".to_string());
        found.map(|e| e.label().map(ToString::to_string).unwrap_or_default())
    })
    .unwrap()
}

#[gpui_kit::test]
async fn remote_http_shows_the_banner(cx: &mut TestAppContext) {
    let (handle, _rt) = connected(cx, remote(Protocol::Http)).await;
    let text = banner(cx, handle).unwrap();
    assert!(text.contains("search.example:8108"), "{text}");
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.try_find("transport.edit".to_string()).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn insecure_certificates_show_the_banner(cx: &mut TestAppContext) {
    let mut profile = remote(Protocol::Https);
    profile.tls.allow_insecure = true;
    let (handle, _rt) = connected(cx, profile).await;
    assert!(banner(cx, handle).unwrap().contains("certificate"));
}

#[gpui_kit::test]
async fn both_risks_take_a_line_each(cx: &mut TestAppContext) {
    let mut profile = remote(Protocol::Http);
    profile.tls.allow_insecure = true;
    let (handle, _rt) = connected(cx, profile).await;
    let text = banner(cx, handle).unwrap();
    assert!(
        text.contains("search.example") && text.contains("certificate"),
        "{text}"
    );
}

#[gpui_kit::test]
async fn https_and_local_show_no_banner(cx: &mut TestAppContext) {
    let (handle, _rt) = connected(cx, remote(Protocol::Https)).await;
    assert_eq!(banner(cx, handle), None);
    let (handle, _rt) = connected(cx, Profile::new("local", vec![node()])).await;
    assert_eq!(banner(cx, handle), None);
}

#[gpui_kit::test]
async fn a_temporary_connection_has_nothing_to_edit(cx: &mut TestAppContext) {
    fake_backend(cx, Some((vec![], vec![])));
    fake_documents(cx);
    install_search_fake(cx);
    fake_admin(cx);
    fake_connector(cx, |profile| Ok(server_for(profile)));
    let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
    let (handle, state, _rt) = open_app(cx, Vec::new(), secrets);
    let temp = remote(Protocol::Http);
    let id = temp.id;
    cx.update(|cx| {
        state.update(cx, |s, cx| {
            s.resolve(Launch::Temporary {
                profile: temp,
                key_from_stdin: false,
                key_from_env: Some("k".to_string().into()),
            })
            .unwrap();
            s.connect(id, None, cx);
        });
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("figure.version").is_some()
    })
    .await;
    assert!(banner(cx, handle).is_some());
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.try_find("transport.edit".to_string()).is_none());
    })
    .unwrap();
}

/// The style of a built button cannot be read from a window, so this reads its builder chain.
const DESTRUCTIVE: [(&str, &str); 14] = [
    ("schema_view.rs", "Button::new(\"schema.truncate\")"),
    ("schema_view.rs", "Button::new(\"schema.delete\")"),
    ("documents/mod.rs", "Button::new(\"documents.delete\")"),
    ("documents/mod.rs", "Button::new(id)"),
    ("search/queries.rs", "Button::new(\"search.saved.delete\")"),
    ("resources/keys.rs", "\"keys.delete\".into()"),
    ("resources/presets.rs", "\"presets.delete\".into()"),
    ("resources/ai.rs", "\"ai.delete\".into()"),
    ("resources/stopwords.rs", "\"stopwords.delete\".into()"),
    ("resources/analytics.rs", "\"analytics.delete\".into()"),
    ("resources/aliases.rs", "\"aliases.delete\".into()"),
    ("resources/sets.rs", "format!(\"{prefix}.delete\")"),
    ("resources/stemming.rs", "\"stemming.delete\".into()"),
    ("resources/operations.rs", "\"ops.snapshot.run\".into()"),
];

fn views() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/views")
}

#[test]
fn every_destructive_button_is_red() {
    for (file, marker) in DESTRUCTIVE {
        let source = std::fs::read_to_string(views().join(file)).unwrap();
        let start = source.find(marker).unwrap_or_else(|| panic!("{marker}"));
        let chain = &source[start..source.len().min(start + 420)];
        assert!(chain.contains(".danger()"), "{file}: {marker}");
    }
}

/// A new delete button has to join the list above, or say why it is local.
#[test]
fn the_audit_knows_every_delete_button() {
    const LOCAL: [&str; 0] = [];
    let mut stack = vec![views()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let buttons = |l: &&str| l.contains("Button::new(\"") || l.contains(".delete\".into()");
            for line in source.lines().filter(buttons) {
                let id = line.split('"').nth(1).unwrap();
                if id.ends_with("delete") || id.ends_with("truncate") {
                    let known = DESTRUCTIVE.iter().any(|(_, m)| m.contains(id));
                    assert!(known || LOCAL.contains(&id), "{id} is not in the audit");
                }
            }
        }
    }
}
