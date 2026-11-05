# M2 Collections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A collection list and schema view per connection, create (form and JSON) and clone, schema changes with a diff preview and progress, truncate and delete with type-to-confirm, and the read-only unlock.

**Architecture:** `tessera-client` gains the collection endpoints, request bodies, a long timeout for slow operations and a shared read-only flag. `tessera-core` gains the schema draft (form model with JSON round trip), validation and the schema diff. The new `tessera-widgets` crate holds the JSON editor and the confirmation dialogs. `tessera-app` adds a collections tab and a collection tab per connection, the create/edit dialog and the unlock flow, behind a `CollectionsBackend` global that UI tests replace. Three PRs, in order: A (Tasks 1–7), B (Tasks 8–14), C (Tasks 15–18).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11, feature `tree-sitter`), reqwest 0.13.5, tokio 1.54.1, serde_json 1.0.154, wiremock 0.6.5, insta.

**Spec:** [`docs/superpowers/specs/2026-11-05-m2-collections-design.md`](../specs/2026-11-05-m2-collections-design.md). Section numbers below (§4.2 …) refer to it.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI. Direction: `app → widgets → core → client`.
- Direct dependencies are pinned exactly (`=x.y.z`) in the workspace `Cargo.toml`. Confirm a new one with `cargo search <crate> --limit 1`.
- Never guess a GPUI or GPUI Kit API. Read the pinned source under `~/.cargo/registry/src/*/gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0` and `gpui-pre-0.3.11` first. The code editor is `gpui_base::input::EditorState` (re-exported by `gpui-component`), built with `EditorState::new(window, cx).language("json")`.
- Every UI string goes through `t!()`, same keys in `assets/locales/en.yml` and `de.yml`. The key-parity test stays green.
- No blocking I/O on the GPUI thread. Network calls go through the tokio bridge (`runtime::spawn`, `AbortOnDrop`).
- No key in logs, files, errors, `Debug` or `Display`. Redaction tests are never removed.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. No commit with a failing check.
- Commits: `git commit -s`, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense: `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, key `xyz`, live tests with `TESSERA_IT_URL=http://localhost:18108 TESSERA_IT_KEY=xyz cargo test -- --ignored`. Never send anything to port 8108.
- UX rules: environment visible everywhere; errors inline and in full; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown on the control via `Kbd`.
- Shortcuts are bound in a key context of the view that owns them (`COLLECTIONS`, `COLLECTION`, `SCHEMA_DIALOG`), never globally, so ⌘D / ⌘E do not collide with M1's sidebar bindings.
- NF5: the Linux release binary stays below 40 MB (41,943,040 bytes). Each PR records the size in its description.

## Review Focus

1. A collection whose field has a type this Tessera does not know (for example `"int8[]"`) or an unknown attribute must open in clone and edit, and an unchanged round trip must send it back exactly. Test in Task 9.
2. Creating a collection whose name exists answers 409. The dialog must keep the draft and show the server message inline, not close. Test in Task 13.
3. JSON with a trailing comma must show the error with its line, lock the form and disable Create; nothing is sent. Test in Task 13.
4. The type-to-confirm text must match exactly: `Books`, `books ` and ` books` do not enable Delete for `books`. Test in Task 11.
5. A PATCH that takes longer than the profile timeout (5 s) must still succeed, because it uses the long timeout. Test in Task 8.

---

## PR A · Browse (branch `feat/m2-collections`, from `main`)

### Task 1: Collection read endpoints

**Files:**
- Create: `crates/tessera-client/src/collections.rs` (models and endpoints)
- Modify: `crates/tessera-client/src/lib.rs` (`mod collections; pub use collections::{Collection, Field, Alias, SchemaChange};`), `crates/tessera-client/src/send.rs` only if the path needs a pre-encoded variant
- Test: `crates/tessera-client/tests/collections.rs` (wiremock), `crates/tessera-client/tests/live.rs` (add `#[ignore]` tests)

**Interfaces:**
- Produces:
  - `Collection` and `Field` as in §4.1. Both `Deserialize, Serialize, Clone, Debug, PartialEq`. `Collection` uses `#[serde(default)]` on every field except `name`, and `#[serde(flatten)] extra: Map<String, Value>`. `Field { name: String, #[serde(rename = "type")] r#type: String, #[serde(flatten)] attributes: Map<String, Value> }`.
  - `Field::flag(&self, name: &str) -> bool` with server defaults (`index` and `store` true, everything else false); `Field::attr(&self, name: &str) -> Option<&Value>`.
  - `Alias { name: String, collection_name: String }`, `SchemaChange { collection: String, validated_docs: u64, altered_docs: u64 }` (`#[serde(default)]` on the counts).
  - `Client::collections(&self) -> Result<Vec<Collection>, Error>`, `Client::collection(&self, name: &str) -> Result<Collection, Error>`, `Client::aliases(&self) -> Result<Vec<Alias>, Error>`; same on `NodeClient` through the existing `*_on(pin)` pattern.
  - `pub(crate) fn collection_path(name: &str, rest: &str) -> String` that yields `collections/{encoded}{rest}`, encoding the name as one path segment (`url::form_urlencoded` is wrong here; use `percent_encoding::utf8_percent_encode` with the `PATH_SEGMENT` set from the `url` crate's re-export, or the `percent-encoding` crate pinned at the version already in `Cargo.lock`).

- [ ] **Step 1: Write the failing wiremock tests** in `tests/collections.rs`:
  - `collections_decodes_full_schemas`: `GET /collections` answers the 30.2 body captured in the design (fields with `truncate_len`, `curation_sets`, `metadata`); assert `num_documents`, `fields.len()`, `synonym_sets`, and `extra.is_empty()`.
  - `unknown_attributes_survive`: a field with `"future_flag": true` and a collection with `"future_option": 1`; assert `field.attr("future_flag") == Some(&json!(true))` and `collection.extra["future_option"] == json!(1)`.
  - `name_is_one_path_segment`: `collection("a/b c?x")` requests path `/collections/a%2Fb%20c%3Fx`.
  - `aliases_unwraps_the_list`: `{"aliases":[{"name":"cur","collection_name":"books"}]}` → one `Alias`.
  - `missing_collection_is_http_404`: 404 with `{"message":"Not Found"}` → `Error::Http { status: 404, .. }` with `typesense_message() == Some("Not Found")`.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test collections`. Expected: compile errors.
- [ ] **Step 3: Implement `collections.rs`** and the three endpoints with `get_ok`.
- [ ] **Step 4: Live tests** in `tests/live.rs` (`#[ignore]`): create a uniquely named collection with plain `reqwest` in the test setup (the client's `create_collection` arrives in Task 8), then `collection(name)` and `collections()` find it and `aliases()` returns a `Vec`. Clean up in the test. Run them against the dev server on 18108.
- [ ] **Step 5: Run** `cargo test -p tessera-client` and the live tests. Expected: PASS.
- [ ] **Step 6: Commit.** `feat(client): read collections and aliases`.

### Task 2: Redaction of embedding secrets

**Files:**
- Modify: `crates/tessera-core/src/redact.rs`
- Test: inline tests and the existing redaction fixtures

**Interfaces:**
- Produces: `SECRET_KEYS` = `["api_key", "value", "access_token", "refresh_token", "client_secret"]`; `pub fn mask_value(v: &mut Value)` made public (the schema view masks a `Value` before display).

- [ ] **Step 1: Write the failing test** `embedding_credentials_are_masked`: a collection JSON with `fields[0].embed.model_config` holding all four secrets; after `json()` none of the four secret strings occur and each key maps to `"••••"`; `model_name` is unchanged. Add the same body to the curl snapshot test so "Copy as curl" masks it too.
- [ ] **Step 2: Run** `cargo test -p tessera-core redact`. Expected: FAIL.
- [ ] **Step 3: Extend `SECRET_KEYS`** and make `mask_value` public with a doc comment. Update `mask_text` for non-JSON bodies so it covers the new keys.
- [ ] **Step 4: Run** `cargo test -p tessera-core`. Expected: PASS (update the insta snapshot with `cargo insta review` only for the new case).
- [ ] **Step 5: Commit.** `fix(core): mask embedding model credentials`.

### Task 3: tessera-widgets with JsonEditor and Confirm

**Files:**
- Create: `crates/tessera-widgets/Cargo.toml` (deps: `gpui-kit` with feature `tree-sitter`, `rust-i18n`, `serde_json`; `[lints] workspace = true`), `crates/tessera-widgets/src/lib.rs`, `src/json_editor.rs`, `src/confirm.rs`
- Modify: workspace `Cargo.toml` (`tessera-widgets = { path = "crates/tessera-widgets" }`), `crates/tessera-app/Cargo.toml` (depend on it), move `crates/tessera-app/src/views/confirm.rs` → `crates/tessera-widgets/src/confirm.rs` and update its callers
- Test: `crates/tessera-widgets/tests/json_editor.rs` (`#[gpui_kit::test]`)

**Interfaces:**
- Produces:
  - `JsonEditor` (an entity): `JsonEditor::new(text: &str, read_only: bool, window: &mut Window, cx: &mut Context<Self>) -> Self`; `fn text(&self, cx: &App) -> String`; `fn set_text(&mut self, text: &str, window, cx)`; `fn error(&self) -> Option<&JsonError>`; emits `JsonEditorEvent::Changed` on every edit. `JsonError { line: usize, column: usize, message: String }`, from `serde_json::Error::line()/column()`. Test ids `json.editor` and `json.error`.
  - `tessera_widgets::confirm::open(...)` with M1's exact signature (moved, not changed).
  - i18n: the widgets crate calls `rust_i18n::i18n!("../../assets/locales", fallback = "en")` itself, since `t!()` only works in the crate that declares it.

- [ ] **Step 1: Write the failing UI tests:**
  - `shows_parse_error_with_line`: text `"{\n  \"a\": 1,\n}"` → `error()` has `line == 3` and the element `json.error` contains "3".
  - `valid_json_has_no_error`.
  - `read_only_ignores_typing`: typing into a read-only editor leaves `text()` unchanged.
- [ ] **Step 2: Run** `cargo test -p tessera-widgets`. Expected: compile errors.
- [ ] **Step 3: Implement** `JsonEditor` over `EditorState` (language `json`, line numbers on, folding on), parsing on every change event. Move `confirm.rs`; keep its tests green from the app.
- [ ] **Step 4: Measure NF5.** `cargo build --release -p tessera-app` on macOS and record the size; the Linux size is read from the CI build job of this PR. If Linux exceeds 41,943,040 bytes, drop the `tree-sitter` feature (§13) and note it in the PR.
- [ ] **Step 5: Run** the checks. Expected: PASS.
- [ ] **Step 6: Commit.** `feat(widgets): add the widgets crate with a JSON editor`.

### Task 4: Collections backend, cache and favorites

**Files:**
- Create: `crates/tessera-app/src/collections.rs` (backend seam and cache)
- Modify: `crates/tessera-core/src/profile.rs` (`favorite_collections`), `crates/tessera-app/src/connection.rs` (`CollectionsCache` on `ConnectionState`), `crates/tessera-app/src/lib.rs` (install `tokio_backend`), `crates/tessera-app/src/state.rs` (`toggle_favorite`)
- Test: inline tests in `collections.rs` and `state.rs`; `crates/tessera-app/tests/common/mod.rs` gains `fake_backend`

**Interfaces:**
- Consumes: Task 1 endpoints.
- Produces:
  - `Profile.favorite_collections: Vec<String>` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`; an M1 `connections.json` still loads (test).
  - `trait CollectionsBackend: Send + Sync` with `fn list(&self, client: Client) -> BoxFuture<Result<(Vec<Collection>, Vec<Alias>), Error>>` and `fn get(&self, client: Client, name: String) -> BoxFuture<Result<Collection, Error>>`. PR B and C add the write methods. `struct CollectionsBackendGlobal(pub Arc<dyn CollectionsBackend>)`; `fn tokio_backend(handle: Handle) -> Arc<dyn CollectionsBackend>` loads collections and aliases concurrently.
  - `struct CollectionsCache { pub collections: Option<Vec<Collection>>, pub aliases: Vec<Alias>, pub loaded_at: Option<Instant>, pub error: Option<SharedString>, pub loading: bool }`.
  - `ConnectionState::load_collection_list(&mut self, cx)`, `ConnectionState::invalidate_collections(&mut self, cx)`, field `pub collection_cache: CollectionsCache`.
  - `AppState::toggle_favorite(&mut self, profile: Uuid, collection: &str, cx)`: updates the profile, saves (temporary connections: memory only).
  - `fn aliases_of<'a>(aliases: &'a [Alias], collection: &str) -> impl Iterator<Item = &'a str>`.

- [ ] **Step 1: Write the failing tests:** `m1_file_loads_without_favorites` (core); `favorite_is_saved` and `temporary_favorite_is_not_saved` (state, with a temp store path); `cache_load_fills_list_and_aliases` and `cache_error_is_kept_inline` (fake backend); `invalidate_clears_and_reloads`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** The overview keeps its `CollectionsFetcher`; do not merge the two in M2.
- [ ] **Step 4: Run** the checks. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): load collections through a backend seam`.

### Task 5: Collections tab and sidebar entries

**Files:**
- Create: `crates/tessera-app/src/views/collections_list.rs`, `crates/tessera-app/tests/ui_collections.rs`
- Modify: `views/dock.rs` (`CollectionsPanel`), `views/root.rs` (open/focus tabs; an `OpenCollections(Uuid)` and `OpenCollection(Uuid, String)` event on `AppState`), `views/sidebar.rs` (Collections entry and favorites under it), `actions.rs` (`NewCollection`, `ToggleFavorite`, `FocusSearch`; key context `COLLECTIONS`), locales

**Interfaces:**
- Consumes: Task 4 cache and favorites.
- Produces:
  - `CollectionsPanel::new(connection: Entity<ConnectionState>, state: Entity<AppState>, window, cx)`; panel name `"Collections"`; tab title like the overview's (dot, "Collections", environment badge), test id `tab.{profile_id}.collections`.
  - Row test ids `collections.row.{name}`; search `collections.search`; header cells `collections.col.{key}`; empty state `collections.empty`; error `collections.error`.
  - `fn sort_rows(rows: &mut [Row], key: SortKey, ascending: bool, favorites: &[String])` with favorites always first (unit tested).
  - Sidebar ids: `sidebar.{profile_id}.collections`, `sidebar.{profile_id}.collection.{name}`.

- [ ] **Step 1: Write the failing UI tests** (fake backend with `books` 10,000 docs, alias `books_current`, and `authors`): `list_shows_counts_and_aliases`; `search_filters_by_name_and_alias` (typing `current` leaves only `books`); `favorites_come_first_and_show_in_sidebar` (⌘D on the focused `authors` row); `enter_opens_collection_tab` (asserts the event; Task 6 renders the tab); `empty_cluster_shows_explanation`; `load_error_shows_inline_in_full`; `opening_twice_focuses_the_existing_tab`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_collections`. Expected: FAIL.
- [ ] **Step 3: Implement** with GPUI Kit `Table` as in `views/stats_table.rs` and `request_log.rs`. Counts use the locale's grouping (`num-format` is not a dependency: group with a small helper and test it). The created column formats `created_at` with `jiff` in local time. Write actions are not rendered yet; "New collection" appears in PR B (UX rule 4).
- [ ] **Step 4: Run** the checks. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): list collections per connection`.

### Task 6: Collection tab with the schema view

**Files:**
- Create: `crates/tessera-app/src/views/schema_view.rs`
- Modify: `views/dock.rs` (`CollectionPanel`), `views/root.rs`, locales
- Test: `crates/tessera-app/tests/ui_collections.rs`

**Interfaces:**
- Consumes: `JsonEditor` (Task 3), `redact::mask_value` (Task 2), `CollectionsBackend::get` (Task 4).
- Produces:
  - `CollectionPanel::new(connection, name: String, window, cx)`; panel name `"Collection"`; title = collection name with the connection's dot and badge; test id `tab.{profile_id}.collection.{name}`. It loads with `get` on open and on ⌘R.
  - `const KNOWN_ATTRIBUTES: [&str; 14]` in the order of §7.4; `fn attribute_columns(fields: &[Field]) -> Vec<String>` = known ones, then unknown ones in first-seen order (unit tested).
  - Test ids: `schema.field.{name}`, `schema.cell.{field}.{attr}`, `schema.options`, `schema.json`, `schema.copy_json`.

- [ ] **Step 1: Write the failing tests:** `attribute_columns_append_unknown`; UI `schema_shows_fields_and_options`; `unknown_attribute_gets_a_column`; `embedding_key_is_masked_in_table_and_json` (a field with `embed.model_config.api_key = "sk-embed-1234"`; neither `schema.cell…` nor `json.editor` text contains it); `copy_json_masks_secrets`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** Masking happens once on load, on a clone of the `Value`; the masked value feeds both the table and the JSON view.
- [ ] **Step 4: Run** the checks. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): show a collection's schema`.

### Task 7: Seed data, CI and PR A docs

**Files:**
- Create: `xtask/Cargo.toml` (`publish = false`, deps `reqwest` with `blocking` only if already in the lock graph, else use `tokio` + async `reqwest`; `serde_json`), `xtask/src/main.rs`, `xtask/src/seed.rs`, `.cargo/config.toml` (`[alias] xtask = "run -p xtask --"`)
- Modify: workspace `members` (add `xtask`), `.github/workflows/ci.yml` (run `cargo xtask seed --url http://localhost:8108` against the service container in the integration job, then the live tests), `docs/building.md`, `CHANGELOG.md`, `README.md` if it lists features

**Interfaces:**
- Produces: `cargo xtask seed --url <url> [--key <key>] [--reset]`; exit code 2 with usage when `--url` is missing; refusal (exit 1, names the collection) when a seed collection exists without `--reset`. `fn books(n: usize) -> impl Iterator<Item = Value>` etc., generated from a fixed `u64` seed with a small xorshift (no `rand` dependency).

- [ ] **Step 1: Write the failing tests** (unit, in `seed.rs`): `same_seed_same_documents`; `books_ref_points_at_existing_authors`; `places_are_valid_geopoints` (lat in −90..90, lng in −180..180); `args_without_url_are_an_error`.
- [ ] **Step 2: Run** `cargo test -p xtask`. Expected: FAIL.
- [ ] **Step 3: Implement** the five sets of §8, the alias `books_current` and the synonym set `books_synonyms`, imports in batches of 1,000 with `action=create`, and a final per-collection count check.
- [ ] **Step 4: Run** `cargo xtask seed --url http://localhost:18108 --reset` against the dev server; open the app and check the list by hand. Expected: five collections with the §8 counts.
- [ ] **Step 5: CHANGELOG** (Added: collection list, schema view, favorites, `cargo xtask seed`, `tessera-widgets`; Fixed: embedding credential masking) and docs.
- [ ] **Step 6: Run** the checks. Commit `build: add cargo xtask seed` and `docs: document collections browsing`. Open PR A.

## PR B · Create (branch `feat/m2-create`, from `feat/m2-collections`)

### Task 8: Write endpoints, request bodies, long timeout, shared read-only flag

**Files:**
- Modify: `crates/tessera-client/src/send.rs` (body and timeout), `lib.rs` (`read_only: Arc<AtomicBool>`, `set_read_only`, `is_read_only`), `collections.rs`
- Test: `tests/collections.rs`, `tests/client.rs` (guard), `tests/live.rs`

**Interfaces:**
- Produces:
  - `pub const LONG_TIMEOUT: Duration = Duration::from_secs(30 * 60);`
  - `Client::set_read_only(&self, read_only: bool)`, `Client::is_read_only(&self) -> bool` (shared by clones).
  - `Client::create_collection(&self, schema: &Value) -> Result<Collection, Error>` (POST, 201), `update_collection(&self, name: &str, patch: &Value) -> Result<(), Error>` (PATCH, `LONG_TIMEOUT`), `delete_collection(&self, name: &str) -> Result<(), Error>` (DELETE, `LONG_TIMEOUT`), `truncate_collection(&self, name: &str) -> Result<u64, Error>` (DELETE `…/documents?truncate=true`, `LONG_TIMEOUT`), `schema_changes(&self) -> Result<Vec<SchemaChange>, Error>`; the same on `NodeClient`.
  - `send(node, method, path, query, body: Option<&Value>, timeout: Option<Duration>)`: the body is serialized once, sent with `Content-Type: application/json`, and recorded in the `RequestEvent` as today.

- [ ] **Step 1: Write the failing tests:** `create_sends_the_body_verbatim` (wiremock `body_json` matcher); `patch_outlives_the_profile_timeout` (client timeout 1 s, mock delays 2 s, PATCH succeeds; a GET with the same delay times out); `truncate_returns_num_deleted`; `post_and_patch_are_never_retried` (refused connection, one event each); `unlock_then_lock` (§4.4: read-only client, write refused with no event; `set_read_only(false)` on a clone, write passes; `set_read_only(true)`, refused again); live `collection_lifecycle` (create → PATCH add/drop/change → `schema_changes` → truncate → delete; 409 on a duplicate create and 400 on `{"name":"x","fields":[]}` carry the server message).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** The M1 guard test stays unchanged and green.
- [ ] **Step 4: Run** unit and live tests. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(client): create, change, truncate and delete collections`.

### Task 9: SchemaDraft

**Files:**
- Create: `crates/tessera-core/src/schema.rs` (`pub mod schema;` in `lib.rs`)
- Test: inline tests

**Interfaces:**
- Consumes: `tessera_client::Collection`.
- Produces: `SchemaDraft`, `FieldDraft`, `FieldType` (§5.1) with `FieldType::ALL: [FieldType; 18]`, `FieldType::as_str()`, `FieldType::parse(&str) -> FieldType` (unknown → `Other`); `FieldDraft::new(name, FieldType) -> FieldDraft` with server defaults (`index` true, `store` true, `sort` `None`); `SchemaDraft::to_json(&self) -> Value`; `SchemaDraft::from_json(text: &str) -> Result<SchemaDraft, DraftError>`; `SchemaDraft::from_value(&Value) -> Result<SchemaDraft, DraftError>`; `SchemaDraft::from_collection(&Collection) -> SchemaDraft`; `SchemaDraft::blank_secrets(&mut self) -> usize` (count of blanked embedding credentials); `DraftError { line: Option<usize>, column: Option<usize>, message: String }`.

- [ ] **Step 1: Write the failing tests:**
  - `form_and_json_are_equal` — the §1 AC: at least five schemas (plain strings and facets; numbers with `default_sorting_field`; `string[]` with `optional`; `float[]` with `num_dim` and `vec_dist`; `reference` with `async_reference`; nested `object` with `enable_nested_fields`). For each, build the draft with `FieldDraft` setters and parse the hand-typed JSON with `from_json`; assert `to_json()` of both are equal as `Value` after sorting `fields` by name.
  - `defaults_are_omitted`: `FieldDraft::new("t", FieldType::String)` → `{"name":"t","type":"string"}`.
  - `unknown_type_and_attribute_round_trip` (Review Focus 1): `{"name":"c","fields":[{"name":"x","type":"int8[]","future":1}],"future_top":true}` → `to_json()` equals the input.
  - `from_collection_drops_server_defaults`: the 30.2 response of `m2probe` (design §4.1) yields fields without `truncate_len`, `stem_dictionary: ""`, `locale: ""`, `index: true`, `store: true`.
  - `clone_blanks_embedding_secrets`: `blank_secrets()` returns 1 and `to_json()` has `"api_key": ""`.
  - `json_error_has_line`: trailing comma → `DraftError { line: Some(3), .. }`.
- [ ] **Step 2: Run** `cargo test -p tessera-core schema`. Expected: FAIL.
- [ ] **Step 3: Implement.** Key order of `to_json`: collection `name`, `fields`, then the options alphabetically, then `extra`; each field `name`, `type`, then attributes alphabetically, then `extra`. `serde_json` needs the `preserve_order` feature for this; check whether it is already enabled in the workspace and, if not, enable it and note the change in the PR.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(core): add the schema draft model`.

### Task 10: Schema validation

**Files:**
- Create: `crates/tessera-core/src/schema/validate.rs` (turn `schema.rs` into `schema/mod.rs`)
- Test: inline table test

**Interfaces:**
- Produces: `validate(draft: &SchemaDraft) -> Vec<Issue>`; `Issue { path: IssuePath, key: &'static str, severity: Severity }`; `enum IssuePath { Collection, Field(usize) }`; `enum Severity { Error, Warning }`; `fn has_errors(issues: &[Issue]) -> bool`. `key` is an i18n key under `schema.issue.*` (the app translates; core stays string-free).

- [ ] **Step 1: Write the failing table test** `validation_table` with one row per check in §5.2, each asserting the exact `(path, key, severity)`, plus a valid schema with no issues. Include `default_sorting_field` on a plain `string` (error) and on a `string` with `sort: true` (no issue).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(core): validate collection schemas`.

### Task 11: ConfirmDanger and FieldTypeSelect

**Files:**
- Create: `crates/tessera-widgets/src/confirm_danger.rs`, `src/field_type_select.rs`
- Test: `crates/tessera-widgets/tests/confirm_danger.rs`

**Interfaces:**
- Produces:
  - `confirm_danger::open(opts: DangerOptions, on_confirm: impl Fn(&mut Window, &mut App) + 'static, window, cx)`; `DangerOptions { title: SharedString, consequences: Vec<SharedString>, expected: SharedString, action_label: SharedString, prod: bool }`. Test ids `danger.input`, `danger.consequence.{i}`, `danger.ok`, `danger.cancel`.
  - `FieldTypeSelect`: a `Select` over `FieldType::ALL` emitting `FieldTypeChanged(FieldType)`; test id `field_type`.

- [ ] **Step 1: Write the failing tests:** `focus_starts_in_the_text_field`; `button_disabled_until_exact_match` — with expected `books`, the inputs `Books`, `books `, ` books` and `book` leave `danger.ok` disabled; `books` enables it (Review Focus 4); `enter_with_mismatch_does_nothing`; `esc_cancels_without_running`; `consequences_are_listed`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement** on `WindowExt::open_dialog` like `confirm.rs`, with `on_ok` returning `false` unless the text matches and the button is the target.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(widgets): add the type-to-confirm dialog and field type select`.

### Task 12: Read-only unlock

**Files:**
- Create: `crates/tessera-app/src/views/unlock.rs`
- Modify: `connection.rs` (`writes_unlocked`, `fn unlock(&mut self, cx)`, `fn lock(&mut self, cx)`, `fn ensure_writable(...)`), `views/env_badge.rs` (open-lock variant), title bar, sidebar, tab titles, `actions.rs` (`LockAgain`, ⌘⇧L / Ctrl+Shift+L), locales
- Test: `crates/tessera-app/tests/ui_unlock.rs`

**Interfaces:**
- Consumes: `confirm_danger::open` (Task 11), `Client::set_read_only` (Task 8).
- Produces:
  - `ConnectionState::ensure_writable(&mut self, window, cx, then: impl FnOnce(&mut Window, &mut App) + 'static)`: runs `then` at once for a writable connection; for a read-only one it opens the unlock dialog (expected text = profile name, consequence per §7.5) and runs `then` after the unlock.
  - `unlock` sets `writes_unlocked = true` and `client.set_read_only(false)`; `lock` reverses both. Disconnect calls `lock`.
  - Banner test id `unlocked.banner`, button `unlocked.lock`.

- [ ] **Step 1: Write the failing UI tests:** `write_on_read_only_opens_unlock_and_sends_nothing` (fake backend records calls: none before the name is typed); `typing_the_name_unlocks_and_continues`; `banner_and_open_lock_show_while_unlocked`; `lock_again_restores_the_guard` (⌘⇧L, then a write opens the dialog again); `disconnect_locks`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** The unlock is never written to `connections.json` (assert in a test that the saved file is byte-identical before and after an unlock).
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): unlock writes on read-only connections`.

### Task 13: Create and clone dialog

**Files:**
- Create: `crates/tessera-app/src/views/schema_dialog/mod.rs` (dialog, mode Create | Clone | Edit), `form.rs` (field rows, options), `sync.rs` (form ↔ JSON)
- Modify: `collections.rs` (backend `create`), `views/collections_list.rs` and `schema_view.rs` (New collection ⌘⇧N, Clone ⌥⌘D), `actions.rs` (`NewCollection`, `CloneCollection`, `ShowForm` ⌘1, `ShowJson` ⌘2, `AddField`, `SubmitSchema` ⌘↩; key context `SCHEMA_DIALOG`), locales
- Test: `crates/tessera-app/tests/ui_schema_dialog.rs`

**Interfaces:**
- Consumes: `SchemaDraft` (Task 9), `validate` (Task 10), `JsonEditor` (Task 3), `FieldTypeSelect` (Task 11), `ensure_writable` (Task 12), `Client::create_collection` (Task 8).
- Produces:
  - `SchemaDialog::open_create(connection: Entity<ConnectionState>, window, cx)`, `SchemaDialog::open_clone(connection, source: Collection, window, cx)`; PR C adds `open_edit`.
  - `CollectionsBackend::create(&self, client: Client, schema: Value) -> BoxFuture<Result<Collection, Error>>`.
  - Test ids: `schema.name`, `schema.field.{i}.name`, `schema.field.{i}.type`, `schema.add_field`, `schema.mode.form`, `schema.mode.json`, `schema.json_locked`, `schema.issues`, `schema.server_error`, `schema.submit`, `schema.cancel`.

- [ ] **Step 1: Write the failing UI tests:**
  - `form_edit_updates_json` and `json_edit_updates_form`.
  - `invalid_json_locks_form_and_disables_create` (Review Focus 3): trailing comma → `schema.json_locked` names line 3, `schema.submit` disabled, backend not called.
  - `create_sends_the_json_tab_body`: the backend receives exactly `SchemaDraft::from_json(json_text).to_json()`.
  - `validation_error_disables_create` (duplicate field names).
  - `conflict_keeps_draft_and_shows_message` (Review Focus 2): backend answers 409 `{"message":"A collection with name `books` already exists."}` → the dialog stays open, `schema.server_error` shows the full message, the draft is unchanged.
  - `clone_prefills_with_copy_name_and_blank_secrets`.
  - `create_on_read_only_asks_for_unlock_first`.
  - `success_reloads_list_and_opens_tab`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** The JSON text is the source of truth for the request. A `syncing: bool` guard stops form → JSON → form loops. If cursor jumps make the live two-way sync unusable in practice, apply the fallback in §13 and record it in the ledger and PR.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): create and clone collections`.

### Task 14: PR B docs

- [ ] CHANGELOG (Added: create and clone with form and JSON, read-only unlock), `docs/spec.md` §7.1 notes the unlock as delivered, screenshots or GIF for the PR. Run the checks, commit `docs: document creating collections`, open PR B against `feat/m2-collections`.

## PR C · Change and remove (branch `feat/m2-change`, from `feat/m2-create`)

### Task 15: SchemaDiff

**Files:**
- Create: `crates/tessera-core/src/schema/diff.rs`
- Test: inline table test

**Interfaces:**
- Consumes: `Collection`, `SchemaDraft`, `FieldDraft`.
- Produces: `diff(old: &Collection, new: &SchemaDraft) -> SchemaDiff`; `SchemaDiff { added: Vec<FieldDraft>, dropped: Vec<String>, changed: Vec<FieldChange>, synonym_sets: Option<Vec<String>>, metadata: Option<Option<Map<String, Value>>>, consequences: Vec<Consequence> }`; `FieldChange { name: String, before: FieldDraft, after: FieldDraft }`; `enum Consequence { RewritesDocuments(u64), ClearsDefaultSortingField(String), BreaksReference { collection: String, field: String } }`; `SchemaDiff::is_empty()`; `SchemaDiff::patch_body() -> Value`; `diff_with_references(old, new, all: &[Collection]) -> SchemaDiff` adds `BreaksReference`.

- [ ] **Step 1: Write the failing table test** `diff_table`: add only; drop only; type change → drop + add in `patch_body` (`[{"name":"n","drop":true},{"name":"n","type":"int64",…}]`); attribute change (facet on); synonym sets changed; metadata changed; no change → `is_empty()`; dropping the `default_sorting_field` → `ClearsDefaultSortingField`; any add or change → `RewritesDocuments(num_documents)`; an unchanged field with `embed` is absent from `patch_body`; dropping `authors.id` while `books_ref` references it → `BreaksReference`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** Field comparison goes through `SchemaDraft::from_collection` on the old side, so server defaults never show up as changes.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(core): diff collection schemas`.

### Task 16: Edit schema with review and progress

**Files:**
- Modify: `views/schema_dialog/mod.rs` (`open_edit`, review and progress steps), `collections.rs` (backend `update`, `schema_changes`), `collections_list.rs` and `schema_view.rs` (Edit schema ⌘E), locales
- Test: `tests/ui_schema_dialog.rs`

**Interfaces:**
- Consumes: Task 15, `Client::update_collection`, `Client::schema_changes`.
- Produces: `SchemaDialog::open_edit(connection, collection: Collection, window, cx)`; backend `update(&self, client, name, patch: Value)` and `schema_changes(&self, client)`; test ids `schema.review`, `schema.review.back`, `schema.review.apply`, `schema.review.body`, `schema.progress`, `schema.stop_waiting`.

- [ ] **Step 1: Write the failing UI tests:** `edit_locks_name_and_fixed_options`; `review_lists_changes_and_consequences`; `review_focus_is_on_back`; `empty_diff_disables_review`; `apply_sends_patch_body`; `progress_shows_schema_change_counts` (fake backend: `schema_changes` returns `validated 500, altered 200` while `update` is pending); `stop_waiting_closes_and_says_server_continues`; `server_error_returns_to_review_with_message`; `edit_on_read_only_asks_for_unlock`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** `schema_changes` is polled every second on the client's active node while `update` runs, and the poll stops when `update` returns or the dialog closes.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): change a collection's schema with a diff preview`.

### Task 17: Truncate and delete

**Files:**
- Modify: `collections.rs` (backend `truncate`, `delete`), `collections_list.rs`, `schema_view.rs`, `views/root.rs` (close the tab of a deleted collection), `actions.rs` (`TruncateCollection` ⌘⇧⌫, `DeleteCollection` ⌘⌫), locales
- Test: `tests/ui_collections.rs`

**Interfaces:**
- Consumes: `confirm_danger::open`, `ensure_writable`, `Client::truncate_collection`, `Client::delete_collection`, `CollectionsBackend::list` (fresh aliases on open).
- Produces: backend `truncate(&self, client, name) -> BoxFuture<Result<u64, Error>>` and `delete(&self, client, name) -> BoxFuture<Result<(), Error>>`.

- [ ] **Step 1: Write the failing UI tests:** `truncate_needs_exact_name_and_states_count`; `delete_lists_aliases_that_will_dangle` (aliases reloaded when the dialog opens); `delete_closes_the_collection_tab_and_updates_list`; `truncate_sets_count_to_zero`; `failure_shows_inline_in_dialog`; `prod_dialog_carries_badge`; `delete_on_read_only_asks_for_unlock_first` (two dialogs in sequence: unlock, then delete).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(app): truncate and delete collections`.

### Task 18: PR C docs and M2 close-out

- [ ] CHANGELOG (Added: schema changes with diff and progress, truncate, delete), `docs/spec.md` §14 marks M2's scope as delivered where M1 did the same, `docs/design/ux.md` gains the danger dialogs if it lists dialogs, GIF for the PR. Run the checks, commit `docs: close out M2`, open PR C against `feat/m2-create`.
