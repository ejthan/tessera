// SPDX-License-Identifier: GPL-3.0-or-later

//! "Copy as curl" for a logged request. The API key is always the `$TYPESENSE_API_KEY`
//! placeholder; the event never carries the key, and its key prefix is not used here.

use tessera_client::Method;
use url::Url;

use crate::log::LogEntry;
use crate::redact;

/// A POSIX shell command that repeats the request, with secrets in the body masked.
pub fn command(entry: &LogEntry) -> String {
    let e = &entry.event;
    let body = e.request_body.as_ref();
    let mut out = request_with(
        &e.node,
        &e.method,
        &e.path,
        e.query.as_deref(),
        &e.request_headers,
        body.map(|b| &*b.text),
    );
    if body.is_some_and(|b| b.original_len > b.text.len()) {
        out.insert_str(0, ": 'body truncated at 1 MiB'\n");
    }
    out
}

/// The same command for a request that has no log entry, with only the API key header.
pub fn request(
    node: &Url,
    method: &Method,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> String {
    request_with(node, method, path, query, &[], body)
}

fn request_with(
    node: &Url,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
    body: Option<&str>,
) -> String {
    let mut url = node.clone();
    url.set_path(path);
    url.set_query(query.map(redact::query).as_deref());
    url.set_fragment(None);

    let mut lines = Vec::new();
    let mut first = String::from("curl");
    // curl waits for a body after `-X HEAD`; `-I` is the right flag.
    if *method == Method::HEAD {
        first.push_str(" -I");
    } else if *method != Method::GET {
        first.push_str(&format!(" -X {method}"));
    }
    first.push_str(&format!(" {}", quote(url.as_str())));
    lines.push(first);
    lines.push(r#"-H "X-TYPESENSE-API-KEY: $TYPESENSE_API_KEY""#.to_string());
    for (name, value) in headers {
        lines.push(format!("-H {}", quote(&format!("{name}: {value}"))));
    }

    let mut heredoc = None;
    if let Some(body) = body {
        // shortcut: bodies round-trip through serde_json::Value, so integers beyond i64/u64 and float
        // formatting can change; enable arbitrary_precision if replay fidelity matters.
        let text = redact::json(body);
        let mut tag = String::from("EOF");
        while text.lines().any(|l| l == tag) {
            tag.push('_');
        }
        lines.push(format!("--data-binary @- <<'{tag}'"));
        heredoc = Some((text, tag));
    }

    let mut out = lines.join(" \\\n  ");
    if let Some((text, tag)) = heredoc {
        out.push_str(&format!("\n{text}\n{tag}"));
    }
    out
}

/// Single-quotes `s` for a POSIX shell.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use tessera_client::{Body, Origin};

    use super::*;
    use crate::log::tests::entry;

    fn body(text: &str, original_len: usize) -> Body {
        Body {
            text: text.into(),
            original_len,
        }
    }

    #[test]
    fn curl_get_with_query() {
        let mut e = entry(1, Method::GET, Origin::User, Some(200));
        e.event.node = "https://ts.example.net:8443".parse().expect("url");
        e.event.path = "/collections/books/documents/search".into();
        e.event.query = Some("q=dune&query_by=title".into());
        insta::assert_snapshot!(command(&e));
    }

    #[test]
    fn curl_post_with_body_heredoc() {
        let mut e = entry(1, Method::POST, Origin::User, Some(201));
        e.event.path = "/collections".into();
        e.event.request_headers = vec![("Content-Type".into(), "application/json".into())];
        let json = r#"{"name":"books","fields":[{"name":"title","type":"string"}]}"#;
        e.event.request_body = Some(body(json, json.len()));
        insta::assert_snapshot!(command(&e));
    }

    #[test]
    fn curl_masks_embedding_credentials() {
        let mut e = entry(1, Method::POST, Origin::User, Some(201));
        e.event.path = "/collections".into();
        let json = r#"{"name":"c","fields":[{"name":"emb","type":"float[]","embed":{"from":["title"],"model_config":{"model_name":"openai/text-embedding-3-small","api_key":"sk-embed-1234","access_token":"at-5678","refresh_token":"rt-9012","client_secret":"cs-3456"}}}]}"#;
        e.event.request_body = Some(body(json, json.len()));
        let out = command(&e);
        for s in ["sk-embed-1234", "at-5678", "rt-9012", "cs-3456"] {
            assert!(!out.contains(s), "{out}");
        }
        insta::assert_snapshot!(out);
    }

    #[test]
    fn curl_escapes_single_quotes() {
        let mut e = entry(1, Method::GET, Origin::User, Some(200));
        e.event.query = Some("q=it's".into());
        e.event.request_headers = vec![("X-Note".into(), "it's".into())];
        insta::assert_snapshot!(command(&e));
    }

    #[test]
    fn curl_head_uses_dash_i() {
        let e = entry(1, Method::HEAD, Origin::User, Some(200));
        insta::assert_snapshot!(command(&e));
    }

    #[test]
    fn curl_escapes_quote_in_path() {
        let mut e = entry(1, Method::GET, Origin::User, Some(200));
        e.event.path = "/a'b".into();
        insta::assert_snapshot!(command(&e));
    }

    #[test]
    fn curl_marks_truncated_body() {
        let mut e = entry(1, Method::PUT, Origin::User, Some(200));
        e.event.request_body = Some(body(r#"{"a":"b","c":"#, 2_000_000));
        insta::assert_snapshot!(command(&e));
    }

    #[test]
    fn curl_masks_key_in_query() {
        let mut e = entry(1, Method::GET, Origin::User, Some(200));
        e.event.query = Some("q=a&X-Typesense-Api-Key=s3cr3t".into());
        let out = command(&e);
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(out.contains("q=a&X-Typesense-Api-Key="), "{out}");
    }

    #[test]
    fn request_without_an_entry_masks_the_body() {
        let node: Url = "https://ts.example.net:8443".parse().expect("url");
        let out = request(
            &node,
            &Method::GET,
            "/collections",
            Some("q=a"),
            Some(r#"{"api_key":"s3cr3t"}"#),
        );
        assert!(out.contains("$TYPESENSE_API_KEY"), "{out}");
        assert!(
            out.contains("'https://ts.example.net:8443/collections?q=a'"),
            "{out}"
        );
        assert!(!out.contains("s3cr3t"), "{out}");
    }

    #[test]
    fn curl_never_contains_key() {
        let mut e = entry(1, Method::POST, Origin::User, Some(201));
        e.event.key_prefix = "abcd".into();
        let json = r#"{"api_key":"s3cr3t-value","description":"x"}"#;
        e.event.request_body = Some(body(json, json.len()));
        let out = command(&e);
        assert!(out.contains("$TYPESENSE_API_KEY"));
        assert!(!out.contains("abcd") && !out.contains("s3cr3t"));
    }
}
