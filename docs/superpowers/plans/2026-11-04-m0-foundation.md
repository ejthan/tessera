# M0 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust workspace whose GPUI Kit app starts on macOS, Linux and Windows and shows a local Typesense server's version after "Connect".

**Architecture:** Three crates with a strict direction `tessera-app → tessera-core → tessera-client`; only `tessera-app` depends on GPUI Kit. Network I/O runs on one tokio runtime; GPUI tasks await it through an abort-on-drop handle. The app reaches the network through a swappable `Connector` global so UI tests can answer without real I/O.

**Tech Stack:** Rust 1.99.1 (edition 2024), gpui-kit 0.9.0 (pins gpui-pre 0.3.11), reqwest 0.13 (rustls), tokio, rust-i18n, wiremock, GitHub Actions, Docker (Typesense 30.2).

**Spec:** `docs/superpowers/specs/2026-11-04-m0-foundation-design.md` (implements M0 of `docs/spec.md`). Read both before starting.

## Global Constraints

- Everything in the repository is English, except `assets/locales/de.yml`.
- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- Direct dependencies are pinned exactly (`=x.y.z`) in `[workspace.dependencies]`; crates use `dep.workspace = true`. `Cargo.lock` is committed.
- Every crate has `publish = false`, `edition = "2024"`, `license = "GPL-3.0-or-later"`.
- Library crates (`tessera-client`, `tessera-core`): `clippy::unwrap_used = "deny"`, `missing_docs = "warn"`. No `unwrap()`/`expect()` outside tests anywhere, except documented invariants at startup in `main.rs`.
- Only `tessera-app` depends on `gpui-kit`.
- Never guess a GPUI Kit API. Look it up in the pinned source (`~/.cargo/registry/src/*/gpui-kit-0.9.0`, `gpui-component-0.9.0`, `gpui-pre-0.3.11`) or https://gpui-kit.com/llms-full.txt. When docs and source disagree, the source wins.
- All UI text goes through `t!()`; no string literal is rendered directly.
- The API key never appears in logs, `Debug`, `Display` or error text.
- Commits use `git commit -s` and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and the affected tests before every commit.
- Default request timeout: 5 s. Tokio runtime: multi-thread, 2 workers. Minimum window size: 960×600. Minimum supported Typesense: 30.0. Binary size limit: 40 MB stripped.

**Pinned versions** (crates.io, 2026-11-04): `gpui-kit =0.9.0`, `reqwest =0.13.5` (`default-features = false`, features `rustls`, `json`, `http2`, `charset`), `tokio =1.54.1`, `serde =1.0.229` (`derive`), `serde_json =1.0.154`, `thiserror =2.0.21`, `secrecy =0.10.3`, `url =2.5.8`, `semver =1.0.28`, `rust-i18n =4.2.4`, `sys-locale =0.3.2`, `tracing =0.1.44`, `tracing-subscriber =0.3.23`, `anyhow =1.0.104`, `wiremock =0.6.5`.

## Review Focus

1. **Server still starting.** Typesense 30.2 answers `GET /health` with `503 {"ok":false}` and `GET /debug` with `503 {"message":"Not Ready or Lagging"}` until it is ready. The user should see "Not Ready or Lagging", never a raw JSON blob. Pinned by `health_503_not_ok_is_not_an_error` (Task 2) and `connect_error_shows_typesense_message` (Task 6).
2. **Version without patch.** The real server reports `"30.2"`, not `"30.2.0"`. It must parse as supported, and the UI must show the server's own string (`v30.2`). Pinned by the `30.2` row in Task 1 and `connect_success_shows_version_in_status_bar` (Task 7).
3. **URL shapes people type.** `http://localhost:8108/` (trailing slash), `https://host/typesense` (path prefix) and `localhost:8108` (no scheme). The first two must hit `/health` and `/typesense/health`; the last must produce an inline "Enter a URL that starts with http:// or https://" instead of a request. Pinned by `base_url_with_path_prefix_keeps_prefix`, `rejects_non_http_scheme` (Task 2) and `invalid_url_shows_inline_error` (Task 6).
4. **Unreachable host.** A closed port must fail fast with a `Network` error, and a silent host must fail with `Timeout` after the configured timeout, not hang the dialog. Pinned by `closed_port_is_network_error` and `slow_server_times_out` (Task 2).
5. **Clicking Connect twice.** The second attempt must replace the first; the first request's result must never overwrite the second's. Pinned by `dropping_handle_aborts_task` (Task 5) and the replace-the-task rule in Task 6.

---

## File structure

```
Cargo.toml, Cargo.lock, rust-toolchain.toml, deny.toml, .gitignore
LICENSE, README.md, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, CHANGELOG.md, CLAUDE.md
crates/tessera-core/src/lib.rs            ServerCapabilities
crates/tessera-client/src/lib.rs          Client, RequestHook, re-exports
crates/tessera-client/src/error.rs        Error
crates/tessera-client/src/models.rs       Health, Debug, RaftState
crates/tessera-client/tests/client.rs     wiremock unit tests
crates/tessera-client/tests/live.rs       #[ignore] integration tests
crates/tessera-app/src/main.rs            runtime + app start + smoke mode
crates/tessera-app/src/runtime.rs         Tokio global, spawn, AbortOnDrop
crates/tessera-app/src/i18n.rs            locale selection
crates/tessera-app/src/actions.rs         actions + key bindings
crates/tessera-app/src/connection.rs      ServerInfo, Connection entity, Connector
crates/tessera-app/src/views/{mod,root,welcome,connect_dialog,overview,status_bar}.rs
crates/tessera-app/tests/ui.rs            #[gpui_kit::test] UI tests
crates/tessera-app/tests/locales.rs       en/de key parity
assets/locales/en.yml, assets/locales/de.yml
spec/typesense-openapi.yml
dev/docker-compose.yml
docs/building.md, docs/architecture/async.md, docs/design/ux.md
.github/workflows/ci.yml
```

`tessera-app` is a binary crate. For integration tests in `tests/` to reach its modules, give it both `src/lib.rs` (declaring the modules, `pub fn run()`) and a thin `src/main.rs` calling `tessera_app::run()`. Adjust the file list above accordingly.

---

### Task 1: Workspace, governance and tessera-core

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `.gitignore`, `LICENSE`, `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CHANGELOG.md`, `CLAUDE.md`
- Create: `crates/tessera-core/Cargo.toml`, `crates/tessera-core/src/lib.rs`

**Interfaces:**
- Produces: `tessera_core::ServerCapabilities { pub version: Option<semver::Version>, pub supported: bool }` and `ServerCapabilities::from_version(raw: &str) -> ServerCapabilities`.

- [ ] **Step 1: Workspace and governance files**

`Cargo.toml`: `[workspace] members = ["crates/*"]`, `resolver = "3"`, `[workspace.package]` with edition, license, `publish = false`, `[workspace.dependencies]` with the pinned versions above, `[workspace.lints]` with the lint levels from Global Constraints (library crates opt in with `[lints] workspace = true`; the app overrides `missing_docs`), and `[profile.release] strip = true`.
`rust-toolchain.toml`: `channel = "1.99.1"`, components `rustfmt`, `clippy`.
`LICENSE`: the GPL-3.0 text from https://www.gnu.org/licenses/gpl-3.0.txt.
`CODE_OF_CONDUCT.md`: Contributor Covenant 2.1. Ask the maintainer which enforcement contact to name; do not invent an email address.
`CONTRIBUTING.md`: DCO sign-off (`git commit -s`), the per-feature order from `docs/spec.md` §15 rule 3, how to run tests.
`SECURITY.md`: report through GitHub private vulnerability reporting; response target 7 days.
`CHANGELOG.md`: Keep a Changelog header with an `## [Unreleased]` section.
`README.md`: one-paragraph description, status "pre-alpha", the notice "Not affiliated with or endorsed by Typesense, Inc.", license, link to `docs/building.md`.
`CLAUDE.md`: the rules from `docs/spec.md` §15 (all 12), the UX rules from §8.5, pointers to `docs/spec.md` and `docs/superpowers/`.
`deny.toml`: `[licenses] allow = ["GPL-3.0-or-later", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "MIT", "MIT-0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "MPL-2.0", "CC0-1.0", "BSL-1.0"]`, `[bans] multiple-versions = "warn"`, `[advisories]` defaults.

- [ ] **Step 2: Write the failing tests in `crates/tessera-core/src/lib.rs`**

```rust
#[test] fn parses_full_version() { let c = ServerCapabilities::from_version("30.2.0"); assert_eq!(c.version, Some(Version::new(30, 2, 0))); assert!(c.supported); }
#[test] fn fills_missing_patch() { assert_eq!(ServerCapabilities::from_version("30.2").version, Some(Version::new(30, 2, 0))); }
#[test] fn strips_v_and_keeps_rc_as_prerelease() { let v = ServerCapabilities::from_version("v30.0.rc1").version.unwrap(); assert_eq!((v.major, v.minor, v.patch), (30, 0, 0)); assert_eq!(v.pre.as_str(), "rc1"); }
#[test] fn old_version_is_unsupported() { assert!(!ServerCapabilities::from_version("29.0").supported); }
#[test] fn garbage_is_unknown_but_supported() { let c = ServerCapabilities::from_version("nightly"); assert_eq!(c.version, None); assert!(c.supported); }
```

Note: `30.0.0-rc1` compares below `30.0.0` in semver. Treat any 30.0 pre-release as supported (compare major/minor only).

- [ ] **Step 3: Run `cargo test -p tessera-core`**, expect compile failure (`ServerCapabilities` missing).

- [ ] **Step 4: Implement `ServerCapabilities::from_version`** with `semver`. Normalize: strip leading `v`; split off a suffix after the third dot-separated part or after a non-numeric part into a pre-release; pad missing minor/patch with `0`. `supported = version.map_or(true, |v| (v.major, v.minor) >= (30, 0))`.

- [ ] **Step 5: Verify:** `cargo test -p tessera-core` passes; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean; `cargo deny check` passes. If `cargo deny` reports a license not in the list, check it against the FSF list of GPL-compatible licenses: add it if compatible, otherwise stop and ask.

- [ ] **Step 6: Commit** `chore: add workspace, governance files and tessera-core`.

---

### Task 2: tessera-client

**Files:**
- Create: `crates/tessera-client/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/models.rs`, `tests/client.rs`

**Interfaces:**
- Produces (all `pub`, documented):
  - `Client::new(base_url: &str, api_key: SecretString) -> Result<Client, Error>`
  - `Client::with_timeout(self, timeout: Duration) -> Client` (default 5 s)
  - `Client::with_hook(self, hook: RequestHook) -> Client`
  - `Client::base_url(&self) -> &Url`
  - `async fn health(&self) -> Result<Health, Error>` (`GET health`, no key needed but sent anyway)
  - `async fn debug(&self) -> Result<Debug, Error>` (`GET debug`)
  - `Health { pub ok: bool }`, `Debug { pub version: String, pub state: RaftState }`, `enum RaftState { Leader, Follower, Other(i64), Unknown }`
  - `type RequestHook = Arc<dyn Fn(&RequestEvent) + Send + Sync>`; `RequestEvent { pub method: reqwest::Method, pub path: String, pub status: Option<u16>, pub duration: Duration }`
  - `enum Error { Network(reqwest::Error), Timeout, Http { status: u16, message: String, body: String }, Decode(serde_json::Error), InvalidUrl(String) }` with `thiserror`.

- [ ] **Step 1: Write the failing tests in `tests/client.rs`** (tokio `#[tokio::test]`, wiremock `MockServer`; key `SecretString::from("test-key-123")`)

```rust
health_ok                        // 200 {"ok":true}  -> Health { ok: true }
health_503_not_ok_is_not_an_error // 503 {"ok":false} -> Ok(Health { ok: false })
debug_parses_version_and_leader  // 200 {"state":1,"version":"30.2"} -> version "30.2", RaftState::Leader
debug_maps_states                // state 4 -> Follower, 3 -> Other(3), missing -> Unknown
sends_api_key_header             // mock matches header("X-TYPESENSE-API-KEY", "test-key-123")
http_error_keeps_full_message    // 401 {"message":"Forbidden - a valid `x-typesense-api-key` header must be sent."}
                                 //   -> Error::Http { status: 401, message: <that exact string>, .. }
http_error_without_message_uses_body // 500 "oops" -> message == "oops"
base_url_with_path_prefix_keeps_prefix // Client::new(&format!("{}/typesense", server.uri()), ..); mock path "/typesense/health" answers
trailing_slash_base_works        // "{uri}/" hits "/health" (not "//health")
rejects_non_http_scheme          // Client::new("localhost:8108", ..) and ("ftp://x", ..) -> Err(Error::InvalidUrl(_))
closed_port_is_network_error     // "http://127.0.0.1:9" -> Err(Error::Network(_))
slow_server_times_out            // mock delays 500 ms, with_timeout(100 ms) -> Err(Error::Timeout)
hook_fires_once_per_request      // AtomicUsize counter; one health() -> 1 event with path "/health" and status Some(200)
key_never_printed                // format!("{client:?}"), and Debug/Display of an Http error, do not contain "test-key-123"
```

- [ ] **Step 2: Run `cargo test -p tessera-client`**, expect compile failure.

- [ ] **Step 3: Implement.** One `reqwest::Client` built once in `new`. URL handling: parse with `Url::parse`; reject schemes other than `http`/`https` with `InvalidUrl(input)`; ensure the path ends in `/`; join relative paths (`"health"`, `"debug"`). Map `reqwest::Error::is_timeout()` to `Timeout`, other send errors to `Network`. Read the body as text; for `/health` accept 200 and 503 if the body parses as `Health`; any other non-2xx becomes `Http` with `message` from a `{"message": String}` body or the raw body. Implement `Debug` for `Client` by hand with the key shown as `"••••"`. The hook gets the request path relative to the server root (`/health`, `/typesense/health` for a prefix). Emit one `tracing::debug!` per request with method, path, status and milliseconds.

- [ ] **Step 4: Run `cargo test -p tessera-client`**, expect all pass. Clippy and fmt clean.

- [ ] **Step 5: Commit** `feat(client): add Typesense client with health and debug`.

---

### Task 3: Dev server, vendored spec, live tests

**Files:**
- Create: `dev/docker-compose.yml`, `spec/typesense-openapi.yml`, `crates/tessera-client/tests/live.rs`

**Interfaces:**
- Consumes: `Client::new`, `health`, `debug` (Task 2).
- Produces: env vars for live tests: `TESSERA_IT_URL` (default `http://localhost:8108`), `TESSERA_IT_KEY` (default `xyz`).

- [ ] **Step 1: `dev/docker-compose.yml`:** service `typesense`, image `typesense/typesense:${TYPESENSE_VERSION:-30.2}`, port `8108:8108`, named volume at `/data`, command `--data-dir /data --api-key=${TYPESENSE_API_KEY:-xyz} --enable-cors=false`, a healthcheck on `/health`. A comment states that `xyz` is for local development only.

- [ ] **Step 2: Vendor the spec.** Download `openapi.yml` from typesense/typesense-api-spec at commit `2e764a81d93c71c6b086a9098fd3b674dd33456f` to `spec/typesense-openapi.yml`; prepend a YAML comment with the source URL, commit hash and date (2026-09-10).

- [ ] **Step 3: Write `tests/live.rs`**, every test `#[ignore = "needs Typesense, see dev/docker-compose.yml"]`:

```rust
live_health_ok                // eventually ok: poll health() up to 30 s until ok == true
live_debug_is_leader_and_30x  // debug(): state == RaftState::Leader, version starts with "30."
live_wrong_key_is_401         // key "wrong" -> Error::Http { status: 401, message } with message containing "x-typesense-api-key"
```

- [ ] **Step 4: Verify:** `docker compose -f dev/docker-compose.yml up -d`, then `cargo test -p tessera-client -- --ignored` passes; `docker compose -f dev/docker-compose.yml down -v`. Also confirm `cargo test -p tessera-client` (without `--ignored`) still passes without a server.

- [ ] **Step 5: Commit** `test(client): add live tests, dev server and vendored OpenAPI spec`.

---

### Task 4: App shell, i18n and smoke mode

**Files:**
- Create: `crates/tessera-app/Cargo.toml` (`[[bin]] name = "tessera"`; dev-dependency `gpui-kit` with feature `test-support`), `src/lib.rs`, `src/main.rs`, `src/i18n.rs`, `src/actions.rs`, `src/views/mod.rs`, `src/views/root.rs`, `src/views/status_bar.rs`, `assets/locales/en.yml`, `assets/locales/de.yml`, `tests/locales.rs`, `tests/ui.rs`

**Interfaces:**
- Produces: `tessera_app::run()`; `views::root::RootView::new(window: &mut Window, cx: &mut Context<RootView>) -> RootView`; `actions::{NewConnection, Quit}` via `gpui_kit::actions!(tessera, [NewConnection, Quit])` and `actions::bind_keys(cx: &mut App)`; `i18n::init()`.
- Test ids used by later tasks: `"titlebar.connection"`, `"sidebar"`, `"status.text"`.

- [ ] **Step 1: Locale files.** Flat dotted keys, `_version: 1` format of rust-i18n. Put every key and text from the table at the end of this task into both files now (later tasks only use them).

- [ ] **Step 2: Write `tests/locales.rs`**

```rust
#[test] fn en_and_de_have_same_keys()   // parse both files line by line: key = text before the first ':' on lines not starting with '#' or '_'; assert the sets are equal
#[test] fn german_lookup_works()        // tessera_app::i18n::init(); assert_eq!(rust_i18n::t!("status.not_connected", locale = "de"), "Nicht verbunden")
```

- [ ] **Step 3: Write the shell UI test in `tests/ui.rs`**

```rust
#[gpui_kit::test]
fn shell_shows_not_connected(cx: &mut TestAppContext)
// open RootView in a test window; render_frame; assert window.find("status.text") label/value contains "Not connected"
// and window.find("titlebar.connection") contains "No connection"
```

Look up the exact test-window setup in `gpui-kit-0.9.0/TESTING.md` and `tests/` of the pinned crate.

- [ ] **Step 4: Run `cargo test -p tessera-app`**, expect failure.

- [ ] **Step 5: Implement.** `i18n.rs`: `rust_i18n::i18n!("../../assets/locales", fallback = "en")` at crate root (lib.rs), `init()` sets the locale to `"de"` when `sys_locale::get_locale()` starts with `de`, else `"en"`. `run()`: initialize `tracing_subscriber::fmt()` with an env filter defaulting to `info` (enable the `env-filter` feature on `tracing-subscriber`), build the tokio runtime (2 workers, `enable_all`; `expect` with a message is allowed here), `gpui_kit::application().run(|cx| { gpui_kit::init(cx); i18n::init(); actions::bind_keys(cx); open the window })`, shut the runtime down after `run` returns. Window: title from `t!("app.title")`, minimum size 960×600, root `gpui_kit::component::Root` wrapping `RootView`. `RootView` renders `TitleBar` (connection label, test id `titlebar.connection`), a `DockArea` with a left sidebar panel (test id `sidebar`, empty while disconnected) and a center tab panel, and the status bar (`views::status_bar`, test id `status.text`). Theme: GPUI Kit default following the system appearance. `Quit` calls `cx.quit()`; bind `cmd-q` on macOS, `ctrl-q` elsewhere; `NewConnection` is bound to `cmd-n` / `ctrl-n` (its handler arrives in Task 6). Smoke mode: if `TESSERA_SMOKE` is `1`, register `window.on_next_frame` after opening the window and call `cx.quit()` there.

- [ ] **Step 6: Verify:** `cargo test -p tessera-app` passes. `cargo run -p tessera-app` opens a window titled "Tessera" with title bar, empty sidebar, empty center and status bar "Not connected"; resizing below 960×600 is blocked. `TESSERA_SMOKE=1 cargo run -p tessera-app; echo $?` prints `0` within a few seconds. `LANG=de_DE.UTF-8` shows "Nicht verbunden" (on macOS the system language decides; check with the app's locale set to German in System Settings if `LANG` is ignored).

- [ ] **Step 7: Commit** `feat(app): add app shell, i18n and smoke mode`.

**Locale texts (exact copy):**

| Key | en | de |
|---|---|---|
| `app.title` | Tessera | Tessera |
| `titlebar.no_connection` | No connection | Keine Verbindung |
| `status.not_connected` | Not connected | Nicht verbunden |
| `status.connected` | Connected | Verbunden |
| `sidebar.overview` | Overview | Übersicht |
| `welcome.title` | Welcome to Tessera | Willkommen bei Tessera |
| `welcome.body` | Browse collections, run searches and manage keys on any Typesense cluster. | Collections durchsuchen, Suchen ausführen und Keys verwalten, auf jedem Typesense-Cluster. |
| `welcome.add_connection` | Add connection | Verbindung hinzufügen |
| `welcome.add_connection_hint` | Enter the server URL and an API key. | Server-URL und API-Key eingeben. |
| `welcome.run_locally` | Run Typesense locally | Typesense lokal starten |
| `welcome.run_locally_hint` | Copy this Docker command. Tessera shows it but never runs it. | Kopiere diesen Docker-Befehl. Tessera zeigt ihn nur an und führt ihn nie selbst aus. |
| `welcome.copy` | Copy | Kopieren |
| `welcome.copied` | Copied | Kopiert |
| `welcome.trademark` | Tessera is not affiliated with or endorsed by Typesense, Inc. | Tessera steht in keiner Verbindung zu Typesense, Inc. und wird nicht von Typesense, Inc. unterstützt. |
| `connect.title` | Connect to Typesense | Mit Typesense verbinden |
| `connect.url` | Server URL | Server-URL |
| `connect.api_key` | API key | API-Key |
| `connect.show_key` | Show key | Key anzeigen |
| `connect.hide_key` | Hide key | Key verbergen |
| `connect.submit` | Connect | Verbinden |
| `connect.cancel` | Cancel | Abbrechen |
| `connect.error.invalid_url` | Enter a URL that starts with http:// or https://. | Gib eine URL ein, die mit http:// oder https:// beginnt. |
| `connect.error.network` | Could not reach %{host}. | %{host} ist nicht erreichbar. |
| `connect.error.timeout` | %{host} did not answer within 5 seconds. | %{host} hat nicht innerhalb von 5 Sekunden geantwortet. |
| `connect.error.http` | Typesense answered %{status}: %{message} | Typesense antwortete mit %{status}: %{message} |
| `connect.error.decode` | Unexpected answer from %{host}. Is this a Typesense server? | Unerwartete Antwort von %{host}. Ist das ein Typesense-Server? |
| `overview.title` | Overview | Übersicht |
| `overview.version` | Version | Version |
| `overview.role` | Raft role | Raft-Rolle |
| `overview.health` | Health | Zustand |
| `overview.healthy` | Healthy | Bereit |
| `overview.unhealthy` | Unhealthy | Nicht bereit |
| `overview.unsupported` | Tessera supports Typesense 30.0 and later. Some features may not work with %{version}. | Tessera unterstützt Typesense ab 30.0. Mit %{version} funktionieren manche Funktionen eventuell nicht. |
| `role.leader` | Leader | Leader |
| `role.follower` | Follower | Follower |
| `role.other` | State %{state} | Status %{state} |
| `role.unknown` | Unknown | Unbekannt |

---

### Task 5: Tokio bridge

**Files:**
- Create: `crates/tessera-app/src/runtime.rs`, `docs/architecture/async.md`
- Modify: `crates/tessera-app/src/lib.rs` (register the global in `run()`)

**Interfaces:**
- Produces: `runtime::Tokio(pub tokio::runtime::Handle)` implementing `gpui_kit::Global`; `runtime::spawn<F>(cx: &App, future: F) -> AbortOnDrop<F::Output> where F: Future + Send + 'static, F::Output: Send + 'static`; `AbortOnDrop<T>` implementing `Future<Output = Result<T, tokio::task::JoinError>>` and `Drop` (calls `abort()`); `AbortOnDrop::from_join_handle(h: JoinHandle<T>) -> Self` for tests.

- [ ] **Step 1: Write the failing tests** (unit tests in `runtime.rs`, plain `#[tokio::test]`, no GPUI):

```rust
awaits_result            // AbortOnDrop::from_join_handle(tokio::spawn(async { 7 })).await.unwrap() == 7
dropping_handle_aborts_task
// a oneshot sender is moved into a spawned task that sleeps 10 s and then sends;
// drop the AbortOnDrop; the receiver must yield Err(RecvError) within 1 s (the task and its sender were dropped)
```

- [ ] **Step 2: Run `cargo test -p tessera-app runtime`**, expect failure.

- [ ] **Step 3: Implement** `AbortOnDrop` by pinning the inner `JoinHandle` (it is `Unpin`) and forwarding `poll`. `spawn` reads `cx.global::<Tokio>()` and calls `handle.spawn(future)`. Register `Tokio(runtime.handle().clone())` with `cx.set_global` in `run()`.

- [ ] **Step 4: Run tests**, expect pass.

- [ ] **Step 5: Write `docs/architecture/async.md`** (one page): why two executors, the `spawn` + `AbortOnDrop` pattern with the code sample from the design §5, the rule "keep the GPUI `Task` in view state; replacing it cancels the request", ignoring `this.update` errors after the view is gone, and "no blocking I/O on the main thread".

- [ ] **Step 6: Commit** `feat(app): add tokio bridge with abort-on-drop`.

---

### Task 6: Connection state, welcome screen and connect dialog

**Files:**
- Create: `crates/tessera-app/src/connection.rs`, `src/views/welcome.rs`, `src/views/connect_dialog.rs`
- Modify: `src/views/root.rs`, `src/lib.rs`, `tests/ui.rs`

**Interfaces:**
- Consumes: `tessera_client::{Client, Error, RaftState}`, `tessera_core::ServerCapabilities`, `runtime::spawn`.
- Produces:
  - `ServerInfo { pub url: Url, pub version: String, pub state: RaftState, pub healthy: bool, pub capabilities: ServerCapabilities, pub latency: Duration }` (`Clone`)
  - `type Connector = Arc<dyn Fn(String, SecretString) -> Pin<Box<dyn Future<Output = Result<ServerInfo, tessera_client::Error>> + Send>> + Send + Sync>`; global `ConnectorGlobal(pub Connector)`
  - `connection::tokio_connector(handle: tokio::runtime::Handle) -> Connector`: builds a `Client`, runs `health` then `debug` on the runtime, measures `debug` latency, returns `ServerInfo`. A `JoinError` can only be a panic here (abort happens only when the future is dropped), so propagate it with `std::panic::resume_unwind(e.into_panic())`.
  - `Connection` entity: `pub info: Option<ServerInfo>`, `fn set(&mut self, info: ServerInfo, cx: &mut Context<Self>)` (notifies).
  - `connection::error_text(err: &tessera_client::Error, host: &str) -> String` using the `connect.error.*` keys (`Http` → `connect.error.http` with the full `message`; `InvalidUrl` → `connect.error.invalid_url`).
  - Test ids: `"welcome.add_connection"`, `"welcome.copy"`, `"connect.url"`, `"connect.key"`, `"connect.key_toggle"`, `"connect.submit"`, `"connect.cancel"`, `"connect.error"`.

- [ ] **Step 1: Write the failing UI tests in `tests/ui.rs`.** Each test installs a fake `ConnectorGlobal` that returns `Box::pin(async { … })` immediately.

```rust
new_connection_opens_dialog_with_url_focused
// press "cmd-n" (or dispatch NewConnection); wait_for find("connect.url") visible; assert it is focused and its value == "http://localhost:8108"
welcome_button_opens_dialog        // click("welcome.add_connection") -> "connect.url" visible
connect_error_shows_typesense_message
// fake returns Err(Error::Http { status: 503, message: "Not Ready or Lagging".into(), body: String::new() })
// press "enter" in the dialog; wait_for find("connect.error") contains "Not Ready or Lagging"; dialog still open
invalid_url_shows_inline_error
// input "localhost:8108" into connect.url (the real tokio_connector is NOT needed: the fake returns Err(Error::InvalidUrl(..)) for any URL without "://")
// press enter; "connect.error" contains "http:// or https://"
escape_closes_dialog               // press "escape" -> "connect.url" no longer present
key_is_masked_by_default           // find("connect.key") reports a masked value; click("connect.key_toggle") reveals it
```

- [ ] **Step 2: Run `cargo test -p tessera-app --test ui`**, expect failure.

- [ ] **Step 3: Implement.**
  - `lib.rs::run()` installs `ConnectorGlobal(tokio_connector(handle))` and a `Connection` entity owned by `RootView`.
  - `WelcomeView` (center area while disconnected): logo-free title, body, two action cards (Add connection with the `⌘N`/`Ctrl+N` hint, Run Typesense locally with the Docker command `docker run -p 8108:8108 -v "$(pwd)/typesense-data:/data" typesense/typesense:30.2 --data-dir /data --api-key=xyz` and a Copy button that writes it to the clipboard and swaps its label to `welcome.copied` for 2 s), trademark line. Layout follows canvas screen 1.
  - `ConnectDialog` via GPUI Kit's dialog component: URL input prefilled `http://localhost:8108`, masked key input prefilled from `TYPESENSE_API_KEY` (wrapped in `SecretString`; read once at dialog creation), show/hide toggle, Cancel and Connect. Focus the URL input on open. Enter submits, Escape closes. On submit: clear the error, disable Connect and show a spinner, call the connector, and store the GPUI task in `self.connect_task` (replacing any previous task, which cancels it). On `Ok(info)`: `connection.set(info)`, close the dialog. On `Err(e)`: show `error_text(&e, host)` in the `connect.error` element below the fields, re-enable Connect.
  - The key must not be cloned into any `String`; pass the `SecretString` through.

- [ ] **Step 4: Run `cargo test -p tessera-app`**, expect all pass.

- [ ] **Step 5: Manual check** with `docker compose -f dev/docker-compose.yml up -d` and `TYPESENSE_API_KEY=xyz cargo run -p tessera-app`: ⌘N, Enter connects (dialog closes); with key `wrong` the dialog shows the Forbidden message in full; with the server stopped it shows "Could not reach localhost:8108."

- [ ] **Step 6: Commit** `feat(app): add welcome screen and connect dialog`.

---

### Task 7: Overview, sidebar and status bar after connect

**Files:**
- Create: `crates/tessera-app/src/views/overview.rs`
- Modify: `src/views/root.rs`, `src/views/status_bar.rs`, `tests/ui.rs`

**Interfaces:**
- Consumes: `Connection`, `ServerInfo`, `ServerCapabilities` (Tasks 1, 6).
- Produces: test ids `"overview.version"`, `"overview.role"`, `"overview.health"`, `"overview.unsupported"`, `"sidebar.overview"`.

- [ ] **Step 1: Write the failing UI tests**

```rust
connect_success_shows_version_in_status_bar
// fake returns Ok(ServerInfo { url: "http://localhost:8108/", version: "30.2", state: Leader, healthy: true, capabilities: from_version("30.2"), latency: 11 ms })
// connect; wait_for: "status.text" contains "Connected", "localhost:8108", "v30.2", "Leader", "11 ms"
// "overview.version" contains "30.2"; "sidebar.overview" visible; "titlebar.connection" contains "localhost:8108"
old_server_shows_warning           // version "29.0" -> "overview.unsupported" visible and contains "29.0"
unhealthy_server_shows_unhealthy   // healthy: false -> "overview.health" contains "Unhealthy"
```

- [ ] **Step 2: Run them**, expect failure.

- [ ] **Step 3: Implement.** `RootView` observes `Connection`; when `info` is `Some`, the center shows `OverviewView` instead of the welcome screen, the sidebar shows the host with one entry "Overview" (selected), and the title bar label shows `host:port`. `OverviewView`: three cards (version as the raw server string, Raft role via `role.*` keys, health with green "Healthy" or red "Unhealthy"), plus the warning banner (`overview.unsupported`) above the cards when `capabilities.supported` is false. Status bar when connected: `● {status.connected} · {host:port} · v{version} · {role} · {latency_ms} ms`, separators rendered as `·`.

- [ ] **Step 4: Run `cargo test -p tessera-app`**, expect all pass.

- [ ] **Step 5: Manual check** against the dev server in light and dark system appearance; compare with canvas screens 1–3 for spacing and hierarchy (the canvas is the M1 target; M0 shows only the subset in the design §7).

- [ ] **Step 6: Commit** `feat(app): show overview and status after connecting`.

---

### Task 8: CI and remaining docs

**Files:**
- Create: `.github/workflows/ci.yml`, `docs/building.md`, `docs/design/ux.md`
- Modify: `CHANGELOG.md`, `README.md`

- [ ] **Step 1: `docs/building.md`:** prerequisites per OS, copied from the GPUI Kit 0.9 docs: macOS 15+ with Xcode Command Line Tools; Windows 10+ with Visual Studio 2022 Build Tools (Desktop development with C++), MSVC toolchain, CMake on PATH; Ubuntu 24.04 packages `gcc g++ clang libfontconfig-dev libwayland-dev libwebkit2gtk-4.1-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev vulkan-validationlayers libvulkan1`. Then `cargo run -p tessera-app`, the dev server, and how to run live tests.

- [ ] **Step 2: `docs/design/ux.md`:** link to the canvas (https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d), one line per screen describing it, and the five UX rules.

- [ ] **Step 3: `.github/workflows/ci.yml`** with jobs:
  - `lint` (ubuntu-24.04): install the Linux packages, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `EmbarkStudios/cargo-deny-action` (pin a release tag).
  - `test` (matrix ubuntu-24.04, macos-15, windows-2025): `cargo test --workspace --locked`.
  - `integration` (ubuntu-24.04, matrix `typesense: ["30.0", "30.2"]`): service `typesense/typesense:${{ matrix.typesense }}` with `--data-dir /tmp --api-key=xyz`, port 8108; `cargo test -p tessera-client --locked -- --ignored`.
  - `build` (matrix macos-15 arm64, the current macOS Intel runner label (check GitHub's runner list; `macos-15-intel` at the time of writing), ubuntu-24.04, windows-2025): `cargo build --release --locked -p tessera-app` (stripped through `[profile.release] strip = true` from Task 1); fail if the binary exceeds 41,943,040 bytes (`shell: bash`, `wc -c`); upload the binary as an artifact; smoke start with `TESSERA_SMOKE=1` and a 60 s timeout (Linux: install `xvfb mesa-vulkan-drivers` and run under `xvfb-run -a`).
  - Cache with `Swatinem/rust-cache` (pin a release tag).
- [ ] **Step 4: Validate the workflow** with `actionlint` if available (`brew install actionlint` is allowed on the maintainer's machine only after asking); otherwise review the YAML against GitHub's schema by reading it once more.
- [ ] **Step 5: `CHANGELOG.md`** `Unreleased` → `Added`: connect to a Typesense server and see its version, role and health; English and German UI. `README.md`: build/run section pointing to `docs/building.md`.
- [ ] **Step 6: Commit** `ci: add lint, test, integration and build workflow`.

---

### Task 9: Publish and prove M0 on all platforms

This task touches GitHub. Ask the maintainer before each outward step.

- [ ] **Step 1: Ask** for the repository owner and name, then create the public repo with `gh repo create <owner>/<name> --public --source . --remote origin` and push `main`, only after the maintainer confirms.
- [ ] **Step 2: Watch the first CI run** (`gh run watch`). Fix failures in small commits.
- [ ] **Step 3: If the smoke start fails on Linux or Windows because no renderer is available** (and not because of a bug), apply the fallback from the design §12: keep the macOS smoke start, mark the other smoke steps `continue-on-error: true` with a comment linking the design, start the app once manually in a Linux and a Windows VM, record the result in the PR or CHANGELOG, and update the M0 acceptance note in `docs/spec.md` §14.
- [ ] **Step 4: Acceptance:** all jobs green (or the documented fallback), and "Connect" against `dev/docker-compose.yml` shows `v30.2` in the status bar on macOS. Record a short GIF of the flow for the milestone demo.
