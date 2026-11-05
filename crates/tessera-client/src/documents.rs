// SPDX-License-Identifier: GPL-3.0-or-later

//! Document endpoints.

use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::collections::{SEGMENT, collection_path};
use crate::{Client, Error, LONG_TIMEOUT};

/// Which page of a collection to fetch, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentQuery {
    /// 1-based page number.
    pub page: u32,
    /// Documents per page.
    pub per_page: u32,
    /// Typesense `filter_by` expression.
    pub filter_by: Option<String>,
    /// Typesense `sort_by` expression.
    pub sort_by: Option<String>,
    /// Comma-separated fields to leave out of the answer.
    pub exclude_fields: Option<String>,
}

/// One page of documents, without highlights or facets.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPage {
    /// Documents matching the filter.
    pub found: u64,
    /// Documents in the collection.
    pub out_of: u64,
    /// The page number the server answered.
    pub page: u32,
    /// Server-side search time.
    pub search_time_ms: u64,
    /// The documents of this page.
    pub documents: Vec<Map<String, Value>>,
}

#[derive(Deserialize)]
struct RawPage {
    #[serde(default)]
    found: u64,
    #[serde(default)]
    out_of: u64,
    #[serde(default)]
    page: u32,
    #[serde(default)]
    search_time_ms: u64,
    #[serde(default)]
    hits: Vec<RawHit>,
    #[serde(default)]
    facet_counts: Vec<RawFacet>,
}

#[derive(Deserialize)]
struct RawHit {
    document: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawFacet {
    #[serde(default)]
    counts: Vec<RawCount>,
}

#[derive(Deserialize)]
struct RawCount {
    value: String,
}

/// `k=v&k=v` with every value percent-encoded as one segment, so `&&` in a filter survives.
pub(crate) fn encode_query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", utf8_percent_encode(v, SEGMENT)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `s` encoded as one path segment.
pub(crate) fn segment(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}

impl Client {
    async fn search_raw(&self, collection: &str, pairs: &[(&str, &str)]) -> Result<RawPage, Error> {
        let path = collection_path(collection, "/documents/search");
        let body = self.get_ok(None, &path, Some(&encode_query(pairs))).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `GET /collections/{c}/documents/search?q=*`: one page of documents.
    pub async fn search_documents(
        &self,
        collection: &str,
        query: &DocumentQuery,
    ) -> Result<SearchPage, Error> {
        let (page, per_page) = (query.page.to_string(), query.per_page.to_string());
        let mut pairs = vec![("q", "*"), ("per_page", &per_page), ("page", &page)];
        for (key, value) in [
            ("filter_by", &query.filter_by),
            ("sort_by", &query.sort_by),
            ("exclude_fields", &query.exclude_fields),
        ] {
            if let Some(value) = value {
                pairs.push((key, value));
            }
        }
        let raw = self.search_raw(collection, &pairs).await?;
        Ok(SearchPage {
            found: raw.found,
            out_of: raw.out_of,
            page: raw.page,
            search_time_ms: raw.search_time_ms,
            documents: raw.hits.into_iter().map(|h| h.document).collect(),
        })
    }

    /// The number of documents matching `filter_by` (`per_page=0`).
    pub async fn count_documents(
        &self,
        collection: &str,
        filter_by: Option<&str>,
    ) -> Result<u64, Error> {
        let mut pairs = vec![("q", "*"), ("per_page", "0")];
        if let Some(filter) = filter_by {
            pairs.push(("filter_by", filter));
        }
        Ok(self.search_raw(collection, &pairs).await?.found)
    }

    /// Up to `limit` values of facet field `field`, narrowed to those starting with `prefix`.
    pub async fn facet_values(
        &self,
        collection: &str,
        field: &str,
        prefix: &str,
        limit: u32,
    ) -> Result<Vec<String>, Error> {
        let limit = limit.to_string();
        let facet_query = format!("{field}:{prefix}");
        let mut pairs = vec![
            ("q", "*"),
            ("per_page", "0"),
            ("facet_by", field),
            ("max_facet_values", &limit),
        ];
        if !prefix.is_empty() {
            pairs.push(("facet_query", &facet_query));
        }
        let raw = self.search_raw(collection, &pairs).await?;
        Ok(raw
            .facet_counts
            .into_iter()
            .next()
            .map(|f| f.counts.into_iter().map(|c| c.value).collect())
            .unwrap_or_default())
    }

    /// `GET /collections/{c}/documents/{id}`.
    pub async fn document(&self, collection: &str, id: &str) -> Result<Value, Error> {
        let path = collection_path(collection, &format!("/documents/{}", segment(id)));
        let body = self.get_ok(None, &path, None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    async fn write_document(
        &self,
        method: crate::Method,
        path: &str,
        query: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let body = self.write(None, method, path, query, body, None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `POST /collections/{c}/documents`: creates a document (409 when the id exists).
    pub async fn create_document(
        &self,
        collection: &str,
        document: &Value,
    ) -> Result<Value, Error> {
        let path = collection_path(collection, "/documents");
        self.write_document(crate::Method::POST, &path, None, Some(document))
            .await
    }

    /// `POST …/documents?action=upsert`: creates or replaces a document.
    pub async fn upsert_document(
        &self,
        collection: &str,
        document: &Value,
    ) -> Result<Value, Error> {
        let path = collection_path(collection, "/documents");
        let query = encode_query(&[("action", "upsert")]);
        self.write_document(crate::Method::POST, &path, Some(&query), Some(document))
            .await
    }

    /// `PATCH /collections/{c}/documents/{id}`: a deep merge, `null` removes a key.
    pub async fn patch_document(
        &self,
        collection: &str,
        id: &str,
        patch: &Value,
    ) -> Result<Value, Error> {
        let path = collection_path(collection, &format!("/documents/{}", segment(id)));
        self.write_document(crate::Method::PATCH, &path, None, Some(patch))
            .await
    }

    /// `DELETE /collections/{c}/documents/{id}`.
    pub async fn delete_document(&self, collection: &str, id: &str) -> Result<(), Error> {
        let path = collection_path(collection, &format!("/documents/{}", segment(id)));
        self.write(None, crate::Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }

    /// `GET /collections/{c}/documents/export?filter_by=`: the raw JSONL. Uses [`LONG_TIMEOUT`].
    pub async fn export_documents(
        &self,
        collection: &str,
        filter_by: &str,
    ) -> Result<String, Error> {
        let path = collection_path(collection, "/documents/export");
        let query = encode_query(&[("filter_by", filter_by)]);
        match self
            .call(
                None,
                crate::Method::GET,
                &path,
                Some(&query),
                None,
                Some(LONG_TIMEOUT),
            )
            .await?
        {
            (200..300, body) => Ok(body),
            (status, body) => Err(Error::http(status, body)),
        }
    }

    /// A clone whose request events keep their `size` but no response body, for big pages.
    pub fn without_response_bodies(&self) -> Self {
        Self {
            keep_response_body: false,
            ..self.clone()
        }
    }
}
