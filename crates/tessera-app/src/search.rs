// SPDX-License-Identifier: GPL-3.0-or-later

//! Searches of the workbench: the backend seam the search views run through.

use std::sync::Arc;

use gpui_kit::Global;
use tessera_client::{Client, Error, MultiSearch};
use tessera_core::search::results::{MultiResult, SearchResult, parse_multi, parse_result};
use tokio::runtime::Handle;

use crate::collections::{BoxFuture, TokioBackend};

/// One answered search: the server's text, for the Raw tab, and what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchAnswer {
    /// The response body as the server sent it.
    pub text: Arc<str>,
    /// The parsed result.
    pub result: SearchResult,
}

/// One answered multi-search.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiAnswer {
    /// The response body as the server sent it.
    pub text: Arc<str>,
    /// The parsed result.
    pub result: MultiResult,
}

/// Everything the workbench asks of the server. Dropping a returned future cancels its
/// request. Responses are logged with their bodies: the log entry must be the request.
pub trait SearchBackend: Send + Sync {
    /// Searches `collection` with `params`; `tags` are the highlight tags, parsed off the UI
    /// thread.
    fn search(
        &self,
        client: Client,
        collection: String,
        params: Vec<(String, String)>,
        tags: (String, String),
    ) -> BoxFuture<Result<SearchAnswer, Error>>;

    /// Runs `request`. `tags` hold one pair per search; the first applies to all, because the
    /// parser takes one.
    fn multi(
        &self,
        client: Client,
        request: MultiSearch,
        tags: Vec<(String, String)>,
    ) -> BoxFuture<Result<MultiAnswer, Error>>;
}

/// The backend the views use.
pub struct SearchBackendGlobal(pub Arc<dyn SearchBackend>);

impl Global for SearchBackendGlobal {}

impl SearchBackend for TokioBackend {
    fn search(
        &self,
        client: Client,
        collection: String,
        params: Vec<(String, String)>,
        tags: (String, String),
    ) -> BoxFuture<Result<SearchAnswer, Error>> {
        self.run(async move {
            let text = client.search(&collection, &params).await?;
            let result = parse_result(&text, (&tags.0, &tags.1)).map_err(Error::Decode)?;
            Ok(SearchAnswer {
                text: text.into(),
                result,
            })
        })
    }

    fn multi(
        &self,
        client: Client,
        request: MultiSearch,
        tags: Vec<(String, String)>,
    ) -> BoxFuture<Result<MultiAnswer, Error>> {
        self.run(async move {
            let text = client.multi_search(&request).await?;
            let default = ("<mark>".to_string(), "</mark>".to_string());
            let (start, end) = tags.first().unwrap_or(&default);
            let result = parse_multi(&text, request.union, (start, end)).map_err(Error::Decode)?;
            Ok(MultiAnswer {
                text: text.into(),
                result,
            })
        })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn SearchBackend> {
    Arc::new(TokioBackend(handle))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use wiremock::matchers::{path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn search_keeps_the_text_and_parses_with_the_tags() {
        let body = r#"{"found":7,"out_of":9,"page":1,"search_time_ms":2,"hits":[
            {"document":{"id":"1"},"highlight":{"title":{"snippet":"a [b]"}}}]}"#;
        let server = MockServer::start().await;
        Mock::given(path("/collections/c/documents/search"))
            .and(query_param("q", "b"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let client =
            Client::new(&[&server.uri()], "k".to_string().into(), Default::default()).unwrap();
        let answer = tokio_backend(Handle::current())
            .search(
                client,
                "c".into(),
                vec![("q".into(), "b".into())],
                ("[".into(), "]".into()),
            )
            .await
            .unwrap();
        assert_eq!(&*answer.text, body);
        assert_eq!(answer.result.found, 7);
        assert_eq!(answer.result.hits[0].lines[0].text, "a b");
        assert_eq!(answer.result.hits[0].lines[0].marks, vec![2..3]);
    }
}
