// SPDX-License-Identifier: GPL-3.0-or-later

//! Conversation models and NL search models, which share their shape.
//!
//! Every model call runs on [`Client::without_bodies`]: a create carries the provider key, and
//! a read carries its first five characters.

use std::time::Duration;

use serde_json::{Map, Value};

use crate::search::{SearchRoute, search_route};
use crate::{Client, Error, Method, resource_path};

/// Allowed per AI call, because the server calls the provider before it answers.
pub const AI_TIMEOUT: Duration = Duration::from_secs(120);

/// Which family of models a call is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    /// RAG models for `conversation=true`.
    Conversation,
    /// Models for `nl_query=true`.
    NlSearch,
}

impl ModelKind {
    /// The API path of the family.
    pub fn path(self) -> &'static str {
        match self {
            Self::Conversation => "conversations/models",
            Self::NlSearch => "nl_search_models",
        }
    }
}

fn model(text: &str) -> Result<Map<String, Value>, Error> {
    serde_json::from_str(text).map_err(Error::Decode)
}

impl Client {
    /// `GET /{kind}`: a bare array, with each `api_key` masked by the server.
    pub async fn ai_models(&self, kind: ModelKind) -> Result<Vec<Map<String, Value>>, Error> {
        let body = self
            .without_bodies()
            .get_ok(None, kind.path(), None)
            .await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `GET /{kind}/{id}`.
    pub async fn ai_model(&self, kind: ModelKind, id: &str) -> Result<Map<String, Value>, Error> {
        let path = resource_path(kind.path(), id)?;
        let body = self.without_bodies().get_ok(None, &path, None).await?;
        model(&body)
    }

    /// `POST /{kind}`. The server calls the provider before it answers.
    pub async fn create_ai_model(
        &self,
        kind: ModelKind,
        body: &Value,
    ) -> Result<Map<String, Value>, Error> {
        let text = self
            .without_bodies()
            .write(
                None,
                Method::POST,
                kind.path(),
                None,
                Some(body),
                Some(AI_TIMEOUT),
            )
            .await?;
        model(&text)
    }

    /// `PUT /{kind}/{id}`. It merges, and it stores whatever `api_key` it gets, a mask included:
    /// never send a value that was read.
    pub async fn update_ai_model(
        &self,
        kind: ModelKind,
        id: &str,
        body: &Value,
    ) -> Result<Map<String, Value>, Error> {
        let path = resource_path(kind.path(), id)?;
        let text = self
            .without_bodies()
            .write(None, Method::PUT, &path, None, Some(body), Some(AI_TIMEOUT))
            .await?;
        model(&text)
    }

    /// `DELETE /{kind}/{id}`.
    pub async fn delete_ai_model(&self, kind: ModelKind, id: &str) -> Result<(), Error> {
        let path = resource_path(kind.path(), id)?;
        self.without_bodies()
            .write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }

    /// A search that reaches an LLM. Always a `GET`, because `POST /multi_search` loses
    /// `conversation` in [`Client::search`]. The timeout is explicit so that a timeout is not
    /// retried: a retry would call the LLM again and store the turn twice.
    pub async fn ai_search(
        &self,
        collection: &str,
        params: &[(String, String)],
    ) -> Result<String, Error> {
        let SearchRoute::Get { path, query } = search_route(collection, params)? else {
            return Err(Error::Decode(serde::de::Error::custom(
                "the query is too long",
            )));
        };
        match self
            .call(
                None,
                Method::GET,
                &path,
                Some(&query),
                None,
                Some(AI_TIMEOUT),
            )
            .await?
        {
            (200..300, body) => Ok(body),
            (status, body) => Err(Error::http(status, body)),
        }
    }
}
