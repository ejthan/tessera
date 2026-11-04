// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests against the 3-node dev cluster.
//!
//! Start it with
//! `docker compose -f dev/docker-compose.yml --profile cluster up -d ts1 ts2 ts3`, then run
//! `cargo test -p tessera-client --test cluster -- --ignored --test-threads=1`. Single-threaded,
//! because two tests stop and start the shared containers. `TESSERA_CLUSTER_URLS` (comma
//! separated, one URL per node in the order ts1, ts2, ts3) overrides the default
//! `http://localhost:18111,http://localhost:18112,http://localhost:18113`; the key is `xyz`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, RaftState};

fn urls() -> Vec<String> {
    std::env::var("TESSERA_CLUSTER_URLS")
        .unwrap_or_else(|_| {
            "http://localhost:18111,http://localhost:18112,http://localhost:18113".into()
        })
        .split(',')
        .map(|u| u.trim().to_string())
        .collect()
}

fn client() -> Client {
    let urls = urls();
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    Client::new(&refs, SecretString::from("xyz"), ClientOptions::default())
        .expect("TESSERA_CLUSTER_URLS must be http(s) URLs")
}

fn compose(action: &str, service: &str) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/docker-compose.yml");
    let status = Command::new("docker")
        .args(["compose", "-f"])
        .arg(file)
        .args(["--profile", "cluster", action, service])
        .status()
        .expect("docker is installed");
    assert!(status.success(), "docker compose {action} {service}");
}

/// Polls until `done` holds for every node, or panics after `limit`.
async fn wait_for<F, Fut>(what: &str, limit: Duration, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + limit;
    while !done().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Every node healthy and exactly one leader.
async fn cluster_ready(client: &Client) -> bool {
    let mut leaders = 0;
    for i in 0..client.nodes().len() {
        let node = client.node(i);
        if !matches!(node.health().await, Ok(h) if h.ok) {
            return false;
        }
        match node.debug().await {
            Ok(d) if d.state == RaftState::Leader => leaders += 1,
            Ok(d) if d.state == RaftState::Follower => {}
            _ => return false,
        }
    }
    leaders == 1
}

async fn wait_until_ready(client: &Client) {
    wait_for(
        "a healthy cluster with one leader",
        Duration::from_secs(60),
        || cluster_ready(client),
    )
    .await;
}

/// Stops a container for the test and starts it again on drop, even after a failed assert.
struct Stopped(&'static str);

impl Stopped {
    fn new(service: &'static str) -> Self {
        compose("stop", service);
        Self(service)
    }
}

impl Drop for Stopped {
    fn drop(&mut self) {
        compose("start", self.0);
    }
}

#[tokio::test]
#[ignore = "needs the 3-node cluster, see dev/docker-compose.yml"]
async fn cluster_has_one_leader_two_followers() {
    let client = client();
    assert_eq!(client.nodes().len(), 3);
    wait_until_ready(&client).await;
    let mut states = Vec::new();
    for i in 0..3 {
        states.push(client.node(i).debug().await.expect("debug").state);
    }
    let leaders = states.iter().filter(|s| **s == RaftState::Leader).count();
    let followers = states.iter().filter(|s| **s == RaftState::Follower).count();
    assert_eq!((leaders, followers), (1, 2), "{states:?}");
}

#[tokio::test]
#[ignore = "needs the 3-node cluster, see dev/docker-compose.yml"]
async fn stopped_node_fails_others_answer() {
    let client = client();
    wait_until_ready(&client).await;
    {
        let _stopped = Stopped::new("ts3");
        assert!(client.node(2).health().await.is_err());
        assert!(client.node(0).health().await.expect("ts1").ok);
        assert!(client.node(1).health().await.expect("ts2").ok);
    }
    wait_until_ready(&client).await;
}

#[tokio::test]
#[ignore = "needs the 3-node cluster, see dev/docker-compose.yml"]
async fn client_fails_over_from_stopped_node() {
    let client = client();
    wait_until_ready(&client).await;
    client.set_active_node(2);
    {
        let _stopped = Stopped::new("ts3");
        assert!(client.health().await.expect("failover").ok);
        assert_ne!(client.active_node(), 2);
    }
    wait_until_ready(&client).await;
}
