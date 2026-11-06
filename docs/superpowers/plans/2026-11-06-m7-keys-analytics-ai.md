# M7 Keys, Analytics, AI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Manage API keys with a scoped key generator and decoder whose output is byte-identical to `typesense-js`, analytics rules, stemming dictionaries, conversation and NL search models with test panels, and embedding fields in the schema form, on Typesense 30.0 and 30.2.

**Architecture:** `tessera-client` gains `keys`, `analytics`, `stemming` and `models_ai`. `tessera-core` gains `keys/` (scoped keys, drafts, action catalog), `analytics`, `stemming`, `ai`. `tessera-app` gains an `AdminBackend` seam and four resource kinds, built on M6's `ResourceList`, `confirm_write` and `HitsView`. Three PRs, in order: A (Tasks 1–5), B (Tasks 6–9), C (Tasks 10–13).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11), reqwest 0.13.5, tokio 1.54.1, hmac 0.13.0, sha2 0.11.1, base64 0.23.1, jiff 0.2.39, chrono 0.4.45 (app only), serde_json 1.0.154 with `preserve_order`, wiremock 0.6.5.

**Spec:** [`docs/superpowers/specs/2026-11-06-m7-keys-analytics-ai-design.md`](../specs/2026-11-06-m7-keys-analytics-ai-design.md). Section numbers below (§4.2 …) refer to it. Read the section a task names before starting it. §4 lists every server trap found by probing.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI. Direction: `app → widgets → core → client`.
- Direct dependencies are pinned exactly (`=x.y.z`) in the workspace `Cargo.toml`. Task 2 adds `base64 = "=0.23.1"`, `hmac = "=0.13.0"`, `sha2 = { version = "=0.11.1", default-features = false }`, used by `tessera-core` only. All three versions are in `Cargo.lock` already. No new package may enter `Cargo.lock`.
- Never guess a GPUI or GPUI Kit API. Read the pinned source under `~/.cargo/registry/src/*/` first (`gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0`, `gpui-pre-0.3.11`). A pattern named in a task (`curation_dialog.rs`, `test_panel.rs`, `connect_dialog/mod.rs`) is in use and verified.
- Typesense truth is `spec/typesense-openapi.yml`, the v30.2 docs and design §4. Where they disagree, §4 was measured on 30.0 and 30.2 and wins. In particular the documented action names are often wrong (§4.2).
- Every UI string goes through `t!()`, same keys in `assets/locales/en.yml` and `de.yml`, flat keys, under `keys.*`, `scoped.*`, `analytics.*`, `stemming.*`, `ai.*`, `schema.embed.*`. The key-parity test stays green. Core words nothing: it returns enums, and the app translates them.
- No blocking I/O on the GPUI thread and none on a tokio worker. Reading a file goes through the backend on the runtime.
- **Secrets.** A new key's `value` and a provider key are never in a log, file, error, `Debug`, `Display`, request event, history or persisted state. They live in `SecretString` or a masked input only. Redaction tests are never removed. Every AI model call runs on `without_bodies()`, `POST /keys` on `without_response_bodies()`, both inside the client.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. No commit with a failing check.
- Commits: `git commit -s`, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense on port 18108, key `xyz`. **Never send anything to port 8108.** Live tests: `TESSERA_IT_URL=http://localhost:18108 TESSERA_IT_KEY=xyz cargo test -p tessera-client --test live_admin -- --ignored`. Each client and core live test also runs against 30.0: `docker run --rm -d --name t300 -p 18208:8108 --add-host=host.docker.internal:host-gateway --tmpfs /data typesense/typesense:30.0 --data-dir /data --api-key=xyz --enable-cors=false --enable-search-analytics=true --analytics-dir=/data/analytics --analytics-minute-rate-limit=1000`, then the same test command with `TESSERA_IT_URL=http://localhost:18208 TESSERA_IT_ANALYTICS=1 TESSERA_IT_STUB_HOST=host.docker.internal`, then `docker stop t300`. For 30.2 with the flags, the same on port 18209 with the image `typesense/typesense:30.2`. Throwaway names `tessera_it_*`, removed at the end of each test. Keys a test creates are deleted too.
- A test without `TESSERA_IT_ANALYTICS` sends at most 2 analytics events: the default limit is 5 per minute, and refused requests count.
- UX rules: environment visible everywhere (connection name and `env_badge::render` in dialog titles and tab titles, via `dialog_title`); errors inline and in full; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown on the control via `Kbd::binding_for_action(&Action, Some(RESOURCES), window)`.
- Shortcuts are bound in key contexts. Platform constants follow the `#[cfg(target_os = "macos")]` pairs in `crates/tessera-app/src/actions.rs`. New actions go into `ACTION_LABELS` with a `palette.action.<Name>` key. Disabled actions are gated through `gated(context, gates)`.
- Writes go through `ConnectionState::ensure_writable(window, cx, then)`, and the control shows `env_badge::lock(id, false, cx)` while `writes_locked()`.
- Anything a user types or a server returns that ends in a URL path or query passes an allowlist: `name_ok` for a new name, `addressable` plus `SEGMENT` in every path (`resource_path` does both), `encode_query` for query values. Key ids are `u64`, never text. `query_by` names must be in `query_fields`.
- A `PUT` here can be an upsert (analytics rules) or store a mask (AI models). See §4.3 and §4.6 before writing one. Destructive flows confirm first (D81) through `confirm_write`; cancelling sends nothing.
- A write runs detached (`cx.spawn(...).detach()`), so closing a tab never aborts it.
- NF5: the Linux release binary stays below 41,943,040 bytes. Each PR records the size in its description.
- Each task ends with its CHANGELOG entry under `## [Unreleased]` → `### Added` in `CHANGELOG.md`, in the same commit. Plain prose, in the style of the existing entries.

## Review Focus

1. Scoped keys must match the vectors byte for byte. Check that the digest is the base64 text, that exactly the 16 arrayable names are joined, and that the writer refuses numbers rather than print them differently from JavaScript. Task 2.
2. `verify` must run over the exact decoded bytes with `Mac::verify_slice`, never over a re-serialized copy and never with `==` on strings. Task 2.
3. A created key's value must not reach a `RequestEvent`, including through the raw console. Tests in Task 1. It must be unclosable by Esc in the reveal and gone when the dialog goes. Task 4.
4. The action catalog must hold the measured names, not the documented ones: `aliases:upsert`, `stopwords:upsert`, `synonym_sets:upsert`, `stemming/dictionaries/import:create`, `analytics/rules:*`, no `analytics:*`. Tests in Tasks 2 and 5.
5. An analytics edit must read before the `PUT`, because `PUT` creates a rule that is gone. Tests in Tasks 6 and 8.
6. A stemming file must be validated whole before anything is sent: the server stores the lines before a bad one. Tests in Tasks 7 and 9.
7. An AI model `PUT` must never hold what was read: a mask replaces the real provider key and answers 200. Tests in Tasks 10 and 11.
8. A missing `parsed_nl_query` is a failure, even with status 200. Tests in Tasks 10 and 12.
9. A chat turn is a write, a retry would repeat it, and `query_by` must not name a plain vector field: 30.0 does not answer. Tests in Tasks 10 and 12.
10. Embedding schema writes must keep their bodies out of the request log. Test in Task 10.
11. Every model error must pass `scrub`, because a provider can echo the key. Test in Task 11.

---

## PR A · Keys (branch `feat/m7-keys`, from the tip of `feat/m6-reindex`)

### Task 1: Client for API keys

**Files:**
- Create: `crates/tessera-client/src/keys.rs`, `crates/tessera-client/tests/keys.rs`, `crates/tessera-client/tests/live_admin.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-client/src/raw.rs`

**Interfaces (exported from `lib.rs`):**

```rust
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ApiKey {
    pub id: u64,
    #[serde(default)] pub description: String,
    #[serde(default)] pub actions: Vec<String>,
    #[serde(default)] pub collections: Vec<String>,
    #[serde(default)] pub expires_at: Option<i64>,
    #[serde(default)] pub value_prefix: Option<String>,
    #[serde(flatten)] pub extra: Map<String, Value>,   // `value` is taken out before this exists
}
#[derive(Debug, Clone, PartialEq)]
pub struct NewKey { pub description: String, pub actions: Vec<String>, pub collections: Vec<String>, pub expires_at: Option<i64> }
#[derive(Debug, Clone)]
pub struct CreatedKey { pub key: ApiKey, pub value: SecretString }

impl Client {
    pub async fn keys(&self) -> Result<Vec<ApiKey>, Error>;                 // GET /keys, {"keys":[…]}
    pub async fn key(&self, id: u64) -> Result<ApiKey, Error>;              // GET /keys/{id}
    pub async fn create_key(&self, key: &NewKey) -> Result<CreatedKey, Error>; // POST /keys
    pub async fn delete_key(&self, id: u64) -> Result<(), Error>;           // DELETE /keys/{id}
    pub fn with_key(&self, key: SecretString) -> Self;                      // clone with another key
}
```

- `create_key` sends `{"description","actions","collections"}` plus `expires_at` when `Some`, on `self.without_response_bodies()`. Parse the answer into `Value`, `remove("value")`, build `SecretString`, deserialize the rest into `ApiKey`. A 2xx answer without `value` is `Error::Decode` built with `serde::de::Error::custom("the answer has no value")`, and never prints the body.
- `raw()` in `raw.rs`: when the method is `POST` and the trimmed path is `keys` (with or without leading `/`), run on `self.without_response_bodies()`. The returned `RawResponse` still holds the answer.
- `with_key` is `Self { key, ..self.clone() }`.
- Add a `live_admin.rs` skeleton with the helpers of `live_relevance.rs` (`url()`, `client()` from `TESSERA_IT_URL`, `TESSERA_IT_KEY`).

- [ ] **Step 1: Write the failing tests.** In `tests/keys.rs` with `wiremock`: `create_key_body_and_value`; `create_event_holds_no_value` installs a hook, runs `create_key`, and asserts that no event's `response_body` exists and that `format!("{created:?}")` lacks the value; `raw_post_keys_event_holds_no_value`; `raw_get_keys_keeps_its_body`; `create_answer_without_value_is_an_error`; `keys_decodes_unknown_fields_into_extra`. In `live_admin.rs`, `#[ignore]`: `live_key_crud` creates with `expires_at` 4102444800, lists, gets, deletes, gets 404; `live_with_key_uses_the_other_key` proves a search-only key is 401 on `GET /keys`.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test keys`. Expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the wiremock tests, and the live tests on 18108 and on a 30.0 container.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(client): API keys`.

### Task 2: Core scoped keys, action catalog and key drafts

**Files:**
- Create: `crates/tessera-core/src/keys/mod.rs`, `crates/tessera-core/src/keys/scoped.rs`, `crates/tessera-core/src/keys/actions.rs`, `crates/tessera-core/tests/fixtures/scoped_keys.json`, `crates/tessera-core/tests/scoped_keys.rs`, `crates/tessera-core/tests/live_scoped.rs`
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/tessera-core/Cargo.toml`, `crates/tessera-core/src/lib.rs`

**Interfaces:** design §5.2 and §5.3, verbatim.

**Crates, verified in the registry source:**

```rust
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_PAD_INDIFFERENT};
use hmac::{Hmac, KeyInit, Mac};   // hmac 0.13 re-exports KeyInit; `Hmac::<Sha256>::new_from_slice`
use sha2::Sha256;
```

**Algorithm:** §5.1. `digest = STANDARD.encode(mac.finalize().into_bytes())`, 44 ASCII bytes. `scoped = STANDARD.encode(digest + parent[..4] + text)`.

**The 16 arrayable names, exactly:** `query_by`, `query_by_weights`, `facet_by`, `group_by`, `include_fields`, `exclude_fields`, `highlight_fields`, `highlight_full_fields`, `pinned_hits`, `hidden_hits`, `infix`, `override_tags`, `num_typos`, `prefix`, `synonym_sets`, `sort_by`. Only at the top level. A joined number prints by the same rule as any number, a joined boolean as `true`/`false`, a joined string as is.

**Writer:** write the text by hand over `Value`: object keys through `serde_json::to_string(&key)`, strings the same, `null`, booleans, arrays with `,`, objects with `,` and `:`, no whitespace.
- `Number::as_i64`/`as_u64`: `|n| ≤ 9_007_199_254_740_991`, else `ScopedError::Number`.
- `as_f64`: `x == 0.0` writes `0` (this covers `-0.0`). Finite with `1e-6 <= x.abs() && x.abs() < 1e21` writes `format!("{x}")`. Else `ScopedError::Number`.
- A key that is a canonical array index (`key.parse::<u32>()` succeeds, the value is below `u32::MAX`, and `n.to_string() == key`) at any depth is `ScopedError::IndexKey`.
- A top-level `expires_at` that is not an integer is `ScopedError::ExpiresAt`.

**Parent:** `parent.len() < 4` is `ParentProblem::Short`, `!parent.is_ascii()` is `ParentProblem::NotAscii`.

**Decode:** trim the input, `STANDARD_PAD_INDIFFERENT.decode`, length under 50 is `TooShort`, split at 44 and 48, `String::from_utf8` on each of the two text parts, `serde_json::from_str` on the third. `expires_at` from `params`. `verify`: `STANDARD.decode(&digest)`, `Hmac::<Sha256>::new_from_slice(parent)`, `update(text.as_bytes())`, `verify_slice(&decoded)`, and `prefix` equals `parent`'s first 4 bytes.

**Fixture `scoped_keys.json`, exactly this file.** `input` is parsed with `serde_json::from_str`, `generate` must yield `scoped`, `canonical_params` must yield `params`. From npm `typesense` 3.2.0, shasum `3b3dc5e8d76800304808ac6ed5bdb9b5b6398bc6`, parents invented.

```json
[
 {"parent":"abcdefgh","input":"{}","params":"{}","scoped":"eGpCd2JPdEw1RXdPNHpsellTVGI4aGlEMHRaMVFCbUozUHREaFZmRTJxVT1hYmNke30="},
 {"parent":"tessera-parent-0001","input":"{\"filter_by\":\"company_id:124\",\"expires_at\":1906054106}","params":"{\"filter_by\":\"company_id:124\",\"expires_at\":1906054106}","scoped":"cytaWUljNzhIWmRPMnlWT0tOQ2VTOTFPWEEzSWEwK1IrVXZMZW95ajM0bz10ZXNzeyJmaWx0ZXJfYnkiOiJjb21wYW55X2lkOjEyNCIsImV4cGlyZXNfYXQiOjE5MDYwNTQxMDZ9"},
 {"parent":"tessera-parent-0001","input":"{ \"query_by\" : [\"title\",\"author\"], \"filter_by\":\"a:=1\", \"limit_multi_searches\":5 }","params":"{\"query_by\":\"title,author\",\"filter_by\":\"a:=1\",\"limit_multi_searches\":5}","scoped":"RGZ2alRVMVpacTYyK1lpRTBkN2E5bEJMSExxaWYzZlVpQmIzOGkwWmMrdz10ZXNzeyJxdWVyeV9ieSI6InRpdGxlLGF1dGhvciIsImZpbHRlcl9ieSI6ImE6PTEiLCJsaW1pdF9tdWx0aV9zZWFyY2hlcyI6NX0="},
 {"parent":"tessera-parent-0001","input":"{\"filter_by\":\"name:=`Zürich 🏔` && note:=\\\"a\\\\b\\n\\t\\\"\",\"limit_hits\":10}","params":"{\"filter_by\":\"name:=`Zürich 🏔` && note:=\\\"a\\\\b\\n\\t\\\"\",\"limit_hits\":10}","scoped":"MVE4WkNyQUs3c1VydVdFR0N2Q3d2eHZ3RE43TXVEeDNhVG9xeUhZcXc4bz10ZXNzeyJmaWx0ZXJfYnkiOiJuYW1lOj1gWsO8cmljaCDwn4+UYCAmJiBub3RlOj1cImFcXGJcblx0XCIiLCJsaW1pdF9oaXRzIjoxMH0="},
 {"parent":"tessera-parent-0001","input":"{\"q\":\"*\",\"sort_by\":[\"price:desc\",\"_text_match:asc\"],\"num_typos\":[1,2],\"prefix\":[true,false],\"per_page\":1.5,\"cache_ttl\":1000.0,\"z\":-0.0,\"w\":1e3,\"nested\":{\"b\":1,\"a\":[1,\"2\",null,true]}}","params":"{\"q\":\"*\",\"sort_by\":\"price:desc,_text_match:asc\",\"num_typos\":\"1,2\",\"prefix\":\"true,false\",\"per_page\":1.5,\"cache_ttl\":1000,\"z\":0,\"w\":1000,\"nested\":{\"b\":1,\"a\":[1,\"2\",null,true]}}","scoped":"V1MrZzFjNXNXUlk1Y0hCaGMrYUdzZDAybzNYaEV4a2RKbVJEOXU1RlROVT10ZXNzeyJxIjoiKiIsInNvcnRfYnkiOiJwcmljZTpkZXNjLF90ZXh0X21hdGNoOmFzYyIsIm51bV90eXBvcyI6IjEsMiIsInByZWZpeCI6InRydWUsZmFsc2UiLCJwZXJfcGFnZSI6MS41LCJjYWNoZV90dGwiOjEwMDAsInoiOjAsInciOjEwMDAsIm5lc3RlZCI6eyJiIjoxLCJhIjpbMSwiMiIsbnVsbCx0cnVlXX19"},
 {"parent":"tessera-parent-0001","input":"{\"query_by_weights\":[2,1.5],\"include_fields\":\"a,b\",\"tags\":[\"a\",\"b\"]}","params":"{\"query_by_weights\":\"2,1.5\",\"include_fields\":\"a,b\",\"tags\":[\"a\",\"b\"]}","scoped":"N2s4Vk5TWGNmb2t4WkN6RnM3YnBHQ2x3aDM2cHRXcXE4NmdidUlxa3VoOD10ZXNzeyJxdWVyeV9ieV93ZWlnaHRzIjoiMiwxLjUiLCJpbmNsdWRlX2ZpZWxkcyI6ImEsYiIsInRhZ3MiOlsiYSIsImIiXX0="},
 {"parent":"tessera-parent-0001","input":"{\"a\":\"\\u2028\\u007f\\u0001\\u001f/<>&\"}","params":"{\"a\":\" \u007f\\u0001\\u001f/<>&\"}","scoped":"TTk3blZ1VEI5bGNheWVXWTRtZEY5TENwdWhvNTNJUms2Y2F0ckc5WDg4MD10ZXNzeyJhIjoi4oCof1x1MDAwMVx1MDAxZi88PiYifQ=="},
 {"parent":"abcd","input":"{\"filter_by\":\"o:=1\",\"filter_by\":\"o:=2\"}","params":"{\"filter_by\":\"o:=2\"}","scoped":"YWk4Skd1VnI1U01qTTdSL3hFM1JDUXlLUDhQTEhkc3hZM2FhN05COFJlST1hYmNkeyJmaWx0ZXJfYnkiOiJvOj0yIn0="}
]
```

In vector 7, `params` holds U+2028 and U+007F as raw characters, and `\u0001`, `\u001f` as the six-character escapes. The ` ` and `\u007f` above are JSON escapes of the fixture file, which `serde_json` turns into the raw characters. `scoped` decodes to those bytes.

**Action groups, exactly** (`ACTION_GROUPS` in this order; `key` is the locale suffix under `keys.group.`). Names come from design §4.2:

| key | actions |
|---|---|
| `collections` | `collections:create`, `get`, `list`, `update`, `delete`, `*` |
| `documents` | `documents:search`, `get`, `create`, `upsert`, `update`, `delete`, `import`, `export`, `*` |
| `aliases` | `aliases:list`, `get`, `upsert`, `delete`, `*` |
| `synonym_sets` | `synonym_sets:list`, `get`, `upsert`, `delete`, `*` |
| `synonym_items` | `synonym_sets/items:list`, `get`, `upsert`, `delete`, `*` |
| `curation_sets` | `curation_sets:list`, `get`, `upsert`, `delete`, `*` |
| `curation_items` | `curation_sets/items:list`, `get`, `upsert`, `delete`, `*` |
| `stopwords` | `stopwords:list`, `get`, `upsert`, `delete`, `*` |
| `presets` | `presets:list`, `get`, `upsert`, `delete`, `*` |
| `keys` | `keys:list`, `get`, `create`, `delete`, `*` |
| `analytics_rules` | `analytics/rules:list`, `get`, `create`, `upsert`, `delete`, `*` |
| `analytics_events` | `analytics/events:create`, `analytics/events:list`, `analytics/status:list`, `analytics/flush:create` |
| `stemming` | `stemming/dictionaries:list`, `stemming/dictionaries:get`, `stemming/dictionaries:delete`, `stemming/dictionaries/import:create` |
| `conversation_models` | `conversations/models:list`, `get`, `create`, `upsert`, `delete` |
| `nl_models` | `nl_search_models:list`, `get`, `create`, `upsert`, `delete` |
| `operations` | `operations/snapshot:create`, `operations/vote:create`, `operations/cache/clear:create`, `operations/db/compact:create`, `operations/schema_changes:list`, `config:create` |
| `metrics` | `metrics.json:list`, `stats.json:list`, `debug:list` |

The plain `*` is not in a group: the form has one checkbox for it, with the warning.

**Templates:** `SearchOnly`: `["documents:search"]` on `["*"]`. `ImportOnly`: `["documents:import","collections:get"]` on `["*"]`. `Admin`: `["*"]` on `["*"]`.

**`expiry`:** `None` or `>= NEVER` is `Never`, `<= now` is `Expired`, within `EXPIRY_WARNING_DAYS` is `Soon`, else `Later`.

**`check_new_key`:** blank description `NoDescription`, no actions `NoActions`, no collections `NoCollections`, `expires_at <= now` `ExpiryPast`.

- [ ] **Step 1: Write the failing tests.** `tests/scoped_keys.rs`: `vectors_match_typesense_js` over the fixture for `canonical_params` and `generate`; `decode_round_trips_every_vector`, including `verify` with the parent true and with a changed last character false; `verify_uses_the_stored_text` on a key whose JSON has whitespace, built by hand in the test; `unpadded_key_decodes`; `tampered_digest_fails`; `wrong_prefix_fails`; one test per `ScopedError` and `DecodeError`, including `1e21`, `1e-7`, `2^53`, a key `"1"`, `{"query_by":[null]}`, `{"expires_at":"1"}`, `{"expires_at":1.5}`, `[1]`, parents `"abc"` and `"üüüü"`. Unit tests in `keys/`: `expiry` boundaries with `NEVER`; `collections_of` keeps `a{1,2}` whole; `parent_note` with two keys of one prefix; `catalog_has_no_documented_only_names` asserts `aliases:create`, `stopwords:create`, `synonym_sets:create`, `stemming/dictionaries:create` and every `analytics:` name are absent. In `tests/live_scoped.rs`, `#[ignore]`: `live_scoped_key_limits_what_is_visible`: create a collection with `o` values 1, 1, 2, a key with `["documents:search"]`, `generate` with `filter_by o:=1`, search through `Client::with_key`: `found` 2. `live_parent_with_more_actions_is_401` for `["documents:search","documents:get"]`, and `live_embedded_expiry_past_is_401`.
- [ ] **Step 2: Run** `cargo test -p tessera-core --test scoped_keys`. Expected: fails to compile.
- [ ] **Step 3: Implement.** After editing `Cargo.toml`, confirm `git diff Cargo.lock` only adds three lines to `tessera-core`'s dependency list, with versions spelled `base64 0.23.1`, `hmac 0.13.0`, `sha2 0.11.1` as other entries do.
- [ ] **Step 4: Run** the tests, and `live_scoped` on 18108 and a 30.0 container.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(core): scoped keys matching typesense-js`.

### Task 3: Backend seam and the keys tab

**Files:**
- Create: `crates/tessera-app/src/admin.rs`, `crates/tessera-app/src/views/resources/keys.rs`, `crates/tessera-app/tests/ui_keys.rs`
- Modify: `crates/tessera-app/src/lib.rs`, `state.rs`, `actions.rs`, `views/resources/mod.rs`, `views/root.rs`, `views/sidebar.rs`, `crates/tessera-app/tests/common/mod.rs`, `assets/locales/*.yml`

**Interfaces:**

```rust
// admin.rs. Same shape as relevance.rs: trait, Global, impl for TokioBackend, tokio_backend().
pub trait AdminBackend: Send + Sync {
    fn keys(&self, client: Client) -> BoxFuture<Result<Vec<ApiKey>, Error>>;
    fn create_key(&self, client: Client, key: NewKey) -> BoxFuture<Result<CreatedKey, Error>>;
    fn delete_key(&self, client: Client, id: u64) -> BoxFuture<Result<(), Error>>;
    fn scoped_search(&self, client: Client, key: SecretString, collection: String,
                     params: Vec<(String, String)>) -> BoxFuture<Result<String, Error>>;
    // Later tasks add theirs.
}
pub struct AdminBackendGlobal(pub Arc<dyn AdminBackend>);
```

- `ResourceKind::Keys`: extend `ALL`, `key()` (`"keys"`), `shown` returns true. `ResourceView::Keys(Entity<KeysView>)` in every match of `views/resources/mod.rs`. `resource_shortcut`, `OpenKeys` handled in `root.rs` under `can_search`, as `OpenAliases`.
- `OPEN_RESOURCES` grows to `[&str; 9]`, `alt-cmd-6` to `9` and `ctrl-alt-6` to `9`. Task 3 binds index 5 only; the others come with their kinds.
- `KeysView` follows `stopwords.rs`: `ResourceList::new("keys", COLUMNS, window, cx)`, `reload`, `follow_selection`, `h_resizable` with the list and a right side. Columns: `("id", 60., true)`, `("description", 200., false)`, `("prefix", 90., false)`, `("actions", 200., false)`, `("collections", 140., false)`, `("expires", 120., false)`.
- `prefix` cell: `redact::key(value_prefix)`. `actions` and `collections`: joined by `, `. `expires`: `t!("keys.never")`, or the date through `jiff` in the local zone, `warning` set to the column index for `Expired` and `Soon`.
- Right side, view one: actions and collections as text, `expires_at` with its state, the key as read-only `JsonEditor` (`serde_json::to_string_pretty` of `ApiKey`, which holds no secret).
- Delete (`DeleteResource`): `confirm_write` with `Danger { lines: [t!("keys.delete_consequence")], expected: description or id, action: t!("keys.delete"), typed: prod }`. Consequence text: "Clients using this key stop working, and so does every scoped key made from it."
- `empty_state("keys", "https://typesense.org/docs/30.2/api/api-keys.html")`.
- `sidebar.keys` "API keys" / "API-Schlüssel".

- [ ] **Step 1: Write the failing UI tests** in `ui_keys.rs`: `keys_row_opens_the_tab`, `list_shows_prefix_not_value`, `expired_key_is_marked`, `key_without_expiry_reads_never` (with `expires_at` 64723363199), `delete_asks_and_cancel_sends_nothing`, `delete_on_prod_is_typed`, `restricted_key_shows_the_server_message`, `read_only_delete_asks_to_unlock`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_keys`. Expected: fails.
- [ ] **Step 3: Implement.** The `New` control comes in Task 4: this task's toolbar omits it (UX rule 4).
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): keys tab`.

### Task 4: Create dialog and the one-time reveal

**Files:**
- Create: `crates/tessera-app/src/views/resources/key_dialog.rs`
- Modify: `views/resources/keys.rs`, `views/resources/mod.rs`, `tests/ui_keys.rs`, locales

**Behaviour:** design §7.3.

- Open with `NewResource`. Dialog pattern as `name_dialog.rs` (`window.open_dialog`, `dialog_title`, `.on_ok`).
- Fields: description `InputState`; three template buttons; a `*` `Checkbox` with `t!("keys.wildcard_warning")` while checked; `Checkbox::new` per action, in groups from `ACTION_GROUPS` with `t!(format!("keys.group.{}", g.key))`, in a scrolling area of fixed height; "All collections" `Checkbox` (default checked) and a multi-line input (`InputState::multi_line(true)`) for names or regexes, disabled while the box is checked, with the hint "One per line. Regex allowed, for example `org_.*`."; `DatePicker::new(&state).small().cleanable(true)` with `DatePickerState::new(window, cx)`, as in `curation_dialog.rs` lines 241 and 1074. An empty picker is "Never expires".
- `expires_at`: from `state.read(cx).date_time()`, as `curation_dialog.rs` line 528, taken as 23:59:59 of that day in the local zone (`chrono::Local`), then `.timestamp()`.
- Save: `check_new_key` first, each `KeyProblem` a line in the dialog. Then `ensure_writable`, `create_key`, detached.
- Reveal: on success the form dialog closes and a second dialog opens, a `RevealDialog` view holding `value: SecretString` and `description: String`. The input is `InputState::new(window, cx).masked(true)` with the value set once, disabled for editing; eye button as `connect_dialog/render.rs` line 164 (`toggle_masked`). Copy: `cx.write_to_clipboard(ClipboardItem::new_string(value))`, and the button reads "Copied" for 2 s. The dialog is built with `.close_button(false).overlay_closable(false).keyboard(false)` (`gpui-component-0.9.0/src/dialog/dialog.rs` lines 482, 533, 539), and `.on_ok` returns false. The only way out is Done. On Done: drop the state, reload the list, select the new key.
- "Save to a connection profile…": clones `connection.read(cx).profile`, a new `Uuid`, the name `format!("{} · {}", profile.name, description)`, then `ConnectDialog::open_new(state, Some((profile, Some(value.clone()))), window, cx)`. Read `views/connect_dialog/mod.rs` lines 107 to 125 for the exact signature. The reveal stays open behind it.
- Strings: `keys.reveal_title` "Copy the new key", `keys.reveal_warning` "This is the only time the key is shown. Typesense stores only its first characters.", `keys.reveal_done` "Done".

- [ ] **Step 1: Write the failing UI tests** in `ui_keys.rs`: `template_search_only_fills_the_matrix`; `wildcard_warns`; `empty_description_stays_in_the_form_and_sends_nothing`; `past_date_is_refused`; `date_means_the_end_of_that_day`; `create_sends_the_chosen_actions`; `reveal_shows_masked_and_copy_gives_the_value`; `reveal_ignores_escape`; `done_closes_the_reveal_and_selects_the_key`; `read_only_save_asks_to_unlock`. The request log side of the secret is Task 1's test.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): create keys and show the value once`.

### Task 5: Scoped key panel

**Files:**
- Create: `crates/tessera-app/src/views/resources/scoped_keys.rs`, `crates/tessera-app/tests/ui_scoped_keys.rs`, `crates/tessera-core/tests/live_actions.rs`
- Modify: `views/resources/keys.rs`, `actions.rs`, locales

**Behaviour:** design §7.4.

- ⌘1 / ⌘2: bind `ShowDocuments` and `ShowSchema` in `RESOURCES` with `SHOW_FORM`, `SHOW_JSON`, and handle them in `KeysView` and later `AiView`. Buttons as `dock.rs` `CollectionPanel` draws its two views, with `Kbd::binding_for_action`.
- `ScopedKeysView` is a child entity of `KeysView`, so switching keeps its text. It takes the key list on `set_keys`.
- Parent: `InputState::new(window, cx).masked(true)` with the eye button.
- Parameters: `JsonEditor` not read-only, initial text `{\n  "filter_by": "",\n  "expires_at": <now + 86400>\n}`.
- On `Changed` of either: parse, `generate`, show the key in a read-only `InputState` with Copy, or the translated `ScopedError`/parse error inline. An empty `filter_by` value generates as typed: it is the user's text.
- `parent_note` line under the parent, with `jiff::Timestamp::now()`. `Unknown` and `NotSearchOnly` and `Expired` are warnings; `SearchOnly` prints nothing.
- Decode: multi-line input. On change `decode`, then: prefix, `expires_at` date or "No expiry", params in a read-only `JsonEditor`, and the signature line: `verify` when the parent input is filled, else `t!("scoped.not_verified")`. The decoder takes a pasted key and never logs it.
- Test: `Select` with `SelectState::new(names, None, window, cx).searchable(true)`, filled from the connection's collection list as `test_panel.rs` does; Run bound to `RunTest`. Key used: decode input if filled, else the generated key. Params: `q=*`, `per_page=20`, `enable_analytics=false`. Result through `tessera_core::search::results` into `HitsView::set_result` (see `test_panel.rs`). A running test is replaced by a newer one, and is a `Task` held by the view, so closing the tab drops it: it is a read.
- A 401 adds `t!("scoped.forbidden_hint")`: "The parent key must hold exactly one action, documents:search, and be alive. The bootstrap key cannot be a parent."
- The view has no persisted state, and does not derive `Debug`.

- [ ] **Step 1: Write the failing tests.** UI: `typing_generates_a_key_locally` asserts the backend saw no call and the key equals `generate`; `bad_json_shows_inline`; `number_refused_shows_inline`; `parent_with_more_actions_warns`; `bootstrap_warns_as_unknown`; `decode_shows_params_and_expiry`; `signature_valid_and_invalid`; `not_verified_without_parent`; `test_runs_with_the_scoped_key` asserts `FakeAdmin` saw the key; `forbidden_shows_the_hint`; `test_runs_on_a_read_only_profile`; `views_switch_with_their_shortcuts`. Live, `actions_in_the_catalog_grant_what_they_say` in `tests/live_actions.rs`: for one action per group, create a key with that action and assert a status other than 401 on a matching request: `aliases:list` `GET /aliases`, `stopwords:upsert` `PUT /stopwords/tessera_it_w`, `synonym_sets:upsert`, `presets:upsert`, `analytics/rules:list`, `analytics/status:list`, `stemming/dictionaries:list`, `stemming/dictionaries/import:create`, `conversations/models:list`, `nl_search_models:list`, `debug:list`. Remove what it creates.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests, `live_actions` on 18108 and 30.0.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(app): scoped key generator, decoder and test`.

---

## PR B · Analytics and stemming (branch `feat/m7-analytics`, from `feat/m7-keys`)

### Task 6: Client and core for analytics

**Files:**
- Create: `crates/tessera-client/src/analytics.rs`, `crates/tessera-client/tests/analytics.rs`, `crates/tessera-core/src/analytics.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-client/tests/live_admin.rs`, `crates/tessera-core/src/lib.rs`, `dev/docker-compose.yml`, `.github/workflows/ci.yml`

**Interfaces:**

```rust
impl Client {
    pub async fn analytics_rules(&self) -> Result<Vec<Map<String, Value>>, Error>;           // GET /analytics/rules
    pub async fn analytics_rule(&self, name: &str) -> Result<Map<String, Value>, Error>;     // GET, unknown is Http 400
    pub async fn create_analytics_rule(&self, rule: &Value) -> Result<Map<String, Value>, Error>;   // POST
    pub async fn update_analytics_rule(&self, name: &str, rule: &Value) -> Result<Map<String, Value>, Error>; // PUT
    pub async fn delete_analytics_rule(&self, name: &str) -> Result<(), Error>;              // DELETE
    pub async fn analytics_event(&self, event: &Value) -> Result<(), Error>;                 // POST /analytics/events
    pub async fn flush_analytics(&self) -> Result<(), Error>;                                // POST /analytics/flush
}
```

Paths through `resource_path("analytics/rules", name)`. Core: design §5.4 verbatim. `event_types`: `PopularQueries`, `NohitsQueries`: `["search"]`. `Counter`: `["click","conversion","visit"]`. `Log`: `["click","conversion","visit","search"]`. `RuleType::as_str`: `popular_queries`, `nohits_queries`, `counter`, `log`.

`to_body` for `popular_queries`: `{"name","type","collection","event_type","params":{"destination_collection","limit","capture_search_requests","expand_query"[,"meta_fields"]}}`, plus `rule_tag` when not empty, plus `extra` and `extra_params`. For `counter`: `params` with `destination_collection`, `counter_field`, `weight`. For `log`: no `params` unless `extra_params` holds some. `from_rule` drops an empty `rule_tag`, and takes `params: null` or missing as empty.

`check_rule`: `BadName` through `name_ok`, `NoCollection`, `NoDestination` (query and counter rules), `BadLimit` (query rules, `limit` missing or 0), `NoCounterField`, `BadWeight` (counter rules, missing).

`test_event(rule, user_id, target)`: `{"name": rule.name, "event_type": rule.event_type, "data": {"user_id": user_id, "q"|"doc_id": target}}`, `q` for `search` events.

`dev/docker-compose.yml`, service `typesense` only: add `--enable-search-analytics=true --analytics-dir=/data/analytics --analytics-minute-rate-limit=1000` to `command`.

`ci.yml`, `integration` job: under the service `env`, add `TYPESENSE_ENABLE_SEARCH_ANALYTICS: "true"`, `TYPESENSE_ANALYTICS_DIR: /tmp/analytics`, `TYPESENSE_ANALYTICS_MINUTE_RATE_LIMIT: "1000"`; add `options: --add-host=host.docker.internal:host-gateway` to the service; add a step `cargo test -p tessera-client --test live_admin --locked -- --ignored` with `TESSERA_IT_URL`, `TESSERA_IT_ANALYTICS: "1"`, `TESSERA_IT_STUB_HOST: host.docker.internal`, and the same for `tessera-core`'s `live_scoped` and `live_actions`.

- [ ] **Step 1: Write the failing tests.** Wiremock in `tests/analytics.rs`: `rule_name_is_one_path_segment`, `create_posts_an_object`. Core: round trip of each type through `from_rule` and `to_body`, with `extra` kept; `update_body_has_no_name_or_type`; `null_params_read_as_empty`; `unknown_type_gives_none`; `check_rule` per problem; `test_event_uses_q_for_search`. Live, `#[ignore]`: `live_rule_crud` on a `tessera_it_` source and destination: create, duplicate is 400, list, get, `PUT` with `limit` changed reads back, changed type is 400, delete; `live_put_creates_a_missing_rule` documents the upsert; `live_event_is_accepted` sends one event; `live_flush_answers`; `live_aggregate_reaches_the_destination`, only with `TESSERA_IT_ANALYTICS`: send a `search` event with `q`, flush, poll the destination for up to 10 s.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the live tests on 18108, then on containers of 30.0 and 30.2 with the flags.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(core): analytics rules`.

### Task 7: Client and core for stemming

**Files:**
- Create: `crates/tessera-client/src/stemming.rs`, `crates/tessera-core/src/stemming.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-client/tests/live_admin.rs`, `crates/tessera-core/src/lib.rs`

**Interfaces:**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StemmingWord { pub word: String, pub root: String }
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StemmingDictionary { pub id: String, #[serde(default)] pub words: Vec<StemmingWord> }

impl Client {
    pub async fn stemming_dictionaries(&self) -> Result<Vec<String>, Error>;              // {"dictionaries":[…]}
    pub async fn stemming_dictionary(&self, id: &str) -> Result<StemmingDictionary, Error>;
    pub async fn import_stemming(&self, id: &str, jsonl: &str) -> Result<(), Error>;      // POST …/import?id=
    pub async fn delete_stemming(&self, id: &str) -> Result<(), Error>;
}
```

`import_stemming` follows `import_batch` in `documents.rs`: `self.without_bodies()`, the text body route with `Content-Type: text/plain`, `LONG_TIMEOUT`, query `encode_query(&[("id", id)])`. Non-2xx is `Error::http`. An empty `id` is refused with `addressable`.

Core: design §5.5 verbatim, using `StemmingWord` in place of `WordRoot`. `parse_dictionary` skips blank lines. Line numbers are 1-based. It collects at most 100 `BadLine`s. A value that is not a string is `LineProblem::NotText`. An empty or all-space `word` or `root` is `LineProblem::Empty`. Unknown keys are ignored, as on the server.

`to_jsonl` writes `{"word":…,"root":…}` through `serde_json`, one per line, without a final newline. `check_import`: `missing` holds sent words not in `stored` with the last sent root; `changed_root` counts words of `before` whose root differs in `stored`. `collections_using` compares `Field::attr("stem_dictionary")`.

- [ ] **Step 1: Write the failing tests.** Core: `parse_accepts_jsonl`, `parse_names_every_bad_line`, `parse_stops_collecting_at_100`, `empty_word_is_refused`, `numeric_word_is_refused`, `check_import_counts_a_changed_root`, `check_import_finds_a_missing_word`, `last_duplicate_wins_in_the_check`. Client live: `live_stemming_crud` imports two lines, reads, imports a line changing a root and adding a word, reads the merge, deletes, confirms the list; `live_bad_line_keeps_the_lines_before_it` documents the partial store; `live_delete_of_an_unknown_id_answers_200`; `live_odd_id_round_trips` with `a/b c`.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the live tests on 18108 and 30.0.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(core): stemming dictionaries`.

### Task 8: Analytics tab

**Files:**
- Create: `crates/tessera-app/src/views/resources/analytics.rs`, `crates/tessera-app/tests/ui_analytics.rs`
- Modify: `crates/tessera-app/src/admin.rs`, `state.rs`, `actions.rs`, `views/resources/mod.rs`, `views/root.rs`, `views/sidebar.rs`, `tests/common/mod.rs`, locales

**Backend additions:**

```rust
fn rules(&self, client: Client) -> BoxFuture<Result<Vec<Map<String, Value>>, Error>>;
/// `name` is the rule to edit, or `None` for a new one, which `POST`s. An edit reads the rule
/// first, so one that is gone fails with its message instead of being recreated by the `PUT`.
fn save_rule(&self, client: Client, name: Option<String>, body: Value) -> BoxFuture<Result<Map<String, Value>, Error>>;
fn delete_rule(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;
fn send_event(&self, client: Client, event: Value) -> BoxFuture<Result<(), Error>>;
fn flush(&self, client: Client) -> BoxFuture<Result<(), Error>>;
fn top_documents(&self, client: Client, collection: String, sort_by: String) -> BoxFuture<Result<String, Error>>;
```

`top_documents` is `client.search(collection, [("q","*"),("sort_by",sort_by),("per_page","100"),("enable_analytics","false")])`.

**Behaviour:** design §7.5. `ResourceKind::Analytics`, `shown` is `supported`, index 6 of `OPEN_RESOURCES`, `sidebar.analytics` "Analytics" / "Analytics".

- Columns: `("name", 180., false)`, `("type", 120., false)`, `("source", 140., false)`, `("event", 80., false)`, `("destination", 140., false)`, `("tag", 80., false)`.
- `notice("analytics.flags", t!("analytics.flags_notice"), cx)` above the list, always. Text in design §7.5. The docs link is `https://typesense.org/docs/30.2/api/analytics-query-suggestions.html`.
- The form uses `Select` for type, source, event, destination (as `aliases.rs` line 510 builds one from collection names, `.searchable(true)`). Type and name are disabled on edit.
- A rule where `RuleDraft::from_rule` is `None`: a read-only `JsonEditor` and Delete.
- Destination section as design §7.5: `sort_by` is `count:desc` for query rules, `<counter_field>:desc` for `counter`, none for `log`. `counter_field` passes `param_safe` first. Rows through a second `ResourceList` with prefix `analytics.top`, columns `q` and `count` for query rules, `id` and the counter field for counters.
- "Open in search": `state.update(cx, |s, cx| s.open_search(id, Some((vec![draft], false)), None, cx))`, with a `SearchDraft` for the source collection and `q`. Read `views/search/queries.rs` for how a `SearchDraft` is built. Query rules only.
- "Flush now" behind `ensure_writable`, then reload after `cx.background_executor().timer(Duration::from_secs(2))`.
- Test event per design §7.5. On prod, `confirm_write` with `typed: false`.
- Rate limit hint: when the error text contains `rate limit`, add `t!("analytics.rate_hint")`.

- [ ] **Step 1: Write the failing UI tests** in `ui_analytics.rs`: `notice_always_shows`; `form_follows_the_type` (fields per type); `new_posts_and_selects`; `bad_name_sends_nothing`; `edit_cannot_change_name_or_type`; `edit_of_a_vanished_rule_shows_the_message`; `unknown_type_is_json_only_and_deletable`; `destination_shows_counts_sorted`; `open_in_search_opens_the_tab`; `delete_cancel_sends_nothing`; `read_only_save_asks_to_unlock`; `test_event_sends_the_rules_event_type`; `rate_limit_shows_in_full_with_the_hint`; `flush_asks_to_unlock_when_locked`.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): analytics rules, destinations and test events`.

### Task 9: Stemming tab

**Files:**
- Create: `crates/tessera-app/src/views/resources/stemming.rs`, `crates/tessera-app/src/views/resources/stemming_dialog.rs`, `crates/tessera-app/tests/ui_stemming.rs`
- Modify: `crates/tessera-app/src/admin.rs`, `state.rs`, `actions.rs`, `views/resources/mod.rs`, `views/root.rs`, `views/sidebar.rs`, `tests/common/mod.rs`, locales

**Backend additions:**

```rust
fn dictionaries(&self, client: Client) -> BoxFuture<Result<Vec<String>, Error>>;
fn dictionary(&self, client: Client, id: String) -> BoxFuture<Result<StemmingDictionary, Error>>;
fn read_dictionary_file(&self, path: PathBuf) -> BoxFuture<Result<Vec<StemmingWord>, DictionaryFileError>>;
/// Reads the dictionary (a 404 means new), imports, reads again and compares.
fn import_dictionary(&self, client: Client, id: String, words: Vec<StemmingWord>) -> BoxFuture<Result<ImportCheck, Error>>;
fn delete_dictionary(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>>;
```

`DictionaryFileError`: `TooLarge(u64)`, `Read(String)`, `Lines(Vec<BadLine>)`. The size is checked before reading, as `read_set_file` does. Parsing runs on the backend.

**Behaviour:** design §7.8. `ResourceKind::Stemming`, `shown` is `supported`, index 7, `sidebar.stemming` "Stemming" / "Stemming".

- Left list prefix `stemming`, column `("id", 240., false)`. Right list prefix `stemming.words`, columns `("word", 200., false)`, `("root", 200., false)`. Selecting a dictionary loads it. Words render through `ResourceList::set_rows`, so its filter works. A dictionary of 100,000 words stays smooth, because the table is virtual.
- Import button in the toolbar, `NewResource`. The dialog: id prefilled with the selection, `prompt_for_paths` as `views/import_dialog.rs` line 67, a line with the file's word count, `BadLine`s listed with `t!(format!("stemming.line.{problem}"))`, at most 100, and the first ones visible.
- Import button stays disabled until the id passes `name_ok` or is the selected id, and a file parsed clean.
- With an existing id: `confirm_write` with `Danger { lines: [merge text], expected: id, typed: prod }` before sending.
- Result line: `stemming.imported` "`sent` words sent. `changed_root` existing words took a new root." plus `missing` as a warning naming the first 10.
- Delete: `collections_using(&cached_collections, id)`. `typed` when prod or the list is not empty. Lines: "These collections name this dictionary and keep working without its words: a, b."

- [ ] **Step 1: Write the failing UI tests** in `ui_stemming.rs`: `list_and_words_show`; `file_with_a_bad_line_sends_nothing_and_names_the_line`; `too_large_file_says_so`; `import_of_a_new_id_asks_nothing`; `import_into_an_existing_id_says_it_merges`; `cancel_sends_nothing`; `result_states_changed_roots`; `missing_word_warns`; `delete_names_collections`; `delete_is_typed_when_used`; `read_only_import_asks_to_unlock`; `bad_name_keeps_import_disabled`.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): stemming dictionaries`.

---

## PR C · AI (branch `feat/m7-ai`, from `feat/m7-analytics`)

### Task 10: Client and core for AI models

**Files:**
- Create: `crates/tessera-client/src/models_ai.rs`, `crates/tessera-core/src/ai.rs`, `crates/tessera-client/tests/ai.rs`, `crates/tessera-client/tests/live_ai.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-core/src/lib.rs`, `.github/workflows/ci.yml`

**Interfaces:** the client block of design §4.6 and the core block of §5.6, verbatim. `ModelKind::path()`: `conversations/models`, `nl_search_models`. `pub const AI_TIMEOUT: Duration = Duration::from_secs(120);`. Id paths through `resource_path(kind.path(), id)`.

- Every method but `ai_search` runs on `self.without_bodies()`. Create and update pass `Some(AI_TIMEOUT)` to `write`. `ai_search` builds its `GET` with `search_route`; a `SearchRoute::Post` is `Error::Decode` built with `custom("the query is too long")`, and the view refuses that case first. It then calls `self.call(None, Method::GET, &path, Some(&query), None, Some(AI_TIMEOUT))`, so a timeout is not retried.
- `create_collection_on` and `update_collection_on`: a helper `has_embed(&Value) -> bool` is true when any item of `fields` has the key `embed`. Then `self.without_bodies()` and `Some(LONG_TIMEOUT)`. Same for both pin and cluster calls, because both go through these functions.
- `endpoint_key`, `update_body`, `chat_params`, `nl_params`, `nl_outcome`, `set_embedding`, `scrub`: design §5.6. `ModelProblem`: `BadId`, `NoModelName`, `NoApiKey` (create only), `NoHistory`, `BadMaxBytes`, `BadTtl`, `BadTemperature`, `HoldsMask`. `query_fields`: fields with type `string`, `string[]` or `string*` and `index` true, plus `float[]` fields with `embed`; no `.*`.
- `update_body` compares with `stored` key by key, for `model_name`, the endpoint key, `account_id`, `system_prompt`, `max_bytes`, `ttl`, `temperature`, `history_collection`, then each key of `more`. It drops `id` and every `AI_SECRET_KEYS` member from `more`, and adds `api_key` only when the argument is not blank.
- `HoldsMask`: `redact::text_has_mask(&body.to_string())`, or a value under `AI_SECRET_KEYS` containing `***`.

**Live test setup** (`live_ai.rs`). A helper:

```rust
/// A stub LLM the server can reach, or `None` to skip. `TESSERA_IT_STUB_HOST` names this machine
/// as the server sees it, such as `host.docker.internal`.
async fn stub() -> Option<(MockServer, String)> {
    let host = std::env::var("TESSERA_IT_STUB_HOST").ok()?;
    let listener = std::net::TcpListener::bind("0.0.0.0:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    let server = MockServer::builder().listener(listener).start().await;
    // POST /v1/chat/completions → {"choices":[{"message":{"role":"assistant","content":<text>}}]}
    // POST /v1/embeddings → {"data":[{"embedding":[0.1,0.2,0.3,0.4],"index":0}]}
    Some((server, format!("http://{host}:{port}")))
}
```

The NL stub answers content `{"q":"*","filter_by":"o:=1","sort_by":"o:desc"}`, the chat stub `stub answer`. A skipped test prints `skipped: TESSERA_IT_STUB_HOST is not set` and returns.

- [ ] **Step 1: Write the failing tests.** Wiremock in `tests/ai.rs`: `model_events_hold_no_bodies`; `ai_search_is_not_retried_after_a_timeout`; `embed_schema_event_holds_no_body`; `plain_schema_event_keeps_its_body`. Core: `update_body_omits_what_did_not_change`; `update_body_never_echoes_a_mask` with `api_key: "sk-st*********"` in `stored`; `blank_key_keeps_the_stored_one`; `typed_key_is_sent`; `more_with_api_key_is_dropped`; `mask_in_a_new_value_is_refused`; `create_needs_a_key`; `endpoint_key_per_namespace`; `scrub_removes_the_key`, also with an empty secret, which must change nothing; `query_fields_skip_plain_vectors`; `chat_answer_reads_the_turns`; `nl_outcome_parsed`, `nl_outcome_error`, `nl_outcome_missing_is_fell_back`; `set_embedding_keeps_unknown_keys_and_clears_num_dim`. Fixtures are the answers quoted in design §4.6. Live without a stub: `live_lists_answer`, `live_unknown_model_is_404`, `live_put_of_unknown_is_404`, `live_validation_messages` for missing `max_bytes` and missing `history_collection`. Live with the stub: `live_conversation_crud`; `live_put_keeps_what_it_does_not_name`; `live_put_with_a_mask_stores_it` documents the trap on a throwaway model; `live_chat_writes_history_and_answers`; `live_follow_up_keeps_the_conversation`; `live_nl_crud_and_parse`; `live_unknown_nl_model_falls_back`; `live_remote_embedding_field_embeds_a_document`. Built-in, `#[ignore = "downloads a model"]` under its own name: `live_builtin_model`.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test ai` and `cargo test -p tessera-core ai::`. Expected: fails.
- [ ] **Step 3: Implement.** Add `live_ai` to the `integration` job in `ci.yml`, skipping the built-in test with `--skip live_builtin_model`, and to `nightly.yml` without the skip.
- [ ] **Step 4: Run** the live tests with `TESSERA_IT_STUB_HOST=host.docker.internal` on containers of 30.0 and 30.2 started with `--add-host=host.docker.internal:host-gateway`, as in the constraints.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(core): conversation and NL search models`.

### Task 11: AI tab: lists and forms

**Files:**
- Create: `crates/tessera-app/src/views/resources/ai.rs`, `crates/tessera-app/src/views/resources/ai_form.rs`, `crates/tessera-app/tests/ui_ai.rs`
- Modify: `crates/tessera-app/src/admin.rs`, `state.rs`, `actions.rs`, `views/resources/mod.rs`, `views/root.rs`, `views/sidebar.rs`, `tests/common/mod.rs`, locales

**Backend additions:**

```rust
fn ai_models(&self, client: Client, kind: ModelKind) -> BoxFuture<Result<Vec<Map<String, Value>>, Error>>;
fn create_ai_model(&self, client: Client, kind: ModelKind, body: Value) -> BoxFuture<Result<Map<String, Value>, Error>>;
fn update_ai_model(&self, client: Client, kind: ModelKind, id: String, body: Value) -> BoxFuture<Result<Map<String, Value>, Error>>;
fn delete_ai_model(&self, client: Client, kind: ModelKind, id: String) -> BoxFuture<Result<(), Error>>;
fn ai_search(&self, client: Client, collection: String, params: Vec<(String, String)>) -> BoxFuture<Result<String, Error>>;
```

**Behaviour:** design §7.6. `ResourceKind::Ai`, `shown` is `supported`, index 8, `sidebar.ai` "AI" / "KI".

- `AiView` holds two `ModelsView`s, switching with `ShowDocuments` and `ShowSchema`; labels `ai.conversation` "Conversation models", `ai.nl` "NL search models".
- Columns: conversation `("id", 200., false)`, `("model", 220., false)`, `("history", 160., false)`; NL without `history`.
- `ai_form.rs` is `ModelForm`, one entity per `ModelsView`. It holds the `InputState`s, and the API key as `InputState::new(window, cx).masked(true)` with the eye button. On selection change and after every save: `key.update(cx, |k, cx| k.set_value("", window, cx))`.
- Placeholder on edit: `t!("ai.key_keep")` "Set on the server. Leave empty to keep it.". On New: `t!("ai.key_new")` "Sent to the server. Tessera does not keep it.".
- The form never shows the stored key, not even the mask. "More settings" shows `stored` minus the keys the form has and minus `AI_SECRET_KEYS`.
- Model name `CompletionInput`, `set_candidates` per kind: conversation `openai/gpt-4o`, `azure/`, `google/gemini-2.5-flash`, `cloudflare/@cf/mistral/mistral-7b-instruct-v0.1`, `vllm/`; NL `openai/gpt-4.1`, `azure/`, `google/gemini-2.5-flash`, `gcp/gemini-2.5-flash`, `cloudflare/`, `vllm/`.
- Save: `create_body` or `update_body`, each `ModelProblem` inline, `update_body` empty means "Nothing changed" and sends nothing. Then `ensure_writable`, detached. The typed key travels in the `Value` to the backend and is not stored in the view after the send starts: clear the input at send, and keep a `SecretString` copy only until the answer, for `scrub`.
- Every error: `scrub(&error_text(&err, profile), typed_key)`.
- A create answers with the server's masked model: replace the list row with it, select it.
- Delete as design §7.6, with the history note on conversations.
- `empty_state("ai.conversation", "https://typesense.org/docs/30.2/api/conversational-search-rag.html")`, and for NL `https://typesense.org/docs/30.2/api/natural-language-search.html`. No model yet means the empty state says a provider key is needed, and that the server calls the provider at create.

- [ ] **Step 1: Write the failing UI tests** in `ui_ai.rs`: `tabs_switch_with_their_shortcuts`; `key_input_is_masked`; `edit_starts_with_an_empty_key`; `stored_mask_is_never_shown`; `save_without_a_key_sends_no_api_key`; `save_with_a_key_sends_it_and_clears_the_field`; `selection_change_clears_the_key`; `nothing_changed_sends_nothing`; `provider_error_is_scrubbed`; `more_settings_hides_secret_keys`; `history_select_lists_collections`; `endpoint_field_follows_the_namespace`; `delete_says_history_stays`; `read_only_save_asks_to_unlock`; `create_with_no_key_is_refused_inline`.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): conversation and NL search models`.

### Task 12: AI test panels

**Files:**
- Create: `crates/tessera-app/src/views/resources/ai_test.rs`, `crates/tessera-app/tests/ui_ai_test.rs`
- Modify: `views/resources/ai.rs`, `views/resources/mod.rs`, locales

**Behaviour:** design §7.6.

- `AiTestPanel::new(kind, connection, window, cx)` sits under the form of a saved model and takes the model id on `set_model`. It starts with `test_panel.rs` as its pattern for the collection `Select` and `HitsView`.
- `query_by`: an input defaulting to `query_fields(collection).join(",")` on a collection pick. Before Run, each comma-separated name must be in `query_fields`, else inline `t!("ai.bad_query_by", name = name)`.
- Question: a multi-line input. Empty: Run disabled. A `search_route(collection, params)` that is `SearchRoute::Post` is refused inline with `t!("ai.too_long")`.
- `exclude_fields`: `tessera_core::documents::columns::exclude_fields(&collection)`, so an embedding neither reaches the LLM prompt nor the panel. Note that `exclude_fields` also shapes what the model sees.
- Conversation Run: `connection.update(cx, |c, cx| c.ensure_writable(window, cx, run))`, through `RunTest`. The panel says `t!("ai.chat_note")`: "Calls the provider and stores this turn in the history collection." `conversation_id` is kept on the panel. "New conversation" clears it and the turns. A panel-level `Vec<(ChatRole, String)>` shows `chat_answer(..).turns`; `found` and the sources through `HitsView::set_result`.
- The run is `cx.spawn(...).detach()` and writes its result through a `WeakEntity`: a closed tab lets the turn finish and drops the answer. A second Run while one is running stays disabled.
- NL Run: no `ensure_writable`, a button only. Shows per `nl_outcome`: `Parsed` a three-row table of `q`, `filter_by`, `sort_by` for generated and augmented, parse time, raw LLM text in a read-only `JsonEditor` with `plain` mode; `Failed` the message in the error slot; `FellBack` the warning `t!("ai.fell_back")`: "The model did not run. These hits are a plain search of the sentence." All with the hits.
- A timeout shows `error_text`, with the note that the server may still be calling the provider.

- [ ] **Step 1: Write the failing UI tests** in `ui_ai_test.rs`: `chat_needs_unlock_on_a_read_only_profile`; `chat_shows_answer_turns_and_sources`; `follow_up_sends_the_conversation_id`; `new_conversation_clears_it`; `plain_vector_field_is_refused_and_sends_nothing`; `long_question_is_refused_and_sends_nothing`; `nl_runs_on_a_read_only_profile`; `nl_shows_generated_and_augmented`; `nl_fall_back_warns`; `nl_error_shows_inline`; `params_exclude_vector_fields`; `params_disable_analytics`; `closing_the_tab_does_not_panic_on_a_late_answer`.
- [ ] **Step 2: Run** the tests. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): AI test panels`.

### Task 13: Embedding fields, seed, docs and the full check on both versions

**Files:**
- Modify: `crates/tessera-app/src/views/schema_dialog/form.rs`, `views/schema_dialog/mod.rs`, `crates/tessera-app/tests/ui_schema_dialog.rs`, `xtask/src/seed.rs`, `docs/architecture/async.md`, `docs/building.md`, `docs/spec.md` (§14 acceptance note only if something changed), `README.md` (feature list, if it has one), `CHANGELOG.md`, locales

**Embedding form:** design §7.7.

- A `float[]` row without `num_dim` issues gains `Checkbox::new("schema.field.{i}.embed")`. Checked: `FieldDraft.embed` becomes `{"from":[],"model_config":{"model_name":""}}`. Unchecked: `None`, after asking if it held anything.
- The block: source fields (`CompletionInput`, candidates the draft's `string` and `string[]` fields), model (`CompletionInput`, `BUILT_IN_MODELS` plus the four provider examples), and for a model not starting with `ts/`, a masked API key input and an endpoint input (`url`).
- On each change `set_embedding`, then the existing `sync` path to the JSON tab. The API key goes in only when not blank. The JSON tab masks it, as it does today. Read `views/schema_dialog/sync.rs` before touching this.
- `schema.embed.model_note` under a `ts/` model: "The server downloads the model on first use. This can take minutes."
- Edit mode: the block appears on a field being added. A stored embedding keeps the existing `schema.field.secret_hint` line and no controls.
- `validate.rs` stays: a type other than `float[]` and an empty `from` are its issues. Add to the form's problems: a non-`ts/` model with a blank key on create is a warning, not an error, because an `openai_url`-style server may not need one. Keep `blanked` as it is.

**Seed:** add a search-only key described `tessera-seed-search`, removed by `--reset`, printing only its id and prefix. Add a `popular_queries` rule `books_popular` from `books` to a new collection `books_queries` (`q` string, `count` int32), removed by `--reset`. Add a stemming dictionary `seed_plurals` with three lines. `§12.2` lists keys.

**Docs:**
- `async.md`: one paragraph on `AI_TIMEOUT` and detached AI calls.
- `building.md`: a section with the on-screen checks for M7: key dialog in light and dark, the matrix's scrolling, the reveal, Esc on it, the date picker in a dialog, scoped panel, analytics view with a server with flags (`docker compose` recreate on `TYPESENSE_PORT=18108`), stemming import, one real provider for each of conversation, NL and embedding if a key is at hand, the list of what the stub cannot prove from design §8.3.

**Full run:** `live_admin`, `live_ai`, `live_scoped`, `live_actions`, `live`, `live_relevance`, `live_transfer` against 18108 and against fresh containers of 30.0 and 30.2 with flags, `cargo xtask seed --url http://localhost:18108 --key xyz --reset` twice, then stop the containers.

- [ ] **Step 1: Write the failing UI tests** in `ui_schema_dialog.rs`: `auto_embed_writes_the_embed_json`; `model_field_offers_built_ins`; `key_field_is_masked_and_hidden_for_ts_models`; `json_tab_shows_the_mask_not_the_key`; `form_and_json_stay_in_sync_on_embed`; `clone_still_blanks_the_key`; `stored_embedding_has_no_controls_in_edit_mode`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_schema_dialog`. Expected: fails.
- [ ] **Step 3: Implement** the form.
- [ ] **Step 4: Write the seed changes** and run the seed twice: the second run must pass too.
- [ ] **Step 5: Write the docs.**
- [ ] **Step 6: Run** the live tests on both versions. Record the result in the PR description.
- [ ] **Step 7: Run** the checks, CHANGELOG, commit. `feat(app): embedding fields in the schema form` and `docs: M7 keys, analytics, AI`.
