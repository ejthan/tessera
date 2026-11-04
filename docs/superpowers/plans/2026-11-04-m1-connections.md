# M1 Connections and overview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Saved connection profiles with keychain keys, environment tags and a read-only guard, a multi-node client with failover, a request log in a bottom dock, and a live server and cluster overview.

**Architecture:** Logic lives in the GPUI-free crates. `tessera-client` gains nodes, failover, the read-only guard, rich request events and four endpoints. `tessera-core` gains profiles, the profile store, the secret store, importers, the CLI parser, the request log with redaction and curl export, and the metrics history. `tessera-app` holds an `AppState` entity, moves the shell onto GPUI Kit `DockArea`, and renders the dialog, sidebar, log panel and overview. Three PRs, in order: A (Tasks 1–12), B (Tasks 13–17), C (Tasks 18–22).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11), reqwest 0.13.5 (rustls), tokio 1.54.1, keyring 4.2.0, directories 6.0.0, uuid 1.29.0, jiff 0.2.39, wiremock 0.6.5, insta 1.49.0.

**Spec:** [`docs/superpowers/specs/2026-11-04-m1-connections-design.md`](../specs/2026-11-04-m1-connections-design.md). Section numbers below (§4.2 …) refer to it.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI.
- Direct dependencies are pinned exactly (`=x.y.z`) in the workspace `Cargo.toml`. Before adding one, confirm the version with `cargo search <crate> --limit 1`; if it moved, pin the newer one and note it in the PR.
- Never guess a GPUI or GPUI Kit API. Read the pinned source under `~/.cargo/registry/src/*/gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0` and `gpui-pre-0.3.11` first. When the pinned code and the docs disagree, the code wins.
- Every UI string goes through `t!()`, with the same keys in `assets/locales/en.yml` and `de.yml`. The existing i18n key-parity test must stay green.
- No blocking I/O on the GPUI thread. Network, disk and keychain calls go through `runtime::spawn` or tokio's `spawn_blocking` (`docs/architecture/async.md`).
- No key in logs, files, errors, `Debug` or `Display`. Redaction tests are never removed.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --workspace`. No commit with a failing check.
- Commits are signed off: `git commit -s`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense: `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, key `xyz`. Live tests then run with `TESSERA_IT_URL=http://localhost:18108`. Port 8108 on the maintainer's Mac belongs to another project.
- UX rules: environment visible everywhere; errors inline and in full; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown on the control.
- Shortcut labels on controls use GPUI Kit's `Kbd` (check `gpui-component-0.9.0/src/kbd.rs`), never hand-written "Ctrl+…" strings.
- Library crates keep `clippy::unwrap_used = "deny"` and `missing_docs = "warn"`.

## Review Focus

1. An IPv6 host (`::1`) in a node must produce `http://[::1]:8108/`, not an unparseable URL. Test in Task 1.
2. A user pastes `https://ts.example.com/` into the host field. The dialog must show an inline error ("Enter the host without protocol or path"), not save a broken node. Test in Task 10.
3. A typesense-dashboard `config.json` with `port` as a number, a missing `protocol` or a missing `path` must still import (protocol from `tls`, else `http`; path empty). Test in Task 4.
4. A `/metrics.json` without the network or swap fields (some container runtimes omit them) must parse, with those figures shown as "–". Test in Task 7.
5. Two profile edits saved in quick succession must leave the second version on disk. Saves are serialized. Test in Task 8.

---

## PR A · Connections (branch `feat/m1-connections`, from `main` after ejthan/tessera#1 is merged)

### Task 1: Profile model

**Files:**
- Modify: `Cargo.toml` (workspace deps: `uuid = { version = "=1.29.0", features = ["v4", "serde"] }`)
- Modify: `crates/tessera-core/Cargo.toml` (add `serde`, `serde_json`, `uuid`, `url`, `secrecy`, `thiserror`)
- Create: `crates/tessera-core/src/profile.rs`
- Modify: `crates/tessera-core/src/lib.rs` (`pub mod profile;`)
- Test: inline `#[cfg(test)] mod tests` in `profile.rs`

**Interfaces:**
- Produces:
  - `Profile { id: Uuid, name: String, color: ProfileColor, environment: Environment, nodes: Vec<Node>, nearest_node: Option<Node>, timeout_secs: u32, retries: u32, read_only: bool, tls: TlsOptions }`. It derives `Serialize, Deserialize, Clone, Debug, PartialEq`. Every field except `id`, `name` and `nodes` has `#[serde(default)]`.
  - `Profile::new(name: impl Into<String>, nodes: Vec<Node>) -> Profile`. It sets a new v4 id, `Gray`, `Dev`, timeout 5, retries 2, `read_only: false` and default TLS.
  - `enum ProfileColor { Gray, Blue, Teal, Violet, Amber, Red }`, serialized lowercase.
  - `enum Environment { Local, Dev, Staging, Prod, Custom(String) }`. Serialized as a plain string: `"local"`, `"dev"`, `"staging"`, `"prod"` or the custom text. Has `fn is_prod(&self) -> bool` and `fn label(&self) -> &str`.
  - `Node { protocol: Protocol, host: String, port: u16, path: String }`. `enum Protocol { Http, Https }` is serialized lowercase.
  - `Node::url(&self) -> Url`: brackets IPv6 hosts and always ends the path with `/`.
  - `Node::label(&self) -> String`: `host:port`.
  - `Node::is_local(&self) -> bool`: true for `localhost`, `127.0.0.1` and `::1`.
  - `TlsOptions { ca_file: Option<PathBuf>, allow_insecure: bool }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn node_url_brackets_ipv6() {
    let n = Node { protocol: Protocol::Http, host: "::1".into(), port: 8108, path: String::new() };
    assert_eq!(n.url().as_str(), "http://[::1]:8108/");
}
#[test]
fn node_url_keeps_path_prefix() {
    let n = Node { protocol: Protocol::Https, host: "ts.example.com".into(), port: 443, path: "/typesense".into() };
    assert_eq!(n.url().as_str(), "https://ts.example.com/typesense/");
    assert_eq!(n.label(), "ts.example.com:443");
}
#[test]
fn environment_serializes_as_plain_string() {
    assert_eq!(serde_json::to_string(&Environment::Prod).unwrap(), r#""prod""#);
    assert_eq!(serde_json::from_str::<Environment>(r#""qa-eu""#).unwrap(), Environment::Custom("qa-eu".into()));
    assert!(!Environment::Custom("prod-like".into()).is_prod());
}
#[test]
fn profile_round_trips_and_fills_defaults() {
    let p: Profile = serde_json::from_str(r#"{"id":"6f1c2a4e-8d3b-4c1a-9e2f-0b7d5a3c9e11","name":"local","nodes":[{"protocol":"http","host":"localhost","port":8108,"path":""}]}"#).unwrap();
    assert_eq!((p.timeout_secs, p.retries, p.read_only), (5, 2, false));
    assert_eq!(serde_json::from_str::<Profile>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
}
```

- [ ] **Step 2: Run them and see them fail.** Run `cargo test -p tessera-core profile`. Expected: compile errors, the types don't exist yet.
- [ ] **Step 3: Implement `profile.rs`.** Use a custom `Serialize`/`Deserialize` for `Environment`, mapping the four known strings and treating anything else as `Custom`. `Node::url` formats `"{proto}://{host}:{port}{path}/"`, with the host bracketed when it contains `:`, and then normalizes duplicate slashes in the path.
- [ ] **Step 4: Run the tests again.** Run `cargo test -p tessera-core`. Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(core): add connection profile model`.

### Task 2: ProfileStore and connections.json

**Files:**
- Modify: `Cargo.toml` (`directories = "=6.0.0"`); `crates/tessera-core/Cargo.toml` (add `directories`; dev-dep `tempfile` only if it is already in `Cargo.lock`, otherwise create the directories by hand under `std::env::temp_dir()`)
- Create: `crates/tessera-core/src/store.rs`
- Test: inline tests

**Interfaces:**
- Consumes: `Profile` (Task 1).
- Produces:
  - `ConnectionsFile { schema_version: u32, profiles: Vec<Profile>, last_used: Option<Uuid>, reopen_last: bool }`. Derives `Default` with `schema_version = 1`. The constant is `pub const SCHEMA_VERSION: u32 = 1`.
  - `fn config_dir() -> Option<PathBuf>`: `ProjectDirs::from("org", "tessera", "Tessera")?.config_dir()`.
  - `fn load(path: &Path) -> Result<ConnectionsFile, StoreError>`. A missing file returns `Ok(Default)`.
  - `fn save(path: &Path, file: &ConnectionsFile) -> Result<(), StoreError>`. Writes pretty JSON to `path.with_extension("json.tmp")`, then renames. Creates the parent directory.
  - `enum StoreError { Io(std::io::Error), Parse { path: PathBuf, message: String }, NewerSchema(u32) }`.

- [ ] **Step 1: Write the failing tests.**
  - `load_missing_file_is_empty`: returns `Default` with zero profiles.
  - `save_then_load_round_trips`.
  - `save_is_atomic`: create a directory where the `.tmp` file would go so the write fails. `save` returns `Err`, and the previous file content is unchanged.
  - `unparseable_file_is_an_error`: `load` of `"{not json"` returns `StoreError::Parse` with the path. The file is unchanged.
  - `newer_schema_is_refused`: `schema_version: 2` returns `NewerSchema(2)`.
  - `file_never_contains_the_key`: build a profile, put the key `"sk-live-test-1234"` into a `MemoryStore` (Task 3; write this test in Task 3 if Task 3 is not done yet), save, then assert that `!fs::read_to_string(path).contains("sk-live-test-1234")`.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement `store.rs`.**
- [ ] **Step 4: Run `cargo test -p tessera-core`.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(core): store profiles in connections.json`.

### Task 3: SecretStore

**Files:**
- Modify: `Cargo.toml` (`keyring = "=4.2.0"`, default features); `crates/tessera-core/Cargo.toml`
- Create: `crates/tessera-core/src/secrets.rs`
- Test: inline tests, plus `#[ignore]` test `keyring_round_trip`

**Interfaces:**
- Produces:
  - `trait SecretStore: Send + Sync { fn get(&self, id: Uuid) -> Result<Option<SecretString>, SecretError>; fn set(&self, id: Uuid, key: &SecretString) -> Result<(), SecretError>; fn delete(&self, id: Uuid) -> Result<(), SecretError>; }`
  - `struct KeyringStore`, which uses `keyring::Entry::new("tessera", &id.to_string())`. A missing entry (`keyring::Error::NoEntry`) maps to `Ok(None)` on `get` and to `Ok(())` on `delete`.
  - `struct MemoryStore(Mutex<HashMap<Uuid, SecretString>>)` with `Default`.
  - `struct SecretError(String)`: `Display` is the platform message, `Debug` is derived. It never contains the key.
- The pinned API (`keyring-4.2.0/src/v1.rs`): `Entry::new(service, user) -> Result<Entry>`, `set_password(&str)`, `get_password() -> Result<String>`, `delete_credential()`. Errors are `keyring_core::Error`.

- [ ] **Step 1: Write the failing tests.**
  - `memory_store_round_trip`
  - `memory_store_delete_missing_is_ok`
  - `secret_error_never_contains_key`: wrap a keyring error produced while setting `"sk-live-test-1234"` and assert the formatted error lacks the key. Build the error by hand from a `keyring::Error::Invalid`-style variant; check the variant names in `keyring-core`.
  - `keyring_round_trip`, marked `#[ignore = "touches the OS keychain; run manually per platform"]`: set, get, delete with a random UUID.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement `secrets.rs`.** Calls are blocking. The app wraps them in `spawn_blocking` (Task 8).
- [ ] **Step 4: Run `cargo test -p tessera-core` and `cargo test -p tessera-core keyring -- --ignored` on macOS.** Expected: PASS. A Keychain prompt may appear on the first run.
- [ ] **Step 5: Measure the binary size.** Run `cargo build --release -p tessera-app` on Linux (CI or a VM) and macOS, and note both sizes in the commit body. If Linux is above 39.5 MB, stop and raise the spec §14 fallback with the maintainer.
- [ ] **Step 6: Commit.** Message: `feat(core): keep API keys in the OS keychain`.

### Task 4: Import from typesense-dashboard, export

**Files:**
- Create: `crates/tessera-core/src/import.rs`
- Test: inline table tests, fixtures as string literals (keys like `"abc"`, never real keys)

**Interfaces:**
- Consumes: `Profile`, `Node` (Task 1).
- Produces:
  - `ImportCandidate { profile: Profile, key: SecretString, notes: Vec<ImportNote>, duplicate_of: Option<Uuid>, selected: bool }`
  - `enum ImportNote { DifferingClusterKeys, SkippedSameHost }`. `SkippedSameHost` lands on a placeholder candidate with `selected: false`, so the preview can list it.
  - `fn dashboard_config(json: &str, existing: &[Profile]) -> Result<Vec<ImportCandidate>, ImportError>`
  - `fn export_profiles(profiles: &[Profile]) -> String`: pretty JSON `{ "schema_version": 1, "profiles": [...] }`, with no key field because `Profile` has none.

- [ ] **Step 1: Write the failing tests.**
  - `top_level_node_becomes_profile`: the sample in spec §4.4 with no history gives one candidate named `localhost`. The name is the host, `timeout_secs = 10` comes from `connectionTimeoutSeconds`, and the environment is `Dev`.
  - `history_entries_become_profiles`: top level plus two history entries without a tag give three candidates.
  - `cluster_tag_groups_nodes`: two entries with `"clusterTag":"prod-cluster"` give one candidate named `prod-cluster` with two nodes.
  - `differing_cluster_keys_are_noted`: the first key wins, and `notes == [DifferingClusterKeys]`.
  - `same_host_is_skipped`: `host: "SAME"` gives `notes == [SkippedSameHost]` and `selected == false`.
  - `duplicate_is_marked_and_unselected`: an existing profile with equal nodes gives `duplicate_of == Some(id)` and `selected == false`.
  - `lenient_node_fields` (Review Focus 3): `"port": 8108` as a number, a missing `protocol` with `"tls": true` (becomes `https`), and a missing `path` (becomes empty).
  - `export_has_no_keys`: export after import of the sample does not contain `"abc"`.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement the importer.** Use serde structs with `#[serde(default)]` and a port deserializer that accepts a string or a number. Pick the protocol from `protocol`, else from `tls`, else `http`.
- [ ] **Step 4: Run `cargo test -p tessera-core import`.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(core): import typesense-dashboard config.json`.

### Task 5: CLI flags and environment variables

**Files:**
- Create: `crates/tessera-core/src/cli.rs`
- Test: inline table tests

**Interfaces:**
- Consumes: `Profile`, `Node`, `Environment` (Task 1).
- Produces:
  - `enum Launch { Normal, Profile(String), Temporary { profile: Profile, key_from_stdin: bool, key_from_env: Option<SecretString> }, Help }`
  - `fn parse(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Launch, CliError>`. `args` excludes argv[0].
  - `struct CliError(String)`: the message printed before exiting with code 2.
  - `pub const HELP: &str`: the usage text, English only. CLI output is not UI.

Rules (spec §4.4): the temporary connection needs `--host` or `TYPESENSE_HOST`. Flags win over env. Protocol defaults to `http` for local hosts, otherwise `https`. Port defaults to 8108. Environment is `Local` for local hosts, otherwise `Dev`. Read-only is on with `--read-only` or `--env prod`. The profile name is `node.label()`. Read-only is off by default.

- [ ] **Step 1: Write the failing tests.**
  - `no_args_no_env_is_normal`
  - `host_from_env`: `TYPESENSE_HOST=ts.example.com` gives `https`, port 8108, `Dev`.
  - `flag_wins_over_env`: `--port 9000` with `TYPESENSE_PORT=8108` gives 9000.
  - `localhost_defaults`: `--host localhost` gives `http` and `Local`.
  - `env_prod_sets_read_only`
  - `profile_flag`: `--profile prod-ch` gives `Launch::Profile("prod-ch")`.
  - `profile_with_host_is_error`
  - `unknown_flag_is_error`: the message names the flag.
  - `bad_port_is_error`
  - `key_flag_is_rejected`: `--api-key x` errors with a message pointing to `--api-key-stdin`.
  - `api_key_env_alone_is_normal`: `TYPESENSE_API_KEY` without a host gives `Launch::Normal` (the dialog prefill stays in the app).
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement the parser** as a hand-written loop over `args`. Supported forms: `--flag value` and `--flag=value`.
- [ ] **Step 4: Run `cargo test -p tessera-core cli`.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(core): parse CLI flags and TYPESENSE_* variables`.

### Task 6: Multi-node client, options, request events

**Files:**
- Modify: `crates/tessera-client/src/lib.rs` (split the request path out into `send.rs` if `lib.rs` grows past ~250 lines)
- Create: `crates/tessera-client/src/event.rs` (`RequestEvent`, `Body`, `Origin`)
- Modify: `crates/tessera-client/src/error.rs` (add `ReadOnly`, `Tls`)
- Modify callers: `crates/tessera-client/tests/client.rs`, `tests/live.rs`, `crates/tessera-app/src/connection.rs`
- Test: `crates/tessera-client/tests/client.rs`

**Interfaces:**
- Produces:
  - `ClientOptions { timeout: Duration, retries: u32, nearest_node: Option<String>, ca_file: Option<PathBuf>, allow_insecure: bool, read_only: bool }`. `Default` uses `DEFAULT_TIMEOUT` and retries 2.
  - `Client::new(nodes: &[&str], key: SecretString, options: ClientOptions) -> Result<Client, Error>`. It validates each URL as M0 did, and an empty `nodes` is `InvalidUrl`.
  - `Client::with_timeout` stays as a builder.
  - `Client::with_hook(self, RequestHook) -> Self`
  - `Client::nodes(&self) -> &[Url]`
  - `Client::active_node(&self) -> usize`
  - `Client::set_active_node(&self, usize)`
  - `Client::node(&self, index: usize) -> NodeClient`
  - `Client::polling(&self) -> Client`: a clone whose events carry `Origin::Polling`.
  - `NodeClient` has the same endpoint methods for one node and never fails over.
  - `RequestEvent` exactly as in spec §5.4, plus `Body { text: Arc<str>, original_len: usize }` with `pub const BODY_LIMIT: usize = 1 << 20` and `enum Origin { User, Polling }`.
  - `Error::ReadOnly { method: Method }` and `Error::Tls(String)`.
  - `base_url()` stays and returns the active node's URL.
- Event ids come from a process-wide `AtomicU64`.

- [ ] **Step 1: Update the existing tests** to `Client::new(&[&uri], key, ClientOptions::default())`, then add these failing tests:
  - `event_carries_node_origin_and_bodies`: one GET against wiremock gives an event with `node == base`, `origin == User`, a response body equal to the mock body, and a `key_prefix` of the first 4 characters of the key.
  - `event_headers_never_contain_key`: no header name equals `x-typesense-api-key` in any case, and `format!("{event:?}")` lacks the key.
  - `body_is_cut_at_limit_on_char_boundary`: a mock body of `BODY_LIMIT - 1` ASCII bytes followed by `"é…"` ends up with `text.len() <= BODY_LIMIT`, valid UTF-8, and `original_len` equal to the full length.
  - `polling_clone_tags_events`
  - `node_client_targets_one_node`: two mock servers; `client.node(1).health()` hits only the second.
  - `active_node_starts_at_nearest`
- [ ] **Step 2: Run `cargo test -p tessera-client`.** Expected: FAIL.
- [ ] **Step 3: Implement.** All nodes share one `reqwest::Client`, built with `.redirect(Policy::none())`. `ca_file` is read at `new` and added with `add_root_certificate`. `allow_insecure` uses `danger_accept_invalid_certs`. Check which of these the reqwest 0.13.5 `rustls` feature exposes, and add a feature only if one is missing. Keep a single internal `async fn send(&self, node: usize, method, path, query, body) -> Result<(u16, String), Error>` that builds the event and calls the hook. Map rustls certificate errors to `Error::Tls` by checking the source chain for `rustls::Error`; if that is not reachable through reqwest 0.13.5's error type, match `is_connect()` plus the `"certificate"` text and leave a `// shortcut:` comment.
- [ ] **Step 4: Run `cargo test -p tessera-client` and `cargo test -p tessera-app`.** Expected: PASS. The app still compiles with the new signature.
- [ ] **Step 5: Commit.** Message: `feat(client): support several nodes and rich request events`.

### Task 7: Failover, read-only guard, new endpoints

**Files:**
- Modify: `crates/tessera-client/src/lib.rs` (or `send.rs`), `models.rs`
- Test: `crates/tessera-client/tests/client.rs`, `tests/live.rs`

**Interfaces:**
- Consumes: Task 6's client.
- Produces:
  - `Client::metrics() -> Result<Metrics, Error>`, `Client::stats() -> Result<Stats, Error>`, `Client::collections_summary() -> Result<Vec<CollectionSummary>, Error>` and `Client::keys_probe() -> Result<KeyAccess, Error>`. Each also exists on `NodeClient`.
  - `Metrics { cpu_percent: Option<f64>, memory_used: Option<u64>, memory_total: Option<u64>, disk_used: Option<u64>, disk_total: Option<u64>, net_received: Option<u64>, net_sent: Option<u64>, typesense_memory_active: Option<u64>, raw: BTreeMap<String, String> }`. Values arrive as strings; one that doesn't parse becomes `None`.
  - `Stats { latency_ms: BTreeMap<String, f64>, requests_per_second: BTreeMap<String, f64>, total_requests_per_second: Option<f64>, raw: serde_json::Map<String, Value> }`
  - `CollectionSummary { name: String, num_documents: u64 }`
  - `enum KeyAccess { Admin, Restricted }`: 200 is `Admin`, 401 or 403 is `Restricted`, anything else is `Err(Http)`.
- Methods for the guard: `const WRITE_METHODS = [POST, PATCH, PUT, DELETE]`. Methods for failover: `[GET, HEAD, PUT, DELETE]`.
- Failover backoff: `100ms * 2^attempt`, up to `retries` extra attempts, cycling through the other nodes in order. Only `Network` and `Timeout` count as failures (spec §5.2).

- [ ] **Step 1: Write the live tests first** (CLAUDE.md rule 2) in `tests/live.rs`, all `#[ignore]`:
  - `live_metrics_parses`: `cpu_percent.is_some()` and `memory_total > 0`.
  - `live_stats_parses`
  - `live_collections_summary`
  - `live_keys_probe_admin`
  - `live_keys_probe_restricted`: in setup, create a search-only key with raw reqwest `POST /keys` `{"description":"tessera-it","actions":["documents:search"],"collections":["*"]}`, probe with it and expect `Restricted`, then delete the key by id.
- [ ] **Step 2: Run them against the dev server.** Run `TESSERA_IT_URL=http://localhost:18108 cargo test -p tessera-client -- --ignored live_`. Expected: FAIL (methods missing).
- [ ] **Step 3: Write the failing unit tests (wiremock):**
  - `failover_on_refused_connection`: node 0 is a closed port and node 1 a mock; `health()` is ok, `active_node() == 1`, and there are two events.
  - `no_failover_on_http_500`: node 0 answers 500 and node 1 never gets a request.
  - `retries_bound_attempts`: three dead nodes and `retries = 1` give exactly two events and `Err(Network)`.
  - `node_client_never_fails_over`
  - `read_only_blocks_writes_before_sending`, the §7.1 AC: a read-only client with a hook pushing into a `Mutex<Vec<_>>`. For each of POST, PATCH, PUT and DELETE, an internal `request(method, "collections")` returns `Err(Error::ReadOnly{..})`, and both the hook vector and wiremock's received requests stay empty. Expose `request` as `#[doc(hidden)] pub` for this test only.
  - `read_only_allows_get`
  - `metrics_tolerates_missing_fields` (Review Focus 4): a body with only `system_cpu_active_percentage` gives `Ok` with the network fields `None`.
  - `stats_parses_endpoint_maps`: `{"latency_ms":{"GET /collections":1.5},"requests_per_second":{"GET /collections":2.0}}`.
- [ ] **Step 4: Implement** the failover loop around `send`, the guard at the top of `send`, and the four endpoints.
- [ ] **Step 5: Run the unit tests and the live tests.** Expected: PASS.
- [ ] **Step 6: Commit.** Message: `feat(client): fail over between nodes, guard read-only, add metrics and stats`.

### Task 8: AppState, ConnectionState, startup

**Files:**
- Create: `crates/tessera-app/src/state.rs` (`AppState`)
- Modify: `crates/tessera-app/src/connection.rs` (`ConnectionState`, new `Connector` signature)
- Modify: `crates/tessera-app/src/lib.rs` (`run(launch: Launch)`), `src/main.rs` (parse CLI, read stdin key, exit 2 on error)
- Modify: `docs/architecture/async.md` (a section "The Connector global")
- Test: `crates/tessera-app/tests/ui.rs` (update `fake_connector`), new inline tests in `state.rs`

**Interfaces:**
- Consumes: `Profile`, `store::{load, save}`, `SecretStore`, `Launch`, `Client`.
- Produces:
  - `type Connector = Arc<dyn Fn(Profile, SecretString) -> ConnectFuture + Send + Sync>`. `ConnectFuture` resolves to `Result<Connected, Error>`.
  - `Connected { client: Client, info: ServerInfo, access: KeyAccess }`. The real connector runs health, debug, a collections probe and `keys_probe`, the same sequence the dialog test uses.
  - `ServerInfo` stays.
  - `AppState { profiles: Vec<Profile>, last_used: Option<Uuid>, reopen_last: bool, store_path: Option<PathBuf>, secrets: Arc<dyn SecretStore>, connections: Vec<Entity<ConnectionState>>, selected: Option<Uuid>, temporary: Option<(Profile, SecretString)>, load_error: Option<String> }`
  - `AppState::save(&mut self, cx)`: serializes a snapshot and runs it on tokio `spawn_blocking`. Saves are chained: each save awaits the previous save's task before writing (Review Focus 5).
  - `AppState::upsert_profile(&mut self, Profile, Option<SecretString>, cx)`
  - `AppState::delete_profile(&mut self, Uuid, cx)`
  - `AppState::connect(&mut self, Uuid, cx)`
  - `AppState::disconnect(&mut self, Uuid, cx)`
  - `ConnectionState { profile: Profile, client: Client, info: ServerInfo, access: KeyAccess, connecting: bool, error: Option<SharedString> }`
  - Global `SecretStoreGlobal(Arc<dyn SecretStore>)`. The app installs `KeyringStore`, tests install `MemoryStore`.
- `main`: `let launch = cli::parse(&args, &|k| std::env::var(k).ok())`. On `Err`, print it and exit 2. On `Help`, print `HELP` and exit 0. With `key_from_stdin`, read one line before `run`.

- [ ] **Step 1: Update `fake_connector` in `tests/ui.rs`** to the new signature and keep all M0 UI tests green. Add these failing tests:
  - `saves_are_serialized`: two `upsert_profile` calls back to back on an `AppState` with a temp store path, after which `run_until_parked` shows the file holding the second name.
  - `temporary_launch_connects_without_saving`: `AppState` built from `Launch::Temporary` connects through the fake and leaves `profiles.is_empty()` and the store file absent.
- [ ] **Step 2: Run `cargo test -p tessera-app`.** Expected: FAIL.
- [ ] **Step 3: Implement** `state.rs`, the connector and `run(launch)`. The `RootView` now reads `AppState` instead of the M0 `Connection` entity; remove `Connection` once nothing uses it.
- [ ] **Step 4: Update `docs/architecture/async.md`.** Describe what the `Connector` global is, why tests replace it, its new signature, and that keychain and file I/O use `spawn_blocking`.
- [ ] **Step 5: Run the checks.** Expected: PASS.
- [ ] **Step 6: Commit.** Message: `feat(app): hold profiles and connections in AppState`.

### Task 9: Sidebar, environment visibility, i18n minors

**Files:**
- Create: `crates/tessera-app/src/views/sidebar.rs`
- Create: `crates/tessera-app/src/views/env_badge.rs` (badge and color helpers)
- Modify: `views/root.rs` (title bar switcher, prod band and frame), `views/status_bar.rs`, `actions.rs` (remove `NEW_CONNECTION_HINT`; add actions `EditConnection`, `DeleteConnection`, `ConnectSelected`), `assets/locales/en.yml`, `de.yml`
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `AppState` (Task 8).
- Produces:
  - Test ids: `sidebar.profile.{uuid}`, `sidebar.profile.{uuid}.error`, `sidebar.profile.{uuid}.readonly`, `titlebar.env`, `window.prod_frame`, `status.text`.
  - `env_badge::color(ProfileColor, &Theme) -> Hsla`. Use the canvas hexes: Gray `#71717A`, Blue `#2563EB`, Teal `#0D9488`, Violet `#7C3AED`, Amber `#B45309`, Red `#DC2626`.
  - `env_badge::render(&Environment, cx) -> impl IntoElement`
- Prod frame: a 4 px band at the top in `#DC2626` and a 2 px inset border, both rendered while `AppState::selected` is a prod profile. Task 15 switches this to the active tab.
- Status bar text keys: `status.connected` with `%{name} · %{version} · %{role} · %{node} · %{ms}`, `status.unreachable` with `%{count} of %{total} nodes unreachable`, and `status.prod_read_only`. `"v"` and `"ms"` move into the key texts.
- Timeout text: `connect.error.timeout` takes `%{seconds}` from `profile.timeout_secs`.

- [ ] **Step 1: Write the failing UI tests.**
  - `sidebar_lists_profiles_with_badges`: two profiles in a `MemoryStore`-backed `AppState` render both ids, and the prod one has the `.readonly` badge.
  - `enter_on_profile_connects`: focus the entry, press Enter, and the fake connector is called with that profile; the status text contains the name.
  - `connect_failure_shows_inline_under_entry`: a fake 401 makes the `.error` element contain the full Typesense message.
  - `prod_selection_draws_frame`: `window.prod_frame` exists only while the prod profile is selected.
  - `timeout_text_uses_profile_timeout`: a profile with `timeout_secs: 12` and a fake `Error::Timeout` gives text containing `12`.
  - `no_hardcoded_shortcut_labels`: a source scan test that greps `crates/tessera-app/src` for `"Ctrl+` and `"⌘` string literals and asserts none.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement.** The context menu comes from GPUI Kit's context menu (check `gpui-component-0.9.0/src/menu`), and its items show their shortcut with `Kbd`. The sidebar header gets "Add connection", "Import from typesense-dashboard…", "Export profiles…" and the "Reopen last connection at startup" check item. The import and export entries only appear once Task 11 makes them work (UX rule 4): land them in Task 11.
- [ ] **Step 4: Run the checks.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(app): list profiles in the sidebar and show the environment everywhere`.

### Task 10: Connection dialog

**Files:**
- Modify: `crates/tessera-app/src/views/connect_dialog.rs` (rewrite; move the node rows into `views/node_rows.rs` if the file passes ~400 lines)
- Modify: locales
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `AppState::upsert_profile`, `Connector`, `SecretStoreGlobal`.
- Produces:
  - `ConnectDialog::open_new(state, prefill: Option<(Profile, Option<SecretString>)>, window, cx)`
  - `ConnectDialog::open_edit(state, Uuid, window, cx)`
- Test ids: `connect.name`, `connect.color.{gray…red}`, `connect.env.{local,dev,staging,prod,other}`, `connect.node.{i}.host|port|path|protocol|remove`, `connect.add_node`, `connect.key`, `connect.key.toggle`, `connect.read_only`, `connect.advanced`, `connect.timeout`, `connect.retries`, `connect.test`, `connect.result`, `connect.save`, `connect.cancel`, `connect.keychain_error`, `connect.session_only`, `connect.http_warning`.
- Shortcuts: Test ⌘T / Ctrl+T, Save ⌘↩ / Ctrl+↩, Cancel Esc, key toggle ⌥⌘K / Ctrl+Alt+K.
- Host validation: reject a value containing `://`, `/` or whitespace with `connect.error.host_format`. Port must be 1–65535. Timeout must be 1–300 s. Retries must be 0–10.
- Result texts:
  - `connect.result.healthy` with `Server healthy · Typesense %{version} · %{role}`
  - `connect.result.admin` with `Admin key: can list collections and API keys`
  - `connect.result.restricted` with `Restricted key: can read collections, cannot list API keys`
  - `connect.result.tested` with `Tested %{ago} · %{ms} ms`

- [ ] **Step 1: Write the failing UI tests.**
  - `save_creates_profile_and_stores_key`: fill name, host and key, then ⌘↩. `AppState.profiles.len() == 1`, `MemoryStore.get(id)` is the key, and the dialog closes.
  - `choosing_prod_turns_read_only_on`
  - `cancel_during_test_does_not_connect` (M0 minor): the fake connector returns a future that never resolves; start Test, press Esc, `run_until_parked`. No `ConnectionState` exists and the dialog is closed.
  - `key_toggle_reachable_by_keyboard` (M0 minor): Tab from the key field focuses `connect.key.toggle`, and the shortcut unmasks it.
  - `host_with_scheme_is_rejected` (Review Focus 2): host `https://ts.example.com/` shows the inline error and creates no profile.
  - `restricted_key_result`: a fake returning `KeyAccess::Restricted` shows the restricted text.
  - `keychain_failure_offers_session_only`: a `SecretStore` fake returning `Err` shows `connect.keychain_error`. Clicking `connect.session_only` saves the profile without a stored key and connects.
  - `edit_with_empty_key_keeps_stored_key`
  - `remote_http_shows_warning`: host `ts.example.com` with `http` shows `connect.http_warning`, and `localhost` does not.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement.** Check the pinned GPUI Kit components before use: segmented control (`gpui-component-0.9.0/src/button` group or `tab`), `switch.rs`, `collapsible.rs`, `input`. Read the German strings that the dialog and input components need from GPUI Kit's own locale keys (search `gpui-component-0.9.0` for `t!(`), and add the missing `de` entries to our `de.yml` only if the component reads from our locale; otherwise note that in the PR.
- [ ] **Step 4: Run the checks.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(app): add the connection dialog with test and keychain`.

### Task 11: Delete, import preview, export, reopen, temporary connection

**Files:**
- Create: `crates/tessera-app/src/views/import_dialog.rs`, `views/confirm.rs` (medium-danger confirmation)
- Modify: `views/sidebar.rs`, `views/welcome.rs` (add "Import from typesense-dashboard"), `state.rs` (reopen at startup), locales
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `import::dashboard_config`, `import::export_profiles`, `AppState`.
- Produces:
  - `confirm::open(title, body, confirm_label, on_confirm, window, cx)`. Focus starts on Cancel and the confirm button is red.
  - Test ids: `confirm.cancel`, `confirm.ok`, `import.row.{i}`, `import.row.{i}.selected`, `import.submit`, `sidebar.save_temporary`.
- File dialogs: `cx.prompt_for_paths` and `cx.prompt_for_new_path` (`gpui-pre-0.3.11/src/app.rs:2039`, `:2052`). They return a receiver; await it in a GPUI task. Read and write the files on `spawn_blocking`.
- Tests can't drive native file dialogs. Split each flow into `import_from_text(&str)` and `export_to(path)` and test those; the dialog-opening wrapper stays untested and thin.

- [ ] **Step 1: Write the failing UI tests.**
  - `delete_asks_with_cancel_focused`: ⌘⌫ on a profile shows the dialog and the focused element is `confirm.cancel`. Enter keeps the profile; confirming removes the profile and its `MemoryStore` entry.
  - `import_preview_creates_selected_profiles`: `import_from_text(sample)` with one duplicate. Three rows, the duplicate unchecked; submit adds two profiles with keys.
  - `export_writes_profiles_without_keys`: `export_to(temp)` gives a file without the key.
  - `reopen_last_connects_at_startup`: `reopen_last: true` and `last_used` set means a new `AppState` connects through the fake.
  - `temporary_connection_can_be_saved`: `Launch::Temporary` shows a "not saved" entry, and `sidebar.save_temporary` opens the dialog prefilled with host and key.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the checks.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(app): delete, import and export profiles`.

### Task 12: Notices, docs, PR A

**Files:**
- Create: `THIRD_PARTY_NOTICES.md`: the `webpki-root-certs` notice under CDLA-Permissive-2.0, with the license text link and the crate version from `Cargo.lock`
- Modify: `README.md` (link the notices; document the CLI flags and env vars), `CHANGELOG.md` (Unreleased: Added, Changed), `docs/building.md` (cluster profile comes in Task 18; here only the `--host`/`--profile` usage for dev)

- [ ] **Step 1: Write the docs and notices.**
- [ ] **Step 2: Run the full check suite and the live tests.** Run `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`, then `TESSERA_IT_URL=http://localhost:18108 cargo test -p tessera-client -- --ignored`. Expected: all PASS.
- [ ] **Step 3: Manual check on macOS.** Create a profile against the dev server, quit, start again, and the profile is still there with its key (Keychain Access shows service `tessera`). Run `tessera --host localhost --port 18108 --api-key-stdin <<< xyz` and confirm the "not saved" entry. Run `cargo test -p tessera-core keyring -- --ignored`.
- [ ] **Step 4: Commit, push, open the PR.** Use the `superpowers:finishing-a-development-branch` skill. The PR body lists the Linux and macOS binary sizes from CI, the manual keychain check per platform (Windows and Linux checks may be done by the maintainer), and a demo GIF.

---

## PR B · Dock and request log (branch `feat/m1-request-log`, from `main` after PR A)

### Task 13: RequestLog, redaction, curl

**Files:**
- Modify: `Cargo.toml` (dev-dep `insta = "=1.49.0"`), `crates/tessera-core/Cargo.toml` (add `tessera-client` as a dependency for `RequestEvent`; allowed, since core → client is the declared direction)
- Create: `crates/tessera-core/src/log.rs`, `crates/tessera-core/src/redact.rs`, `crates/tessera-core/src/curl.rs`
- Test: inline tests, snapshots in `crates/tessera-core/src/snapshots/`

**Interfaces:**
- Consumes: `tessera_client::{RequestEvent, Origin, Body}`.
- Produces:
  - `LogEntry { connection: Uuid, connection_name: Arc<str>, event: RequestEvent }`
  - `RequestLog::new(user_cap: usize, polling_cap: usize)`, with constants `USER_CAP = 5000` and `POLLING_CAP = 500`
  - `RequestLog::push(&mut self, LogEntry)`
  - `RequestLog::entries(&self, &Filter) -> Vec<&LogEntry>`: oldest first, merged by `event.id`
  - `RequestLog::get(&self, id: u64) -> Option<&LogEntry>`
  - `Filter { errors_only: bool, writes_only: bool, methods: Vec<Method>, connections: Vec<Uuid>, show_polling: bool }`. `Default` means everything except polling. An entry counts as an error when its status is ≥ 400 or absent.
  - `redact::json(text: &str) -> String`: masks string values of keys `api_key` and `value` at any depth as `"••••"`. Non-JSON text passes through unchanged. Output is pretty-printed.
  - `redact::key(prefix: &str) -> String`: `format!("{prefix}••••")`
  - `curl::command(entry: &LogEntry) -> String`

- [ ] **Step 1: Write the failing tests.**
  - `polling_does_not_evict_user_entries`: push 1 user entry and 600 polling entries. `entries(&Filter{show_polling: true, ..})` holds the user entry and 500 polling entries.
  - `user_cap_evicts_oldest`
  - `default_filter_hides_polling_but_errors_show_failed_polling`: a polling entry with `status: None` appears under `errors_only: true, show_polling: false`. Spec §6.1 says "Errors" applies to both buffers, so `errors_only` overrides `show_polling`.
  - `writes_only_filters_methods`
  - `merge_orders_by_id`
  - `redact_masks_nested_api_key_and_value`: `{"a":{"api_key":"sk"},"keys":[{"value":"abc"}]}` contains neither `sk` nor `abc`.
  - `redact_keeps_non_json`
  - `curl_get_with_query` (insta)
  - `curl_post_with_body_heredoc` (insta)
  - `curl_escapes_single_quotes` (insta)
  - `curl_never_contains_key`: the event's `key_prefix` and a body with `api_key` produce output containing `$TYPESENSE_API_KEY` and neither the prefix nor the secret.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement.** Use two `VecDeque`s. The curl format is fixed by spec §6.3. Bodies go through `redact::json` before they are put into the heredoc.
- [ ] **Step 4: Run `cargo test -p tessera-core` and `cargo insta review`.** Accept the snapshots after reading them. Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(core): add request log with redaction and curl export`.

### Task 14: Hook into the log

**Files:**
- Modify: `crates/tessera-app/src/state.rs` (add `log: RequestLog`, channel drain task), `connection.rs` (the connector installs the hook)
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `RequestLog`, `RequestHook`.
- Produces:
  - `AppState::log(&self) -> &RequestLog`
  - The connector gets a `std::sync::mpsc::Sender<LogEntry>` (or `tokio::sync::mpsc::UnboundedSender`) and installs a hook that sends `LogEntry { connection, connection_name, event: event.clone() }`.
  - The channel is `tokio::sync::mpsc::UnboundedSender<LogEntry>`; tokio's channels need no runtime to be awaited. One GPUI task in `AppState` loops on `rx.recv().await`, drains whatever else is queued with `try_recv`, pushes it all into `log`, and calls `cx.notify()` once per batch.
- Fakes don't run the real client. Give the test `Connector` access to the sender, so a UI test can inject events.

- [ ] **Step 1: Write the failing test.** `events_reach_the_log`: inject two events through the sender, `run_until_parked`, and `AppState.log().entries(&Filter::default()).len() == 2`.
- [ ] **Step 2: Run it.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run it again.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(app): collect request events in the log`.

### Task 15: Shell on DockArea, tabs per connection

**Files:**
- Create: `crates/tessera-app/src/views/dock.rs` (the panel wrappers: `SidebarPanel`, `OverviewPanel`, `RequestLogPanel` placeholder until Task 16)
- Modify: `views/root.rs` (replace the hand-built `h_flex` body with a `DockArea`), `views/overview.rs` (becomes the content of `OverviewPanel`), `actions.rs` (`ToggleRequestLog`: cmd-j / ctrl-j)
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `AppState`, `ConnectionState`.
- Produces:
  - `OverviewPanel::new(connection: Entity<ConnectionState>, ...)`
  - Test ids: `tab.{uuid}.overview`, `dock.bottom`.
  - `RootView::active_connection(&self, cx) -> Option<Entity<ConnectionState>>`: the connection of the active center tab. The prod band, frame, title bar and status bar all read this.
- API: `gpui-base-0.9.0/src/dock/dock_area.rs` has `DockArea::new` (line 126), `set_center`, `set_dock`, `toggle_dock`, `add_panel`, `select_panel` and `remove_panel`. The `Panel` trait is in `gpui-component-0.9.0/src/dock/panel.rs:71`. Read both and `dock/test_support.rs` before writing code.

- [ ] **Step 1: Spike, about 1 hour.** Build a throwaway `DockArea` with one left panel, one center tab and one bottom panel in a scratch test. Confirm that a tab title can carry a colored dot and top edge, and that `toggle_dock(Bottom)` works headless. Record the findings in the commit body. If a tab cannot be styled per panel, fall back to a colored dot in the title text and note the deviation in the spec.
- [ ] **Step 2: Write the failing UI tests.**
  - `connecting_opens_overview_tab`: connect gives a `tab.{uuid}.overview`.
  - `reopening_focuses_existing_tab`: connecting the same profile again still gives one tab.
  - `disconnect_closes_tabs`
  - `prod_frame_follows_active_tab`: open staging and prod, select the staging tab, and there is no frame; select the prod tab and there is a frame.
  - `cmd_j_toggles_bottom_dock`
- [ ] **Step 3: Run them.** Expected: FAIL.
- [ ] **Step 4: Implement** and remove Task 9's `selected`-based frame logic.
- [ ] **Step 5: Run the checks.** All M0 and PR A UI tests stay green. Expected: PASS.
- [ ] **Step 6: Commit.** Message: `feat(app): move the shell onto DockArea with tabs per connection`.

### Task 16: Request log panel

**Files:**
- Create: `crates/tessera-app/src/views/request_log.rs`, `views/request_detail.rs`
- Modify: `Cargo.toml` (`jiff = "=0.2.39"`), `crates/tessera-app/Cargo.toml`, `actions.rs` (`CopyAsCurl`: cmd-shift-c / ctrl-shift-c), locales
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `AppState::log`, `Filter`, `redact`, `curl::command`.
- Produces test ids: `log.row.{id}`, `log.filter.all|errors|writes`, `log.filter.polling`, `log.filter.methods`, `log.filter.connections`, `log.detail`, `log.copy_curl`.
- Columns and texts: spec §7.6. The time is local `HH:MM:SS.mmm` from `event.started_at`: convert it with `jiff::Timestamp::try_from(SystemTime)` and `.to_zoned(jiff::tz::TimeZone::system())`, then format with `%H:%M:%S%.3f` (check these names in jiff 0.2.39). Status is the number, or `log.status.failed` in red. Size is formatted as B, KB or MB with one decimal (keys `units.bytes`, `units.kb`, `units.mb`).
- Table: GPUI Kit table (`gpui-component-0.9.0/src/table/`). Check the delegate trait in `delegate.rs` before writing code. It stays scrolled to the bottom while the user is at the bottom.

- [ ] **Step 1: Write the failing UI tests.**
  - `request_appears_in_log`: inject one event, and `log.row.{id}` shows method, path and status.
  - `polling_hidden_until_switch_on`
  - `errors_filter_shows_failed_polling`
  - `detail_shows_redacted_body`: an event with `{"api_key":"sk"}` in its response; select the row, and `log.detail` text lacks `sk` and contains `••••`.
  - `copy_as_curl_uses_placeholder`: select the row, press ⌘⇧C, and the clipboard (`cx.read_from_clipboard()`) contains `$TYPESENSE_API_KEY` and not the key.
  - `connection_column_only_with_two_connections`
  - `full_secret_session_scan` (spec §9 "Secrets"): run a fake session with key `"sk-live-test-1234"` through connect, polling events and copy as curl, then assert the key is absent from the log entries' debug output, the clipboard and `connections.json`.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Measure the release binary size on Linux.** If it is above 39.5 MB, replace `jiff` with `std` UTC formatting (spec §14) and note it.
- [ ] **Step 5: Run the checks.** Expected: PASS.
- [ ] **Step 6: Commit.** Message: `feat(app): add the request log panel`.

### Task 17: Docs, PR B

- [ ] **Step 1: Update the docs.** `CHANGELOG.md` gets the request log and dock entries. `README.md` gets a short "Request log" paragraph with "Copy as curl" and the placeholder.
- [ ] **Step 2: Run the full checks.** Expected: PASS.
- [ ] **Step 3: Commit, push, open the PR.** Use `superpowers:finishing-a-development-branch`. The PR body carries the binary sizes and a GIF.

---

## PR C · Overview (branch `feat/m1-overview`, from `main` after PR B)

### Task 18: Dev cluster, nightly cluster tests

**Files:**
- Modify: `dev/docker-compose.yml` (profile `cluster`: services `ts1`, `ts2`, `ts3`, host ports 18108, 18109 and 18110, a `dev/cluster/nodes` file mounted read-only, `--nodes=/nodes`, `--peering-port=8107`)
- Create: `dev/cluster/nodes` (`ts1:8107:8108,ts2:8107:8108,ts3:8107:8108`)
- Create: `.github/workflows/nightly.yml` (schedule `0 3 * * *` plus `workflow_dispatch`: `docker compose --profile cluster up -d`, then `cargo test -p tessera-client --test cluster -- --ignored`)
- Create: `crates/tessera-client/tests/cluster.rs`
- Modify: `docs/building.md`

Check the Typesense 30.2 clustering docs (https://typesense.org/docs/30.2/guide/high-availability.html) for the exact `--nodes` format and peering flags before writing the compose file.

- [ ] **Step 1: Write the cluster tests,** all `#[ignore = "needs the 3-node cluster, see dev/docker-compose.yml"]`.
  - `cluster_has_one_leader_two_followers`: `debug()` via `node(0..3)` gives exactly one `Leader` and two `Follower`. This confirms `state = 4`; update the M0 note in `models.rs`.
  - `stopped_node_fails_others_answer`: the test runs `docker compose stop ts3` (through `std::process::Command`), after which `node(2).health()` errs and `node(0)` and `node(1)` are ok. It restarts ts3 at the end.
  - `client_fails_over_from_stopped_node`: active node 2 is stopped, `health()` is ok and `active_node() != 2`.
- [ ] **Step 2: Start the cluster and run the tests locally.** Run `docker compose -f dev/docker-compose.yml --profile cluster up -d`, then `cargo test -p tessera-client --test cluster -- --ignored`. Expected: PASS.
- [ ] **Step 3: Commit.** Message: `test: add a 3-node dev cluster and nightly cluster tests`.

### Task 19: MetricsHistory

**Files:**
- Create: `crates/tessera-core/src/metrics.rs`
- Test: inline tests

**Interfaces:**
- Consumes: `tessera_client::Metrics`.
- Produces:
  - `enum Series { Cpu, Memory, Disk, NetRx, NetTx }`
  - `MetricsHistory::new(window: Duration)`. The app passes 15 minutes.
  - `MetricsHistory::push(&mut self, at: Instant, &Metrics)`
  - `MetricsHistory::points(&self, Series) -> impl Iterator<Item = (Duration /* age */, f64)>`
  - `MetricsHistory::latest(&self, Series) -> Option<f64>`
- Rules: points older than `window` are dropped on push. CPU is in percent. Memory and disk are a percentage of the total, with the raw used and total figures available from the last `Metrics`. NetRx and NetTx are in bytes per second from consecutive samples; a decreasing counter skips that rate.

- [ ] **Step 1: Write the failing tests.**
  - `drops_points_older_than_window`
  - `network_rate_from_counters`: 1000 B then 6000 B, 5 s apart, give 1000 B/s.
  - `counter_reset_skips_rate`: 6000 then 100 gives no point.
  - `missing_fields_skip_series`: `net_received: None` means no NetRx point.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them again.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(core): keep 15 minutes of metrics history per node`.

### Task 20: Poller

**Files:**
- Create: `crates/tessera-app/src/poller.rs`
- Modify: `connection.rs` (`ConnectionState` gains `nodes: Vec<NodeStatus>`, `history: Vec<MetricsHistory>` per node, `stats: Option<Stats>`, `collections: Option<Vec<CollectionSummary>>`, `paused: bool`, `interval: Duration`)
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `Client::polling()`, `NodeClient`, `MetricsHistory`, `runtime::spawn`.
- Produces:
  - `NodeStatus { debug: Option<Debug>, healthy: Option<bool>, latency: Option<Duration>, error: Option<SharedString>, next_retry: Option<Instant>, backoff: Duration }`
  - `Poller::start(connection: Entity<ConnectionState>, window: &Window, cx) -> Poller`: dropping it stops polling. `OverviewPanel` owns it.
  - `ConnectionState::set_paused(bool, cx)`, `set_interval(Duration, cx)`, `refresh_collections(cx)`.
  - A `NodeFetcher` seam for tests: the poller gets its per-node fetch function from a global `PollFetcherGlobal(Arc<dyn Fn(NodeClient) -> BoxFuture<NodeSample>>)`, where `NodeSample { health, debug, metrics, stats }` is a struct of `Result`s. The app installs the real fetcher, and tests install fakes, for the same reason as `Connector`.
- Tick: every `interval`, fetch every node whose `next_retry` has passed, in parallel. On error, `backoff = clamp(backoff * 2, 5 s, 30 s)` and `next_retry = now + backoff`. On success, reset the backoff. Stop while `!window.is_visible()` (`cx.observe_window_visibility`, `gpui-pre-0.3.11/src/app/context.rs:426`) or while paused. Resume immediately when the window becomes visible.

- [ ] **Step 1: Write the failing UI tests (simulated clock).**
  - `failing_node_turns_red_others_update`, the §7.2 AC: three nodes, the fake fails node 2. After two ticks node 2 has an `error` and nodes 0 and 1 have two history points each.
  - `backoff_caps_at_30s`
  - `hidden_window_stops_ticks`: count fake calls, set the test window invisible (check `TestWindow` support in `gpui-pre-0.3.11/src/platform/test/window.rs`), advance the clock by 30 s, and the count is unchanged.
  - `pause_stops_ticks`
  - `closing_tab_stops_polling`: drop the panel, advance the clock, and there are no calls.
  - `collections_load_on_open_and_refresh_only`
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them again.** Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(app): poll every node in parallel with per-node backoff`.

### Task 21: Overview view

**Files:**
- Modify: `crates/tessera-app/src/views/overview.rs` (header, key figures). Create `views/node_card.rs`, `views/metric_chart.rs` and `views/stats_table.rs`.
- Modify: `actions.rs` (`TogglePolling`: space within the overview, `Refresh`: cmd-r / ctrl-r), locales
- Test: `crates/tessera-app/tests/ui.rs`

**Interfaces:**
- Consumes: `ConnectionState` (Task 20), `MetricsHistory::points`, `Client::set_active_node`.
- Produces test ids: `overview.live`, `overview.pause`, `overview.interval`, `node.{i}`, `node.{i}.error`, `node.{i}.retry`, `figure.version|collections|documents|ts_memory|cpu|memory|disk|network`, `chart.cpu|memory|disk|network`, `stats.row.{endpoint}`, `stats.empty`, `overview.unsupported` (kept from M0).
- Chart: `LineChart::new(data)` (`gpui-component-0.9.0/src/chart/line_chart.rs:57`). Read the file for the x/y accessor builders. Redraw only when the history changed (spec §14).
- Sorting: clicking a header or pressing Enter on a focused header toggles ascending or descending. The default is requests per second, descending.

- [ ] **Step 1: Write the failing UI tests.**
  - `node_cards_show_roles_and_unreachable`: the fake has node 0 as leader and node 2 failing. `node.0` contains "Leader", and `node.2.error` holds the full error and a retry countdown.
  - `click_node_makes_it_active`: Enter on `node.1` gives `client.active_node() == 1`, and the charts read node 1's history.
  - `figures_show_dash_for_missing_metrics` (Review Focus 4): a sample without network fields renders `figure.network` as "–".
  - `stats_sorted_by_rps_desc`
  - `stats_empty_explains`
  - `space_pauses_polling`: the button text switches to Resume.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Manual check against the dev cluster.** Stop ts3, and its card turns red within one interval while the others keep updating. Hide the window, and polling stops (watch the request log with "Show polling" on). Check idle CPU with polling paused, for NF6.
- [ ] **Step 5: Run the checks.** Expected: PASS.
- [ ] **Step 6: Commit.** Message: `feat(app): show cluster nodes, live metrics and API stats`.

### Task 22: Docs, PR C, M1 close-out

- [ ] **Step 1: Update the docs.**
  - `CHANGELOG.md`: overview entries.
  - `docs/spec.md` §14: mark M1 done in the PR description, not in the table.
  - `docs/design/ux.md`: nothing changes unless the dock spike forced a deviation.
- [ ] **Step 2: Run the full checks, the live tests, and the cluster tests locally.** Expected: PASS.
- [ ] **Step 3: Commit, push, open the PR.** Use `superpowers:finishing-a-development-branch`. The PR body carries the binary sizes for all platforms, NF5 headroom, a GIF of the cluster overview with one node down, and the M1 acceptance checklist from spec §1 with a link to the test that proves each item.
