// SPDX-License-Identifier: GPL-3.0-or-later

//! API keys, analytics, stemming and AI models: the backend seam the admin views run through.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::Global;
use secrecy::SecretString;
use serde_json::{Map, Value};
use tessera_client::{
    ApiKey, Client, CreatedKey, Error, ModelKind, NewKey, StemmingDictionary, StemmingWord,
};
use tessera_core::stemming::{
    BadLine, ImportCheck, STEMMING_FILE_LIMIT, check_import, parse_dictionary, to_jsonl,
};
use tokio::runtime::Handle;

use crate::collections::{BoxFuture, TokioBackend};

/// Why a dictionary file cannot be imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictionaryFileError {
    /// It holds this many bytes, above the limit.
    TooLarge(u64),
    /// It could not be read.
    Read(String),
    /// These lines are bad.
    Lines(Vec<BadLine>),
}

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

    /// The ids of the stemming dictionaries.
    fn dictionaries(&self, client: Client) -> BoxFuture<Result<Vec<String>, Error>>;

    /// Dictionary `id` with its words.
    fn dictionary(
        &self,
        client: Client,
        id: String,
    ) -> BoxFuture<Result<StemmingDictionary, Error>>;

    /// Reads and parses a dictionary file, whose size is checked before reading.
    fn read_dictionary_file(
        &self,
        path: PathBuf,
    ) -> BoxFuture<Result<Vec<StemmingWord>, DictionaryFileError>>;

    /// Reads the dictionary (a 404 means new), imports, reads again and compares.
    fn import_dictionary(
        &self,
        client: Client,
        id: String,
        words: Vec<StemmingWord>,
    ) -> BoxFuture<Result<ImportCheck, Error>>;

    /// Deletes dictionary `id`.
    fn delete_dictionary(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>>;

    /// The models of `kind`.
    fn ai_models(
        &self,
        client: Client,
        kind: ModelKind,
    ) -> BoxFuture<Result<Vec<Map<String, Value>>, Error>>;

    /// Creates a model. The server calls the provider first.
    fn create_ai_model(
        &self,
        client: Client,
        kind: ModelKind,
        body: Value,
    ) -> BoxFuture<Result<Map<String, Value>, Error>>;

    /// Changes model `id`. `body` holds no value that was read: `PUT` stores a mask as the key.
    fn update_ai_model(
        &self,
        client: Client,
        kind: ModelKind,
        id: String,
        body: Value,
    ) -> BoxFuture<Result<Map<String, Value>, Error>>;

    /// Deletes model `id`.
    fn delete_ai_model(
        &self,
        client: Client,
        kind: ModelKind,
        id: String,
    ) -> BoxFuture<Result<(), Error>>;

    /// A search that reaches an LLM, and answers the JSON text.
    fn ai_search(
        &self,
        client: Client,
        collection: String,
        params: Vec<(String, String)>,
    ) -> BoxFuture<Result<String, Error>>;
}

/// The backend the views use.
pub struct AdminBackendGlobal(pub Arc<dyn AdminBackend>);

impl Global for AdminBackendGlobal {}

impl AdminBackend for TokioBackend {
    fn ai_models(
        &self,
        client: Client,
        kind: ModelKind,
    ) -> BoxFuture<Result<Vec<Map<String, Value>>, Error>> {
        self.run(async move { client.ai_models(kind).await })
    }

    fn create_ai_model(
        &self,
        client: Client,
        kind: ModelKind,
        body: Value,
    ) -> BoxFuture<Result<Map<String, Value>, Error>> {
        self.run(async move { client.create_ai_model(kind, &body).await })
    }

    fn update_ai_model(
        &self,
        client: Client,
        kind: ModelKind,
        id: String,
        body: Value,
    ) -> BoxFuture<Result<Map<String, Value>, Error>> {
        self.run(async move { client.update_ai_model(kind, &id, &body).await })
    }

    fn delete_ai_model(
        &self,
        client: Client,
        kind: ModelKind,
        id: String,
    ) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_ai_model(kind, &id).await })
    }

    fn ai_search(
        &self,
        client: Client,
        collection: String,
        params: Vec<(String, String)>,
    ) -> BoxFuture<Result<String, Error>> {
        self.run(async move { client.ai_search(&collection, &params).await })
    }

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

    fn dictionaries(&self, client: Client) -> BoxFuture<Result<Vec<String>, Error>> {
        self.run(async move { client.stemming_dictionaries().await })
    }

    fn dictionary(
        &self,
        client: Client,
        id: String,
    ) -> BoxFuture<Result<StemmingDictionary, Error>> {
        self.run(async move { client.stemming_dictionary(&id).await })
    }

    fn read_dictionary_file(
        &self,
        path: PathBuf,
    ) -> BoxFuture<Result<Vec<StemmingWord>, DictionaryFileError>> {
        self.run(async move {
            let read = |e: std::io::Error| DictionaryFileError::Read(e.to_string());
            let size = tokio::fs::metadata(&path).await.map_err(read)?.len();
            if size > STEMMING_FILE_LIMIT as u64 {
                return Err(DictionaryFileError::TooLarge(size));
            }
            let text = tokio::fs::read_to_string(&path).await.map_err(read)?;
            parse_dictionary(&text).map_err(DictionaryFileError::Lines)
        })
    }

    fn import_dictionary(
        &self,
        client: Client,
        id: String,
        words: Vec<StemmingWord>,
    ) -> BoxFuture<Result<ImportCheck, Error>> {
        self.run(async move {
            let before = match client.stemming_dictionary(&id).await {
                Ok(dictionary) => dictionary.words,
                Err(Error::Http { status: 404, .. }) => Vec::new(),
                Err(e) => return Err(e),
            };
            client.import_stemming(&id, &to_jsonl(&words)).await?;
            let stored = client.stemming_dictionary(&id).await?;
            Ok(check_import(&before, &words, &stored.words))
        })
    }

    fn delete_dictionary(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>> {
        self.run(async move { client.delete_stemming(&id).await })
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

    #[tokio::test]
    async fn an_import_of_a_new_id_compares_with_nothing() {
        let server = MockServer::start().await;
        let stored = json!({"id": "d", "words": [{"word": "a", "root": "x"}]});
        Mock::given(method("GET"))
            .and(path("/stemming/dictionaries/d"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"message":"Not found"}"#))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/stemming/dictionaries/d"))
            .respond_with(ResponseTemplate::new(200).set_body_json(stored))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/stemming/dictionaries/import"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;
        let word = StemmingWord {
            word: "a".into(),
            root: "x".into(),
        };
        let backend = tokio_backend(Handle::current());
        let check = backend
            .import_dictionary(client(&server), "d".into(), vec![word.clone(), word])
            .await
            .expect("import");
        assert_eq!((check.sent, check.kept_root), (2, 0));
        assert!(check.missing.is_empty());
    }

    #[tokio::test]
    async fn a_dictionary_file_is_checked_on_the_backend() {
        let dir = tempfile::tempdir().expect("dir");
        let backend = tokio_backend(Handle::current());
        let good = dir.path().join("good.jsonl");
        std::fs::write(&good, "{\"word\":\"a\",\"root\":\"b\"}\n").expect("write");
        assert_eq!(
            backend.read_dictionary_file(good).await.map(|w| w.len()),
            Ok(1)
        );
        let bad = dir.path().join("bad.jsonl");
        std::fs::write(&bad, "nope").expect("write");
        let read = backend.read_dictionary_file(bad).await;
        assert!(matches!(read, Err(DictionaryFileError::Lines(l)) if l.len() == 1));
        let big = dir.path().join("big.jsonl");
        std::fs::write(&big, vec![b' '; STEMMING_FILE_LIMIT + 1]).expect("write");
        let read = backend.read_dictionary_file(big).await;
        assert!(matches!(read, Err(DictionaryFileError::TooLarge(_))));
        let gone = backend.read_dictionary_file(dir.path().join("no")).await;
        assert!(matches!(gone, Err(DictionaryFileError::Read(_))));
    }
}
