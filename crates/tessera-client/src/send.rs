// SPDX-License-Identifier: GPL-3.0-or-later

//! The one path every request takes.

use std::time::{Duration, Instant, SystemTime};

use reqwest::Method;
use reqwest::header::HeaderValue;
use secrecy::ExposeSecret;
use serde_json::Value;

use crate::event::{Body, RequestEvent, next_id};
use crate::export::{STREAM_STALL, StreamLog};
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
        let Prepared { request, event } = self.prepare(node, &method, path, query, body)?;
        let request = request.timeout(timeout.unwrap_or(self.timeout));
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
            method = %event.method,
            path = %event.path,
            status = ?status,
            ms = duration.as_millis() as u64,
            "typesense request"
        );
        if let Some(hook) = &self.hook {
            let text = result.as_ref().ok().map(|(_, _, text)| text);
            let mut event = event;
            event.id = id;
            event.started_at = started_at;
            event.response_body = text
                .filter(|_| self.keep_response_body)
                .map(|t| Body::new(t));
            event.status = status;
            event.duration = duration;
            event.size = text.map(|t| t.len() as u64);
            event.error = result.as_ref().err().map(ToString::to_string);
            hook(&event);
        }
        result
    }
}

impl Client {
    /// Sends a `GET` whose answer is read as a stream. No reqwest timeout, which would be a
    /// total deadline over the body (`RequestBuilder::timeout`); the wait for the answer to
    /// start is cut by [`STREAM_STALL`] instead, and `next_line` cuts the gaps. The returned
    /// log reports one event when it is dropped.
    pub(crate) async fn send_stream(
        &self,
        node: usize,
        path: &str,
        query: Option<&str>,
    ) -> Result<(reqwest::Response, StreamLog), Error> {
        let Prepared { request, event } = self.prepare(node, &Method::GET, path, query, None)?;
        let mut log = StreamLog::new(self.hook.clone(), event);
        let sent = tokio::time::timeout(STREAM_STALL, request.send()).await;
        let response = match sent {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                let e = Error::from_reqwest(e);
                log.end(Some(e.to_string()));
                return Err(e);
            }
            Err(_) => {
                log.end(Some(Error::Timeout.to_string()));
                return Err(Error::Timeout);
            }
        };
        log.event.status = Some(response.status().as_u16());
        Ok((response, log))
    }
}

/// A request ready to send and the event that will report it, with the answer still empty.
pub(crate) struct Prepared {
    /// Without a timeout: `send_with` sets one, a stream sets none.
    pub(crate) request: reqwest::RequestBuilder,
    pub(crate) event: RequestEvent,
}

impl Client {
    /// The checks and the request every send shares: the read-only guard, the URL, the key
    /// header, and an event with everything known before the answer.
    pub(crate) fn prepare(
        &self,
        node: usize,
        method: &Method,
        path: &str,
        query: Option<&str>,
        body: Option<(String, &'static str)>,
    ) -> Result<Prepared, Error> {
        if self.is_read_only()
            && !crate::READ_METHODS.contains(method)
            && !(self.read_post && *method == Method::POST)
        {
            return Err(Error::ReadOnly {
                method: method.clone(),
            });
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
            .filter(|_| self.keep_request_body)
            .and_then(|_| body.as_ref().map(|(text, _)| Body::new(text)));
        let mut request = self
            .http
            .request(method.clone(), url.clone())
            .header("X-TYPESENSE-API-KEY", key_header);
        if let Some((body, content_type)) = body {
            request_headers.push(("content-type".to_string(), content_type.to_string()));
            request = request.header("content-type", content_type).body(body);
        }
        let event = RequestEvent {
            id: 0,
            started_at: SystemTime::now(),
            node: base.clone(),
            method: method.clone(),
            path: url.path().to_string(),
            query: url.query().map(str::to_string),
            request_headers,
            key_prefix: key_prefix(key),
            request_body,
            response_body: None,
            status: None,
            duration: Duration::ZERO,
            size: None,
            origin: self.origin,
            error: None,
        };
        Ok(Prepared { request, event })
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
