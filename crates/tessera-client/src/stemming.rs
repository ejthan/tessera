// SPDX-License-Identifier: GPL-3.0-or-later

//! Stemming dictionaries.

use serde::{Deserialize, Serialize};

use crate::documents::encode_query;
use crate::{Client, Error, LONG_TIMEOUT, Method, resource_path};

/// One word and the root it stems to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StemmingWord {
    /// The word as written.
    pub word: String,
    /// What it stems to.
    pub root: String,
}

/// A stored dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StemmingDictionary {
    /// Dictionary id.
    pub id: String,
    /// Its words.
    #[serde(default)]
    pub words: Vec<StemmingWord>,
}

impl Client {
    /// `GET /stemming/dictionaries`: the ids only.
    pub async fn stemming_dictionaries(&self) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct List {
            dictionaries: Vec<String>,
        }
        let body = self.get_ok(None, "stemming/dictionaries", None).await?;
        let list: List = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(list.dictionaries)
    }

    /// `GET /stemming/dictionaries/{id}`. An unknown id answers 404.
    pub async fn stemming_dictionary(&self, id: &str) -> Result<StemmingDictionary, Error> {
        let path = resource_path("stemming/dictionaries", id)?;
        let body = self.get_ok(None, &path, None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `POST /stemming/dictionaries/import?id=`: one JSON object per line. The import merges
    /// into an existing dictionary, and a bad line stops it after the lines before it were
    /// stored, so validate the whole text first. The event keeps no bodies, a file can be big.
    pub async fn import_stemming(&self, id: &str, jsonl: &str) -> Result<(), Error> {
        crate::collections::addressable(id)?;
        let query = encode_query(&[("id", id)]);
        let (status, _, text) = self
            .without_bodies()
            .send_with(
                self.active_node(),
                Method::POST,
                "stemming/dictionaries/import",
                Some(&query),
                Some((jsonl.to_string(), "text/plain")),
                Some(LONG_TIMEOUT),
            )
            .await?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(Error::http(status, text))
        }
    }

    /// `DELETE /stemming/dictionaries/{id}`. The server answers 200 for an unknown id too.
    pub async fn delete_stemming(&self, id: &str) -> Result<(), Error> {
        let path = resource_path("stemming/dictionaries", id)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }
}
