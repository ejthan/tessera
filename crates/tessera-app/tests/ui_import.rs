// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of profile import and export in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};
use secrecy::{ExposeSecret as _, SecretString};
use tessera_app::views::import_dialog::ImportDialog;
use tessera_core::profile::{Node, Profile, Protocol};
use tessera_core::secrets::{MemoryStore, SecretError, SecretStore};
use tessera_core::store::StoreError;
use uuid::Uuid;

use common::*;

const DASHBOARD_CONFIG: &str = r#"{
  "apiKey": "k-top",
  "node": {"host": "localhost", "port": "8108", "protocol": "http", "path": ""},
  "history": [
    {"apiKey": "k-a", "node": {"host": "a.example", "port": "443", "protocol": "https", "path": ""}},
    {"apiKey": "k-b", "node": {"host": "b.example", "port": "443", "protocol": "https", "path": ""}}
  ]
}"#;

#[gpui_kit::test]
async fn import_preview_creates_selected_profiles(cx: &mut TestAppContext) {
    let existing = Profile::new(
        "mine",
        vec![Node {
            protocol: Protocol::Https,
            host: "a.example".into(),
            port: 443,
            path: String::new(),
        }],
    );
    let secrets = Arc::new(MemoryStore::default());
    let (handle, state, _rt) = open_app(cx, vec![existing.clone()], secrets.clone());
    let dialog_state = state.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        ImportDialog::import_from_text(dialog_state, DASHBOARD_CONFIG, window, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "import.row.0".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("import.row.2").is_some());
        assert!(window.try_find("import.row.3").is_none());
        let checked = |window: &mut gpui_kit::Window, i: usize| {
            window.find(format!("import.row.{i}.selected")).checked()
        };
        assert_eq!(checked(window, 0), Some(true));
        assert_eq!(checked(window, 1), Some(false), "the duplicate");
        assert_eq!(checked(window, 2), Some(true));
        assert!(label(window, "import.submit").contains('2'));
        window.click("import.submit", cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "import.submit".into(), false).await;
    state.read_with(cx, |s, _| {
        let names: Vec<&str> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["mine", "localhost", "b.example"]);
        for (p, key) in s.profiles[1..].iter().zip(["k-top", "k-b"]) {
            let stored = secrets.get(p.id).unwrap().expect("key stored");
            assert_eq!(stored.expose_secret(), key);
        }
    });
}

/// A store that refuses every write.
struct NoWrites;

impl SecretStore for NoWrites {
    fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
        Err(SecretError::new("Keychain access was denied"))
    }
    fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Ok(())
    }
}

#[gpui_kit::test]
async fn import_keychain_failure_is_listed_per_row(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(NoWrites));
    let dialog_state = state.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        ImportDialog::import_from_text(dialog_state, DASHBOARD_CONFIG, window, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "import.row.0".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("import.submit", cx)
    })
    .unwrap();
    wait_for_label(
        cx,
        handle,
        "import.row.0.error".into(),
        "Keychain access was denied",
    )
    .await;
    state.read_with(cx, |s, _| {
        assert_eq!(s.profiles.len(), 3, "imported without keys")
    });
}

#[gpui_kit::test]
async fn import_of_invalid_json_shows_the_error(cx: &mut TestAppContext) {
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    cx.update_window(handle.into(), |_, window, cx| {
        ImportDialog::import_from_text(state, "{ nope", window, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "import.error".into(), "config.json").await;
}

#[gpui_kit::test]
async fn export_writes_profiles_without_keys(cx: &mut TestAppContext) {
    let (staging, _, _) = two_profiles();
    let secrets = Arc::new(MemoryStore::default());
    secrets
        .set(staging.id, &SecretString::from("secret-key-42"))
        .unwrap();
    let (_handle, state, _rt) = open_app(cx, vec![staging], secrets);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let written = state.update(cx, |s, cx| s.export_to(path.clone(), cx));
    written.await.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("staging-eu"), "{text}");
    assert!(text.contains("schema_version"), "{text}");
    assert!(!text.contains("secret-key-42"), "{text}");
}

#[gpui_kit::test]
async fn invalid_and_same_rows_are_locked(cx: &mut TestAppContext) {
    let config = r#"{
      "apiKey": "k", "node": {"host": "https://x.example/"},
      "history": [{"apiKey": "k", "node": {"host": "SAME"}}]
    }"#;
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    cx.update_window(handle.into(), |_, window, cx| {
        ImportDialog::import_from_text(state, config, window, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "import.row.1".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for i in 0..2 {
            let row = window.find(format!("import.row.{i}.selected"));
            assert_eq!(row.checked(), Some(false), "row {i}");
            assert_eq!(row.disabled(), Some(true), "row {i}");
        }
        assert_eq!(window.find("import.submit").disabled(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn load_error_blocks_the_import(cx: &mut TestAppContext) {
    let _rt = tokio(cx);
    let (handle, state) = open_state(
        cx,
        Err(StoreError::NewerSchema(9)),
        Arc::new(MemoryStore::default()),
    );
    let dialog_state = state.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        ImportDialog::import_from_text(dialog_state, DASHBOARD_CONFIG, window, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "import.blocked".into(), "schema version 9").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("import.submit").disabled(), Some(true));
        window.click("import.submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    state.read_with(cx, |s, _| assert!(s.profiles.is_empty()));
}
