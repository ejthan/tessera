// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask seed`: fills a Typesense server with the six dev sets.
//!
//! There is no default URL on purpose: a seed must never write to a server
//! nobody named.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Seed collections in drop order (referencing ones first).
const COLLECTIONS: [&str; 6] = [
    "books_ref",
    "books",
    "authors",
    "places",
    "articles_vec",
    "products",
];
const ALIAS: &str = "books_current";
const SYNONYMS: &str = "books_synonyms";
const AUTHORS: usize = 500;
const PRODUCTS: usize = 200_000;

/// Parsed command line of `seed`.
pub struct Options {
    url: String,
    key: String,
    reset: bool,
}

impl Options {
    /// Parses the arguments after `seed`; `--url` is required.
    pub fn parse(args: &[String]) -> Result<Self> {
        let (mut url, mut key, mut reset) = (None, "xyz".to_string(), false);
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--url" => url = Some(it.next().context("--url needs a value")?.clone()),
                "--key" => key = it.next().context("--key needs a value")?.clone(),
                "--reset" => reset = true,
                other => bail!("unknown argument {other}"),
            }
        }
        let url = url.context("--url is required (there is no default)")?;
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            key,
            reset,
        })
    }
}

/// Small xorshift64 generator so every run gives the same documents.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const WORDS: [&str; 16] = [
    "silent", "garden", "winter", "river", "machine", "empire", "shadow", "ocean", "paper",
    "glass", "iron", "summer", "night", "forest", "signal", "harbor",
];
const GENRES: [&str; 6] = [
    "fantasy", "crime", "history", "science", "romance", "poetry",
];
const CITIES: [&str; 5] = ["Zurich", "Berlin", "Lisbon", "Oslo", "Prague"];

fn title(rng: &mut Rng) -> String {
    format!("The {} {}", rng.pick(&WORDS), rng.pick(&WORDS))
}

fn books(n: usize) -> impl Iterator<Item = Value> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    (0..n).map(move |i| {
        json!({
            "id": i.to_string(),
            "title": title(&mut rng),
            "genre": rng.pick(&GENRES),
            "year": 1900 + rng.below(125),
            "rating": rng.below(50) as f64 / 10.0,
            "pages": 80 + rng.below(900),
        })
    })
}

fn authors(n: usize) -> impl Iterator<Item = Value> {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    (0..n).map(move |i| {
        json!({
            "id": i.to_string(),
            "name": format!("{} {}", rng.pick(&WORDS), rng.pick(&WORDS)),
            "born": 1800 + rng.below(200),
        })
    })
}

/// Books that reference an author id below `author_count`.
fn books_ref(n: usize, author_count: usize) -> impl Iterator<Item = Value> {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    (0..n).map(move |i| {
        json!({
            "id": i.to_string(),
            "title": title(&mut rng),
            "author_id": rng.below(author_count as u64).to_string(),
        })
    })
}

fn places(n: usize) -> impl Iterator<Item = Value> {
    let mut rng = Rng(0xE703_7ED1_A0B4_28DB);
    (0..n).map(move |i| {
        json!({
            "id": i.to_string(),
            "name": format!("{} {}", rng.pick(&CITIES), rng.pick(&WORDS)),
            "location": [rng.range(-90.0, 90.0), rng.range(-180.0, 180.0)],
        })
    })
}

fn articles_vec(n: usize) -> impl Iterator<Item = Value> {
    let mut rng = Rng(0x8EBC_6AF0_9C88_C6E3);
    (0..n).map(move |i| {
        let embedding: Vec<f64> = (0..8).map(|_| rng.range(-1.0, 1.0)).collect();
        json!({ "id": i.to_string(), "title": title(&mut rng), "embedding": embedding })
    })
}

fn products(n: usize) -> impl Iterator<Item = Value> {
    let mut rng = Rng(0xC2B2_AE3D_27D4_EB4F);
    let words = move |rng: &mut Rng, k: usize| {
        (0..k)
            .map(|_| rng.pick(&WORDS))
            .collect::<Vec<_>>()
            .join(" ")
    };
    (0..n).map(move |i| {
        json!({
            "id": i.to_string(),
            "title": words(&mut rng, 3),
            "description": words(&mut rng, 25),
            "brand": format!("brand{:03}", rng.below(200)),
            "category": rng.pick(&WORDS),
            "tags": [rng.pick(&WORDS), rng.pick(&WORDS), rng.pick(&WORDS)],
            "price": rng.below(50_000) as f64 / 100.0,
            "stock": rng.below(1000),
            "rating": rng.below(50) as f64 / 10.0,
            "active": rng.below(5) != 0,
            "created_at": 1_700_000_000u64 + i as u64,
            "dimensions": {"w": rng.below(100), "h": rng.below(100)},
        })
    })
}

fn schemas() -> Vec<Value> {
    vec![
        json!({
            "name": "authors",
            "fields": [{"name": "name", "type": "string"}, {"name": "born", "type": "int32", "facet": true}],
        }),
        json!({
            "name": "books",
            "fields": [
                {"name": "title", "type": "string"},
                {"name": "genre", "type": "string", "facet": true},
                {"name": "year", "type": "int32", "facet": true},
                {"name": "rating", "type": "float", "sort": true},
                {"name": "pages", "type": "int32", "sort": true},
            ],
            "default_sorting_field": "pages",
            "synonym_sets": [SYNONYMS],
        }),
        json!({
            "name": "books_ref",
            "fields": [
                {"name": "title", "type": "string"},
                {"name": "author_id", "type": "string", "reference": "authors.id"},
            ],
        }),
        json!({
            "name": "places",
            "fields": [{"name": "name", "type": "string"}, {"name": "location", "type": "geopoint"}],
        }),
        json!({
            "name": "articles_vec",
            "fields": [
                {"name": "title", "type": "string"},
                {"name": "embedding", "type": "float[]", "num_dim": 8},
            ],
        }),
        json!({
            "name": "products",
            "enable_nested_fields": true,
            "default_sorting_field": "created_at",
            "fields": [
                {"name": "title", "type": "string"},
                {"name": "description", "type": "string"},
                {"name": "brand", "type": "string", "facet": true},
                {"name": "category", "type": "string", "facet": true},
                {"name": "tags", "type": "string[]", "facet": true},
                {"name": "price", "type": "float", "sort": true},
                {"name": "stock", "type": "int32"},
                {"name": "rating", "type": "float", "sort": true},
                {"name": "active", "type": "bool", "facet": true},
                {"name": "created_at", "type": "int64"},
                {"name": "dimensions", "type": "object"},
            ],
        }),
    ]
}

struct Server {
    http: reqwest::Client,
    url: String,
    key: String,
}

impl Server {
    async fn send(&self, req: reqwest::RequestBuilder) -> Result<(reqwest::StatusCode, String)> {
        let res = req.header("x-typesense-api-key", &self.key).send().await?;
        Ok((res.status(), res.text().await?))
    }

    /// Sends a request and fails with the server's message unless it succeeds.
    async fn ok(&self, req: reqwest::RequestBuilder) -> Result<String> {
        let (status, body) = self.send(req).await?;
        if !status.is_success() {
            bail!("{status}: {body}");
        }
        Ok(body)
    }

    async fn exists(&self, collection: &str) -> Result<bool> {
        let (status, body) = self
            .send(
                self.http
                    .get(format!("{}/collections/{collection}", self.url)),
            )
            .await?;
        match status.as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            _ => bail!("checking {collection}: {status}: {body}"),
        }
    }

    /// DELETE that treats 404 as already gone.
    async fn drop(&self, path: &str) -> Result<()> {
        let (status, body) = self
            .send(self.http.delete(format!("{}{path}", self.url)))
            .await?;
        if !status.is_success() && status.as_u16() != 404 {
            bail!("DELETE {path}: {status}: {body}");
        }
        Ok(())
    }

    /// Imports in batches of 1,000 without collecting the iterator.
    async fn import(&self, collection: &str, docs: impl Iterator<Item = Value>) -> Result<()> {
        let mut docs = docs.peekable();
        while docs.peek().is_some() {
            let body = docs
                .by_ref()
                .take(1000)
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let url = format!(
                "{}/collections/{collection}/documents/import?action=create",
                self.url
            );
            let res = self.ok(self.http.post(url).body(body)).await?;
            // Typesense answers 200 with one JSON result per line.
            if let Some(bad) = res.lines().find(|l| !l.contains("\"success\":true")) {
                bail!("import into {collection} failed: {bad}");
            }
        }
        Ok(())
    }
}

impl Server {
    /// Imports `docs` and checks that the collection holds `expected` documents.
    async fn load(
        &self,
        name: &str,
        expected: usize,
        docs: impl Iterator<Item = Value>,
    ) -> Result<()> {
        self.import(name, docs).await?;
        let info: Value = serde_json::from_str(
            &self
                .ok(self.http.get(format!("{}/collections/{name}", self.url)))
                .await?,
        )?;
        let got = info["num_documents"].as_u64().unwrap_or(0);
        if got != expected as u64 {
            bail!("{name}: expected {expected} documents, found {got}");
        }
        println!("{name}: {got} documents");
        Ok(())
    }
}

/// Seeds the server described by `opts`.
pub async fn run(opts: &Options) -> Result<()> {
    let s = Server {
        http: reqwest::Client::new(),
        url: opts.url.clone(),
        key: opts.key.clone(),
    };
    if opts.reset {
        s.drop(&format!("/aliases/{ALIAS}")).await?;
        for c in COLLECTIONS {
            s.drop(&format!("/collections/{c}")).await?;
        }
        s.drop(&format!("/synonym_sets/{SYNONYMS}")).await?;
    }
    for c in COLLECTIONS {
        if s.exists(c).await? {
            bail!("collection {c} already exists; use --reset to drop the seed collections");
        }
    }
    // The books schema names the set, so the set comes first.
    let synonyms = json!({"items": [
        {"id": "novel", "synonyms": ["novel", "book", "story"]},
        {"id": "sci", "synonyms": ["science", "sci"]},
    ]});
    s.ok(s
        .http
        .put(format!("{}/synonym_sets/{SYNONYMS}", opts.url))
        .json(&synonyms))
        .await?;

    for schema in schemas() {
        s.ok(s
            .http
            .post(format!("{}/collections", opts.url))
            .json(&schema))
            .await?;
    }
    s.load("authors", AUTHORS, authors(AUTHORS)).await?;
    s.load("books", 10_000, books(10_000)).await?;
    s.load("books_ref", 2_000, books_ref(2_000, AUTHORS))
        .await?;
    s.load("places", 1_000, places(1_000)).await?;
    s.load("articles_vec", 1_000, articles_vec(1_000)).await?;
    s.load("products", PRODUCTS, products(PRODUCTS)).await?;
    s.ok(s
        .http
        .put(format!("{}/aliases/{ALIAS}", opts.url))
        .json(&json!({"collection_name": "books"})))
        .await?;
    println!("alias {ALIAS} -> books, synonym set {SYNONYMS}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_documents() {
        assert!(books(50).eq(books(50)));
        assert!(places(50).eq(places(50)));
        assert!(articles_vec(50).eq(articles_vec(50)));
    }

    #[test]
    fn products_are_reproducible() {
        assert!(products(50).eq(products(50)));
    }

    #[test]
    fn product_ids_are_unique_and_prices_positive() {
        let mut ids = std::collections::HashSet::new();
        for p in products(2_000) {
            assert!(ids.insert(p["id"].as_str().unwrap_or_default().to_string()));
            assert!(p["price"].as_f64().unwrap_or(-1.0) > 0.0);
        }
    }

    #[test]
    fn schemas_has_products() {
        let all = schemas();
        let p = all.iter().find(|s| s["name"] == "products");
        let p = p.expect("products schema");
        assert_eq!(p["enable_nested_fields"], true);
        assert_eq!(p["default_sorting_field"], "created_at");
    }

    #[test]
    fn books_ref_points_at_existing_authors() {
        let ids: Vec<String> = authors(AUTHORS)
            .map(|a| a["id"].as_str().unwrap_or_default().to_string())
            .collect();
        for b in books_ref(2_000, AUTHORS) {
            let id = b["author_id"].as_str().unwrap_or_default();
            assert!(ids.iter().any(|a| a == id), "unknown author {id}");
        }
    }

    #[test]
    fn places_are_valid_geopoints() {
        for p in places(1_000) {
            let lat = p["location"][0].as_f64().unwrap_or(f64::NAN);
            let lng = p["location"][1].as_f64().unwrap_or(f64::NAN);
            assert!((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lng));
        }
    }

    #[test]
    fn args_without_url_are_an_error() {
        assert!(Options::parse(&["--reset".to_string()]).is_err());
        assert!(Options::parse(&[]).is_err());
    }
}
