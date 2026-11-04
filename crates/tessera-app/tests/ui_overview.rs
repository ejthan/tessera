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
        assert!(
            !error.contains('\n'),
            "the same failure shows once: {error}"
        );
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
async fn hanging_node_keeps_others_on_interval(cx: &mut TestAppContext) {
    let seen = fake_poll_async(cx, |node| {
        if node.url().host_str() == Some("n2") {
            Box::pin(std::future::pending())
        } else {
            Box::pin(async { ok_sample() })
        }
    });
    let (_handle, conn, _rt) = cluster(cx).await;
    advance(cx, Duration::from_secs(10));
    conn.read_with(cx, |c, _| {
        for i in 0..2 {
            let ages: Vec<u64> = c.history[i]
                .points(Series::Cpu)
                .map(|(age, _)| age.as_secs())
                .collect();
            assert_eq!(ages, [10, 5, 0], "node {i} samples every 5 s");
        }
    });
    let asked = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|u| u.contains("n2"))
        .count();
    assert_eq!(asked, 1);
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
    let polls = |host: &str| polls(&seen, host);
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

/// How often each host was polled.
fn polls(seen: &Arc<Mutex<Vec<String>>>, host: &str) -> usize {
    let seen = seen.lock().unwrap();
    seen.iter().filter(|u| u.contains(host)).count()
}

#[gpui_kit::test]
async fn recovered_node_resets_backoff(cx: &mut TestAppContext) {
    let calls = Arc::new(Mutex::new(0));
    let counter = calls.clone();
    let seen = fake_poll(cx, move |node| {
        if node.url().host_str() != Some("n0") {
            return ok_sample();
        }
        let mut calls = counter.lock().unwrap();
        *calls += 1;
        if *calls <= 3 { down() } else { ok_sample() }
    });
    let (_handle, conn, _rt) = cluster(cx).await;
    // Failures at 0, 5 and 15 s; the retry at 35 s succeeds.
    advance(cx, Duration::from_secs(35));
    assert_eq!(polls(&seen, "n0"), 4);
    conn.read_with(cx, |c, _| {
        assert_eq!(c.nodes[0].backoff, Duration::ZERO);
        assert_eq!(c.nodes[0].next_retry, None);
        assert_eq!(c.nodes[0].error, None);
    });
    advance(cx, Duration::from_secs(10));
    assert_eq!(polls(&seen, "n0"), 6, "back on the 5 s interval");
}

#[gpui_kit::test]
async fn metrics_failure_keeps_node_up(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    let seen = fake_poll(cx, |node| {
        let mut sample = ok_sample();
        if node.url().host_str() == Some("n1") {
            sample.metrics = Err(Error::Timeout);
        }
        sample
    });
    let (_handle, conn, _rt) = cluster(cx).await;
    advance(cx, Duration::from_secs(10));
    assert_eq!(polls(&seen, "n1"), 3);
    conn.read_with(cx, |c, _| {
        let node = &c.nodes[1];
        assert!(
            node.error
                .as_deref()
                .unwrap_or_default()
                .starts_with("n1:8108")
        );
        assert_eq!(node.backoff, Duration::ZERO);
        assert_eq!(node.healthy, Some(true));
        assert_eq!(c.history[1].points(Series::Cpu).count(), 0);
    });
}

#[gpui_kit::test]
async fn not_ready_node_backs_off(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    let seen = fake_poll(cx, |node| {
        let mut sample = ok_sample();
        if node.url().host_str() == Some("n2") {
            sample.health = Ok(tessera_client::Health { ok: false });
        }
        sample
    });
    let (_handle, conn, _rt) = cluster(cx).await;
    advance(cx, Duration::from_secs(10));
    assert_eq!(polls(&seen, "n2"), 2, "asked at 0 and 5 s, then after 10 s");
    conn.read_with(cx, |c, _| {
        let node = &c.nodes[2];
        assert_eq!(node.healthy, Some(false));
        assert_eq!(node.backoff, Duration::from_secs(10));
        let error = node.error.as_deref().unwrap_or_default();
        assert!(
            error.contains("n2:8108") && error.contains("not ready"),
            "{error}"
        );
    });
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
    let polls = |host: &str| polls(&seen, host);
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

/// Renders the window and returns the label of element `id`.
fn text(cx: &mut TestAppContext, handle: WindowHandle<Root>, id: &str) -> String {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window
            .find(id.to_string())
            .label()
            .unwrap_or_default()
            .to_string()
    })
    .unwrap()
}

/// Tabs to element `id` and presses `key` there.
fn press_on(cx: &mut TestAppContext, handle: WindowHandle<Root>, id: &str, key: &str) {
    cx.update_window(handle.into(), |_, window, cx| {
        focus(window, id, cx);
        window.press(key, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// `ok_sample` with the role and CPU value of the given host.
fn sample_for(node: &tessera_client::NodeClient) -> tessera_app::poller::NodeSample {
    let mut sample = ok_sample();
    match node.url().host_str() {
        Some("n2") => return down(),
        Some("n1") => {
            sample.debug.as_mut().unwrap().state = tessera_client::RaftState::Follower;
            sample.metrics.as_mut().unwrap().cpu_percent = Some(50.0);
        }
        _ => {}
    }
    sample
}

#[gpui_kit::test]
async fn node_cards_show_roles_and_unreachable(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    fake_poll(cx, sample_for);
    let (handle, _conn, _rt) = cluster(cx).await;
    let card = text(cx, handle, "node.0");
    assert!(
        card.contains("Leader") && card.contains("n0:8108"),
        "{card}"
    );
    assert!(card.contains("active node"), "{card}");
    assert!(text(cx, handle, "node.1").contains("Follower"));
    let error = text(cx, handle, "node.2.error");
    assert!(error.starts_with("n2:8108 did not answer"), "{error}");
    assert!(error.contains("retry in 5 s"), "{error}");
    advance(cx, Duration::from_secs(2));
    assert_eq!(text(cx, handle, "node.2.retry"), "retry in 3 s");
}

#[gpui_kit::test]
async fn click_node_makes_it_active(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    fake_poll(cx, sample_for);
    let (handle, conn, _rt) = cluster(cx).await;
    assert!(text(cx, handle, "chart.cpu").contains("12.5 %"));
    press_on(cx, handle, "node.1", "enter");
    let active = conn.read_with(cx, |c, _| {
        c.connected.as_ref().unwrap().client.active_node()
    });
    assert_eq!(active, 1);
    assert!(text(cx, handle, "node.1").contains("active node"));
    assert!(!text(cx, handle, "node.0").contains("active node"));
    let chart = text(cx, handle, "chart.cpu");
    assert!(chart.contains("50.0 %"), "node 1's history: {chart}");
}

#[gpui_kit::test]
async fn figures_show_dash_for_missing_metrics(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    let calls = Arc::new(Mutex::new(0u64));
    fake_poll(cx, move |_| {
        let mut calls = calls.lock().unwrap();
        *calls += 1;
        let mut sample = ok_sample();
        let metrics = sample.metrics.as_mut().unwrap();
        // Counters at 0 and 5000 bytes, then a sample without the network fields.
        let counter = (*calls < 3).then_some((*calls - 1) * 5000);
        metrics.net_received = counter;
        metrics.net_sent = counter;
        sample
    });
    fake_connector(cx, |p| Ok(server_for(p)));
    let p = profile("single", &["n0"]);
    let (handle, state, _rt) = open_app(cx, vec![p.clone()], store(&[&p]));
    open_tab(cx, handle, &state, &p).await;
    advance(cx, TICK);
    assert_eq!(
        text(cx, handle, "figure.network"),
        "rx 1000 B/s · tx 1000 B/s"
    );
    advance(cx, TICK);
    assert_eq!(text(cx, handle, "figure.network"), "–");
    assert_eq!(text(cx, handle, "figure.cpu"), "12.5 %");
    assert_eq!(text(cx, handle, "figure.memory"), "1 B of 4 B");
}

/// Stats with three endpoints at 1, 3 and 2 requests per second.
fn stats() -> tessera_client::Stats {
    let map = |values: [f64; 3]| {
        ["GET /a", "GET /b", "GET /c"]
            .into_iter()
            .map(String::from)
            .zip(values)
            .collect()
    };
    tessera_client::Stats {
        latency_ms: map([5.0, 1.0, 9.0]),
        requests_per_second: map([1.0, 3.0, 2.0]),
        total_requests_per_second: Some(6.0),
        raw: Default::default(),
    }
}

/// The endpoints of the stats table from top to bottom.
fn stats_order(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Vec<&'static str> {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let mut rows: Vec<_> = ["GET /a", "GET /b", "GET /c"]
            .into_iter()
            .map(|e| (window.find(format!("stats.row.{e}")).bounds().origin.y, e))
            .collect();
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        rows.into_iter().map(|(_, e)| e).collect()
    })
    .unwrap()
}

#[gpui_kit::test]
async fn stats_sorted_by_rps_desc(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    fake_poll(cx, |_| {
        let mut sample = ok_sample();
        sample.stats = Ok(stats());
        sample
    });
    let (handle, _conn, _rt) = cluster(cx).await;
    assert_eq!(stats_order(cx, handle), ["GET /b", "GET /c", "GET /a"]);
    cx.update_window(handle.into(), |_, window, cx| {
        // The table sits below the charts.
        let down =
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-2000.)));
        window.scroll("overview", down, cx);
        window.click("stats.sort.rps", cx);
    })
    .unwrap();
    assert_eq!(stats_order(cx, handle), ["GET /a", "GET /c", "GET /b"]);
    press_on(cx, handle, "stats.sort.latency", "enter");
    assert_eq!(stats_order(cx, handle), ["GET /c", "GET /a", "GET /b"]);
    assert!(text(cx, handle, "stats.row.GET /c").contains("9.0 ms"));
}

#[gpui_kit::test]
async fn stats_empty_explains(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    let (handle, _conn, _rt) = cluster(cx).await;
    let empty = text(cx, handle, "stats.empty");
    assert!(empty.contains("receives requests"), "{empty}");
}

#[gpui_kit::test]
async fn stats_show_active_node_error(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    fake_poll(cx, |_| {
        let mut sample = ok_sample();
        sample.stats = Err(Error::Timeout);
        sample
    });
    let (handle, _conn, _rt) = cluster(cx).await;
    let error = text(cx, handle, "stats.error");
    assert!(error.starts_with("n0:8108 did not answer"), "{error}");
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.try_find("stats.empty").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn space_pauses_polling(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    let (handle, conn, _rt) = cluster(cx).await;
    assert_eq!(text(cx, handle, "overview.pause"), "Pause");
    assert_eq!(text(cx, handle, "overview.live"), "Live · every 5 s");
    press_on(cx, handle, "node.0", "space");
    assert_eq!(text(cx, handle, "overview.pause"), "Resume");
    assert_eq!(text(cx, handle, "overview.live"), "Paused");
    assert!(conn.read_with(cx, |c, _| c.paused));
    let active = conn.read_with(cx, |c, _| {
        c.connected.as_ref().unwrap().client.active_node()
    });
    assert_eq!(active, 0, "Space does not also click the focused card");
    press_on(cx, handle, "node.0", "space");
    assert_eq!(text(cx, handle, "overview.pause"), "Pause");
}

#[gpui_kit::test]
async fn refresh_shortcut_reloads_collections(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    let origins = fake_collections(cx);
    let (handle, _conn, _rt) = cluster(cx).await;
    assert_eq!(text(cx, handle, "figure.collections"), "1");
    assert_eq!(text(cx, handle, "figure.documents"), "7");
    press_on(cx, handle, "node.0", REFRESH);
    assert_eq!(*origins.lock().unwrap(), [Origin::Polling, Origin::User]);
}

#[gpui_kit::test]
async fn status_bar_counts_unreachable_nodes(cx: &mut TestAppContext) {
    rust_i18n::set_locale("en");
    fake_poll(cx, sample_for);
    let (handle, _conn, _rt) = cluster(cx).await;
    assert_eq!(
        text(cx, handle, "status.unreachable"),
        "1 of 3 nodes unreachable"
    );
}
