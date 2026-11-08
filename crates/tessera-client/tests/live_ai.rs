// SPDX-License-Identifier: GPL-3.0-or-later

//! AI models against a real Typesense server and a local stub LLM. Nothing here reaches a paid
//! provider: every model names the stub, or an address that refuses connections.
//!
//! Run `cargo test -p tessera-client --test live_ai -- --ignored --skip live_builtin_model`.
//! `TESSERA_IT_URL` and `TESSERA_IT_KEY` override the defaults `http://localhost:18108` and
//! `xyz`. `TESSERA_IT_STUB_HOST` names this machine as the server sees it.

use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{Client, ClientOptions, Error, ModelKind};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const STUB_KEY: &str = "sk-stub-not-a-real-key";
const DEAD_URL: &str = "http://127.0.0.1:1";
const NL_CONTENT: &str = r#"{"q":"*","filter_by":"o:=1","sort_by":"o:desc"}"#;

fn client() -> Client {
    let url = std::env::var("TESSERA_IT_URL").unwrap_or_else(|_| "http://localhost:18108".into());
    let key = std::env::var("TESSERA_IT_KEY").unwrap_or_else(|_| "xyz".into());
    Client::new(&[&url], SecretString::from(key), ClientOptions::default())
        .expect("TESSERA_IT_URL must be an http(s) URL")
}

fn status(error: &Error) -> Option<u16> {
    match error {
        Error::Http { status, .. } => Some(*status),
        _ => None,
    }
}

fn message(error: &Error) -> String {
    match error {
        Error::Http { body, .. } => body.clone(),
        other => other.to_string(),
    }
}

/// A stub LLM the server can reach, or `None` to skip. `TESSERA_IT_STUB_HOST` names this machine
/// as the server sees it, such as `host.docker.internal`.
async fn stub(content: &str) -> Option<(MockServer, String)> {
    let Ok(host) = std::env::var("TESSERA_IT_STUB_HOST") else {
        println!("skipped: TESSERA_IT_STUB_HOST is not set");
        return None;
    };
    let listener = std::net::TcpListener::bind("0.0.0.0:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    let server = MockServer::builder().listener(listener).start().await;
    let chat = json!({"choices": [{"message": {"role": "assistant", "content": content}}]});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat))
        .mount(&server)
        .await;
    let embedding = json!({"data": [{"embedding": [0.1, 0.2, 0.3, 0.4], "index": 0}]});
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(embedding))
        .mount(&server)
        .await;
    Some((server, format!("http://{host}:{port}")))
}

async fn history(c: &Client, name: &str) {
    c.create_collection(&json!({"name": name, "fields": [
        {"name": "conversation_id", "type": "string"},
        {"name": "model_id", "type": "string"},
        {"name": "timestamp", "type": "int32"},
        {"name": "role", "type": "string", "index": false},
        {"name": "message", "type": "string", "index": false},
    ]}))
    .await
    .expect("create history");
}

async fn books(c: &Client, name: &str) {
    c.create_collection(&json!({"name": name, "fields": [
        {"name": "t", "type": "string"},
        {"name": "o", "type": "int32"},
    ]}))
    .await
    .expect("create books");
    c.create_document(name, &json!({"id": "1", "t": "hello", "o": 1}))
        .await
        .expect("create document");
}

fn conversation_model(id: &str, url: &str, history: &str) -> Value {
    json!({
        "id": id,
        "model_name": "openai/gpt-4o",
        "api_key": STUB_KEY,
        "openai_url": url,
        "max_bytes": 16384,
        "history_collection": history,
    })
}

/// `api_url` is the whole address, unlike `openai_url`: the server posts to it as it stands.
fn nl_model(id: &str, url: &str) -> Value {
    json!({
        "id": id,
        "model_name": "openai/gpt-4.1",
        "api_key": STUB_KEY,
        "api_url": format!("{url}/v1/chat/completions"),
        "max_bytes": 16384,
    })
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn chat(model: &str, q: &str, conversation: Option<&str>) -> Vec<(String, String)> {
    let mut all = params(&[
        ("q", q),
        ("query_by", "t"),
        ("conversation", "true"),
        ("conversation_model_id", model),
    ]);
    if let Some(id) = conversation {
        all.push(("conversation_id".into(), id.into()));
    }
    all
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_lists_answer() {
    let c = client();
    c.ai_models(ModelKind::Conversation).await.expect("models");
    c.ai_models(ModelKind::NlSearch).await.expect("NL models");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_unknown_model_is_404() {
    let c = client();
    for kind in [ModelKind::Conversation, ModelKind::NlSearch] {
        let error = c.ai_model(kind, "tessera_it_nope").await.expect_err("read");
        assert_eq!(status(&error), Some(404), "{error:?}");
        let error = c
            .delete_ai_model(kind, "tessera_it_nope")
            .await
            .expect_err("delete");
        assert_eq!(status(&error), Some(404), "{error:?}");
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_put_of_unknown_is_404() {
    let c = client();
    let body = json!({"system_prompt": "x"});
    for kind in [ModelKind::Conversation, ModelKind::NlSearch] {
        let error = c
            .update_ai_model(kind, "tessera_it_nope", &body)
            .await
            .expect_err("no upsert");
        assert_eq!(status(&error), Some(404), "{error:?}");
        assert!(message(&error).contains("Model not found"), "{error:?}");
    }
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_validation_messages() {
    let c = client();
    let model = |id: &str| json!({"id": id, "model_name": "openai/gpt-4o", "api_key": STUB_KEY, "openai_url": DEAD_URL});
    let mut no_max_bytes = model("tessera_it_v1");
    no_max_bytes["history_collection"] = "tessera_it_none".into();
    let error = c
        .create_ai_model(ModelKind::Conversation, &no_max_bytes)
        .await
        .expect_err("no max_bytes");
    assert!(message(&error).contains("max_bytes"), "{error:?}");

    let mut no_history = model("tessera_it_v2");
    no_history["max_bytes"] = 1000.into();
    let error = c
        .create_ai_model(ModelKind::Conversation, &no_history)
        .await
        .expect_err("no history_collection");
    assert!(message(&error).contains("history_collection"), "{error:?}");

    let error = c
        .create_ai_model(ModelKind::NlSearch, &model("tessera_it_v3"))
        .await
        .expect_err("no max_bytes");
    assert!(message(&error).contains("max_bytes"), "{error:?}");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_conversation_crud() {
    let Some((_stub, url)) = stub("stub answer").await else {
        return;
    };
    let (c, h, id) = (client(), "tessera_it_ai_crud_h", "tessera_it_ai_crud");
    history(&c, h).await;
    let body = conversation_model(id, &url, h);
    let created = c.create_ai_model(ModelKind::Conversation, &body).await;
    let taken = c.create_ai_model(ModelKind::Conversation, &body).await;
    let listed = c.ai_models(ModelKind::Conversation).await;
    let read = c.ai_model(ModelKind::Conversation, id).await;
    let deleted = c.delete_ai_model(ModelKind::Conversation, id).await;
    let gone = c.ai_model(ModelKind::Conversation, id).await;
    c.delete_collection(h).await.expect("delete history");

    let created = created.expect("create");
    assert_eq!(created["id"], id);
    assert_ne!(created["api_key"], STUB_KEY, "the answer masks the key");
    assert_eq!(status(&taken.expect_err("taken")), Some(409));
    assert!(listed.expect("list").iter().any(|m| m["id"] == id));
    let read = read.expect("read");
    assert!(read["api_key"].as_str().is_some_and(|k| k.contains("***")));
    assert_eq!(read["history_collection"], h);
    deleted.expect("delete");
    assert_eq!(status(&gone.expect_err("gone")), Some(404));
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_put_keeps_what_it_does_not_name() {
    let Some((_stub, url)) = stub("stub answer").await else {
        return;
    };
    let (c, h, id) = (client(), "tessera_it_ai_put_h", "tessera_it_ai_put");
    history(&c, h).await;
    c.create_ai_model(ModelKind::Conversation, &conversation_model(id, &url, h))
        .await
        .expect("create");
    let before = c.ai_model(ModelKind::Conversation, id).await.expect("read");
    let put = c
        .update_ai_model(
            ModelKind::Conversation,
            id,
            &json!({"system_prompt": "changed"}),
        )
        .await;
    let after = c.ai_model(ModelKind::Conversation, id).await;
    let _ = c.delete_ai_model(ModelKind::Conversation, id).await;
    c.delete_collection(h).await.expect("delete history");

    put.expect("put");
    let after = after.expect("read");
    assert_eq!(after["system_prompt"], "changed");
    assert_eq!(after["max_bytes"], before["max_bytes"]);
    assert_eq!(after["api_key"], before["api_key"]);
}

/// Documents the trap of D77 on a throwaway model: why an edit never sends what it read.
#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_put_with_a_mask_stores_it() {
    let Some((_stub, url)) = stub("stub answer").await else {
        return;
    };
    let (c, h, id) = (client(), "tessera_it_ai_mask_h", "tessera_it_ai_mask");
    history(&c, h).await;
    c.create_ai_model(ModelKind::Conversation, &conversation_model(id, &url, h))
        .await
        .expect("create");
    let put = c
        .update_ai_model(
            ModelKind::Conversation,
            id,
            &json!({"api_key": "zz123*****"}),
        )
        .await;
    let after = c.ai_model(ModelKind::Conversation, id).await;
    let _ = c.delete_ai_model(ModelKind::Conversation, id).await;
    c.delete_collection(h).await.expect("delete history");

    put.expect("the server takes the mask and answers 200");
    let masked = after.expect("read")["api_key"].as_str().map(str::to_string);
    let masked = masked.expect("api_key");
    assert_eq!(masked, "zz123*****", "the mask replaced the key");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_chat_writes_history_and_answers() {
    let Some((_stub, url)) = stub("stub answer").await else {
        return;
    };
    let (c, h, b, id) = (
        client(),
        "tessera_it_ai_chat_h",
        "tessera_it_ai_chat_b",
        "tessera_it_ai_chat",
    );
    history(&c, h).await;
    books(&c, b).await;
    c.create_ai_model(ModelKind::Conversation, &conversation_model(id, &url, h))
        .await
        .expect("create");
    let answer = c.ai_search(b, &chat(id, "hello", None)).await;
    let stored = c.search(h, &params(&[("q", "*")])).await;
    let _ = c.delete_ai_model(ModelKind::Conversation, id).await;
    c.delete_collection(b).await.expect("delete books");
    c.delete_collection(h).await.expect("delete history");

    let answer: Value = serde_json::from_str(&answer.expect("chat")).expect("json");
    let conversation = &answer["conversation"];
    assert_eq!(conversation["answer"], "stub answer");
    assert!(conversation["conversation_id"].is_string());
    let turns = &conversation["conversation_history"]["conversation"];
    assert_eq!(turns[0]["user"], "hello");
    assert_eq!(turns[1]["assistant"], "stub answer");
    assert_eq!(answer["found"], 1);
    let stored: Value = serde_json::from_str(&stored.expect("history")).expect("json");
    assert_eq!(stored["found"], 2, "one document per message");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_follow_up_keeps_the_conversation() {
    let Some((_stub, url)) = stub("stub answer").await else {
        return;
    };
    let (c, h, b, id) = (
        client(),
        "tessera_it_ai_up_h",
        "tessera_it_ai_up_b",
        "tessera_it_ai_up",
    );
    history(&c, h).await;
    books(&c, b).await;
    c.create_ai_model(ModelKind::Conversation, &conversation_model(id, &url, h))
        .await
        .expect("create");
    let first = c
        .ai_search(b, &chat(id, "hello", None))
        .await
        .expect("first");
    let first: Value = serde_json::from_str(&first).expect("json");
    let conversation = first["conversation"]["conversation_id"]
        .as_str()
        .expect("id");
    let second = c.ai_search(b, &chat(id, "again", Some(conversation))).await;
    let _ = c.delete_ai_model(ModelKind::Conversation, id).await;
    c.delete_collection(b).await.expect("delete books");
    c.delete_collection(h).await.expect("delete history");

    let second: Value = serde_json::from_str(&second.expect("second")).expect("json");
    assert_eq!(second["conversation"]["conversation_id"], conversation);
    let turns = &second["conversation"]["conversation_history"]["conversation"];
    assert_eq!(turns.as_array().map(Vec::len), Some(4), "{turns}");
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_nl_crud_and_parse() {
    let Some((_stub, url)) = stub(NL_CONTENT).await else {
        return;
    };
    let (c, b, id) = (client(), "tessera_it_ai_nl_b", "tessera_it_ai_nl");
    books(&c, b).await;
    let created = c
        .create_ai_model(ModelKind::NlSearch, &nl_model(id, &url))
        .await;
    let listed = c.ai_models(ModelKind::NlSearch).await;
    let put = c
        .update_ai_model(ModelKind::NlSearch, id, &json!({"temperature": 0.5}))
        .await;
    let found = c
        .ai_search(
            b,
            &params(&[
                ("q", "hello"),
                ("query_by", "t"),
                ("nl_query", "true"),
                ("nl_model_id", id),
                ("nl_query_debug", "true"),
            ]),
        )
        .await;
    let deleted = c.delete_ai_model(ModelKind::NlSearch, id).await;
    c.delete_collection(b).await.expect("delete books");

    assert_ne!(created.expect("create")["api_key"], STUB_KEY);
    assert!(listed.expect("list").iter().any(|m| m["id"] == id));
    assert_eq!(put.expect("put")["temperature"], 0.5);
    deleted.expect("delete");
    let found: Value = serde_json::from_str(&found.expect("search")).expect("json");
    let parsed = &found["parsed_nl_query"];
    assert_eq!(parsed["generated_params"]["filter_by"], "o:=1");
    assert_eq!(parsed["augmented_params"]["sort_by"], "o:desc");
    assert_eq!(parsed["llm_response"]["content"], NL_CONTENT);
    // 30.2 left the field out on one CI run with a fast stub.
    assert!(
        parsed.get("parse_time_ms").is_none_or(Value::is_u64),
        "{parsed}"
    );
    assert_eq!(found["found"], 1);
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_unknown_nl_model_falls_back() {
    let (c, b) = (client(), "tessera_it_ai_back_b");
    books(&c, b).await;
    let found = c
        .ai_search(
            b,
            &params(&[
                ("q", "hello"),
                ("query_by", "t"),
                ("nl_query", "true"),
                ("nl_model_id", "tessera_it_nope"),
            ]),
        )
        .await;
    c.delete_collection(b).await.expect("delete books");

    let found: Value = serde_json::from_str(&found.expect("search")).expect("json");
    assert_eq!(found["found"], 1, "a plain search of the sentence");
    // 30.2 reports the model's absence here, with no parameters: nothing ran.
    let generated = &found["parsed_nl_query"]["generated_params"];
    assert!(
        generated.as_object().is_none_or(|g| g.is_empty()),
        "{found}"
    );
}

#[tokio::test]
#[ignore = "needs Typesense, see dev/docker-compose.yml"]
async fn live_remote_embedding_field_embeds_a_document() {
    let Some((_stub, url)) = stub("stub answer").await else {
        return;
    };
    let (c, name) = (client(), "tessera_it_ai_embed");
    let created = c
        .create_collection(&json!({"name": name, "fields": [
            {"name": "t", "type": "string"},
            {"name": "e", "type": "float[]", "embed": {
                "from": ["t"],
                "model_config": {
                    "model_name": "openai/text-embedding-3-small",
                    // The server caches an embedder by name and key, with the URL of its
                    // first use, and every run has another stub port.
                    "api_key": format!("{STUB_KEY}-{}", url.rsplit(':').next().unwrap_or("")),
                    "url": url,
                }
            }},
        ]}))
        .await;
    let stored = c
        .create_document(name, &json!({"id": "1", "t": "hello"}))
        .await;
    let read = c.document(name, "1").await;
    let schema = c.collection(name).await;
    let _ = c.delete_collection(name).await;

    created.expect("create");
    stored.expect("store");
    let read = read.expect("read");
    assert_eq!(read["e"].as_array().map(Vec::len), Some(4), "{read}");
    let schema = schema.expect("schema");
    let field = schema.fields.iter().find(|f| f.name == "e").expect("e");
    assert_eq!(field.attr("num_dim"), Some(&json!(4)));
}

#[tokio::test]
#[ignore = "downloads a model"]
async fn live_builtin_model() {
    let (c, name) = (client(), "tessera_it_ai_builtin");
    let created = c
        .create_collection(&json!({"name": name, "fields": [
            {"name": "t", "type": "string"},
            {"name": "e", "type": "float[]", "embed": {
                "from": ["t"],
                "model_config": {"model_name": "ts/all-MiniLM-L12-v2"}
            }},
        ]}))
        .await;
    let _ = c.delete_collection(name).await;
    let created = created.expect("create");
    let field = created.fields.iter().find(|f| f.name == "e").expect("e");
    assert_eq!(field.attr("num_dim"), Some(&json!(384)));
}
