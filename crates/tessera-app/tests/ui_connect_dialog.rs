// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the connection dialog in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, ScrollDelta, TestAppContext, point, px};
use secrecy::{ExposeSecret as _, SecretString};
use tessera_app::connection::ConnectorGlobal;
use tessera_app::views::connect_dialog::ConnectDialog;
use tessera_client::{Error, KeyAccess};
use tessera_core::profile::{Environment, Profile, Protocol};
use tessera_core::secrets::{MemoryStore, SecretError, SecretStore};
use tessera_core::store::StoreError;
use uuid::Uuid;

use common::*;

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
async fn typing_prod_as_other_turns_read_only_on(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("connect.env.other", cx);
        window.render_frame(cx);
        fill(window, "connect.env.name", "Prod", cx);
    })
    .unwrap();
    // Input events reach the dialog once the update above has finished.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("connect.read_only").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn advanced_tls_and_nearest_node_are_saved(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(label(window, "connect.advanced").contains("system certificates"));
        window.click("connect.advanced", cx);
        window.render_frame(cx);
        assert!(window.try_find("connect.insecure_warning").is_none());
        // The open section runs below the fold of the 800 px test window.
        let down = ScrollDelta::Pixels(point(px(0.), px(-600.)));
        window.scroll("connect.advanced", down, cx);
        window.render_frame(cx);
        assert!(window.find("connect.ca_file").visible());
        window.click("connect.insecure", cx);
        window.render_frame(cx);
        assert!(window.find("connect.insecure_warning").visible());
        assert!(label(window, "connect.advanced").contains("insecure"));
        fill(window, "connect.nearest.host", "near.example.com", cx);
        window.click("connect.nearest.protocol", cx);
        fill(window, "connect.key", "k", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    let profile = state.read_with(cx, |s, _| s.profiles[0].clone());
    assert!(profile.tls.allow_insecure);
    let nearest = profile.nearest_node.expect("nearest node saved");
    assert_eq!(
        (nearest.host.as_str(), nearest.protocol),
        ("near.example.com", Protocol::Https)
    );
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
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, _, _| {
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
    assert_eq!(
        *seen.lock().unwrap(),
        ["k-new"],
        "one probe round, reused by Save"
    );
}

#[gpui_kit::test]
async fn save_after_test_reuses_the_result(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(TEST, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "Admin key").await;
    cx.update_window(handle.into(), |_, window, cx| window.press(SAVE, cx))
        .unwrap();
    wait_for_close(cx, handle).await;
    wait_for_label(cx, handle, "status.text".into(), "Connected").await;
    assert_eq!(seen.lock().unwrap().len(), 1, "Save did not probe again");
    state.read_with(cx, |s, cx| {
        let id = s.profiles[0].id;
        assert!(s.connection(id, cx).unwrap().read(cx).connected.is_some());
        assert_eq!(s.last_used, Some(id));
    });
}

#[gpui_kit::test]
async fn changed_values_mark_the_result_stale(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let handle = open(cx);
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(TEST, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "Admin key").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!label(window, "connect.result").contains("Values changed"));
        fill(window, "connect.name", "renamed", cx);
        window.render_frame(cx);
        assert!(label(window, "connect.result").contains("Values changed since this test."));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn failed_test_offers_save_anyway(cx: &mut TestAppContext) {
    let calls = Arc::new(Mutex::new(0));
    let count = calls.clone();
    fake_connector(cx, move |_| {
        *count.lock().unwrap() += 1;
        Err(Error::Timeout)
    });
    let secrets = Arc::new(MemoryStore::default());
    let (handle, state, _rt) = open_app(cx, Vec::new(), secrets.clone());
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.name", "down-now", cx);
        fill(window, "connect.key", "k-later", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "did not answer").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("connect.save").label(), Some("Save anyway"));
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    cx.run_until_parked();
    assert_eq!(*calls.lock().unwrap(), 1, "Save anyway does not connect");
    let id = state.read_with(cx, |s, _| {
        assert_eq!(s.profiles.len(), 1);
        assert!(s.connections.is_empty());
        s.profiles[0].id
    });
    assert_eq!(secrets.get(id).unwrap().unwrap().expose_secret(), "k-later");
}

/// A store whose writes wait until the test opens the gate.
struct GateStore {
    sets: AtomicUsize,
    gate: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl SecretStore for GateStore {
    fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.gate.lock().unwrap().recv().ok();
        Ok(())
    }
    fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Ok(())
    }
}

#[gpui_kit::test]
async fn shortcuts_wait_for_a_running_save(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let (open_gate, gate) = std::sync::mpsc::channel();
    let store = Arc::new(GateStore {
        sets: AtomicUsize::new(0),
        gate: Mutex::new(gate),
    });
    let (handle, state, _rt) = open_app(cx, Vec::new(), store.clone());
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    let saving = store.clone();
    cx.wait_for(handle.into(), Duration::from_millis(1000), move |_, _| {
        saving.sets.load(Ordering::SeqCst) == 1
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(TEST, cx);
        window.press(SAVE, cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.find("connect.name").visible(),
            "Esc waits for the save"
        );
        assert_eq!(window.find("connect.save").disabled(), Some(true));
    })
    .unwrap();
    // Extra openings keep a wrongly started second write from blocking the runtime.
    for _ in 0..3 {
        open_gate.send(()).unwrap();
    }
    wait_for_close(cx, handle).await;
    cx.run_until_parked();
    assert_eq!(store.sets.load(Ordering::SeqCst), 1);
    assert_eq!(seen.lock().unwrap().len(), 1);
    state.read_with(cx, |s, _| assert_eq!(s.profiles.len(), 1));
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
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert!(s.profiles.is_empty(), "nothing written yet")
    });
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
        assert_eq!(s.session_keys[&id].expose_secret(), "k-session");
    });
    assert_eq!(*seen.lock().unwrap(), ["k-session"]);
}

#[gpui_kit::test]
async fn cancel_at_keychain_error_leaves_no_profile(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(FailingStore));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_label(
        cx,
        handle,
        "connect.keychain_error".into(),
        "Secret Service",
    )
    .await;
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    wait_for_close(cx, handle).await;
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert!(s.profiles.is_empty());
        assert!(s.connections.is_empty());
    });
}

/// A store that refuses every read, to prove a session key is used first.
struct NoReads;

impl SecretStore for NoReads {
    fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
        Err(SecretError::new("read attempted"))
    }
    fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
        Ok(())
    }
    fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Ok(())
    }
}

#[gpui_kit::test]
async fn edit_test_uses_the_session_key(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let profile = Profile::new("session", vec![node()]);
    let (handle, state, _rt) = open_app(cx, vec![profile.clone()], Arc::new(NoReads));
    state.update(cx, |s, _| {
        s.session_keys
            .insert(profile.id, "k-mem".to_string().into());
    });
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&profile), cx);
        window.press(EDIT, cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| window.press(TEST, cx))
        .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "Admin key").await;
    assert_eq!(*seen.lock().unwrap(), ["k-mem"]);
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

/// A store that only counts its writes.
#[derive(Default)]
struct CountingStore(AtomicUsize);

impl SecretStore for CountingStore {
    fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Ok(())
    }
}

#[gpui_kit::test]
async fn load_error_blocks_saving_a_connection(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let _rt = tokio(cx);
    let store = Arc::new(CountingStore::default());
    let (handle, state) = open_state(cx, Err(StoreError::NewerSchema(9)), store.clone());
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.render_frame(cx);
        assert!(label(window, "connect.blocked").contains("schema version 9"));
        assert_eq!(window.find("connect.save").disabled(), Some(true));
        window.press(SAVE, cx);
        window.click("connect.save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(seen.lock().unwrap().is_empty(), "Save does not even test");
    assert_eq!(store.0.load(Ordering::SeqCst), 0, "nothing in the keychain");
    state.read_with(cx, |s, _| {
        assert!(s.profiles.is_empty());
        assert!(s.connections.is_empty());
    });
    // Testing writes nothing, so it still works.
    cx.update_window(handle.into(), |_, window, cx| window.press(TEST, cx))
        .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "Admin key").await;
    assert_eq!(store.0.load(Ordering::SeqCst), 0);
}

#[gpui_kit::test]
async fn save_anyway_closes_the_old_connection_of_an_edited_profile(cx: &mut TestAppContext) {
    // Staging connects; the edited prod values fail their test.
    fake_connector(cx, |p| {
        if p.environment == Environment::Prod {
            Err(Error::Timeout)
        } else {
            Ok(server("30.2", true))
        }
    });
    let (staging, _, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(entry(&staging), cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &entry(&staging), cx);
        window.press(EDIT, cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("connect.env.prod", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "connect.result".into(), "did not answer").await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("connect.save").label(), Some("Save anyway"));
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.profiles[0].environment, Environment::Prod);
        assert!(s.profiles[0].read_only);
        assert!(
            s.connections.is_empty(),
            "the old staging connection is closed"
        );
        assert_eq!(s.selected, None);
    });
    // Sidebar and window agree: the profile is prod, and no stale staging connection shows.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(format!("{}.env", entry(&staging))).label(),
            Some("PROD")
        );
        assert_eq!(
            window.find("titlebar.connection").label(),
            Some("No connection")
        );
        assert_eq!(window.find("status.text").label(), Some("Not connected"));
        assert!(window.try_find("window.prod_band").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn prefill_of_a_saved_profile_gets_a_new_id(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, _, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets);
    let dialog_state = state.clone();
    let prefill = staging.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        ConnectDialog::open_new(
            dialog_state,
            Some((prefill, Some(SecretString::from("k")))),
            window,
            cx,
        );
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| window.press(SAVE, cx))
        .unwrap();
    wait_for_close(cx, handle).await;
    state.read_with(cx, |s, _| {
        assert_eq!(s.profiles.len(), 2, "the saved profile was not overwritten");
        assert_eq!(s.profiles[0], staging);
    });
}
