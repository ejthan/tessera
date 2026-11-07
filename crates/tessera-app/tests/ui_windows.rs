// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of several windows on one app state. A real close on macOS is in the QA checklist:
//! the test platform's `quit` does nothing, so a test cannot see the app end.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use gpui_kit::base::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle};
use std::sync::Arc;
use std::time::Duration;
use tessera_app::open_main_window;
use tessera_app::state::{AppState, UpdateFound};
use tessera_core::profile::Profile;
use tessera_core::secrets::MemoryStore;
use tessera_core::store;

use common::*;

const NEW_WINDOW: &str = key("alt-cmd-n", "ctrl-alt-n");

fn secrets() -> Arc<MemoryStore> {
    Arc::default()
}

/// The id of `profile`'s overview tab title.
fn tab(profile: &Profile) -> String {
    format!("tab.{}.overview", profile.id)
}

fn second_window(cx: &mut TestAppContext, state: &Entity<AppState>) -> WindowHandle<Root> {
    let state = state.clone();
    let window = cx
        .update(|cx| open_main_window(state, false, Vec::new(), cx))
        .unwrap();
    window.downcast::<Root>().unwrap()
}

/// Brings `handle` to the front, as a click on it would.
fn activate(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
}

/// Clicks `profile` in the sidebar of `handle`.
fn click_profile(cx: &mut TestAppContext, handle: WindowHandle<Root>, profile: &Profile) {
    let id = entry(profile);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn tabs(cx: &mut TestAppContext, handle: WindowHandle<Root>, profile: &Profile) -> usize {
    let id = tab(profile);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.find_all(id).len()
    })
    .unwrap()
}

fn prod_band(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.try_find("window.prod_band").is_some()
    })
    .unwrap()
}

fn owner(cx: &TestAppContext, state: &Entity<AppState>, profile: &Profile) -> Option<u64> {
    cx.read(|cx| {
        state
            .read(cx)
            .window_of(profile.id)
            .map(|window| window.window_id().as_u64())
    })
}

fn id_of(handle: WindowHandle<Root>) -> u64 {
    gpui_kit::AnyWindowHandle::from(handle).window_id().as_u64()
}

#[gpui_kit::test]
async fn a_connection_opens_in_exactly_one_window(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, _rt) = open_app(cx, vec![staging.clone(), prod], secrets);
    let second = second_window(cx, &state);
    activate(cx, second);
    click_profile(cx, second, &staging);
    wait_for_element(cx, second, tab(&staging), true).await;
    assert_eq!(tabs(cx, first, &staging), 0);
    assert_eq!(tabs(cx, second, &staging), 1);
    assert_eq!(owner(cx, &state, &staging), Some(id_of(second)));
}

#[gpui_kit::test]
async fn a_later_open_goes_to_the_owner_and_brings_it_forward(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, _rt) = open_app(cx, vec![staging.clone(), prod], secrets);
    let second = second_window(cx, &state);
    activate(cx, second);
    click_profile(cx, second, &staging);
    wait_for_element(cx, second, tab(&staging), true).await;

    activate(cx, first);
    click_profile(cx, first, &staging);
    assert_eq!(tabs(cx, first, &staging), 0);
    assert_eq!(tabs(cx, second, &staging), 1);
    let active = cx.update(|cx| cx.active_window()).unwrap();
    assert_eq!(active.window_id().as_u64(), id_of(second));
}

#[gpui_kit::test]
async fn each_window_shows_the_band_of_its_own_connection(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, _rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets);
    let second = second_window(cx, &state);
    activate(cx, first);
    click_profile(cx, first, &prod);
    wait_for_element(cx, first, tab(&prod), true).await;
    activate(cx, second);
    click_profile(cx, second, &staging);
    wait_for_element(cx, second, tab(&staging), true).await;
    assert!(prod_band(cx, first));
    assert!(!prod_band(cx, second));
}

#[gpui_kit::test]
async fn closing_the_last_tab_clears_the_owner(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, _rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets);
    activate(cx, first);
    click_profile(cx, first, &staging);
    wait_for_element(cx, first, tab(&staging), true).await;
    click_profile(cx, first, &prod);
    wait_for_element(cx, first, tab(&prod), true).await;
    assert_eq!(owner(cx, &state, &staging), Some(id_of(first)));
    // Clicking the tab focuses it, so the close shortcut reaches its tab group.
    cx.update_window(first.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(tab(&staging), cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(first.into(), |_, window, cx| window.press(CLOSE_TAB, cx))
        .unwrap();
    wait_for_element(cx, first, tab(&staging), false).await;
    assert_eq!(owner(cx, &state, &staging), None);
    assert_eq!(owner(cx, &state, &prod), Some(id_of(first)));
}

#[gpui_kit::test]
async fn closing_a_window_clears_its_owners(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, _rt) = open_app(cx, vec![staging.clone(), prod], secrets);
    let second = second_window(cx, &state);
    activate(cx, second);
    click_profile(cx, second, &staging);
    wait_for_element(cx, second, tab(&staging), true).await;
    cx.update_window(second.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(owner(cx, &state, &staging), None);
    // The connection stays open, and the window left shows it again.
    activate(cx, first);
    click_profile(cx, first, &staging);
    assert_eq!(tabs(cx, first, &staging), 1);
    assert_eq!(owner(cx, &state, &staging), Some(id_of(first)));
}

#[gpui_kit::test]
async fn only_the_primary_window_saves_the_layout(cx: &mut TestAppContext) {
    let (first, state, _rt) = open_app(cx, Vec::new(), secrets());
    let second = second_window(cx, &state);
    let before = cx.read(|cx| state.read(cx).layout.clone());
    cx.update_window(second.into(), |_, window, cx| window.press(TOGGLE_LOG, cx))
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| state.read(cx).layout.clone()), before);
    cx.update_window(first.into(), |_, window, cx| window.press(TOGGLE_LOG, cx))
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_ne!(cx.read(|cx| state.read(cx).layout.clone()), before);
}

#[gpui_kit::test]
async fn a_second_window_runs_no_update_check(cx: &mut TestAppContext) {
    let admin = fake_admin(cx);
    let (_first, state, _rt) = open_app(cx, Vec::new(), secrets());
    state.update(cx, |s, _| s.settings.update_check = true);
    second_window(cx, &state);
    cx.run_until_parked();
    assert_eq!(*admin.calls.lock().unwrap(), Vec::<String>::new());
}

#[gpui_kit::test]
async fn a_toast_goes_to_the_active_window_alone(cx: &mut TestAppContext) {
    let (first, state, _rt) = open_app(cx, Vec::new(), secrets());
    let second = second_window(cx, &state);
    activate(cx, second);
    state.update(cx, |_, cx| cx.emit(UpdateFound("v9".into())));
    cx.run_until_parked();
    let toasts = |cx: &mut TestAppContext, handle: WindowHandle<Root>| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.notifications(cx).len()
        })
        .unwrap()
    };
    assert_eq!(toasts(cx, first), 0);
    assert_eq!(toasts(cx, second), 1);
}

#[gpui_kit::test]
async fn new_window_opens_one_window(cx: &mut TestAppContext) {
    let (first, _state, _rt) = open_app(cx, Vec::new(), secrets());
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update_window(first.into(), |_, window, cx| window.press(NEW_WINDOW, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);
}

#[gpui_kit::test]
fn quit_hooks_flush_after_the_last_window_closed(cx: &mut TestAppContext) {
    // Poll the quit handler for the whole SHUTDOWN_TIMEOUT, as `pending_save_finishes_on_quit`.
    cx.executor().set_block_on_ticks(usize::MAX..=usize::MAX);
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let (first, state, _rt) = {
        let rt = tokio(cx);
        let (handle, state) = open_state_at(
            cx,
            Ok(store::ConnectionsFile::default()),
            Some(path.clone()),
            secrets(),
        );
        (handle, state, rt)
    };
    cx.update(|cx| cx.set_quit_mode(gpui_kit::QuitMode::LastWindowClosed));
    cx.update_window(first.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    state.update(cx, |s, cx| {
        s.upsert_profile(Profile::new("kept", vec![node()]), None, cx)
            .detach();
    });
    cx.quit();
    let file = store::load(&path).unwrap();
    assert_eq!(file.profiles.len(), 1, "save finished before quit returned");
}
