// SPDX-License-Identifier: GPL-3.0-or-later

//! Stopword sets.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Client, Error, Method, resource_path};

/// A stored stopword set.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StopwordSet {
    /// Set id.
    pub id: String,
    /// The stored words, which are not the sent words: the server lowercases, splits at spaces
    /// and removes duplicates.
    pub stopwords: Vec<String>,
    /// Locale, unchecked by the server.
    #[serde(default)]
    pub locale: Option<String>,
}

impl Client {
    /// `GET /stopwords`.
    pub async fn stopwords(&self) -> Result<Vec<StopwordSet>, Error> {
        #[derive(Deserialize)]
        struct List {
            stopwords: Vec<StopwordSet>,
        }
        let body = self.get_ok(None, "stopwords", None).await?;
        let list: List = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(list.stopwords)
    }

    /// `GET /stopwords/{id}`, whose answer alone is wrapped in `{"stopwords":…}`.
    pub async fn stopword_set(&self, id: &str) -> Result<StopwordSet, Error> {
        #[derive(Deserialize)]
        struct Wrapped {
            stopwords: StopwordSet,
        }
        let body = self
            .get_ok(None, &resource_path("stopwords", id)?, None)
            .await?;
        let wrapped: Wrapped = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(wrapped.stopwords)
    }

    /// `PUT /stopwords/{id}`. Answers nothing, because the server's answer echoes the input:
    /// read the set back for what was stored.
    pub async fn upsert_stopwords(
        &self,
        id: &str,
        words: &[String],
        locale: Option<&str>,
    ) -> Result<(), Error> {
        let path = resource_path("stopwords", id)?;
        let mut body = json!({ "stopwords": words });
        if let Some(locale) = locale {
            body["locale"] = Value::from(locale);
        }
        self.write(None, Method::PUT, &path, None, Some(&body), None)
            .await
            .map(drop)
    }

    /// `DELETE /stopwords/{id}`.
    pub async fn delete_stopwords(&self, id: &str) -> Result<(), Error> {
        let path = resource_path("stopwords", id)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }
}
