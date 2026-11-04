// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;

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
