// SPDX-License-Identifier: GPL-3.0-or-later

//! API keys: the backend seam the admin views run through.

use std::sync::Arc;

use gpui_kit::Global;
use secrecy::SecretString;
use tessera_client::{ApiKey, Client, CreatedKey, Error, NewKey};
use tokio::runtime::Handle;

use crate::collections::{BoxFuture, TokioBackend};

/// Everything the admin views ask of the server. Every write here is one the caller runs
/// detached, so closing a tab cannot abort it.
pub trait AdminBackend: Send + Sync {
    /// The keys of the server.
    fn keys(&self, client: Client) -> BoxFuture<Result<Vec<ApiKey>, Error>>;

    /// Creates a key; the only answer that holds its value.
    fn create_key(&self, client: Client, key: NewKey) -> BoxFuture<Result<CreatedKey, Error>>;

    /// Deletes key `id`.
    fn delete_key(&self, client: Client, id: u64) -> BoxFuture<Result<(), Error>>;

    /// Searches `collection` with `key` instead of the connection's, and answers the JSON text.
    fn scoped_search(
        &self,
        client: Client,
        key: SecretString,
        collection: String,
        params: Vec<(String, String)>,
    ) -> BoxFuture<Result<String, Error>>;
}

/// The backend the views use.
pub struct AdminBackendGlobal(pub Arc<dyn AdminBackend>);

impl Global for AdminBackendGlobal {}

impl AdminBackend for TokioBackend {
    fn keys(&self, client: Client) -> BoxFuture<Result<Vec<ApiKey>, Error>> {
        self.run(async move { client.keys().await })
    }

    fn create_key(&self, client: Client, key: NewKey) -> BoxFuture<Result<CreatedKey, Error>> {
        self.run(async move { client.create_key(&key).await })
    }

    fn delete_key(&self, client: Client, id: u64) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_key(id).await })
    }

    fn scoped_search(
        &self,
        client: Client,
        key: SecretString,
        collection: String,
        params: Vec<(String, String)>,
    ) -> BoxFuture<Result<String, Error>> {
        self.run(async move { client.with_key(key).search(&collection, &params).await })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn AdminBackend> {
    Arc::new(TokioBackend(handle))
}
