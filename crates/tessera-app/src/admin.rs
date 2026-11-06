// SPDX-License-Identifier: GPL-3.0-or-later

//! API keys and analytics: the backend seam the admin views run through.

use std::sync::Arc;

use gpui_kit::Global;
use secrecy::SecretString;
use serde_json::{Map, Value};
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

    /// The analytics rules of the server.
    fn rules(&self, client: Client) -> BoxFuture<Result<Vec<Map<String, Value>>, Error>>;

    /// `name` is the rule to edit, or `None` for a new one, which `POST`s. An edit reads the rule
    /// first, so one that is gone fails with its message instead of being recreated by the
    /// `PUT`.
    fn save_rule(
        &self,
        client: Client,
        name: Option<String>,
        body: Value,
    ) -> BoxFuture<Result<Map<String, Value>, Error>>;

    /// Deletes rule `name`.
    fn delete_rule(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;

    /// Sends an analytics event.
    fn send_event(&self, client: Client, event: Value) -> BoxFuture<Result<(), Error>>;

    /// Writes what the server holds in memory to the destinations.
    fn flush(&self, client: Client) -> BoxFuture<Result<(), Error>>;

    /// The first documents of `collection` by `sort_by`, without feeding analytics.
    fn top_documents(
        &self,
        client: Client,
        collection: String,
        sort_by: String,
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

    fn rules(&self, client: Client) -> BoxFuture<Result<Vec<Map<String, Value>>, Error>> {
        self.run(async move { client.analytics_rules().await })
    }

    fn save_rule(
        &self,
        client: Client,
        name: Option<String>,
        body: Value,
    ) -> BoxFuture<Result<Map<String, Value>, Error>> {
        self.run(async move {
            match name {
                None => client.create_analytics_rule(&body).await,
                Some(name) => {
                    client.analytics_rule(&name).await?;
                    client.update_analytics_rule(&name, &body).await
                }
            }
        })
    }

    fn delete_rule(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_analytics_rule(&name).await })
    }

    fn send_event(&self, client: Client, event: Value) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.analytics_event(&event).await })
    }

    fn flush(&self, client: Client) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.flush_analytics().await })
    }

    fn top_documents(
        &self,
        client: Client,
        collection: String,
        sort_by: String,
    ) -> BoxFuture<Result<String, Error>> {
        self.run(async move {
            let params = [
                ("q", "*"),
                ("sort_by", sort_by.as_str()),
                ("per_page", "100"),
                ("enable_analytics", "false"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()));
            client.search(&collection, &params).await
        })
    }
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn AdminBackend> {
    Arc::new(TokioBackend(handle))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tessera_client::ClientOptions;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn client(server: &MockServer) -> Client {
        Client::new(
            &[server.uri().as_str()],
            "k".to_string().into(),
            ClientOptions::default(),
        )
        .expect("client")
    }

    #[tokio::test]
    async fn an_edit_of_a_vanished_rule_never_puts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/analytics/rules/gone"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"message":"Not found"}"#))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(0)
            .mount(&server)
            .await;
        let backend = tokio_backend(Handle::current());
        let saved = backend
            .save_rule(client(&server), Some("gone".into()), json!({}))
            .await;
        assert!(matches!(saved, Err(Error::Http { status: 404, .. })));
    }
}
