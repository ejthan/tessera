// SPDX-License-Identifier: GPL-3.0-or-later

//! Synonym sets and curation sets, which share their endpoints.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Client, Error, Method, collections, documents, resource_path};

/// Which kind of set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetKind {
    /// `synonym_sets`.
    Synonym,
    /// `curation_sets`.
    Curation,
}

impl SetKind {
    /// The path root, which is also the collection attribute that attaches the kind.
    pub fn path(self) -> &'static str {
        match self {
            Self::Synonym => "synonym_sets",
            Self::Curation => "curation_sets",
        }
    }
}

/// A stored set. Items stay JSON: a rule has fields this client does not know.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ItemSet {
    /// Set name.
    pub name: String,
    /// The items as the server stores them.
    #[serde(default)]
    pub items: Vec<Value>,
}

fn item_path(kind: SetKind, name: &str, id: &str) -> Result<String, Error> {
    let set = resource_path(kind.path(), name)?;
    collections::addressable(id)?;
    Ok(format!("{set}/items/{}", documents::segment(id)))
}

impl Client {
    /// `GET /{kind}`. The answer is a bare array.
    pub async fn sets(&self, kind: SetKind) -> Result<Vec<ItemSet>, Error> {
        let body = self.get_ok(None, kind.path(), None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `GET /{kind}/{name}`.
    pub async fn set(&self, kind: SetKind, name: &str) -> Result<ItemSet, Error> {
        let body = self
            .get_ok(None, &resource_path(kind.path(), name)?, None)
            .await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `PUT /{kind}/{name}` with `{"items":…}`. Replaces the set, or creates it without any
    /// difference in the answer, so a caller that means "new" lists first.
    pub async fn upsert_set(
        &self,
        kind: SetKind,
        name: &str,
        items: &[Value],
    ) -> Result<ItemSet, Error> {
        let path = resource_path(kind.path(), name)?;
        let body = json!({ "items": items });
        let text = self
            .write(None, Method::PUT, &path, None, Some(&body), None)
            .await?;
        serde_json::from_str(&text).map_err(Error::Decode)
    }

    /// `DELETE /{kind}/{name}`.
    pub async fn delete_set(&self, kind: SetKind, name: &str) -> Result<(), Error> {
        let path = resource_path(kind.path(), name)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }

    /// `GET /{kind}/{name}/items/{id}`.
    pub async fn set_item(&self, kind: SetKind, name: &str, id: &str) -> Result<Value, Error> {
        let body = self.get_ok(None, &item_path(kind, name, id)?, None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `PUT /{kind}/{name}/items/{id}`. The id comes from the path, so `id` is removed from a
    /// copy of the body. Answers nothing: the answer echoes the input, not what was stored.
    pub async fn upsert_set_item(
        &self,
        kind: SetKind,
        name: &str,
        id: &str,
        item: &Value,
    ) -> Result<(), Error> {
        let path = item_path(kind, name, id)?;
        let mut body = item.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove("id");
        }
        self.write(None, Method::PUT, &path, None, Some(&body), None)
            .await
            .map(drop)
    }

    /// `DELETE /{kind}/{name}/items/{id}`.
    pub async fn delete_set_item(&self, kind: SetKind, name: &str, id: &str) -> Result<(), Error> {
        let path = item_path(kind, name, id)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }
}
