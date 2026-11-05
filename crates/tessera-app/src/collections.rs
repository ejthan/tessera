// SPDX-License-Identifier: GPL-3.0-or-later

//! Collections of a connection: the backend seam the views load through, and the cache.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{Global, SharedString};
use serde_json::Value;
use tessera_client::{Alias, Client, Collection, Error, SchemaChange};
use tokio::runtime::Handle;

use crate::runtime::AbortOnDrop;

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Collections with their schemas, and aliases.
pub type CollectionList = (Vec<Collection>, Vec<Alias>);

/// Everything the collection views ask of the server. The app installs [`tokio_backend`]; UI
/// tests install fakes, because GPUI's test executor does not wait for real network I/O.
/// Dropping a returned future cancels its requests.
pub trait CollectionsBackend: Send + Sync {
    /// All collections with their schemas, and all aliases.
    fn list(&self, client: Client) -> BoxFuture<Result<CollectionList, Error>>;
    /// One collection with its schema.
    fn get(&self, client: Client, name: String) -> BoxFuture<Result<Collection, Error>>;
    /// Creates a collection from `schema`, the request body as is.
    fn create(&self, client: Client, schema: Value) -> BoxFuture<Result<Collection, Error>>;
    /// Changes the schema of collection `name` with `patch`, the PATCH body as is.
    fn update(&self, client: Client, name: String, patch: Value) -> BoxFuture<Result<(), Error>>;
    /// Deletes all documents of collection `name`; the number deleted.
    fn truncate(&self, client: Client, name: String) -> BoxFuture<Result<u64, Error>>;
    /// Deletes collection `name` with its documents.
    fn delete(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;
    /// The schema changes in progress, asked of the client's active node: the node a PATCH
    /// from the same client went to.
    fn schema_changes(&self, client: Client) -> BoxFuture<Result<Vec<SchemaChange>, Error>>;
}

/// The backend the views use.
pub struct CollectionsBackendGlobal(pub Arc<dyn CollectionsBackend>);

impl Global for CollectionsBackendGlobal {}

struct TokioBackend(Handle);

impl TokioBackend {
    /// Runs `fut` on the runtime; the task is aborted when the returned future is dropped.
    fn run<T: Send + 'static>(
        &self,
        fut: impl Future<Output = T> + Send + 'static,
    ) -> BoxFuture<T> {
        let task = AbortOnDrop::from_join_handle(self.0.spawn(fut));
        Box::pin(async move {
            match task.await {
                Ok(value) => value,
                Err(e) => std::panic::resume_unwind(e.into_panic()),
            }
        })
    }
}

impl CollectionsBackend for TokioBackend {
    fn list(&self, client: Client) -> BoxFuture<Result<CollectionList, Error>> {
        self.run(async move {
            let (collections, aliases) = tokio::join!(client.collections(), client.aliases());
            Ok((collections?, aliases?))
        })
    }

    fn get(&self, client: Client, name: String) -> BoxFuture<Result<Collection, Error>> {
        self.run(async move { client.collection(&name).await })
    }

    fn create(&self, client: Client, schema: Value) -> BoxFuture<Result<Collection, Error>> {
        self.run(async move { client.create_collection(&schema).await })
    }

    fn update(&self, client: Client, name: String, patch: Value) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.update_collection(&name, &patch).await })
    }

    fn truncate(&self, client: Client, name: String) -> BoxFuture<Result<u64, Error>> {
        self.run(async move { client.truncate_collection(&name).await })
    }

    fn delete(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_collection(&name).await })
    }

    fn schema_changes(&self, client: Client) -> BoxFuture<Result<Vec<SchemaChange>, Error>> {
        self.run(async move { client.node(client.active_node()).schema_changes().await })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn CollectionsBackend> {
    Arc::new(TokioBackend(handle))
}

/// The collection list of a connection, shared by the views that show it.
#[derive(Debug, Default)]
pub struct CollectionsCache {
    /// `None` until the first load succeeds.
    pub collections: Option<Vec<Collection>>,
    /// All aliases, as of the last load.
    pub aliases: Vec<Alias>,
    /// When the last load succeeded.
    pub loaded_at: Option<Instant>,
    /// Full text of the last failed load.
    pub error: Option<SharedString>,
    /// A load is running.
    pub loading: bool,
}

/// The names of the aliases that point to `collection`.
pub fn aliases_of<'a>(aliases: &'a [Alias], collection: &str) -> impl Iterator<Item = &'a str> {
    let collection = collection.to_owned();
    aliases
        .iter()
        .filter(move |a| a.collection_name == collection)
        .map(|a| a.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_of_picks_the_targets_aliases() {
        let alias = |name: &str, to: &str| Alias {
            name: name.into(),
            collection_name: to.into(),
        };
        let all = [alias("a", "books"), alias("b", "cds"), alias("c", "books")];
        assert_eq!(aliases_of(&all, "books").collect::<Vec<_>>(), ["a", "c"]);
    }
}
