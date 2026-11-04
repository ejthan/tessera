// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the overview's polling, on GPUI's simulated clock.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, WindowVisibility};
use tessera_app::connection::ConnectionState;
use tessera_app::state::AppState;
use tessera_client::{Error, Origin};
use tessera_core::metrics::Series;
use tessera_core::profile::{Node, Profile};
use tessera_core::secrets::{MemoryStore, SecretStore as _};

use common::*;

const TICK: Duration = Duration::from_secs(5);

/// A profile named `name` with one node per host, and a store holding its key.
fn profile(name: &str, hosts: &[&str]) -> Profile {
    let nodes = hosts
        .iter()
        .map(|h| Node {
            host: (*h).into(),
            ..node()
        })
        .collect();
    Profile::new(name, nodes)
}

fn store(profiles: &[&Profile]) -> Arc<MemoryStore> {
    let secrets = Arc::new(MemoryStore::default());
    for p in profiles {
        secrets.set(p.id, &"k".to_string().into()).unwrap();
    }
    secrets
}

/// A sample whose every request timed out.
fn down() -> tessera_app::poller::NodeSample {
    tessera_app::poller::NodeSample {
        health: Err(Error::Timeout),
        debug: Err(Error::Timeout),
        metrics: Err(Error::Timeout),
        stats: Err(Error::Timeout),
        latency: Duration::ZERO,
    }
}

/// Clicks `profile`'s sidebar entry, waits for its overview tab and lets the first tick run.
async fn open_tab(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    state: &Entity<AppState>,
    profile: &Profile,
) -> Entity<ConnectionState> {
    let id = entry(profile);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    wait_for_element(cx, handle, format!("tab.{}.overview", profile.id), true).await;
    cx.run_until_parked();
    state.read_with(cx, |s, cx| s.connection(profile.id, cx).unwrap())
}

/// A window with the three-node profile `cluster` (hosts n0, n1, n2) opened in a tab.
async fn cluster(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Root>,
    Entity<ConnectionState>,
    tokio::runtime::Runtime,
) {
    fake_connector(cx, |p| Ok(server_for(p)));
    let p = profile("cluster", &["n0", "n1", "n2"]);
    let (handle, state, rt) = open_app(cx, vec![p.clone()], store(&[&p]));
    let conn = open_tab(cx, handle, &state, &p).await;
    (handle, conn, rt)
}

fn count(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock().unwrap().len()
}

fn advance(cx: &mut TestAppContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn failing_node_turns_red_others_update(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    fake_poll(cx, |node| {
        if node.url().host_str() == Some("n2") {
            down()
        } else {
            ok_sample()
        }
    });
    let (_handle, conn, _rt) = cluster(cx).await;
    advance(cx, TICK);
    conn.read_with(cx, |c, _| {
        let error = c.nodes[2].error.as_deref().unwrap_or_default();
        assert!(error.starts_with("n2:8108 did not answer"), "{error}");
        assert_eq!(c.nodes[2].healthy, None);
        for i in 0..2 {
            assert_eq!(c.nodes[i].error, None);
            assert_eq!(c.nodes[i].healthy, Some(true));
            assert_eq!(c.nodes[i].latency, Some(Duration::from_millis(3)));
            assert_eq!(c.history[i].points(Series::Cpu).count(), 2);
        }
        assert_eq!(c.history[2].points(Series::Cpu).count(), 0);
        assert!(c.stats.is_some(), "stats of the active node n0");
    });
}

#[gpui_kit::test]
async fn backoff_caps_at_30s(cx: &mut TestAppContext) {
    let seen = fake_poll(cx, |node| {
        if node.url().host_str() == Some("n0") {
            down()
        } else {
            ok_sample()
        }
    });
    let (_handle, conn, _rt) = cluster(cx).await;
    let polls = |host: &str| {
        let seen = seen.lock().unwrap();
        seen.iter().filter(|u| u.contains(host)).count()
    };
    let mut backoffs = vec![conn.read_with(cx, |c, _| c.nodes[0].backoff.as_secs())];
    let mut gaps = Vec::new();
    let (mut last, mut seconds) = (polls("n0"), 0);
    for second in 1..=70 {
        advance(cx, Duration::from_secs(1));
        if polls("n0") > last {
            last = polls("n0");
            gaps.push(second - seconds);
            seconds = second;
            backoffs.push(conn.read_with(cx, |c, _| c.nodes[0].backoff.as_secs()));
        }
    }
    assert_eq!(backoffs, [5, 10, 20, 30, 30]);
    assert_eq!(gaps, [5, 10, 20, 30]);
    // The healthy nodes keep the 5 s interval: the first tick plus one every 5 s.
    assert_eq!(polls("n1"), 15);
    assert_eq!(
        conn.read_with(cx, |c, _| c.nodes[1].backoff),
        Duration::ZERO
    );
}

#[gpui_kit::test]
async fn hidden_window_stops_ticks(cx: &mut TestAppContext) {
    let seen = fake_poll(cx, |_| ok_sample());
    let (handle, _conn, _rt) = cluster(cx).await;
    let before = count(&seen);
    assert_eq!(before, 3, "the first tick runs at once");
    cx.simulate_window_visibility_change(handle.into(), WindowVisibility::Hidden);
    advance(cx, Duration::from_secs(30));
    assert_eq!(count(&seen), before);
    cx.simulate_window_visibility_change(handle.into(), WindowVisibility::Visible);
    cx.run_until_parked();
    assert_eq!(count(&seen), before + 3, "showing the window ticks at once");
}

#[gpui_kit::test]
async fn pause_stops_ticks(cx: &mut TestAppContext) {
    let seen = fake_poll(cx, |_| ok_sample());
    let (_handle, conn, _rt) = cluster(cx).await;
    let before = count(&seen);
    conn.update(cx, |c, cx| c.set_paused(true, cx));
    advance(cx, Duration::from_secs(30));
    assert_eq!(count(&seen), before);
    conn.update(cx, |c, cx| c.set_paused(false, cx));
    cx.run_until_parked();
    assert_eq!(count(&seen), before + 3, "resuming ticks at once");
}

#[gpui_kit::test]
async fn closing_tab_stops_polling(cx: &mut TestAppContext) {
    let seen = fake_poll(cx, |_| ok_sample());
    fake_connector(cx, |p| Ok(server_for(p)));
    let (a, b) = (profile("alpha", &["a"]), profile("beta", &["b"]));
    let (handle, state, _rt) = open_app(cx, vec![a.clone(), b.clone()], store(&[&a, &b]));
    open_tab(cx, handle, &state, &a).await;
    open_tab(cx, handle, &state, &b).await;
    let polls = |host: &str| {
        let seen = seen.lock().unwrap();
        seen.iter().filter(|u| u.contains(host)).count()
    };
    assert!(polls("//a:") > 0);
    let tab = format!("tab.{}.overview", a.id);
    // Clicking the tab focuses it, so the close shortcut reaches its tab group.
    cx.update_window(handle.into(), |_, window, cx| window.click(tab.clone(), cx))
        .unwrap();
    wait_for_label(cx, handle, "status.text".into(), "alpha").await;
    cx.update_window(handle.into(), |_, window, cx| window.press(CLOSE_TAB, cx))
        .unwrap();
    wait_for_element(cx, handle, tab, false).await;
    cx.run_until_parked();
    let (a_before, b_before) = (polls("//a:"), polls("//b:"));
    advance(cx, Duration::from_secs(30));
    assert_eq!(polls("//a:"), a_before, "the closed tab polls no more");
    assert!(polls("//b:") > b_before, "the open tab keeps polling");
}

#[gpui_kit::test]
async fn collections_load_on_open_and_refresh_only(cx: &mut TestAppContext) {
    let origins = fake_collections(cx);
    let (_handle, conn, _rt) = cluster(cx).await;
    assert_eq!(*origins.lock().unwrap(), [Origin::Polling]);
    advance(cx, Duration::from_secs(30));
    assert_eq!(
        origins.lock().unwrap().len(),
        1,
        "ticks do not load collections"
    );
    conn.update(cx, |c, cx| c.refresh_collections(cx));
    cx.run_until_parked();
    assert_eq!(*origins.lock().unwrap(), [Origin::Polling, Origin::User]);
    conn.read_with(cx, |c, _| {
        let collections = c.collections.as_ref().unwrap();
        assert_eq!(collections[0].num_documents, 7);
    });
}
