// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the sidebar in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext, WindowHandle, base::Root};
use secrecy::{ExposeSecret as _, SecretString};
use tessera_app::actions::ToggleReopenLast;
use tessera_app::state::AppState;
use tessera_client::Error;
use tessera_core::cli::Launch;
use tessera_core::profile::Profile;
use tessera_core::secrets::{MemoryStore, SecretError, SecretStore};
use tessera_core::store::{self, ConnectionsFile};
use uuid::Uuid;

use common::*;

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

#[gpui_kit::test]
async fn delete_asks_with_cancel_focused(cx: &mut TestAppContext) {
    let (staging, prod, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets.clone());
    let ask = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            focus(window, &entry(&staging), cx);
            window.press(DELETE, cx);
        })
        .unwrap();
    };
    ask(cx);
    wait_for_element(cx, handle, "confirm.cancel".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("confirm.cancel").focused(), Some(true));
        let body = label(window, "confirm.body");
        assert!(body.contains("staging-eu"), "{body:?}");
        window.press("enter", cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "confirm.cancel".into(), false).await;
    // Focus goes back to the entry once the dialog has left.
    let back = entry(&staging);
    cx.wait_for(
        handle.into(),
        Duration::from_millis(1000),
        move |window, _| window.find(back.clone()).focused() == Some(true),
    )
    .await;
    state.read_with(cx, |s, _| assert_eq!(s.profiles.len(), 2, "Enter cancels"));
    assert!(secrets.get(staging.id).unwrap().is_some());

    ask(cx);
    wait_for_element(cx, handle, "confirm.ok".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("confirm.ok", cx)
    })
    .unwrap();
    let gone = secrets.clone();
    let id = staging.id;
    cx.wait_for(handle.into(), Duration::from_millis(1000), move |_, _| {
        gone.get(id).unwrap().is_none()
    })
    .await;
    state.read_with(cx, |s, _| {
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.profiles[0].id, prod.id);
    });
    assert!(secrets.get(prod.id).unwrap().is_some());
}

#[gpui_kit::test]
async fn delete_disconnects_a_connected_profile(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, _, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(entry(&staging), cx);
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    menu_item(cx, handle, entry(&staging), 2, "Delete…").await;
    wait_for_element(cx, handle, "confirm.ok".into(), true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("confirm.ok", cx)
    })
    .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "Not connected").await;
    state.read_with(cx, |s, cx| {
        assert!(s.profiles.is_empty());
        assert!(s.connection(staging.id, cx).is_none());
    });
}

#[gpui_kit::test]
async fn reopen_last_connects_at_startup(cx: &mut TestAppContext) {
    let seen = recording_connector(cx, server("30.2", true));
    let (staging, prod, secrets) = two_profiles();
    let file = |reopen_last| ConnectionsFile {
        profiles: vec![staging.clone(), prod.clone()],
        last_used: Some(staging.id),
        reopen_last,
        ..Default::default()
    };
    let mut off = AppState::new(Ok(file(false)), None, secrets.clone());
    assert_eq!(off.resolve(Launch::Normal).unwrap(), None);
    let _rt = tokio(cx);
    let (handle, state) = open_state(cx, Ok(file(true)), secrets);
    // What `run` does before the window opens.
    state.update(cx, |s, cx| {
        let open = s.resolve(Launch::Normal).unwrap();
        assert_eq!(open, Some(staging.id));
        s.connect(staging.id, None, cx);
    });
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    assert_eq!(*seen.lock().unwrap(), ["k"]);
}

#[gpui_kit::test]
async fn temporary_connection_can_be_saved(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let secrets = Arc::new(MemoryStore::default());
    let (handle, state, _rt) = open_app(cx, Vec::new(), secrets.clone());
    let mut node = node();
    node.host = "ts.internal".into();
    let temp = Profile::new("ts.internal", vec![node]);
    state.update(cx, |s, cx| {
        s.resolve(Launch::Temporary {
            profile: temp.clone(),
            key_from_stdin: false,
            key_from_env: Some(SecretString::from("k-tmp")),
        })
        .unwrap();
        cx.notify();
    });
    let not_saved = format!("{}.not_saved", entry(&temp));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(not_saved.clone()).label(), Some("not saved"));
        window.click("sidebar.save_temporary", cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("connect.name").value(), Some("ts.internal"));
        assert_eq!(
            window.find("connect.node.0.host").value(),
            Some("ts.internal")
        );
        window.click("connect.key", cx);
        window.press(TOGGLE_KEY, cx);
        window.render_frame(cx);
        assert_eq!(window.find("connect.key").value(), Some("k-tmp"));
        window.press(SAVE, cx);
    })
    .unwrap();
    wait_for_close(cx, handle).await;
    wait_for_element(cx, handle, not_saved, false).await;
    state.read_with(cx, |s, cx| {
        assert!(s.temporary.is_none());
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(
            s.profiles[0].id, temp.id,
            "the entry became the saved profile"
        );
        let conn = s.connection(temp.id, cx).expect("connection stays open");
        assert!(conn.read(cx).connected.is_some(), "still connected");
    });
    let stored = secrets.get(temp.id).unwrap().expect("key stored");
    assert_eq!(stored.expose_secret(), "k-tmp");
}

/// A store whose deletes fail.
struct NoDeletes;

impl SecretStore for NoDeletes {
    fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
        Ok(())
    }
    fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Err(SecretError::new("Keychain access was denied"))
    }
}

/// Presses Delete on `profile`'s entry and waits for the confirmation.
async fn ask_delete(cx: &mut TestAppContext, handle: WindowHandle<Root>, profile: &Profile) {
    let id = entry(profile);
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, &id, cx);
        window.press(DELETE, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "confirm.ok".into(), true).await;
}

#[gpui_kit::test]
async fn failed_key_removal_shows_in_the_header(cx: &mut TestAppContext) {
    let (staging, _, _) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], Arc::new(NoDeletes));
    ask_delete(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("confirm.ok", cx)
    })
    .unwrap();
    wait_for_label(
        cx,
        handle,
        "sidebar.header_error".into(),
        "Keychain access was denied",
    )
    .await;
    cx.update_window(handle.into(), |_, window, _| {
        assert!(label(window, "sidebar.header_error").contains("staging-eu"));
    })
    .unwrap();
    state.read_with(cx, |s, _| assert!(s.profiles.is_empty()));
}

#[gpui_kit::test]
async fn enter_on_focused_delete_deletes(cx: &mut TestAppContext) {
    let (staging, _, secrets) = two_profiles();
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets.clone());
    ask_delete(cx, handle, &staging).await;
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, "confirm.ok", cx);
        window.press("enter", cx);
    })
    .unwrap();
    let gone = secrets.clone();
    let id = staging.id;
    cx.wait_for(handle.into(), Duration::from_millis(1000), move |_, _| {
        gone.get(id).unwrap().is_none()
    })
    .await;
    state.read_with(cx, |s, _| assert!(s.profiles.is_empty()));
}

#[gpui_kit::test]
async fn reopen_toggle_is_saved(cx: &mut TestAppContext) {
    let _rt = tokio(cx);
    let (staging, _, secrets) = two_profiles();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let file = ConnectionsFile {
        profiles: vec![staging.clone()],
        ..Default::default()
    };
    let (handle, _state) = open_state_at(cx, Ok(file), Some(path.clone()), secrets);
    cx.update_window(handle.into(), |_, window, cx| {
        // From inside the sidebar, as the header menu does.
        focus(window, &entry(&staging), cx);
        window.dispatch_action(Box::new(ToggleReopenLast), cx);
    })
    .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(1000), move |_, _| {
        store::load(&path).is_ok_and(|f| f.reopen_last)
    })
    .await;
}
