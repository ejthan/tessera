// SPDX-License-Identifier: GPL-3.0-or-later

//! API keys.

use secrecy::SecretString;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Client, Error, Method};

/// A stored key. It holds no secret: `value` is taken out of the one answer that has it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ApiKey {
    /// Key id.
    pub id: u64,
    /// What the key is for.
    #[serde(default)]
    pub description: String,
    /// Allowed actions. The server takes any text here.
    #[serde(default)]
    pub actions: Vec<String>,
    /// Allowed collections, as names or regexes.
    #[serde(default)]
    pub collections: Vec<String>,
    /// Unix time it stops working. A key without expiry reports a date far away.
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// The first 4 characters of the value.
    #[serde(default)]
    pub value_prefix: Option<String>,
    /// Every other field.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What `POST /keys` is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct NewKey {
    /// What the key is for.
    pub description: String,
    /// Allowed actions.
    pub actions: Vec<String>,
    /// Allowed collections, as names or regexes.
    pub collections: Vec<String>,
    /// Unix time it stops working, or never.
    pub expires_at: Option<i64>,
}

/// A new key and its value, which the server shows only this once.
#[derive(Debug, Clone)]
pub struct CreatedKey {
    /// The stored key.
    pub key: ApiKey,
    /// The value.
    pub value: SecretString,
}

impl Client {
    /// `GET /keys`.
    pub async fn keys(&self) -> Result<Vec<ApiKey>, Error> {
        #[derive(Deserialize)]
        struct List {
            keys: Vec<ApiKey>,
        }
        let body = self.get_ok(None, "keys", None).await?;
        let list: List = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(list.keys)
    }

    /// `GET /keys/{id}`.
    pub async fn key(&self, id: u64) -> Result<ApiKey, Error> {
        let body = self.get_ok(None, &format!("keys/{id}"), None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `POST /keys`. The answer stays out of the request event, whoever listens, and an error
    /// never prints it.
    pub async fn create_key(&self, key: &NewKey) -> Result<CreatedKey, Error> {
        let mut body = json!({
            "description": key.description,
            "actions": key.actions,
            "collections": key.collections,
        });
        if let Some(expires_at) = key.expires_at {
            body["expires_at"] = Value::from(expires_at);
        }
        let answer = self
            .without_response_bodies()
            .write(None, Method::POST, "keys", None, Some(&body), None)
            .await?;
        let mut answer: Value = serde_json::from_str(&answer).map_err(Error::Decode)?;
        let value = answer.as_object_mut().and_then(|a| a.remove("value"));
        let Some(Value::String(value)) = value else {
            return Err(Error::Decode(serde_json::Error::custom(
                "the answer has no value",
            )));
        };
        Ok(CreatedKey {
            key: serde_json::from_value(answer).map_err(Error::Decode)?,
            value: value.into(),
        })
    }

    /// `DELETE /keys/{id}`.
    pub async fn delete_key(&self, id: u64) -> Result<(), Error> {
        let path = format!("keys/{id}");
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }

    /// A clone that sends another key.
    pub fn with_key(&self, key: SecretString) -> Self {
        Self {
            key,
            ..self.clone()
        }
    }
}
