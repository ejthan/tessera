// SPDX-License-Identifier: GPL-3.0-or-later

//! Polls every node of a connection while its overview tab is open (spec §7.8).
//!
//! The polled data lives on [`ConnectionState`], so a reopened tab still shows the history;
//! the [`Poller`] only drives it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, SharedString, Subscription, Task, Window,
};
use tessera_client::{Client, CollectionSummary, Debug, Error, Health, Metrics, NodeClient, Stats};
use tessera_core::metrics::MetricsHistory;
use tokio::runtime::Handle;

use crate::connection::{ConnectionState, host_label, node_error_text};
use crate::runtime::AbortOnDrop;

/// Polling interval until the user picks another one.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
/// How much metrics history each node keeps.
pub const HISTORY_WINDOW: Duration = Duration::from_secs(15 * 60);
/// First wait after a node failed; it doubles on each further failure.
const MIN_BACKOFF: Duration = Duration::from_secs(5);
/// Longest wait between attempts on a failing node.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// What polling knows about one node.
#[derive(Debug, Clone, Default)]
pub struct NodeStatus {
    /// The last `/debug` answer; kept while the node is unreachable.
    pub debug: Option<Debug>,
    /// `/health` of the last tick; `None` when it failed or has not run.
    pub healthy: Option<bool>,
    /// Duration of the last successful `/debug`.
    pub latency: Option<Duration>,
    /// Full text of every request of the last tick that failed. A failed `/metrics.json` or
    /// `/stats.json` alone shows here too, but keeps the node up and polled at the interval.
    pub error: Option<SharedString>,
    /// The node is skipped until then, after `/health` or `/debug` failed.
    pub next_retry: Option<Instant>,
    /// Current wait between attempts: zero while the node answers, else 5 s doubling to 30 s.
    pub backoff: Duration,
}

/// The four answers of one node in one tick.
#[derive(Debug)]
pub struct NodeSample {
    /// `GET /health`.
    pub health: Result<Health, Error>,
    /// `GET /debug`.
    pub debug: Result<Debug, Error>,
    /// `GET /metrics.json`.
    pub metrics: Result<Metrics, Error>,
    /// `GET /stats.json`.
    pub stats: Result<Stats, Error>,
    /// Duration of `GET /debug`, like the connect latency.
    pub latency: Duration,
}

/// Future returned by a [`PollFetcher`].
pub type SampleFuture = Pin<Box<dyn Future<Output = NodeSample> + Send>>;
/// Fetches one node's sample. Dropping the future cancels its requests.
pub type PollFetcher = Arc<dyn Fn(NodeClient) -> SampleFuture + Send + Sync>;

/// The node fetcher polling uses. The app installs [`tokio_fetcher`]; UI tests install fakes,
/// because GPUI's test executor does not wait for real network I/O.
pub struct PollFetcherGlobal(pub PollFetcher);

impl Global for PollFetcherGlobal {}

/// Future returned by a [`CollectionsFetcher`].
pub type CollectionsFuture =
    Pin<Box<dyn Future<Output = Result<Vec<CollectionSummary>, Error>> + Send>>;
/// Loads the collection summaries through a client.
pub type CollectionsFetcher = Arc<dyn Fn(Client) -> CollectionsFuture + Send + Sync>;

/// The collections loader of the overview; [`tokio_collections`] in the app, fakes in tests.
pub struct CollectionsFetcherGlobal(pub CollectionsFetcher);

impl Global for CollectionsFetcherGlobal {}

/// Awaits a tokio task, re-raising its panic. It is only aborted when the future is dropped.
async fn join<T>(task: AbortOnDrop<T>) -> T {
    match task.await {
        Ok(value) => value,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// The real node fetcher: the four requests of a node concurrently on the tokio runtime.
pub fn tokio_fetcher(handle: Handle) -> PollFetcher {
    Arc::new(move |node| {
        let task = AbortOnDrop::from_join_handle(handle.spawn(async move {
            let timed_debug = async {
                let start = Instant::now();
                let debug = node.debug().await;
                (debug, start.elapsed())
            };
            let (health, (debug, latency), metrics, stats) =
                tokio::join!(node.health(), timed_debug, node.metrics(), node.stats());
            NodeSample {
                health,
                debug,
                metrics,
                stats,
                latency,
            }
        }));
        Box::pin(join(task))
    })
}

/// The real collections loader, on the tokio runtime.
pub fn tokio_collections(handle: Handle) -> CollectionsFetcher {
    Arc::new(move |client| {
        let task = AbortOnDrop::from_join_handle(
            handle.spawn(async move { client.collections_summary().await }),
        );
        Box::pin(join(task))
    })
}

/// Drives polling of one connection; owned by its overview tab. Dropping it stops polling
/// and cancels the requests in flight.
pub struct Poller {
    connection: Entity<ConnectionState>,
    visible: bool,
    /// Paused, interval, connected and visible as of the last (re)start of `task`.
    running_for: Option<(bool, Duration, bool, bool)>,
    task: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl Poller {
    /// Starts polling `connection` right away and loads its collections once.
    pub fn start(
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        connection.update(cx, |c, cx| c.load_collections(true, cx));
        cx.new(|cx: &mut Context<Self>| {
            let subscriptions = [
                cx.observe(&connection, |this, _, cx| this.restart(cx)),
                cx.observe_window_visibility(window, |this, visibility, _, cx| {
                    this.visible = visibility.is_visible();
                    this.restart(cx);
                }),
            ];
            let mut this = Self {
                connection,
                visible: window.is_visible(),
                running_for: None,
                task: None,
                _subscriptions: subscriptions,
            };
            this.restart(cx);
            this
        })
    }

    /// Restarts the tick loop, which ticks at once, when pausing, the interval, the
    /// connection or the window visibility changed; stops it while it must not run.
    fn restart(&mut self, cx: &mut Context<Self>) {
        let conn = self.connection.read(cx);
        let key = (
            conn.paused,
            conn.interval,
            conn.connected.is_some(),
            self.visible,
        );
        if self.running_for == Some(key) {
            return;
        }
        self.running_for = Some(key);
        self.task = None;
        let (paused, interval, connected, visible) = key;
        if paused || !connected || !visible {
            return;
        }
        let connection = self.connection.clone();
        self.task = Some(cx.spawn(async move |_, cx| {
            loop {
                let at = cx.background_executor().now();
                let jobs = connection.update(cx, |c, cx| due_jobs(c, at, cx));
                let mut samples = Vec::with_capacity(jobs.len());
                for (index, job) in jobs {
                    samples.push((index, job.await));
                }
                connection.update(cx, |c, cx| {
                    apply(c, at, samples);
                    cx.notify();
                });
                let elapsed = cx.background_executor().now().saturating_duration_since(at);
                cx.background_executor()
                    .timer(interval.saturating_sub(elapsed))
                    .await;
            }
        }));
    }
}

/// Starts the fetch of every node whose retry time has passed, each on the background
/// executor so the nodes run in parallel.
fn due_jobs(
    conn: &mut ConnectionState,
    at: Instant,
    cx: &mut Context<ConnectionState>,
) -> Vec<(usize, Task<NodeSample>)> {
    let Some(connected) = &conn.connected else {
        return Vec::new();
    };
    let client = connected.client.polling();
    // A nearest node outside the profile's nodes makes the client one node longer.
    let count = client.nodes().len();
    conn.nodes.resize_with(count, NodeStatus::default);
    conn.history
        .resize_with(count, || MetricsHistory::new(HISTORY_WINDOW));
    let fetch = cx.global::<PollFetcherGlobal>().0.clone();
    (0..count)
        .filter(|&i| conn.nodes[i].next_retry.is_none_or(|t| t <= at))
        .map(|i| (i, cx.background_executor().spawn(fetch(client.node(i)))))
        .collect()
}

/// Applies one tick's samples, taken at `at`.
fn apply(conn: &mut ConnectionState, at: Instant, samples: Vec<(usize, NodeSample)>) {
    let Some(client) = conn.connected.as_ref().map(|c| c.client.clone()) else {
        return;
    };
    let active = client.active_node();
    for (i, sample) in samples {
        let host = client
            .nodes()
            .get(i)
            .map(|u| host_label(u.as_str()))
            .unwrap_or_default();
        let errors: Vec<String> = [
            sample.health.as_ref().err(),
            sample.debug.as_ref().err(),
            sample.metrics.as_ref().err(),
            sample.stats.as_ref().err(),
        ]
        .into_iter()
        .flatten()
        .map(|e| node_error_text(e, &host, &conn.profile))
        .collect();
        let (Some(node), Some(history)) = (conn.nodes.get_mut(i), conn.history.get_mut(i)) else {
            continue;
        };
        node.healthy = sample.health.as_ref().ok().map(|h| h.ok);
        node.error = (!errors.is_empty()).then(|| errors.join("\n").into());
        match sample.debug {
            Ok(debug) if sample.health.is_ok() => {
                node.debug = Some(debug);
                node.latency = Some(sample.latency);
                node.backoff = Duration::ZERO;
                node.next_retry = None;
            }
            _ => {
                node.latency = None;
                node.backoff = (node.backoff * 2).clamp(MIN_BACKOFF, MAX_BACKOFF);
                node.next_retry = Some(at + node.backoff);
            }
        }
        if let Ok(metrics) = &sample.metrics {
            history.push(at, metrics);
        }
        if i == active {
            conn.stats = sample.stats.ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use secrecy::SecretString;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn tokio_fetcher_asks_one_node_for_all_four() {
        let server = MockServer::start().await;
        for (route, body) in [
            ("/health", r#"{"ok":true}"#),
            ("/debug", r#"{"state":4,"version":"30.2"}"#),
            ("/metrics.json", r#"{"system_cpu_active_percentage":"7.5"}"#),
            ("/stats.json", r#"{"latency_ms":{}}"#),
        ] {
            Mock::given(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        // Node 0 refuses connections; node 1 is the mock, which must not see a failover.
        let nodes = ["http://127.0.0.1:9", &server.uri()];
        let client = Client::new(&nodes, SecretString::from("k"), Default::default())
            .unwrap()
            .polling();
        let sample = tokio_fetcher(Handle::current())(client.node(1)).await;
        assert!(sample.health.unwrap().ok);
        assert_eq!(
            sample.debug.unwrap().state,
            tessera_client::RaftState::Follower
        );
        assert_eq!(sample.metrics.unwrap().cpu_percent, Some(7.5));
        assert!(sample.stats.is_ok());
        assert!(
            tokio_fetcher(Handle::current())(client.node(0))
                .await
                .health
                .is_err()
        );
    }
}
