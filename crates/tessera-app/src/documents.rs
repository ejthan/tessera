// SPDX-License-Identifier: GPL-3.0-or-later

//! Documents of a collection: the backend seam the documents view loads through.

use std::sync::Arc;

use gpui_kit::Global;
use tessera_client::{Client, DocumentQuery, Error};
use tessera_core::documents::page::DocumentPage;
use tokio::runtime::Handle;

use crate::collections::{BoxFuture, TokioBackend};

/// One answered page with the figures of its query.
#[derive(Debug, Clone, PartialEq)]
pub struct PageResult {
    /// Documents matching the query.
    pub found: u64,
    /// Documents in the collection.
    pub out_of: u64,
    /// Server-side search time.
    pub search_time_ms: u64,
    /// The page's rows.
    pub page: DocumentPage,
}

/// Everything the documents view asks of the server. Dropping a returned future cancels its
/// request.
pub trait DocumentsBackend: Send + Sync {
    /// One page of `collection`, with previews built off the UI thread.
    fn page(
        &self,
        client: Client,
        collection: String,
        query: DocumentQuery,
    ) -> BoxFuture<Result<PageResult, Error>>;
}

/// The backend the views use.
pub struct DocumentsBackendGlobal(pub Arc<dyn DocumentsBackend>);

impl Global for DocumentsBackendGlobal {}

impl DocumentsBackend for TokioBackend {
    fn page(
        &self,
        client: Client,
        collection: String,
        query: DocumentQuery,
    ) -> BoxFuture<Result<PageResult, Error>> {
        self.run(async move {
            let page = client
                .without_response_bodies()
                .search_documents(&collection, &query)
                .await?;
            Ok(PageResult {
                found: page.found,
                out_of: page.out_of,
                search_time_ms: page.search_time_ms,
                page: DocumentPage::from_documents(page.documents),
            })
        })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn DocumentsBackend> {
    Arc::new(TokioBackend(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn page_builds_rows_from_the_answer() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/collections/c/documents/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"found":7,"out_of":9,"page":1,"search_time_ms":2,
                    "hits":[{"document":{"id":"1","title":"a"}}]}"#,
            ))
            .mount(&server)
            .await;
        let client =
            Client::new(&[&server.uri()], "k".to_string().into(), Default::default()).unwrap();
        let query = DocumentQuery {
            page: 1,
            per_page: 250,
            filter_by: None,
            sort_by: None,
            exclude_fields: None,
        };
        let result = tokio_backend(Handle::current())
            .page(client, "c".into(), query)
            .await
            .unwrap();
        assert_eq!(
            (result.found, result.out_of, result.search_time_ms),
            (7, 9, 2)
        );
        assert_eq!(&*result.page.rows[0].cells["title"], "a");
    }
}
