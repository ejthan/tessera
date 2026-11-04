// SPDX-License-Identifier: GPL-3.0-or-later

/// Everything that can go wrong talking to a Typesense server.
///
/// No variant ever contains the API key.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server could not be reached (DNS, refused connection, TLS, reset).
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
    /// The base URL is not an `http` or `https` URL.
    #[error("invalid server URL: {0}")]
    InvalidUrl(String),
}

impl Error {
    pub(crate) fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            Self::Timeout
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
