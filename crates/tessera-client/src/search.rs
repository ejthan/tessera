// SPDX-License-Identifier: GPL-3.0-or-later

//! Search, multi-search and presets.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::collections::collection_path;
use crate::documents::encode_query;
use crate::{Client, Error, Method, resource_path};

/// An encoded query of this many bytes or more goes by `POST /multi_search`: the server
/// answers 400 to a `GET` at exactly 4000 bytes and accepts 3999.
pub const GET_QUERY_LIMIT: usize = 4000;

/// The server's default `limit_multi_searches`.
pub const MULTI_SEARCH_LIMIT: usize = 50;

/// The request a search is sent as.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchRoute {
    /// `GET {path}?{query}`.
    Get {
        /// Path below the node, for example `collections/c/documents/search`.
        path: String,
        /// The encoded query.
        query: String,
    },
    /// `POST /multi_search[?{query}]` with a JSON body.
    Post {
        /// The encoded query, if any.
        query: Option<String>,
        /// The JSON body.
        body: Value,
    },
}

/// Several searches in one request.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiSearch {
    /// One merged list of hits instead of one result per search.
    pub union: bool,
    /// The searches, each with its `collection`.
    pub searches: Vec<Map<String, Value>>,
    /// Union only: the page, sent in the URL because the server ignores it elsewhere.
    pub page: Option<u32>,
    /// Union only: hits per page, sent in the URL.
    pub per_page: Option<u32>,
}

/// A stored search preset.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// Preset name.
    pub name: String,
    /// Its parameters, or `{"searches": […]}`.
    pub value: Value,
}

/// How `collection` is searched with `params`: by `GET`, or by a one-search `POST` when the
/// encoded query exceeds [`GET_QUERY_LIMIT`]. The `POST` body holds the values as strings,
/// which the server accepts for every parameter.
pub fn search_route(collection: &str, params: &[(String, String)]) -> Result<SearchRoute, Error> {
    let path = collection_path(collection, "/documents/search")?;
    let pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (&**k, &**v)).collect();
    let query = encode_query(&pairs);
    if query.len() < GET_QUERY_LIMIT {
        return Ok(SearchRoute::Get { path, query });
    }
    let mut search = Map::new();
    search.insert("collection".into(), collection.into());
    for (k, v) in params {
        search.insert(k.clone(), v.clone().into());
    }
    Ok(SearchRoute::Post {
        query: None,
        body: json!({ "searches": [search] }),
    })
}

/// `POST /multi_search` for `request`. A union carries `page` and `per_page` in the URL,
/// `page` always, because the server otherwise answers `"page":0`.
pub fn multi_route(request: &MultiSearch) -> SearchRoute {
    let mut body = Map::new();
    let mut query = None;
    if request.union {
        body.insert("union".into(), true.into());
        let page = request.page.unwrap_or(1).to_string();
        let mut pairs = vec![("page", page.as_str())];
        let per_page = request.per_page.map(|n| n.to_string());
        if let Some(per_page) = &per_page {
            pairs.push(("per_page", per_page));
        }
        query = Some(encode_query(&pairs));
    }
    body.insert("searches".into(), request.searches.clone().into());
    SearchRoute::Post {
        query,
        body: Value::Object(body),
    }
}

/// `{"code":…,"error":…}` at the top of `value`, which the server sends with status 200.
fn embedded(value: &Value, body: &str) -> Option<Error> {
    let code = u16::try_from(value.get("code")?.as_u64()?).ok()?;
    let error = value.get("error")?.as_str()?;
    Some(Error::embedded(code, error.to_string(), body.to_string()))
}

fn preset_path(name: &str) -> Result<String, Error> {
    resource_path("presets", name)
}

impl Client {
    /// A clone that may send `POST` although the connection is read-only, and fails over with
    /// it. Only for the search endpoints; no public path reaches it.
    pub(crate) fn as_read_post(&self) -> Self {
        Self {
            read_post: true,
            ..self.clone()
        }
    }

    async fn post_search(&self, query: Option<&str>, body: &Value) -> Result<String, Error> {
        self.as_read_post()
            .write(None, Method::POST, "multi_search", query, Some(body), None)
            .await
    }

    /// Searches `collection` and answers the server's JSON text. A long query goes by
    /// `POST /multi_search`; the answer of that single search is unwrapped, so both routes
    /// give the same text. A failure the server reports with status 200 is [`Error::Http`].
    pub async fn search(
        &self,
        collection: &str,
        params: &[(String, String)],
    ) -> Result<String, Error> {
        match search_route(collection, params)? {
            SearchRoute::Get { path, query } => self.get_ok(None, &path, Some(&query)).await,
            SearchRoute::Post { query, body } => {
                let text = self.post_search(query.as_deref(), &body).await?;
                let answer: Value = serde_json::from_str(&text).map_err(Error::Decode)?;
                if let Some(error) = embedded(&answer, &text) {
                    return Err(error);
                }
                let first = answer["results"].get(0).ok_or_else(|| {
                    Error::Decode(serde::de::Error::custom("the answer has no results"))
                })?;
                match embedded(first, &text) {
                    Some(error) => Err(error),
                    None => Ok(first.to_string()),
                }
            }
        }
    }

    /// `POST /multi_search`. The text is returned as it came, because a federated search
    /// reports each failure inside its own result; only a top-level failure (a union's) is
    /// [`Error::Http`].
    pub async fn multi_search(&self, request: &MultiSearch) -> Result<String, Error> {
        let SearchRoute::Post { query, body } = multi_route(request) else {
            unreachable!("multi_route always posts");
        };
        let text = self.post_search(query.as_deref(), &body).await?;
        match serde_json::from_str::<Value>(&text) {
            Ok(answer) => embedded(&answer, &text).map_or(Ok(text), Err),
            Err(_) => Ok(text),
        }
    }

    /// `GET /presets`.
    pub async fn presets(&self) -> Result<Vec<Preset>, Error> {
        #[derive(Deserialize)]
        struct List {
            presets: Vec<Item>,
        }
        #[derive(Deserialize)]
        struct Item {
            name: String,
            value: Value,
        }
        let body = self.get_ok(None, "presets", None).await?;
        let list: List = serde_json::from_str(&body).map_err(Error::Decode)?;
        Ok(list
            .presets
            .into_iter()
            .map(|i| Preset {
                name: i.name,
                value: i.value,
            })
            .collect())
    }

    /// `GET /presets/{name}`: the whole answer, `{"name","value"}`.
    pub async fn preset(&self, name: &str) -> Result<Value, Error> {
        let body = self.get_ok(None, &preset_path(name)?, None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `PUT /presets/{name}` with `{"value": …}`. Overwrites silently.
    pub async fn upsert_preset(&self, name: &str, value: &Value) -> Result<(), Error> {
        let path = preset_path(name)?;
        let body = json!({ "value": value });
        self.write(None, Method::PUT, &path, None, Some(&body), None)
            .await
            .map(drop)
    }

    /// `DELETE /presets/{name}`.
    pub async fn delete_preset(&self, name: &str) -> Result<(), Error> {
        let path = preset_path(name)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }
}
