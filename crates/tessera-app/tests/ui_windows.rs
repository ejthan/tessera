// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of several windows on one app state. A real close on macOS is in the QA checklist:
//! the test platform's `quit` does nothing, so a test cannot see the app end.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use gpui_kit::base::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, Focusable as _, TestAppContext, WindowHandle};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tessera_app::actions::{OpenKeys, OpenPalette};
use tessera_app::connection::CollectionChanged;
use tessera_app::jobs::JobsGlobal;
use tessera_app::open_main_window;
use tessera_app::state::{AppState, UpdateFound};
use tessera_app::views::root::RootView;
use tessera_client::{DirtyValues, ImportAction, ImportParams};
use tessera_core::profile::Profile;
use tessera_core::search::params::SearchDraft;
use tessera_core::search::saved::SavedQuery;
use tessera_core::secrets::MemoryStore;
use tessera_core::store;
use tessera_core::transfer::JobEnd;
use tessera_core::transfer::import::{ImportJob, ImportProgress};
use tessera_core::transfer::source::{Format, ReadOptions, Source};

use common::*;

const NEW_WINDOW: &str = key("alt-cmd-n", "ctrl-alt-n");
const CLOSE_WINDOW: &str = key("alt-cmd-w", "ctrl-alt-w");

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
    let rt = tokio(cx);
    let (first, state) = open_state_at(
        cx,
        Ok(store::ConnectionsFile::default()),
        Some(path.clone()),
        secrets(),
    );
    cx.update(|cx| cx.set_quit_mode(gpui_kit::QuitMode::LastWindowClosed));
    state.update(cx, |s, cx| {
        s.upsert_profile(Profile::new("kept", vec![node()]), None, cx)
            .detach();
    });
    // The window was the last thing to hold the state, as in `run`.
    drop(state);
    cx.update_window(first.into(), |_, window, _| window.remove_window())
        .unwrap();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    cx.quit();
    let file = store::load(&path).unwrap();
    assert_eq!(file.profiles.len(), 1, "save finished before quit returned");
    drop(rt);
}

#[gpui_kit::test]
fn closing_the_last_window_writes_a_layout_still_waiting(cx: &mut TestAppContext) {
    cx.executor().set_block_on_ticks(usize::MAX..=usize::MAX);
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.json");
    let (first, state, _rt) = open_app(cx, Vec::new(), secrets());
    state.update(cx, |s, _| s.load_layout(Some(path.clone())));
    let open = cx.read(|cx| state.read(cx).layout.bottom_open);
    drop(state);
    cx.update_window(first.into(), |_, window, cx| window.press(TOGGLE_LOG, cx))
        .unwrap();
    assert!(!path.exists(), "the save waits a moment");
    cx.update_window(first.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.quit();
    assert_eq!(tessera_core::layout::load(&path).bottom_open, !open);
}

/// Staging on `first`, prod on `second`, with prod touched last and `first` in front.
async fn one_connection_each(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Root>,
    WindowHandle<Root>,
    Entity<AppState>,
    Profile,
    Profile,
    tokio::runtime::Runtime,
) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, rt) = open_app(cx, vec![staging.clone(), prod.clone()], secrets);
    let second = second_window(cx, &state);
    activate(cx, first);
    click_profile(cx, first, &staging);
    wait_for_element(cx, first, tab(&staging), true).await;
    activate(cx, second);
    click_profile(cx, second, &prod);
    wait_for_element(cx, second, tab(&prod), true).await;
    activate(cx, first);
    (first, second, state, staging, prod, rt)
}

#[gpui_kit::test]
async fn a_chord_acts_on_the_connection_of_its_window(cx: &mut TestAppContext) {
    let (first, second, _state, staging, prod, _rt) = one_connection_each(cx).await;
    cx.update_window(first.into(), |_, window, cx| {
        window.dispatch_action(Box::new(OpenKeys), cx);
    })
    .unwrap();
    wait_for_element(cx, first, format!("tab.{}.keys", staging.id), true).await;
    let keys = format!("tab.{}.keys", prod.id);
    let shown = cx
        .update_window(second.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find(keys).is_some()
        })
        .unwrap();
    assert!(!shown, "prod stays as it was");
    assert_eq!(cx.update(|cx| cx.active_window()), Some(first.into()));
}

#[gpui_kit::test]
async fn the_palette_lists_the_saved_queries_of_its_window(cx: &mut TestAppContext) {
    let (first, _second, state, staging, prod, _rt) = one_connection_each(cx).await;
    let query = |profile: &Profile| SavedQuery {
        id: uuid::Uuid::new_v4(),
        profile: profile.id,
        name: format!("kept query {}", profile.name),
        tags: vec![],
        union: false,
        searches: vec![SearchDraft::new("books")],
    };
    let (ours, theirs) = (query(&staging), query(&prod));
    let ids = (ours.id, theirs.id);
    cx.update(|cx| {
        state.update(cx, |s, cx| {
            s.save_query(ours, cx).detach();
            s.save_query(theirs, cx).detach();
        });
    });
    cx.run_until_parked();
    cx.update_window(first.into(), |_, window, cx| {
        window.dispatch_action(Box::new(OpenPalette), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(first.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("kept", cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait_for_element(cx, first, format!("palette.saved.{}", ids.0), true).await;
    let other = format!("palette.saved.{}", ids.1);
    let shown = cx
        .update_window(first.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find(other).is_some()
        })
        .unwrap();
    assert!(!shown);
}

#[gpui_kit::test]
async fn the_shortcut_closes_a_lone_tab_and_keeps_the_window(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, state, _rt) = open_app(cx, vec![staging.clone(), prod], secrets);
    activate(cx, first);
    click_profile(cx, first, &staging);
    wait_for_element(cx, first, tab(&staging), true).await;
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
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    wait_for_element(cx, first, "welcome.connect_local".into(), true).await;
}

#[gpui_kit::test]
async fn the_shortcut_leaves_a_lone_tab_while_focus_is_in_the_log(cx: &mut TestAppContext) {
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (staging, prod, secrets) = two_profiles();
    let (first, _state, _rt) = open_app(cx, vec![staging.clone(), prod], secrets);
    activate(cx, first);
    click_profile(cx, first, &staging);
    wait_for_element(cx, first, tab(&staging), true).await;
    cx.update_window(first.into(), |root, window, cx| {
        let view = root.downcast::<Root>().unwrap().read(cx).view().clone();
        let view = view.downcast::<RootView>().unwrap();
        let log = view.read(cx).log_panel().read(cx).focus_handle(cx);
        log.focus(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(first.into(), |_, window, cx| window.press(CLOSE_TAB, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(tabs(cx, first, &staging), 1);
}

#[gpui_kit::test]
async fn close_window_closes_this_window_alone(cx: &mut TestAppContext) {
    let (first, state, _rt) = open_app(cx, Vec::new(), secrets());
    let second = second_window(cx, &state);
    activate(cx, second);
    cx.update_window(second.into(), |_, window, cx| {
        window.press(CLOSE_WINDOW, cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(cx.update(|cx| cx.windows().contains(&first.into())));
}

#[gpui_kit::test]
async fn a_finished_job_reloads_once_and_toasts_in_the_front_window(cx: &mut TestAppContext) {
    let (first, second, state, staging, _prod, _rt) = one_connection_each(cx).await;
    let fake = fake_transfer(cx);
    activate(cx, second);
    let conn = cx.read(|cx| state.read(cx).connection(staging.id, cx).unwrap());
    let jobs = cx.read(|cx| cx.global::<JobsGlobal>().0.clone());
    let changes = Rc::new(Cell::new(0));
    let seen = changes.clone();
    let _watch = cx.update(|cx| {
        cx.subscribe(&conn, move |_, _: &CollectionChanged, _| {
            seen.set(seen.get() + 1)
        })
    });
    let job = ImportJob {
        source: Source::Bytes(Arc::from(&b"{}\n"[..])),
        options: ReadOptions::new(Format::Jsonl),
        collection: "books".into(),
        params: ImportParams {
            action: ImportAction::Create,
            batch_size: 1000,
            dirty_values: DirtyValues::CoerceOrReject,
        },
    };
    let id = cx.update(|cx| {
        jobs.update(cx, |jobs, cx| {
            jobs.start_import(&conn, job, "books.jsonl".into(), cx)
        })
    });
    cx.run_until_parked();
    fake.finish(
        id,
        import_outcome(JobEnd::Finished, ImportProgress::default()),
    );
    cx.run_until_parked();
    let toasts = |cx: &mut TestAppContext, handle: WindowHandle<Root>| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.notifications(cx).len()
        })
        .unwrap()
    };
    assert_eq!(toasts(cx, first), 0);
    assert_eq!(toasts(cx, second), 1);
    assert_eq!(changes.get(), 1);
}
