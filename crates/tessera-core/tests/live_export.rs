// SPDX-License-Identifier: GPL-3.0-or-later

//! An exported curl command against a real Typesense server: it must get the answer the client
//! gets.
//!
//! Start one with `docker compose -f dev/docker-compose.yml up -d`, then run
//! `cargo test -p tessera-core --test live_export -- --ignored`. `TESSERA_IT_URL` and
//! `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and `xyz`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{Client, ClientOptions, MultiSearch};
use tessera_core::search::export::{ExportLang, ExportTarget, export, route_of};

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    let pair = |(k, v): &(&str, &str)| ((*k).to_string(), (*v).to_string());
    items.iter().map(pair).collect()
}

/// Runs the exported curl command in a shell and parses what it prints.
fn run_curl(client: &Client, target: &ExportTarget, key: &str) -> Value {
    let route = route_of(target).unwrap();
    let code = export(ExportLang::Curl, client.base_url(), &route, target).unwrap();
    assert!(code.contains("$TYPESENSE_API_KEY"), "{code}");
    let out = std::process::Command::new("sh")
        .args(["-c", &code])
        .env("TYPESENSE_API_KEY", key)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Takes the one value that differs between two runs.
fn no_time(mut answer: Value) -> Value {
    answer.as_object_mut().unwrap().remove("search_time_ms");
    answer
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_curl_gets_what_the_client_gets() {
    let url = std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into());
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into());
    let client = Client::new(
        &[&url],
        SecretString::from(key.clone()),
        ClientOptions::default(),
    )
    .unwrap();
    let name = "tessera_it_export";
    let _ = client.delete_collection(name).await;
    client
        .create_collection(&json!({"name": name, "fields": [
            {"name": "title", "type": "string"},
            {"name": "genre", "type": "string", "facet": true},
            {"name": "year", "type": "int32", "facet": true, "sort": true},
        ]}))
        .await
        .unwrap();
    let docs = [
        ("1", r#"it's "x" one"#, "a b", 2),
        ("2", r#"it's "x" two"#, "a b", 3),
        ("3", r#"it's "x" three"#, "c`d", 4),
        ("4", "other", "a b", 0),
    ];
    for (id, title, genre, year) in docs {
        let doc = json!({"id": id, "title": title, "genre": genre, "year": year});
        client.create_document(name, &doc).await.unwrap();
    }

    let filter = "genre:=`a b` && year:>1";
    let params = pairs(&[
        ("q", r#"it's "x""#),
        ("query_by", "title"),
        ("filter_by", filter),
        ("sort_by", "year:desc"),
        ("facet_by", "genre,year"),
    ]);
    let target = ExportTarget::Single {
        collection: name,
        params: &params,
    };
    let answer: Value = serde_json::from_str(&client.search(name, &params).await.unwrap()).unwrap();
    assert_eq!(answer["found"], 2);
    assert_eq!(no_time(run_curl(&client, &target, &key)), no_time(answer));

    // Over the URL limit the search goes by POST, and the curl answer wraps it.
    let ids: Vec<String> = (0..1000).map(|i| format!("nope{i}")).collect();
    let long = format!("{filter} && id:!=[{}]", ids.join(","));
    assert!(long.len() > 5000);
    let params = pairs(&[
        ("q", r#"it's "x""#),
        ("query_by", "title"),
        ("filter_by", &long),
    ]);
    let target = ExportTarget::Single {
        collection: name,
        params: &params,
    };
    let answer: Value = serde_json::from_str(&client.search(name, &params).await.unwrap()).unwrap();
    assert_eq!(answer["found"], 2);
    let mut curl = run_curl(&client, &target, &key);
    assert_eq!(no_time(curl["results"][0].take()), no_time(answer));

    let search = |q: &str| {
        let search = json!({"collection": name, "q": q, "query_by": "title"});
        search.as_object().unwrap().clone()
    };
    let request = MultiSearch {
        union: false,
        searches: vec![search(r#"it's "x""#), search("other")],
        page: None,
        per_page: None,
    };
    let target = ExportTarget::Multi(&request);
    let mut answer: Value =
        serde_json::from_str(&client.multi_search(&request).await.unwrap()).unwrap();
    assert_eq!(answer["results"][1]["found"], 1);
    let mut curl = run_curl(&client, &target, &key);
    for ix in 0..2 {
        assert_eq!(
            no_time(curl["results"][ix].take()),
            no_time(answer["results"][ix].take())
        );
    }

    client.delete_collection(name).await.unwrap();
}
