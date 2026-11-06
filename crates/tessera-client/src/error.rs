// SPDX-License-Identifier: GPL-3.0-or-later

use reqwest::Method;

/// Everything that can go wrong talking to a Typesense server.
///
/// No variant ever contains the API key.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server could not be reached (DNS, refused connection, reset).
    #[error("network error: {0}")]
    Network(#[source] reqwest::Error),
    /// The server did not answer within the client's timeout.
    #[error("request timed out")]
    Timeout,
    /// The server answered with a non-success status.
    #[error("HTTP {status}: {message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Typesense's `message` field, or the raw body if there is none. Never truncated.
        message: String,
        /// The raw response body.
        body: String,
    },
    /// The response body was not the JSON we expected.
    #[error("unexpected response: {0}")]
    Decode(#[source] serde_json::Error),
    /// The base URL is not an `http` or `https` URL, or there is no node.
    #[error("invalid server URL: {0}")]
    InvalidUrl(String),
    /// The API key holds a character that cannot be sent in an HTTP header, such as a line
    /// break or a non-ASCII character. Never retried.
    #[error("the API key contains a character that is not allowed in an HTTP header")]
    InvalidKey,
    /// The connection is read-only and the request would write.
    #[error("read-only connection: {method} is not allowed")]
    ReadOnly {
        /// The refused method.
        method: Method,
    },
    /// The id is `.` or `..`, which a URL cannot hold: it would address another resource.
    #[error("the id {0} cannot be part of a URL")]
    UnaddressableId(String),
    /// TLS failed: an untrusted or invalid certificate, a failed handshake, or an
    /// unusable CA file.
    #[error("TLS error: {0}")]
    Tls(String),
}

impl Error {
    /// For an [`Error::Http`] whose body carried Typesense's `{"message": …}`, that message.
    /// `None` when the body was something else, such as a proxy's HTML error page.
    pub fn typesense_message(&self) -> Option<&str> {
        match self {
            // `http()` falls back to the raw body when there is no `message` field.
            Self::Http { message, body, .. } if message != body => Some(message),
            _ => None,
        }
    }

    pub(crate) fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            Self::Timeout
        } else if let Some(tls) = rustls_error(&e) {
            Self::Tls(tls.to_string())
        } else {
            Self::Network(e)
        }
    }

    pub(crate) fn http(status: u16, body: String) -> Self {
        #[derive(serde::Deserialize)]
        struct Message {
            message: String,
        }
        let message = serde_json::from_str::<Message>(&body)
            .map(|m| m.message)
            .unwrap_or_else(|_| body.clone());
        Self::Http {
            status,
            message,
            body,
        }
    }
}

/// The `rustls::Error` somewhere in `e`'s source chain, if any.
fn rustls_error<'a>(e: &'a (dyn std::error::Error + 'static)) -> Option<&'a rustls::Error> {
    let mut next = Some(e);
    while let Some(err) = next {
        if let Some(tls) = err.downcast_ref::<rustls::Error>() {
            return Some(tls);
        }
        // hyper and tokio-rustls nest io::Errors around the rustls error, and
        // io::Error::source() skips the error it wraps, so step into it instead.
        next = match err.downcast_ref::<std::io::Error>() {
            Some(io) => io.get_ref().map(|inner| inner as _),
            None => err.source(),
        };
    }
    None
}
