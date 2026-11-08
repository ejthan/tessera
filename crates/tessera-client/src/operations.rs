// SPDX-License-Identifier: GPL-3.0-or-later

//! Server operations. Each acts on one node and replicates nothing, so they exist on
//! [`NodeClient`] alone, and a retry on another node cannot happen.

use std::time::Duration;

use serde_json::{Map, Value};

use crate::{Error, LONG_TIMEOUT, Method, NodeClient, encode_query};

/// Whether a body says `"success": false`. A body without it passes.
fn refused(body: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|v| v["success"] == Value::Bool(false))
}

impl NodeClient {
    /// `POST /operations/snapshot`. `path` is on this node's disk. Missing directories are
    /// created, and a used directory keeps its files.
    pub async fn snapshot(&self, path: &str) -> Result<(), Error> {
        let query = encode_query(&[("snapshot_path", path)]);
        let long = Some(LONG_TIMEOUT);
        self.operation("operations/snapshot", Some(&query), None, long)
            .await
    }

    /// `POST /operations/vote`. `false` is the server's refusal, which a leader and a single
    /// node answer. `true` means the election started; the role moves seconds later.
    pub async fn vote(&self) -> Result<bool, Error> {
        let (_, body) = self.post("operations/vote", None, None, None).await?;
        Ok(!refused(&body))
    }

    /// `POST /operations/db/compact`.
    pub async fn compact_db(&self) -> Result<(), Error> {
        let long = Some(LONG_TIMEOUT);
        self.operation("operations/db/compact", None, None, long)
            .await
    }

    /// `POST /operations/cache/clear`, the search response cache only.
    pub async fn clear_cache(&self) -> Result<(), Error> {
        self.operation("operations/cache/clear", None, None, None)
            .await
    }

    /// `POST /config` with one key. The value lasts until this node restarts, cannot be read
    /// back, and an unknown key answers success.
    pub async fn set_config(&self, key: &str, value: &Value) -> Result<(), Error> {
        let body = Value::Object(Map::from_iter([(key.to_string(), value.clone())]));
        self.operation("config", None, Some(&body), None).await
    }

    /// [`Self::post`], with a `2xx` that says `"success": false` as an error.
    async fn operation(
        &self,
        path: &str,
        query: Option<&str>,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let (status, answer) = self.post(path, query, body, timeout).await?;
        if refused(&answer) {
            return Err(Error::http(status, answer));
        }
        Ok(())
    }

    /// Status and body of a `2xx` from this node; any other status is [`Error::Http`].
    async fn post(
        &self,
        path: &str,
        query: Option<&str>,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<(u16, String), Error> {
        let pin = Some(self.index);
        match self
            .client
            .call(pin, Method::POST, path, query, body, timeout)
            .await?
        {
            (status @ 200..300, answer) => Ok((status, answer)),
            (status, answer) => Err(Error::http(status, answer)),
        }
    }
}
