// SPDX-License-Identifier: GPL-3.0-or-later

//! The paths of the vendored OpenAPI file, for the console's path completion.

use std::sync::OnceLock;

use tessera_client::collection_path;

const SPEC: &str = include_str!("../../../spec/typesense-openapi.yml");
const PLACEHOLDER: &str = "{collectionName}";
const MAX_CANDIDATES: usize = 30;

/// One method on one path of the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// Upper case.
    pub method: &'static str,
    /// As in the file, for example `/collections/{collectionName}`.
    pub template: &'static str,
}

/// A line scanner, no YAML dependency: two-space `/…:` lines open a path, four-space method
/// lines add a route.
fn scan(spec: &'static str) -> Vec<Route> {
    let mut routes = Vec::new();
    let mut template = None;
    let paths = spec
        .lines()
        .skip_while(|line| *line != "paths:")
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with(' '));
    for line in paths {
        if line.starts_with("  /") {
            template = line.trim().strip_suffix(':');
            continue;
        }
        let method = match line {
            "    get:" => "GET",
            "    post:" => "POST",
            "    put:" => "PUT",
            "    patch:" => "PATCH",
            "    delete:" => "DELETE",
            _ => continue,
        };
        if let Some(template) = template {
            routes.push(Route { method, template });
        }
    }
    routes
}

/// Every route of the file, scanned once.
pub fn routes() -> &'static [Route] {
    static ROUTES: OnceLock<Vec<Route>> = OnceLock::new();
    ROUTES.get_or_init(|| scan(SPEC))
}

/// Path templates that start with `typed`, at most 30, without duplicates. Once `typed` has
/// reached `{collectionName}`, that template is listed per collection, name encoded.
pub fn path_candidates(typed: &str, collections: &[String]) -> Vec<String> {
    let typed = format!("/{}", typed.trim_start_matches('/'));
    let mut out: Vec<String> = Vec::new();
    let mut add = |candidate: String| {
        if candidate.starts_with(&typed) && !out.contains(&candidate) && out.len() < MAX_CANDIDATES
        {
            out.push(candidate);
        }
    };
    for route in routes() {
        match route.template.split_once(PLACEHOLDER) {
            Some((head, tail)) if typed.starts_with(head) => {
                for path in collections
                    .iter()
                    .filter_map(|c| collection_path(c, tail).ok())
                {
                    add(format!("/{path}"));
                }
            }
            _ => add(route.template.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_has_46_paths() {
        let mut paths: Vec<_> = routes().iter().map(|r| r.template).collect();
        paths.dedup();
        assert_eq!(paths.len(), 46);
    }

    #[test]
    fn routes_has_get_and_post_of_multi_search() {
        let methods: Vec<_> = routes()
            .iter()
            .filter(|r| r.template == "/multi_search")
            .map(|r| r.method)
            .collect();
        assert_eq!(methods, ["POST"]);
        assert!(routes().contains(&Route {
            method: "GET",
            template: "/collections/{collectionName}",
        }));
    }

    #[test]
    fn candidates_expand_collections() {
        let collections = ["products".to_string(), "a b".to_string()];
        let list = path_candidates("/collections/", &collections);
        for expected in [
            "/collections/products",
            "/collections/a%20b",
            "/collections/products/documents/search",
        ] {
            assert!(list.iter().any(|c| c == expected), "{expected}: {list:?}");
        }
        assert!(!list.iter().any(|c| c.contains(PLACEHOLDER)));
        let list = path_candidates("collections/pro", &collections);
        assert!(list.iter().all(|c| c.starts_with("/collections/products")));
        assert!(!list.is_empty());
    }

    #[test]
    fn candidates_keep_other_placeholders_before_the_point() {
        let list = path_candidates("/co", &["a".to_string()]);
        assert!(list.iter().any(|c| c == "/collections"));
        assert!(list.iter().any(|c| c == "/collections/{collectionName}"));
        let list = path_candidates("/keys", &[]);
        assert_eq!(list, ["/keys", "/keys/{keyId}"]);
    }

    #[test]
    fn candidates_are_unique_and_capped() {
        let collections: Vec<String> = (0..100).map(|i| format!("c{i}")).collect();
        let list = path_candidates("/collections/", &collections);
        assert_eq!(list.len(), MAX_CANDIDATES);
        let mut unique = list.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), list.len());
    }
}
