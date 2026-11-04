// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

/// Response of `GET /health`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Health {
    /// `true` once the node is ready to serve requests.
    pub ok: bool,
}

/// Response of `GET /debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Debug {
    /// Server version exactly as reported, for example `"30.2"`.
    pub version: String,
    /// The node's Raft role.
    pub state: RaftState,
}

/// Raft role of a node, from the numeric `state` field of `GET /debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaftState {
    /// `state == 1`.
    Leader,
    /// `state == 4`.
    Follower,
    /// Any other reported state.
    Other(i64),
    /// The server did not report a state.
    Unknown,
}

impl From<Option<i64>> for RaftState {
    fn from(state: Option<i64>) -> Self {
        match state {
            Some(1) => Self::Leader,
            Some(4) => Self::Follower,
            Some(n) => Self::Other(n),
            None => Self::Unknown,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct RawDebug {
    pub version: String,
    pub state: Option<i64>,
}

/// Response of `GET /metrics.json`. The server sends every value as a string; a figure that is
/// missing (some container runtimes omit network or swap) or does not parse is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    /// `system_cpu_active_percentage`.
    pub cpu_percent: Option<f64>,
    /// `system_memory_used_bytes`.
    pub memory_used: Option<u64>,
    /// `system_memory_total_bytes`.
    pub memory_total: Option<u64>,
    /// `system_disk_used_bytes`.
    pub disk_used: Option<u64>,
    /// `system_disk_total_bytes`.
    pub disk_total: Option<u64>,
    /// `system_network_received_bytes`, cumulative since the server started.
    pub net_received: Option<u64>,
    /// `system_network_sent_bytes`, cumulative since the server started.
    pub net_sent: Option<u64>,
    /// `typesense_memory_active_bytes`.
    pub typesense_memory_active: Option<u64>,
    /// Every field as sent, including per-CPU and other `typesense_memory_*` figures.
    pub raw: BTreeMap<String, String>,
}

impl Metrics {
    pub(crate) fn from_json(body: &str) -> Result<Self, serde_json::Error> {
        let map: Map<String, Value> = serde_json::from_str(body)?;
        // Strings are the documented form; anything else is kept as its JSON text.
        let raw: BTreeMap<String, String> = map
            .into_iter()
            .map(|(k, v)| match v {
                Value::String(s) => (k, s),
                other => (k, other.to_string()),
            })
            .collect();
        let float = |key: &str| raw.get(key).and_then(|v| v.parse::<f64>().ok());
        let int = |key: &str| raw.get(key).and_then(|v| v.parse::<u64>().ok());
        Ok(Self {
            cpu_percent: float("system_cpu_active_percentage"),
            memory_used: int("system_memory_used_bytes"),
            memory_total: int("system_memory_total_bytes"),
            disk_used: int("system_disk_used_bytes"),
            disk_total: int("system_disk_total_bytes"),
            net_received: int("system_network_received_bytes"),
            net_sent: int("system_network_sent_bytes"),
            typesense_memory_active: int("typesense_memory_active_bytes"),
            raw,
        })
    }
}

/// Response of `GET /stats.json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Stats {
    /// Average latency per endpoint, keyed like `"GET /collections"`. Empty on a fresh server.
    #[serde(default)]
    pub latency_ms: BTreeMap<String, f64>,
    /// Requests per second per endpoint, keyed like `latency_ms`.
    #[serde(default)]
    pub requests_per_second: BTreeMap<String, f64>,
    /// `total_requests_per_second`.
    #[serde(default)]
    pub total_requests_per_second: Option<f64>,
    /// Every other field as sent, such as `search_latency_ms`.
    #[serde(flatten)]
    pub raw: Map<String, Value>,
}

/// One entry of `GET /collections?exclude_fields=fields`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CollectionSummary {
    /// Collection name.
    pub name: String,
    /// Number of documents.
    pub num_documents: u64,
}

/// What the API key may do, from the status of `GET /keys`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAccess {
    /// `200`: the key can list API keys, so it is an admin key.
    Admin,
    /// `401` or `403`: the key cannot list API keys. Typesense also answers `401` to a wrong
    /// key, so this only means "restricted" once an authenticated call (such as `debug`)
    /// has confirmed the key.
    Restricted,
}
