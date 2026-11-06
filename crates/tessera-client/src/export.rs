// SPDX-License-Identifier: GPL-3.0-or-later

//! Streaming `documents/export`.

use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use tokio::io::AsyncRead;
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};
use tokio_util::io::StreamReader;

use crate::collections::collection_path;
use crate::documents::encode_query;
use crate::event::{RequestEvent, RequestHook};
use crate::{Client, Error};

/// How long a stream may stay silent: the wait for the answer to start and the gap between
/// two reads. The largest gap in a 1 GB export was 0.08 s.
pub const STREAM_STALL: Duration = Duration::from_secs(120);

/// The longest line of an export the client holds (256 MiB).
pub const MAX_LINE: usize = 256 << 20;

/// What to export.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportQuery {
    /// Typesense `filter_by` expression.
    pub filter_by: Option<String>,
    /// Comma-separated fields to keep.
    pub include_fields: Option<String>,
    /// Comma-separated fields to leave out.
    pub exclude_fields: Option<String>,
}

/// Reports one event when the stream ends or is dropped.
pub(crate) struct StreamLog {
    hook: Option<RequestHook>,
    pub(crate) event: RequestEvent,
    start: Instant,
    pub(crate) bytes: u64,
    finished: bool,
    error: Option<String>,
}

impl StreamLog {
    pub(crate) fn new(hook: Option<RequestHook>, mut event: RequestEvent) -> Self {
        event.id = crate::event::next_id();
        Self {
            hook,
            event,
            start: Instant::now(),
            bytes: 0,
            finished: false,
            error: None,
        }
    }

    /// The answer is complete or failed; `error` is shown instead of "cancelled".
    pub(crate) fn end(&mut self, error: Option<String>) {
        self.finished = true;
        self.error = error;
    }
}

impl Drop for StreamLog {
    fn drop(&mut self) {
        let Some(hook) = &self.hook else {
            return;
        };
        let mut event = self.event.clone();
        event.duration = self.start.elapsed();
        event.size = Some(self.bytes);
        event.error = if self.finished {
            self.error.take()
        } else {
            Some("cancelled".to_string())
        };
        hook(&event);
    }
}

/// The lines of an export, read as they arrive.
pub struct ExportStream {
    lines: FramedRead<Pin<Box<dyn AsyncRead + Send>>, LinesCodec>,
    log: StreamLog,
}

impl std::fmt::Debug for ExportStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportStream")
            .field("bytes", &self.log.bytes)
            .finish_non_exhaustive()
    }
}

impl ExportStream {
    fn new(reader: Pin<Box<dyn AsyncRead + Send>>, max_line: usize, log: StreamLog) -> Self {
        Self {
            lines: FramedRead::new(reader, LinesCodec::new_with_max_length(max_line)),
            log,
        }
    }

    /// The next document line, or `None` at the end. A silence of [`STREAM_STALL`] is
    /// [`Error::Timeout`].
    pub async fn next_line(&mut self) -> Result<Option<String>, Error> {
        let next = tokio::time::timeout(STREAM_STALL, self.lines.next()).await;
        let result = match next {
            Err(_) => Err(Error::Timeout),
            Ok(None) => Ok(None),
            Ok(Some(Ok(line))) => {
                self.log.bytes += line.len() as u64 + 1;
                Ok(Some(line))
            }
            Ok(Some(Err(e))) => Err(line_error(e, self.lines.decoder().max_length())),
        };
        match &result {
            Ok(None) => self.log.end(None),
            Err(e) => self.log.end(Some(e.to_string())),
            Ok(Some(_)) => {}
        }
        result
    }
}

fn line_error(e: LinesCodecError, limit: usize) -> Error {
    match e {
        LinesCodecError::MaxLineLengthExceeded => Error::LineTooLong { limit },
        LinesCodecError::Io(e) => Error::Stream(e.to_string()),
    }
}

impl Client {
    /// `GET /collections/{c}/documents/export`, streamed. Goes to the active node without
    /// failover and has no total deadline; see [`STREAM_STALL`]. The export ignores curation
    /// sets, so [`Self::count_documents`] agrees with its line count.
    pub async fn export_stream(
        &self,
        collection: &str,
        query: &ExportQuery,
    ) -> Result<ExportStream, Error> {
        let path = collection_path(collection, "/documents/export")?;
        let pairs: Vec<(&str, &str)> = [
            ("filter_by", &query.filter_by),
            ("include_fields", &query.include_fields),
            ("exclude_fields", &query.exclude_fields),
        ]
        .into_iter()
        .filter_map(|(key, value)| Some((key, value.as_deref()?)))
        .collect();
        let query = (!pairs.is_empty()).then(|| encode_query(&pairs));
        let (response, mut log) = self
            .send_stream(self.active_node(), &path, query.as_deref())
            .await?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let text = response.text().await.map_err(Error::from_reqwest);
            return match text {
                Ok(text) => {
                    log.bytes = text.len() as u64;
                    log.end(None);
                    Err(Error::http(status, text))
                }
                Err(e) => {
                    log.end(Some(e.to_string()));
                    Err(e)
                }
            };
        }
        // reqwest's error does not survive `io::Error::other` as a type, so it reaches
        // `next_line` as text.
        let bytes = response
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        Ok(ExportStream::new(
            Box::pin(StreamReader::new(bytes)),
            MAX_LINE,
            log,
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn stream(data: &'static [u8], max: usize) -> ExportStream {
        let event = RequestEvent {
            id: 0,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            node: "http://localhost:1".parse().unwrap(),
            method: crate::Method::GET,
            path: "/".into(),
            query: None,
            request_headers: vec![],
            key_prefix: String::new(),
            request_body: None,
            response_body: None,
            status: None,
            duration: Duration::ZERO,
            size: None,
            origin: crate::Origin::User,
            error: None,
        };
        ExportStream::new(
            Box::pin(std::io::Cursor::new(data)),
            max,
            StreamLog::new(None, event),
        )
    }

    #[tokio::test]
    async fn a_line_over_the_limit_is_an_error() {
        let mut s = stream(
            b"{\"id\":\"1\"}\n0123456789012345678901234567890123456789\n",
            20,
        );
        assert!(s.next_line().await.unwrap().is_some());
        let e = s.next_line().await.unwrap_err();
        assert!(matches!(e, Error::LineTooLong { limit: 20 }), "{e:?}");
        assert!(e.to_string().contains("exceeds 20 bytes"));
    }
}
