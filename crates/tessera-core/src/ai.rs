// SPDX-License-Identifier: GPL-3.0-or-later

//! AI models: what the form sends, and what the test panels read. Words nothing, the app
//! translates the enums.

use serde_json::{Map, Value, json};
use tessera_client::{Collection, ModelKind};

use crate::redact::{MASK, text_has_mask};
use crate::relevance::name_ok;
use crate::schema::FieldDraft;

/// Keys of a model that hold a credential.
pub const AI_SECRET_KEYS: [&str; 4] = ["api_key", "access_token", "refresh_token", "client_secret"];

/// The models the server ships.
pub const BUILT_IN_MODELS: [&str; 2] = ["ts/all-MiniLM-L12-v2", "ts/e5-small"];

/// What the form edits. The API key is apart, so it is never kept with the rest.
#[derive(Clone, Default, PartialEq)]
pub struct ModelDraft {
    /// Id, for a new model. Empty means the server generates one.
    pub id: String,
    /// `namespace/model`.
    pub model_name: String,
    /// Endpoint URL, sent under [`endpoint_key`].
    pub endpoint: String,
    /// Account id, for `cloudflare/`.
    pub account_id: String,
    /// System prompt.
    pub system_prompt: String,
    /// Maximum bytes of context, as typed.
    pub max_bytes: String,
    /// History collection, conversation models only.
    pub history_collection: String,
    /// Seconds a conversation is kept, as typed. Conversation models only.
    pub ttl: String,
    /// Temperature, as typed. NL search models only.
    pub temperature: String,
    /// Every other key.
    pub more: Map<String, Value>,
}

/// Why a draft cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProblem {
    /// The id is not an allowed name.
    BadId,
    /// No model name.
    NoModelName,
    /// A new model without any credential.
    NoApiKey,
    /// A conversation model without a history collection.
    NoHistory,
    /// `max_bytes` is not a positive integer.
    BadMaxBytes,
    /// `ttl` is not a positive integer.
    BadTtl,
    /// `temperature` is not a number.
    BadTemperature,
    /// The body holds a mask, which the server would store as the key.
    HoldsMask,
}

/// One speaker of a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    /// The person asking.
    User,
    /// The model.
    Assistant,
}

/// What a chat turn answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatAnswer {
    /// The model's answer.
    pub answer: String,
    /// Id to continue the conversation with.
    pub conversation_id: String,
    /// All turns so far.
    pub turns: Vec<(ChatRole, String)>,
}

/// What an NL search made of the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NlOutcome {
    /// The model ran.
    Parsed(ParsedNl),
    /// The model failed, with the message.
    Failed(String),
    /// The server fell back to a plain search of the sentence, without a word.
    FellBack,
}

/// The parameters an NL model produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedNl {
    /// What the model wrote.
    pub generated: Map<String, Value>,
    /// What the server searched with.
    pub augmented: Map<String, Value>,
    /// Parse time in milliseconds.
    pub parse_ms: u64,
    /// The model's raw text, with `nl_query_debug`.
    pub llm: Option<String>,
}

/// The key the model's endpoint URL goes under, or `None` when the namespace has none.
pub fn endpoint_key(kind: ModelKind, model_name: &str) -> Option<&'static str> {
    let (namespace, _) = model_name.split_once('/')?;
    match (kind, namespace) {
        (ModelKind::Conversation, "openai") => Some("openai_url"),
        (ModelKind::Conversation, "vllm") => Some("vllm_url"),
        (ModelKind::NlSearch, "openai" | "vllm") => Some("api_url"),
        (_, "azure") => Some("url"),
        _ => None,
    }
}

fn text_of(stored: &Map<String, Value>, key: &str) -> String {
    match stored.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The keys the form has its own control for, given what the model is.
fn form_keys(kind: ModelKind, model_name: &str) -> Vec<&'static str> {
    let mut keys = vec![
        "id",
        "model_name",
        "account_id",
        "system_prompt",
        "max_bytes",
    ];
    keys.extend(endpoint_key(kind, model_name));
    match kind {
        ModelKind::Conversation => keys.extend(["history_collection", "ttl"]),
        ModelKind::NlSearch => keys.push("temperature"),
    }
    keys
}

/// The form's values for a stored model. Holds no credential, not even the server's mask.
pub fn draft_of(kind: ModelKind, stored: &Map<String, Value>) -> ModelDraft {
    let model_name = text_of(stored, "model_name");
    let keys = form_keys(kind, &model_name);
    let more = stored
        .iter()
        .filter(|(k, _)| !keys.contains(&k.as_str()) && !AI_SECRET_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let of_kind = |wanted: ModelKind, key: &str| {
        if kind == wanted {
            text_of(stored, key)
        } else {
            String::new()
        }
    };
    ModelDraft {
        id: text_of(stored, "id"),
        endpoint: endpoint_key(kind, &model_name)
            .map(|key| text_of(stored, key))
            .unwrap_or_default(),
        account_id: text_of(stored, "account_id"),
        system_prompt: text_of(stored, "system_prompt"),
        max_bytes: text_of(stored, "max_bytes"),
        history_collection: of_kind(ModelKind::Conversation, "history_collection"),
        ttl: of_kind(ModelKind::Conversation, "ttl"),
        temperature: of_kind(ModelKind::NlSearch, "temperature"),
        model_name,
        more,
    }
}

/// The form's values as JSON, checked. Blank optional values are left out.
fn checked(kind: ModelKind, draft: &ModelDraft) -> Result<Map<String, Value>, ModelProblem> {
    let mut body = Map::new();
    let model_name = draft.model_name.trim();
    if model_name.is_empty() {
        return Err(ModelProblem::NoModelName);
    }
    body.insert("model_name".into(), model_name.into());
    if let Some(key) = endpoint_key(kind, model_name) {
        body.insert(key.into(), draft.endpoint.trim().into());
    }
    body.insert("account_id".into(), draft.account_id.trim().into());
    body.insert("system_prompt".into(), draft.system_prompt.as_str().into());
    let max_bytes = draft.max_bytes.trim().parse::<u64>();
    match max_bytes {
        Ok(n) if n > 0 => body.insert("max_bytes".into(), n.into()),
        _ => return Err(ModelProblem::BadMaxBytes),
    };
    match kind {
        ModelKind::Conversation => {
            let history = draft.history_collection.trim();
            if history.is_empty() {
                return Err(ModelProblem::NoHistory);
            }
            body.insert("history_collection".into(), history.into());
            let ttl = draft.ttl.trim();
            if !ttl.is_empty() {
                match ttl.parse::<u64>() {
                    Ok(n) if n > 0 => body.insert("ttl".into(), n.into()),
                    _ => return Err(ModelProblem::BadTtl),
                };
            }
        }
        ModelKind::NlSearch => {
            let temperature = draft.temperature.trim();
            if !temperature.is_empty() {
                let number = temperature
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .ok_or(ModelProblem::BadTemperature)?;
                body.insert("temperature".into(), number.into());
            }
        }
    }
    Ok(body)
}

/// Same value, also for a number the server wrote as `0` and the form parsed as `0.0`.
fn same(stored: Option<&Value>, value: &Value) -> bool {
    stored == Some(value)
        || stored
            .and_then(Value::as_f64)
            .is_some_and(|n| Some(n) == value.as_f64())
}

fn guarded(body: Map<String, Value>) -> Result<Value, ModelProblem> {
    let masked = AI_SECRET_KEYS.iter().any(|key| {
        body.get(*key)
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains("***"))
    });
    let body = Value::Object(body);
    if masked || text_has_mask(&body.to_string()) {
        return Err(ModelProblem::HoldsMask);
    }
    Ok(body)
}

/// The body of a `POST`. `api_key` is what was typed. A model with credentials of another shape
/// (`gcp/` tokens) holds them in `more`.
pub fn create_body(
    kind: ModelKind,
    draft: &ModelDraft,
    api_key: &str,
) -> Result<Value, ModelProblem> {
    let id = draft.id.trim();
    if !id.is_empty() && !name_ok(id) {
        return Err(ModelProblem::BadId);
    }
    let mut body = checked(kind, draft)?;
    body.retain(|_, v| v.as_str() != Some(""));
    for (key, value) in &draft.more {
        body.entry(key.clone()).or_insert_with(|| value.clone());
    }
    if !api_key.trim().is_empty() {
        body.insert("api_key".into(), api_key.trim().into());
    }
    let has_credential = AI_SECRET_KEYS.iter().any(|key| {
        body.get(*key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    });
    if !has_credential {
        return Err(ModelProblem::NoApiKey);
    }
    if !id.is_empty() {
        body.insert("id".into(), id.into());
    }
    guarded(body)
}

/// The body of a `PUT`: only what changed against `stored`, and `api_key` only when one was
/// typed, because the server stores whatever it gets, a mask included (D77). A key that left
/// `more` is not sent, since `PUT` cannot remove. An empty object means nothing changed.
pub fn update_body(
    kind: ModelKind,
    stored: &Map<String, Value>,
    draft: &ModelDraft,
    api_key: &str,
) -> Result<Value, ModelProblem> {
    let mut body = checked(kind, draft)?;
    // A cleared text is sent as empty, a text never set stays unset.
    body.retain(|key, value| {
        let unset = value.as_str() == Some("") && !stored.contains_key(key);
        !unset && !same(stored.get(key), value)
    });
    for (key, value) in &draft.more {
        let dropped = key == "id" || AI_SECRET_KEYS.contains(&key.as_str());
        if !dropped && !same(stored.get(key), value) {
            body.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if !api_key.trim().is_empty() {
        body.insert("api_key".into(), api_key.trim().into());
    }
    guarded(body)
}

/// `text` with `secret` replaced by the mask, because a provider's message can echo the key.
pub fn scrub(text: &str, secret: &str) -> String {
    let secret = secret.trim();
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, MASK)
}

/// What a chat or NL test can name in `query_by`: indexed text fields and auto-embedding fields.
/// A plain vector field gets no answer from 30.0 under `conversation=true`.
pub fn query_fields(c: &Collection) -> Vec<String> {
    c.fields
        .iter()
        .filter(|f| !f.name.contains('*'))
        .filter(|f| match f.r#type.as_str() {
            "string" | "string[]" | "string*" => f.flag("index"),
            "float[]" => f.attr("embed").is_some(),
            _ => false,
        })
        .map(|f| f.name.clone())
        .collect()
}

fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The parameters of a chat turn.
pub fn chat_params(
    model: &str,
    question: &str,
    query_by: &str,
    conversation: Option<&str>,
    exclude: Option<&str>,
) -> Vec<(String, String)> {
    let mut params = vec![
        pair("q", question),
        pair("query_by", query_by),
        pair("conversation", "true"),
        pair("conversation_model_id", model),
    ];
    if let Some(id) = conversation {
        params.push(pair("conversation_id", id));
    }
    if let Some(fields) = exclude {
        params.push(pair("exclude_fields", fields));
    }
    params.push(pair("enable_analytics", "false"));
    params.push(pair("per_page", "5"));
    params
}

/// The parameters of an NL search.
pub fn nl_params(
    model: &str,
    question: &str,
    query_by: &str,
    exclude: Option<&str>,
) -> Vec<(String, String)> {
    let mut params = vec![
        pair("q", question),
        pair("query_by", query_by),
        pair("nl_query", "true"),
        pair("nl_model_id", model),
        pair("nl_query_debug", "true"),
    ];
    if let Some(fields) = exclude {
        params.push(pair("exclude_fields", fields));
    }
    params.push(pair("enable_analytics", "false"));
    params
}

/// The `conversation` of a search answer. `None` when it has none.
pub fn chat_answer(text: &str) -> Option<ChatAnswer> {
    let answer: Value = serde_json::from_str(text).ok()?;
    let conversation = answer.get("conversation")?;
    let turns = conversation["conversation_history"]["conversation"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|turn| {
                    let user = turn.get("user").map(|t| (ChatRole::User, t));
                    let (role, text) =
                        user.or(turn.get("assistant").map(|t| (ChatRole::Assistant, t)))?;
                    Some((role, text.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ChatAnswer {
        answer: conversation["answer"].as_str()?.to_string(),
        conversation_id: conversation["conversation_id"].as_str()?.to_string(),
        turns,
    })
}

fn message_of(error: &Value) -> Option<String> {
    match error {
        Value::Null => None,
        Value::String(message) => Some(message.clone()),
        other => Some(other.to_string()),
    }
}

/// How an NL search went. A missing `parsed_nl_query` is a failure whatever the status: an
/// unknown model answers 200 with a plain search. A failing model answers 200 too, with
/// `parsed_nl_query.error` (measured on 30.0 and 30.2; the docs put `error` beside the hits).
pub fn nl_outcome(text: &str) -> NlOutcome {
    let Ok(answer) = serde_json::from_str::<Value>(text) else {
        return NlOutcome::FellBack;
    };
    let parsed = answer.get("parsed_nl_query");
    let error = answer
        .get("error")
        .or(parsed.and_then(|p| p.get("error")))
        .and_then(message_of);
    if let Some(message) = error {
        return NlOutcome::Failed(message);
    }
    let Some(parsed) = parsed.filter(|p| p.is_object()) else {
        return NlOutcome::FellBack;
    };
    let map = |key: &str| parsed[key].as_object().cloned().unwrap_or_default();
    NlOutcome::Parsed(ParsedNl {
        generated: map("generated_params"),
        augmented: map("augmented_params"),
        parse_ms: parsed["parse_time_ms"].as_u64().unwrap_or(0),
        llm: parsed["llm_response"]["content"]
            .as_str()
            .map(str::to_string),
    })
}

/// Writes `embed` on `field`, keeping every key it does not know. `api_key` stays as it is when
/// `None`, and a blank one removes it. A blank `url` removes it too. The server sets `num_dim`
/// itself.
pub fn set_embedding(
    field: &mut FieldDraft,
    from: &[String],
    model: &str,
    api_key: Option<&str>,
    url: &str,
) {
    let mut embed = match field.embed.take() {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    embed.insert("from".into(), json!(from));
    let mut config = match embed.remove("model_config") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    config.insert("model_name".into(), model.into());
    match api_key {
        Some(key) if key.trim().is_empty() => drop(config.remove("api_key")),
        Some(key) => drop(config.insert("api_key".into(), key.trim().into())),
        None => {}
    }
    if url.trim().is_empty() {
        config.remove("url");
    } else {
        config.insert("url".into(), url.trim().into());
    }
    embed.insert("model_config".into(), config.into());
    field.embed = Some(embed.into());
    field.num_dim = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_client::Field;

    fn stored() -> Map<String, Value> {
        json!({
            "id": "m",
            "model_name": "openai/gpt-4o",
            "api_key": "sk-st*********",
            "openai_url": "http://stub",
            "history_collection": "h",
            "max_bytes": 16384,
            "ttl": 86400,
            "system_prompt": "be nice",
            "k": 1
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    }

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    fn draft() -> ModelDraft {
        ModelDraft {
            model_name: "openai/gpt-4o".into(),
            max_bytes: "16384".into(),
            history_collection: "h".into(),
            ..ModelDraft::default()
        }
    }

    #[test]
    fn draft_of_holds_no_credential() {
        let draft = draft_of(ModelKind::Conversation, &stored());
        assert_eq!(draft.endpoint, "http://stub");
        assert_eq!(draft.max_bytes, "16384");
        assert_eq!(draft.more, map(json!({"k": 1})));
    }

    #[test]
    fn update_body_omits_what_did_not_change() {
        let mut d = draft_of(ModelKind::Conversation, &stored());
        assert_eq!(
            update_body(ModelKind::Conversation, &stored(), &d, ""),
            Ok(json!({}))
        );
        d.system_prompt = "changed".into();
        assert_eq!(
            update_body(ModelKind::Conversation, &stored(), &d, ""),
            Ok(json!({"system_prompt": "changed"}))
        );
    }

    #[test]
    fn update_body_never_echoes_a_mask() {
        let mut d = draft_of(ModelKind::Conversation, &stored());
        d.ttl = "60".into();
        let body = update_body(ModelKind::Conversation, &stored(), &d, "").unwrap_or_default();
        assert!(!body.to_string().contains("***"), "{body}");
        assert_eq!(body, json!({"ttl": 60}));
    }

    #[test]
    fn blank_key_keeps_the_stored_one() {
        let d = draft_of(ModelKind::Conversation, &stored());
        let body = update_body(ModelKind::Conversation, &stored(), &d, "  ").unwrap_or_default();
        assert!(body.get("api_key").is_none());
    }

    #[test]
    fn typed_key_is_sent() {
        let d = draft_of(ModelKind::Conversation, &stored());
        assert_eq!(
            update_body(ModelKind::Conversation, &stored(), &d, "sk-new"),
            Ok(json!({"api_key": "sk-new"}))
        );
    }

    #[test]
    fn more_with_api_key_is_dropped() {
        let mut d = draft_of(ModelKind::Conversation, &stored());
        d.more = map(json!({
            "api_key": "x", "access_token": "x", "refresh_token": "x", "client_secret": "x",
            "id": "other", "k": 2
        }));
        assert_eq!(
            update_body(ModelKind::Conversation, &stored(), &d, ""),
            Ok(json!({"k": 2}))
        );
    }

    #[test]
    fn mask_in_a_new_value_is_refused() {
        let d = draft_of(ModelKind::Conversation, &stored());
        for key in ["sk-st*********", MASK] {
            assert_eq!(
                update_body(ModelKind::Conversation, &stored(), &d, key),
                Err(ModelProblem::HoldsMask)
            );
            assert_eq!(
                create_body(ModelKind::Conversation, &draft(), key),
                Err(ModelProblem::HoldsMask)
            );
        }
        let mut masked = draft();
        masked.system_prompt = MASK.into();
        assert_eq!(
            create_body(ModelKind::Conversation, &masked, "k"),
            Err(ModelProblem::HoldsMask)
        );
    }

    #[test]
    fn create_needs_a_key() {
        assert_eq!(
            create_body(ModelKind::Conversation, &draft(), " "),
            Err(ModelProblem::NoApiKey)
        );
        let mut gcp = draft();
        gcp.more = map(json!({"access_token": "t"}));
        assert!(create_body(ModelKind::Conversation, &gcp, "").is_ok());
    }

    #[test]
    fn create_sends_what_the_form_holds() {
        let mut d = draft();
        d.id = "m1".into();
        d.endpoint = "http://stub".into();
        d.ttl = "60".into();
        d.more = map(json!({"k": 1}));
        assert_eq!(
            create_body(ModelKind::Conversation, &d, "sk"),
            Ok(json!({
                "id": "m1", "model_name": "openai/gpt-4o", "openai_url": "http://stub",
                "max_bytes": 16384, "history_collection": "h", "ttl": 60, "k": 1, "api_key": "sk"
            }))
        );
    }

    #[test]
    fn each_problem_is_named() {
        type Change = fn(&mut ModelDraft);
        let cases: [(Change, ModelProblem); 6] = [
            (|d| d.id = "a b".into(), ModelProblem::BadId),
            (|d| d.model_name = " ".into(), ModelProblem::NoModelName),
            (|d| d.history_collection.clear(), ModelProblem::NoHistory),
            (|d| d.max_bytes = "0".into(), ModelProblem::BadMaxBytes),
            (|d| d.max_bytes.clear(), ModelProblem::BadMaxBytes),
            (|d| d.ttl = "x".into(), ModelProblem::BadTtl),
        ];
        for (change, problem) in cases {
            let mut d = draft();
            change(&mut d);
            assert_eq!(create_body(ModelKind::Conversation, &d, "k"), Err(problem));
        }
        let mut nl = draft();
        nl.history_collection.clear();
        nl.temperature = "warm".into();
        assert_eq!(
            create_body(ModelKind::NlSearch, &nl, "k"),
            Err(ModelProblem::BadTemperature)
        );
        nl.temperature = "0.5".into();
        assert!(create_body(ModelKind::NlSearch, &nl, "k").is_ok());
    }

    #[test]
    fn temperature_zero_is_not_a_change() {
        let stored = map(json!({"model_name": "openai/x", "max_bytes": 1, "temperature": 0}));
        let d = draft_of(ModelKind::NlSearch, &stored);
        assert_eq!(
            update_body(ModelKind::NlSearch, &stored, &d, ""),
            Ok(json!({}))
        );
    }

    #[test]
    fn endpoint_key_per_namespace() {
        use ModelKind::{Conversation, NlSearch};
        for (kind, model, key) in [
            (Conversation, "openai/gpt-4o", Some("openai_url")),
            (Conversation, "azure/x", Some("url")),
            (Conversation, "vllm/x", Some("vllm_url")),
            (Conversation, "google/x", None),
            (Conversation, "cloudflare/@cf/x", None),
            (Conversation, "nope", None),
            (NlSearch, "openai/x", Some("api_url")),
            (NlSearch, "vllm/x", Some("api_url")),
            (NlSearch, "azure/x", Some("url")),
            (NlSearch, "gcp/x", None),
        ] {
            assert_eq!(endpoint_key(kind, model), key, "{model}");
        }
    }

    #[test]
    fn scrub_removes_the_key() {
        let text = "Incorrect API key provided: sk-fake. Again: sk-fake";
        assert_eq!(
            scrub(text, "sk-fake"),
            format!("Incorrect API key provided: {MASK}. Again: {MASK}")
        );
        assert_eq!(scrub(text, ""), text);
        assert_eq!(scrub(text, "  "), text);
    }

    #[test]
    fn query_fields_skip_plain_vectors() {
        let field = |v: Value| serde_json::from_value::<Field>(v).expect("field");
        let collection = Collection {
            fields: vec![
                field(json!({"name": "s", "type": "string"})),
                field(json!({"name": "a", "type": "string[]"})),
                field(json!({"name": "n", "type": "string", "index": false})),
                field(json!({"name": ".*", "type": "string*"})),
                field(json!({"name": "o", "type": "int32"})),
                field(json!({"name": "v", "type": "float[]", "num_dim": 4})),
                field(json!({"name": "e", "type": "float[]", "embed": {"from": ["s"]}})),
            ],
            ..serde_json::from_value(json!({"name": "c"})).expect("collection")
        };
        assert_eq!(query_fields(&collection), ["s", "a", "e"]);
    }

    #[test]
    fn params_keep_analytics_off() {
        let chat = chat_params("m", "q", "t", Some("c"), Some("v"));
        assert!(chat.contains(&pair("conversation_id", "c")));
        assert!(chat.contains(&pair("exclude_fields", "v")));
        assert!(chat.contains(&pair("enable_analytics", "false")));
        assert!(chat.contains(&pair("per_page", "5")));
        let first = chat_params("m", "q", "t", None, None);
        assert!(
            !first
                .iter()
                .any(|(k, _)| k == "conversation_id" || k == "exclude_fields")
        );
        let nl = nl_params("m", "q", "t", Some("v"));
        assert!(nl.contains(&pair("nl_query_debug", "true")));
        assert!(nl.contains(&pair("enable_analytics", "false")));
    }

    #[test]
    fn chat_answer_reads_the_turns() {
        let text = json!({"found": 1, "conversation": {
            "answer": "stub answer",
            "conversation_history": {
                "conversation": [{"user": "hello"}, {"assistant": "stub answer"}],
                "last_updated": 1
            },
            "conversation_id": "c1",
            "query": "hello"
        }})
        .to_string();
        assert_eq!(
            chat_answer(&text),
            Some(ChatAnswer {
                answer: "stub answer".into(),
                conversation_id: "c1".into(),
                turns: vec![
                    (ChatRole::User, "hello".into()),
                    (ChatRole::Assistant, "stub answer".into())
                ],
            })
        );
        assert_eq!(chat_answer("{}"), None);
        assert_eq!(chat_answer("nope"), None);
    }

    #[test]
    fn nl_outcome_parsed() {
        let text = json!({"hits": [], "parsed_nl_query": {
            "augmented_params": {"filter_by": "o:=1", "q": "*", "sort_by": "o:desc"},
            "generated_params": {"filter_by": "o:=1", "q": "*", "sort_by": "o:desc"},
            "llm_response": {"content": "{}", "model": "gpt-4.1"},
            "parse_time_ms": 7
        }})
        .to_string();
        let NlOutcome::Parsed(parsed) = nl_outcome(&text) else {
            panic!("not parsed");
        };
        assert_eq!(parsed.generated["filter_by"], "o:=1");
        assert_eq!(parsed.augmented["q"], "*");
        assert_eq!(parsed.parse_ms, 7);
        assert_eq!(parsed.llm.as_deref(), Some("{}"));
    }

    #[test]
    fn nl_outcome_error() {
        let nested = json!({"hits": [], "parsed_nl_query": {
            "augmented_params": {"q": "s", "sort_by": ""},
            "error": "Failed to get response from OpenAI: 500",
            "generated_params": {}
        }})
        .to_string();
        assert_eq!(
            nl_outcome(&nested),
            NlOutcome::Failed("Failed to get response from OpenAI: 500".into())
        );
        let beside = json!({"hits": [], "error": "bad"}).to_string();
        assert_eq!(nl_outcome(&beside), NlOutcome::Failed("bad".into()));
    }

    #[test]
    fn nl_outcome_missing_is_fell_back() {
        assert_eq!(nl_outcome(r#"{"found":1,"hits":[]}"#), NlOutcome::FellBack);
        assert_eq!(nl_outcome("nope"), NlOutcome::FellBack);
    }

    #[test]
    fn set_embedding_keeps_unknown_keys_and_clears_num_dim() {
        let mut field = FieldDraft {
            num_dim: Some(4),
            embed: Some(json!({
                "from": ["old"], "other": 1,
                "model_config": {"model_name": "x", "indexing_prefix": "p", "api_key": "kept"}
            })),
            ..FieldDraft::new("e", crate::schema::FieldType::FloatArray)
        };
        set_embedding(&mut field, &["t".into()], "ts/e5-small", None, "");
        assert_eq!(field.num_dim, None);
        assert_eq!(
            field.embed,
            Some(json!({
                "from": ["t"], "other": 1,
                "model_config": {"model_name": "ts/e5-small", "indexing_prefix": "p", "api_key": "kept"}
            }))
        );
        set_embedding(&mut field, &[], "openai/x", Some("sk"), "http://stub");
        let config = field.embed.as_ref().map(|e| e["model_config"].clone());
        assert_eq!(
            config.as_ref().map(|c| c["api_key"].clone()),
            Some(json!("sk"))
        );
        assert_eq!(config.map(|c| c["url"].clone()), Some(json!("http://stub")));
        set_embedding(&mut field, &[], "openai/x", Some(" "), "http://stub");
        let config = field.embed.as_ref().map(|e| e["model_config"].clone());
        assert!(config.is_some_and(|c| c.get("api_key").is_none()));
    }
}
