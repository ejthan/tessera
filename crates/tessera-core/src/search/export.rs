// SPDX-License-Identifier: GPL-3.0-or-later

//! A search as code. The route is the one the client sends, so a curl command is the request
//! and not a re-creation of it. No export holds a key: the node's key is read from the
//! environment, and a parameter that would hold one is left out.

use serde_json::{Map, Value, json};
use tessera_client::{Error, Method, MultiSearch, SearchRoute, multi_route, search_route};
use url::Url;

use crate::{curl, redact};

/// What a search can be exported as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportLang {
    /// A POSIX shell command.
    Curl,
    /// The `typesense` npm package.
    JavaScript,
    /// The `typesense` Python package.
    Python,
    /// `reqwest`.
    Rust,
    /// The URL, for a route that fits in one.
    RawUrl,
}

/// What the client libraries are called with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportTarget<'a> {
    /// One search.
    Single {
        /// The collection.
        collection: &'a str,
        /// The parameters.
        params: &'a [(String, String)],
    },
    /// Several searches.
    Multi(&'a MultiSearch),
}

fn public_params(params: &[(String, String)]) -> Vec<(String, String)> {
    let public = params.iter().filter(|(k, _)| !redact::is_secret_param(k));
    public.cloned().collect()
}

fn public_request(request: &MultiSearch) -> MultiSearch {
    let mut request = request.clone();
    for search in &mut request.searches {
        search.retain(|key, _| !redact::is_secret_param(key));
    }
    request
}

/// The route of `target`, without the parameters that would hold a key.
pub fn route_of(target: &ExportTarget) -> Result<SearchRoute, Error> {
    match target {
        ExportTarget::Single { collection, params } => {
            search_route(collection, &public_params(params))
        }
        ExportTarget::Multi(request) => Ok(multi_route(&public_request(request))),
    }
}

/// `route` as code, or `None`: a `Post` route has no raw URL. `node` ends in `/`, as
/// `Client::base_url` does.
pub fn export(
    lang: ExportLang,
    node: &Url,
    route: &SearchRoute,
    target: &ExportTarget,
) -> Option<String> {
    let url = url_of(node, route)?;
    match lang {
        ExportLang::Curl => Some(curl_text(&url, route)),
        ExportLang::JavaScript => Some(javascript(node, call(route, target)?)),
        ExportLang::Python => Some(python(node, call(route, target)?)),
        ExportLang::Rust => Some(rust(&url, route)),
        ExportLang::RawUrl => matches!(route, SearchRoute::Get { .. }).then(|| url.to_string()),
    }
}

fn url_of(node: &Url, route: &SearchRoute) -> Option<Url> {
    let (path, query) = match route {
        SearchRoute::Get { path, query } => (path.as_str(), Some(query.as_str())),
        SearchRoute::Post { query, .. } => ("multi_search", query.as_deref()),
    };
    let mut url = node.join(path).ok()?;
    url.set_query(query);
    Some(url)
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn curl_text(url: &Url, route: &SearchRoute) -> String {
    let (method, body) = match route {
        SearchRoute::Get { .. } => (Method::GET, None),
        SearchRoute::Post { body, .. } => (Method::POST, Some(pretty(body))),
    };
    curl::request(url, &method, url.path(), url.query(), body.as_deref())
}

/// What a client library is asked to do.
enum Call<'a> {
    Search { collection: &'a str, params: Value },
    Multi { request: &'a Value, common: Value },
}

fn call<'a>(route: &'a SearchRoute, target: &ExportTarget<'a>) -> Option<Call<'a>> {
    match (route, target) {
        (SearchRoute::Post { body, .. }, _) => {
            let mut common = Map::new();
            if let ExportTarget::Multi(request) = target
                && request.union
            {
                common.insert("page".into(), request.page.unwrap_or(1).into());
                if let Some(per_page) = request.per_page {
                    common.insert("per_page".into(), per_page.into());
                }
            }
            Some(Call::Multi {
                request: body,
                common: common.into(),
            })
        }
        (SearchRoute::Get { .. }, ExportTarget::Single { collection, params }) => {
            let params = public_params(params).into_iter();
            Some(Call::Search {
                collection,
                params: params.map(|(k, v)| (k, Value::String(v))).collect(),
            })
        }
        (SearchRoute::Get { .. }, ExportTarget::Multi(_)) => None,
    }
}

/// The node as the client libraries take it; the port is the scheme's by default.
fn node_config(node: &Url) -> Value {
    let mut config = json!({
        "host": node.host_str(),
        "port": node.port_or_known_default(),
        "protocol": node.scheme(),
    });
    let prefix = node.path().trim_end_matches('/');
    if !prefix.is_empty() {
        config["path"] = prefix.into();
    }
    config
}

fn javascript(node: &Url, call: Call) -> String {
    let (parameters, perform) = match call {
        Call::Search { collection, params } => {
            let name = Value::from(collection);
            (
                format!("const searchParameters = {};", pretty(&params)),
                format!("client.collections({name}).documents().search(searchParameters)"),
            )
        }
        Call::Multi { request, common } => (
            format!(
                "const searchRequests = {};\nconst commonParams = {};",
                pretty(request),
                pretty(&common)
            ),
            "client.multiSearch.perform(searchRequests, commonParams)".to_string(),
        ),
    };
    format!(
        "import Typesense from \"typesense\";\n\n\
         const client = new Typesense.Client({{\n  nodes: [{}],\n  \
         apiKey: process.env.TYPESENSE_API_KEY,\n}});\n\n\
         {parameters}\n\n\
         const results = await {perform};\n\
         console.log(results);\n",
        node_config(node)
    )
}

/// `value` as a Python expression. A `serde_json` string is a valid Python string.
fn python_value(value: &Value, depth: usize) -> String {
    let (pad, end) = ("    ".repeat(depth + 1), "    ".repeat(depth));
    let block = |open: &str, close: &str, items: Vec<String>| {
        if items.is_empty() {
            return format!("{open}{close}");
        }
        let lines: Vec<String> = items.iter().map(|i| format!("{pad}{i},\n")).collect();
        format!("{open}\n{}{end}{close}", lines.concat())
    };
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(_) | Value::String(_) => value.to_string(),
        Value::Array(items) => {
            let items = items.iter().map(|v| python_value(v, depth + 1));
            block("[", "]", items.collect())
        }
        Value::Object(map) => {
            let items = map.iter().map(|(k, v)| {
                format!(
                    "{}: {}",
                    Value::from(k.as_str()),
                    python_value(v, depth + 1)
                )
            });
            block("{", "}", items.collect())
        }
    }
}

fn python(node: &Url, call: Call) -> String {
    let (parameters, perform) = match call {
        Call::Search { collection, params } => {
            let name = Value::from(collection);
            (
                format!("search_parameters = {}", python_value(&params, 0)),
                format!("client.collections[{name}].documents.search(search_parameters)"),
            )
        }
        Call::Multi { request, common } => (
            format!(
                "search_requests = {}\ncommon_params = {}",
                python_value(request, 0),
                python_value(&common, 0)
            ),
            "client.multi_search.perform(search_requests, common_params)".to_string(),
        ),
    };
    format!(
        "import os\n\nimport typesense\n\n\
         client = typesense.Client({{\n    \"nodes\": {},\n    \
         \"api_key\": os.environ[\"TYPESENSE_API_KEY\"],\n}})\n\n\
         {parameters}\n\n\
         results = {perform}\n\
         print(results)\n",
        python_value(&json!([node_config(node)]), 1)
    )
}

/// `text` as a raw string literal with enough `#` to hold it.
fn raw_string(text: &str) -> String {
    let mut hashes = "#".to_string();
    while text.contains(&format!("\"{hashes}")) {
        hashes.push('#');
    }
    format!("r{hashes}\"{text}\"{hashes}")
}

fn rust(url: &Url, route: &SearchRoute) -> String {
    let (method, body) = match route {
        SearchRoute::Get { .. } => ("get", String::new()),
        SearchRoute::Post { body, .. } => (
            "post",
            format!(
                "    .header(\"Content-Type\", \"application/json\")\n    .body({})\n",
                raw_string(&pretty(body))
            ),
        ),
    };
    let url = url.as_str();
    format!(
        "let response = reqwest::Client::new()\n    \
         .{method}({url:?})\n    \
         .header(\"X-TYPESENSE-API-KEY\", std::env::var(\"TYPESENSE_API_KEY\")?)\n\
         {body}    .send()\n    .await?;\n\
         println!(\"{{}}\", response.text().await?);\n"
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const LANGS: [ExportLang; 4] = [
        ExportLang::Curl,
        ExportLang::JavaScript,
        ExportLang::Python,
        ExportLang::Rust,
    ];

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        let pair = |(k, v): &(&str, &str)| ((*k).to_string(), (*v).to_string());
        items.iter().map(pair).collect()
    }

    fn node() -> Url {
        "https://ts.example.net:8443/".parse().unwrap()
    }

    fn text(lang: ExportLang, node: &Url, target: &ExportTarget) -> String {
        export(lang, node, &route_of(target).unwrap(), target).unwrap()
    }

    fn get() -> Vec<(String, String)> {
        pairs(&[
            ("q", "dune"),
            ("query_by", "title"),
            ("filter_by", "genre:=`a b` && year:>1"),
        ])
    }

    fn long() -> Vec<(String, String)> {
        pairs(&[("q", "*"), ("filter_by", &"a".repeat(4000))])
    }

    fn union() -> MultiSearch {
        let search = |c: &str| {
            let value = json!({"collection": c, "q": "*", "exhaustive": true, "n": null, "l": [1]});
            value.as_object().unwrap().clone()
        };
        MultiSearch {
            union: true,
            searches: vec![search("a"), search("b")],
            page: Some(2),
            per_page: Some(5),
        }
    }

    #[test]
    fn get_route_per_language() {
        let params = get();
        let target = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        for lang in [
            ExportLang::Curl,
            ExportLang::JavaScript,
            ExportLang::Python,
            ExportLang::Rust,
            ExportLang::RawUrl,
        ] {
            insta::assert_snapshot!(format!("get_{lang:?}"), text(lang, &node(), &target));
        }
    }

    #[test]
    fn long_post_route_per_language() {
        let params = long();
        let target = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        assert!(matches!(
            route_of(&target).unwrap(),
            SearchRoute::Post { .. }
        ));
        for lang in LANGS {
            let code = text(lang, &node(), &target).replace(&"a".repeat(4000), "aaa…");
            insta::assert_snapshot!(format!("post_{lang:?}"), code);
        }
    }

    #[test]
    fn union_per_language() {
        let request = union();
        let target = ExportTarget::Multi(&request);
        for lang in LANGS {
            insta::assert_snapshot!(format!("union_{lang:?}"), text(lang, &node(), &target));
        }
    }

    #[test]
    fn hostile_strings_stay_inside_their_literal() {
        let hostile = "a\"b\\c'd\ne";
        let escaped = r#""a\"b\\c'd\ne""#;
        let params = pairs(&[("q", hostile)]);
        let target = ExportTarget::Single {
            collection: "bo\"oks",
            params: &params,
        };
        for lang in [ExportLang::JavaScript, ExportLang::Python] {
            let code = text(lang, &node(), &target);
            assert!(code.contains(escaped), "{code}");
            assert!(code.contains(r#""bo\"oks""#), "{code}");
        }
        let url = "a%22b%5Cc%27d%0Ae";
        for lang in [ExportLang::Curl, ExportLang::Rust, ExportLang::RawUrl] {
            let code = text(lang, &node(), &target);
            assert!(code.contains(url), "{code}");
            assert!(!code.contains(hostile), "{code}");
        }

        // Past the limit the same text travels in the body.
        let padded = format!("{hostile}\"#{}", "x".repeat(5000));
        let params = pairs(&[("q", &padded)]);
        let target = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        let code = text(ExportLang::Rust, &node(), &target);
        assert!(code.contains("r##\""), "raw string must outlast `\"#`");
        assert!(code.contains(r#"a\"b\\c'd\ne"#));
        let code = text(ExportLang::Curl, &node(), &target);
        assert!(code.contains("<<'EOF'"));
        assert!(code.contains(r#"a\"b\\c'd\ne"#));
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_hands_curl_the_whole_url_and_body() {
        let hostile = "it's \"x\" `id` $(id) \\ \n#";
        let params = pairs(&[("q", hostile)]);
        let target = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        let code = text(ExportLang::Curl, &node(), &target);
        let script = format!("curl() {{ printf '%s\\n' \"$1\"; }}\n{code}");
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("TYPESENSE_API_KEY", "unused")
            .output()
            .unwrap();
        let printed = String::from_utf8(out.stdout).unwrap();
        let url: Url = printed.trim().parse().unwrap();
        let q = url.query_pairs().find(|(k, _)| k == "q").unwrap().1;
        assert_eq!(q, hostile);
    }

    #[test]
    fn no_export_contains_the_key() {
        let params = pairs(&[
            ("q", "*"),
            ("api_key", "topsecret"),
            ("x-typesense-api-key", "topsecret"),
            ("scoped_key", "topsecret"),
        ]);
        let single = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        let mut request = union();
        request.searches[0].insert("x-typesense-api-key".into(), "topsecret".into());
        request.searches[1].insert("api_key".into(), "topsecret".into());
        let multi = ExportTarget::Multi(&request);
        for target in [single, multi] {
            for lang in LANGS {
                let code = text(lang, &node(), &target);
                assert!(!code.contains("topsecret"), "{code}");
                assert!(code.contains("TYPESENSE_API_KEY"), "{code}");
            }
        }
        let code = text(ExportLang::RawUrl, &node(), &single);
        assert!(!code.contains("topsecret"), "{code}");
    }

    #[test]
    fn raw_url_of_post_is_none() {
        let params = long();
        let target = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        let route = route_of(&target).unwrap();
        assert_eq!(export(ExportLang::RawUrl, &node(), &route, &target), None);
        let request = union();
        let target = ExportTarget::Multi(&request);
        let route = route_of(&target).unwrap();
        assert_eq!(export(ExportLang::RawUrl, &node(), &route, &target), None);
    }

    #[test]
    fn prefix_goes_to_the_node_config() {
        let node: Url = "http://ts.example.net/search/".parse().unwrap();
        let params = get();
        let target = ExportTarget::Single {
            collection: "books",
            params: &params,
        };
        let js = text(ExportLang::JavaScript, &node, &target);
        assert!(js.contains(r#""path":"/search""#), "{js}");
        assert!(js.contains(r#""port":80"#), "{js}");
        let py = text(ExportLang::Python, &node, &target);
        assert!(py.contains(r#""path": "/search""#), "{py}");
        for lang in [ExportLang::Curl, ExportLang::Rust, ExportLang::RawUrl] {
            let code = text(lang, &node, &target);
            assert!(code.contains("net/search/collections/books"), "{code}");
        }
    }
}
