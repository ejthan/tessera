// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask seed`: fills a Typesense server with the five dev sets.
//!
//! There is no default URL on purpose: a seed must never write to a server
//! nobody named.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Seed collections in drop order (referencing ones first).
const COLLECTIONS: [&str; 5] = ["books_ref", "books", "authors", "places", "articles_vec"];
const ALIAS: &str = "books_current";
const SYNONYMS: &str = "books_synonyms";
const AUTHORS: usize = 500;

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

    async fn import(&self, collection: &str, docs: Vec<Value>) -> Result<()> {
        for batch in docs.chunks(1000) {
            let body = batch
                .iter()
                .map(Value::to_string)
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

    let mut sets: Vec<(&str, Vec<Value>)> = vec![
        ("authors", authors(AUTHORS).collect()),
        ("books", books(10_000).collect()),
        ("books_ref", books_ref(2_000, AUTHORS).collect()),
        ("places", places(1_000).collect()),
        ("articles_vec", articles_vec(1_000).collect()),
    ];
    for schema in schemas() {
        s.ok(s
            .http
            .post(format!("{}/collections", opts.url))
            .json(&schema))
            .await?;
    }
    for (name, docs) in sets.drain(..) {
        let expected = docs.len();
        s.import(name, docs).await?;
        let info: Value = serde_json::from_str(
            &s.ok(s.http.get(format!("{}/collections/{name}", opts.url)))
                .await?,
        )?;
        let got = info["num_documents"].as_u64().unwrap_or(0);
        if got != expected as u64 {
            bail!("{name}: expected {expected} documents, found {got}");
        }
        println!("{name}: {got} documents");
    }
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
