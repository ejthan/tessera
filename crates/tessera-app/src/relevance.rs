// SPDX-License-Identifier: GPL-3.0-or-later

//! Aliases, stopwords and presets: the backend seam the resource views run through.

use std::sync::Arc;

use gpui_kit::Global;
use tessera_client::{Alias, Client, Error, StopwordSet};
use tokio::runtime::Handle;

use crate::collections::{BoxFuture, TokioBackend};

/// Everything the resource views ask of the server. Every write here is one the caller runs
/// detached, so closing a tab cannot abort it.
pub trait RelevanceBackend: Send + Sync {
    /// The stopword sets.
    fn stopwords(&self, client: Client) -> BoxFuture<Result<Vec<StopwordSet>, Error>>;

    /// Stores set `id`, then reads it back, because the answer to the `PUT` echoes the input.
    fn save_stopwords(
        &self,
        client: Client,
        id: String,
        words: Vec<String>,
        locale: Option<String>,
    ) -> BoxFuture<Result<StopwordSet, Error>>;

    /// Deletes stopword set `id`.
    fn delete_stopwords(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>>;

    /// Points alias `name` at `collection`, creating it if needed.
    fn save_alias(
        &self,
        client: Client,
        name: String,
        collection: String,
    ) -> BoxFuture<Result<Alias, Error>>;

    /// Alias `name`.
    fn alias(&self, client: Client, name: String) -> BoxFuture<Result<Alias, Error>>;

    /// Deletes alias `name`.
    fn delete_alias(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;

    /// Deletes preset `name`.
    fn delete_preset(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;
}

/// The backend the views use.
pub struct RelevanceBackendGlobal(pub Arc<dyn RelevanceBackend>);

impl Global for RelevanceBackendGlobal {}

impl RelevanceBackend for TokioBackend {
    fn stopwords(&self, client: Client) -> BoxFuture<Result<Vec<StopwordSet>, Error>> {
        self.run(async move { client.stopwords().await })
    }

    fn save_stopwords(
        &self,
        client: Client,
        id: String,
        words: Vec<String>,
        locale: Option<String>,
    ) -> BoxFuture<Result<StopwordSet, Error>> {
        self.run(async move {
            client
                .upsert_stopwords(&id, &words, locale.as_deref())
                .await?;
            client.stopword_set(&id).await
        })
    }

    fn delete_stopwords(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_stopwords(&id).await })
    }

    fn save_alias(
        &self,
        client: Client,
        name: String,
        collection: String,
    ) -> BoxFuture<Result<Alias, Error>> {
        self.run(async move { client.upsert_alias(&name, &collection).await })
    }

    fn alias(&self, client: Client, name: String) -> BoxFuture<Result<Alias, Error>> {
        self.run(async move { client.alias(&name).await })
    }

    fn delete_alias(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_alias(&name).await })
    }

    fn delete_preset(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_preset(&name).await })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn RelevanceBackend> {
    Arc::new(TokioBackend(handle))
}
