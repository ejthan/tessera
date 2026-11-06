// SPDX-License-Identifier: GPL-3.0-or-later

//! Analytics rules and events.

use serde_json::{Map, Value};

use crate::{Client, Error, Method, resource_path};

fn object(body: &str) -> Result<Map<String, Value>, Error> {
    serde_json::from_str(body).map_err(Error::Decode)
}

impl Client {
    /// `GET /analytics/rules`: a bare array.
    pub async fn analytics_rules(&self) -> Result<Vec<Map<String, Value>>, Error> {
        let body = self.get_ok(None, "analytics/rules", None).await?;
        serde_json::from_str(&body).map_err(Error::Decode)
    }

    /// `GET /analytics/rules/{name}`. An unknown name answers 400, not 404.
    pub async fn analytics_rule(&self, name: &str) -> Result<Map<String, Value>, Error> {
        let path = resource_path("analytics/rules", name)?;
        object(&self.get_ok(None, &path, None).await?)
    }

    /// `POST /analytics/rules`: one rule, because an array answers per item inside a 200.
    pub async fn create_analytics_rule(&self, rule: &Value) -> Result<Map<String, Value>, Error> {
        let body = self
            .write(
                None,
                Method::POST,
                "analytics/rules",
                None,
                Some(rule),
                None,
            )
            .await?;
        object(&body)
    }

    /// `PUT /analytics/rules/{name}`. This is an upsert: an unknown name creates the rule, so
    /// read the rule first. A partial body merges, also inside `params`.
    pub async fn update_analytics_rule(
        &self,
        name: &str,
        rule: &Value,
    ) -> Result<Map<String, Value>, Error> {
        let path = resource_path("analytics/rules", name)?;
        let body = self
            .write(None, Method::PUT, &path, None, Some(rule), None)
            .await?;
        object(&body)
    }

    /// `DELETE /analytics/rules/{name}`.
    pub async fn delete_analytics_rule(&self, name: &str) -> Result<(), Error> {
        let path = resource_path("analytics/rules", name)?;
        self.write(None, Method::DELETE, &path, None, None, None)
            .await
            .map(drop)
    }

    /// `POST /analytics/events`. The server refuses more than 5 a minute unless started with
    /// `--analytics-minute-rate-limit`, and counts the refused ones.
    pub async fn analytics_event(&self, event: &Value) -> Result<(), Error> {
        self.write(
            None,
            Method::POST,
            "analytics/events",
            None,
            Some(event),
            None,
        )
        .await
        .map(drop)
    }

    /// `POST /analytics/flush`: moves the buffered aggregates to their destinations.
    pub async fn flush_analytics(&self) -> Result<(), Error> {
        self.write(None, Method::POST, "analytics/flush", None, None, None)
            .await
            .map(drop)
    }
}
