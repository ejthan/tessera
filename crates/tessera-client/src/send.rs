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
    pub(crate) async fn send(
        &self,
        node: usize,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Option<String>,
    ) -> Result<(u16, String), Error> {
        let base = &self.nodes[node];
        let mut url = base
            .join(path)
            .map_err(|_| Error::InvalidUrl(base.to_string()))?;
        url.set_query(query);
        let key = self.key.expose_secret();
        let mut request_headers = Vec::new();
        let mut request = self
            .http
            .request(method.clone(), url.clone())
            .header("X-TYPESENSE-API-KEY", key)
            .timeout(self.timeout);
        if let Some(body) = &body {
            request_headers.push(("content-type".to_string(), "application/json".to_string()));
            request = request
                .header("content-type", "application/json")
                .body(body.clone());
        }
        let started_at = SystemTime::now();
        let start = Instant::now();
        let result = async {
            let response = request.send().await.map_err(Error::from_reqwest)?;
            let status = response.status().as_u16();
            let text = response.text().await.map_err(Error::from_reqwest)?;
            Ok((status, text))
        }
        .await;
        let event = RequestEvent {
            id: next_id(),
            started_at,
            node: base.clone(),
            method,
            path: url.path().to_string(),
            query: url.query().map(str::to_string),
            request_headers,
            key_prefix: key.chars().take(4).collect(),
            request_body: body.as_deref().map(Body::new),
            response_body: result.as_ref().ok().map(|(_, text)| Body::new(text)),
            status: result.as_ref().ok().map(|(status, _)| *status),
            duration: start.elapsed(),
            size: result.as_ref().ok().map(|(_, text)| text.len() as u64),
            origin: self.origin,
            error: result.as_ref().err().map(ToString::to_string),
        };
        tracing::debug!(
            method = %event.method,
            path = %event.path,
            status = ?event.status,
            ms = event.duration.as_millis() as u64,
            "typesense request"
        );
        if let Some(hook) = &self.hook {
            hook(&event);
        }
        result
    }
}
