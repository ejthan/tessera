// SPDX-License-Identifier: GPL-3.0-or-later

//! The one path every request takes.

use std::time::{Instant, SystemTime};

use reqwest::Method;
use secrecy::ExposeSecret;

use crate::event::{Body, RequestEvent, next_id};
use crate::{Client, Error};

impl Client {
    /// Sends one request to node `node` and reports it to the hook.
    ///
    /// Returns the status and body for any HTTP answer; only transport failures are errors.
    /// A read-only client refuses [`WRITE_METHODS`](crate::WRITE_METHODS) here, before
    /// anything is sent or reported, so no caller can get around it.
    pub(crate) async fn send(
        &self,
        node: usize,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Option<String>,
    ) -> Result<(u16, String), Error> {
        if self.read_only && crate::WRITE_METHODS.contains(&method) {
            return Err(Error::ReadOnly { method });
        }
        let base = &self.nodes[node];
        let mut url = base
            .join(path)
            .map_err(|_| Error::InvalidUrl(base.to_string()))?;
        url.set_query(query);
        let key = self.key.expose_secret();
        let mut request_headers = Vec::new();
        // Event copies of the body are only made when someone listens.
        let request_body = self
            .hook
            .as_ref()
            .and_then(|_| body.as_deref().map(Body::new));
        let mut request = self
            .http
            .request(method.clone(), url.clone())
            .header("X-TYPESENSE-API-KEY", key)
            .timeout(self.timeout);
        if let Some(body) = body {
            request_headers.push(("content-type".to_string(), "application/json".to_string()));
            request = request
                .header("content-type", "application/json")
                .body(body);
        }
        let id = next_id();
        let started_at = SystemTime::now();
        let start = Instant::now();
        // Kept apart from `result`, so a body that fails to arrive still shows the status.
        let mut status = None;
        let result = async {
            let response = request.send().await.map_err(Error::from_reqwest)?;
            let code = response.status().as_u16();
            status = Some(code);
            let text = response.text().await.map_err(Error::from_reqwest)?;
            Ok((code, text))
        }
        .await;
        let duration = start.elapsed();
        tracing::debug!(
            method = %method,
            path = %url.path(),
            status = ?status,
            ms = duration.as_millis() as u64,
            "typesense request"
        );
        if let Some(hook) = &self.hook {
            let text = result.as_ref().ok().map(|(_, text)| text);
            hook(&RequestEvent {
                id,
                started_at,
                node: base.clone(),
                method,
                path: url.path().to_string(),
                query: url.query().map(str::to_string),
                request_headers,
                key_prefix: key_prefix(key),
                request_body,
                response_body: text.map(|t| Body::new(t)),
                status,
                duration,
                size: text.map(|t| t.len() as u64),
                origin: self.origin,
                error: result.as_ref().err().map(ToString::to_string),
            });
        }
        result
    }
}

/// The first 4 characters of keys longer than 8 characters, else nothing, so a short key
/// never shows up whole.
fn key_prefix(key: &str) -> String {
    if key.chars().count() > 8 {
        key.chars().take(4).collect()
    } else {
        String::new()
    }
}
