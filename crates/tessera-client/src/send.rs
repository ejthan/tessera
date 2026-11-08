// SPDX-License-Identifier: GPL-3.0-or-later

//! The one path every request takes.

use std::time::{Duration, Instant, SystemTime};

use reqwest::Method;
use reqwest::header::HeaderValue;
use secrecy::ExposeSecret;
use serde_json::Value;

use crate::event::{Body, RequestEvent, next_id};
use crate::{Client, Error};

impl Client {
    /// Sends one request to node `node` and reports it to the hook.
    ///
    /// Returns the status and body for any HTTP answer; only transport failures are errors.
    /// A read-only client refuses every method but [`READ_METHODS`](crate::READ_METHODS)
    /// here, before anything is sent or reported, so no caller can get around it. A key that
    /// is no valid header value is [`Error::InvalidKey`], also before anything is reported.
    /// `timeout` replaces the client's for this request.
    pub(crate) async fn send(
        &self,
        node: usize,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<(u16, String), Error> {
        let body = body.map(|body| (body.to_string(), "application/json"));
        let (status, _, text) = self
            .send_with(node, method, path, query, body, timeout)
            .await?;
        Ok((status, text))
    }

    /// [`Self::send`] with the body already serialized under its content type, returning the
    /// response headers too. Header values that are not UTF-8 show lossily.
    pub(crate) async fn send_with(
        &self,
        node: usize,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Option<(String, &'static str)>,
        timeout: Option<Duration>,
    ) -> Result<(u16, Vec<(String, String)>, String), Error> {
        if self.is_read_only()
            && !crate::READ_METHODS.contains(&method)
            && !(self.read_post && method == Method::POST)
        {
            return Err(Error::ReadOnly { method });
        }
        let base = &self.nodes[node];
        let mut url = base
            .join(path)
            .map_err(|_| Error::InvalidUrl(base.to_string()))?;
        url.set_query(query);
        let key = self.key.expose_secret();
        // Checked before sending: reqwest would report a bad value as a network error, which
        // fails over. Sensitive, so HTTP/2 never indexes it and `Debug` never prints it.
        let mut key_header = HeaderValue::from_str(key).map_err(|_| Error::InvalidKey)?;
        key_header.set_sensitive(true);
        let mut request_headers = Vec::new();
        // Event copies of the body are only made when someone listens.
        let request_body = self
            .hook
            .as_ref()
            .and_then(|_| body.as_ref().map(|(text, _)| Body::new(text)));
        let mut request = self
            .http
            .request(method.clone(), url.clone())
            .header("X-TYPESENSE-API-KEY", key_header)
            .timeout(timeout.unwrap_or(self.timeout));
        if let Some((body, content_type)) = body {
            request_headers.push(("content-type".to_string(), content_type.to_string()));
            request = request.header("content-type", content_type).body(body);
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
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                    (name.to_string(), value)
                })
                .collect();
            let text = response.text().await.map_err(Error::from_reqwest)?;
            Ok((code, headers, text))
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
            let text = result.as_ref().ok().map(|(_, _, text)| text);
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
                response_body: text
                    .filter(|_| self.keep_response_body)
                    .map(|t| Body::new(t)),
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
