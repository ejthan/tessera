// SPDX-License-Identifier: GPL-3.0-or-later

//! The update check: the one request that does not go to a Typesense node.

use std::time::Duration;

use serde::Deserialize;

use crate::Error;

const TIMEOUT: Duration = Duration::from_secs(10);
/// The most of an answer that is read.
const BODY_LIMIT: usize = 1 << 20;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// The tag of the latest release at `api_url`. A plain request, on purpose: no key, no retry,
/// and no redirect, so it reaches only the host it names. A 404 means no release yet.
pub async fn latest_release(api_url: &str) -> Result<String, Error> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("tessera/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(Error::from_reqwest)?;
    let mut response = client
        .get(api_url)
        .send()
        .await
        .map_err(Error::from_reqwest)?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::from_reqwest)? {
        body.extend_from_slice(&chunk);
        if body.len() > BODY_LIMIT {
            return Err(Error::LineTooLong { limit: BODY_LIMIT });
        }
    }
    if !status.is_success() {
        let body = String::from_utf8_lossy(&body).into_owned();
        return Err(Error::Http {
            status: status.as_u16(),
            message: body.clone(),
            body,
        });
    }
    let release: Release = serde_json::from_slice(&body).map_err(Error::Decode)?;
    Ok(release.tag_name)
}
