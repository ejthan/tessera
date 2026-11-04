# M0 Foundation: design

- **Date:** 2026-11-04
- **Status:** approved in brainstorming, written spec pending review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M0 row in §14)
- **UX mockups:** [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d)

## 1. Goal

M0 lays the foundation every later milestone builds on: a workspace that builds and tests on macOS, Linux and Windows, a GPUI Kit window with the app shell, a working async bridge between GPUI and tokio, and a client that talks to Typesense.

**Acceptance:** the app starts on macOS, Linux and Windows, and "Connect" against a local Typesense shows its version.

The approach is a thin vertical slice. M0 builds exactly what the acceptance criterion proves, plus the guard rails (CI, lints, conventions) later milestones depend on. Everything else waits for the milestone that gives it a purpose.

## 2. Scope

### In M0

- Cargo workspace with `tessera-client`, `tessera-core`, `tessera-app`
- `tessera-client`: `health()`, `debug()`, error type, request hook
- `tessera-core`: `ServerCapabilities` from the server version
- `tessera-app`: tokio bridge, app shell, welcome screen, connect dialog, overview cards, status bar, i18n (EN, DE)
- Tests at every level listed in §8, CI as in §9
- Governance files, `CLAUDE.md`, vendored OpenAPI spec, `dev/docker-compose.yml`, `docs/building.md`, `docs/architecture/async.md`, `docs/design/ux.md`

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| `tessera-widgets` crate | M2 | No reusable widget exists in M0 |
| `xtask seed` and data sets | M2 (small sets), M3 (200k `products`) | Seeding needs import endpoints the client gets later; until then nothing would use the data |
| `xtask dist`, packaging (`cargo-packager`) | M9 | Releases start at 1.0; CI uploads raw binaries until then |
| `xtask icons`, Lucide assets | first milestone that needs icons beyond GPUI Kit's set | |
| Profiles, keychain, env tags, read-only | M1 | |
| Bottom dock (request log), metrics, cluster view | M1 | No dead panels (UX rule 4) |
| Command palette search field | M4 | No dead controls |
| 3-node docker-compose profile | M1 | Comes with the cluster view |
| Layout persistence, several windows | later | |

## 3. Repository layout after M0

```
tessera/
├── Cargo.toml              # workspace, resolver 3, edition 2024, exact pins (=x.y.z)
├── Cargo.lock              # committed
├── rust-toolchain.toml     # pinned stable toolchain
├── deny.toml
├── crates/
│   ├── tessera-client/
│   ├── tessera-core/
│   └── tessera-app/        # binary `tessera`, the only crate depending on gpui-kit
├── assets/locales/         # en.yml, de.yml
├── spec/typesense-openapi.yml   # header comment records the upstream commit hash
├── dev/docker-compose.yml
├── docs/
│   ├── spec.md
│   ├── building.md
│   ├── architecture/async.md
│   ├── design/ux.md
│   └── superpowers/specs/, plans/
├── .github/workflows/ci.yml
├── CLAUDE.md, README.md, LICENSE, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, CHANGELOG.md
```

Conventions for every crate: `publish = false`, an SPDX header (`// SPDX-License-Identifier: GPL-3.0-or-later`) in every source file. Library crates set `clippy::unwrap_used = "deny"` and `missing_docs = "warn"`.

The toolchain is pinned to the current stable release (1.99.1 at the time of writing), unless the MSRV of `gpui-pre` requires something newer.

## 4. Crates

### 4.1 tessera-client

No GPUI dependency. Built on `reqwest` (rustls) and `tokio`.

```rust
pub struct Client { /* reqwest::Client, base Url, SecretString key, hook */ }

impl Client {
    pub fn new(base_url: &str, api_key: SecretString) -> Result<Self, Error>;
    pub fn with_timeout(self, timeout: Duration) -> Self;
    pub fn with_hook(self, hook: RequestHook) -> Self;
    pub async fn health(&self) -> Result<Health, Error>;   // GET /health
    pub async fn debug(&self) -> Result<Debug, Error>;     // GET /debug
}

pub struct Health { pub ok: bool }
pub struct Debug  { pub version: String, pub state: RaftState }
pub enum RaftState { Leader, Follower, Other(i64), Unknown }

pub type RequestHook = Arc<dyn Fn(&RequestEvent) + Send + Sync>;
pub struct RequestEvent { pub method: Method, pub path: String, pub status: Option<u16>, pub duration: Duration }
```

- Fixed request timeout of 5 s in M0; configurable per profile in M1.
- The key goes out in the `X-TYPESENSE-API-KEY` header. `Client` implements `Debug` by hand and prints the key as `••••`.
- The OpenAPI spec documents only `version` for `/debug`. A live 30.2 single node answers `{"state":1,"version":"30.2"}`, which confirms `1` = leader. `4` = follower comes from braft's state enum and is confirmed by the cluster tests in M1. A missing `state` maps to `RaftState::Unknown`. Note that the version has no patch component.
- `with_timeout(Duration)` overrides the 5 s default; tests use it to keep timeout tests fast.
- `Client::new` accepts only `http` and `https` URLs and appends a trailing `/` to the path, so that a path prefix such as `https://host/typesense` keeps working when endpoints are joined.
- The hook is called once per request, after the response or error. In M0 its one consumer is a `tracing` line per request (method, path, status, duration, never the key). M1 attaches the request log and the read-only guard to the same hook.

```rust
#[derive(thiserror::Error)]
pub enum Error {
    Network(reqwest::Error),
    Timeout,
    Http { status: u16, message: String, body: String },
    Decode(serde_json::Error),
    InvalidUrl(String),
}
```

`Http.message` is the `message` field of a Typesense error body when present, otherwise the raw body. It is never truncated (NF7). `Error::typesense_message()` tells the two apart: the app shows a Typesense message in full, but a body without one (a proxy's HTML error page, a wrong path prefix) becomes "{host} answered HTTP {status}, which does not look like Typesense. Check the URL and path prefix." The client never follows redirects, so the API key cannot reach another host. Variants from product spec §5.5 that M0 cannot produce (`Tls`, `Cancelled`, `Unsupported`) are added when a code path first produces them.

### 4.2 tessera-core

No GPUI dependency.

```rust
pub struct ServerCapabilities { pub version: Option<semver::Version>, pub supported: bool }
impl ServerCapabilities { pub fn from_version(raw: &str) -> Self; }
```

Parsing is lenient: a leading `v` is stripped, a missing patch is filled with `0` (`30.2` → `30.2.0`), suffixes such as `.rc1` become pre-release tags. A string that still does not parse yields `version: None`, which counts as unknown rather than as an error. `supported` is true for versions ≥ 30.0 and for unknown versions (we cannot tell, so we do not block).

### 4.3 tessera-app

The binary `tessera`. Modules:

- `main.rs`: builds the tokio runtime, starts `gpui_kit::application()`, calls `gpui_kit::init`, loads the locale, opens the window.
- `runtime.rs`: the tokio bridge (§5).
- `connection.rs`: `ConnectionState` entity (client, `Debug` result, capabilities, latency, status) and the `Connector` global: a function from URL and key to a future of the connection result. The app installs the real one (client on tokio); UI tests install a fake that answers immediately, because GPUI's test executor runs on a simulated clock and does not wait for real network I/O (GPUI Kit testing guide: "provide controlled responses for external dependencies").
- `views/`: `root.rs` (shell), `welcome.rs`, `connect_dialog.rs`, `overview.rs`, `status_bar.rs`.
- `i18n.rs`: `rust_i18n::i18n!("../../assets/locales", fallback = "en")` and locale selection via `sys-locale`.

Exact GPUI Kit APIs (Dock, Dialog, Input, TitleBar, StatusBar, clipboard, test support) are looked up in the source of the pinned `gpui-kit` 0.9.0 during implementation, never guessed.

## 5. Async bridge

- `main` builds one `tokio::runtime::Runtime` (`multi_thread`, 2 workers, `enable_all`) and registers its `Handle` as the GPUI global `Tokio`. The runtime lives in `main` and is shut down after `run()` returns.
- One helper: `Tokio::spawn(cx, future) -> AbortOnDrop<T>`. `AbortOnDrop` wraps the tokio `JoinHandle`, implements `Future<Output = Result<T, JoinError>>`, and calls `abort()` in `Drop`.
- Views keep the GPUI `Task` in their state. Replacing or dropping it (a new click on Connect, closing the view) drops the awaited `AbortOnDrop`, which aborts the tokio task. No `CancellationToken` is needed in M0.

```rust
self.connect_task = Some(cx.spawn(async move |this, cx| {
    let result = io.await;                       // AbortOnDrop<Result<…>>
    this.update(cx, |view, cx| { view.apply(result); cx.notify(); }).ok();
}));
```

- `this.update` fails once the view is gone; that error is ignored on purpose, because there is nothing left to update.
- Progress channels and UI throttling arrive in M5 with import.
- `docs/architecture/async.md` documents this pattern on one page.

## 6. Connect flow and errors

1. The user opens the connect dialog (welcome screen button or ⌘N / Ctrl+N).
2. URL defaults to `http://localhost:8108`; the key field is prefilled from `TYPESENSE_API_KEY` if set, otherwise empty. The key lives only in memory as a `SecretString`; M0 persists nothing. M0 reads no other environment variables, because M1 defines the full environment import (`TYPESENSE_HOST`, `TYPESENSE_PORT` …, product spec §7.1) and M0 should not invent a variable M1 would replace.
3. Enter or "Connect" builds a `Client` and runs `health()` then `debug()` on tokio. The button is disabled and shows a spinner meanwhile.
4. On success the dialog closes, `ConnectionState` is filled (version, Raft state, capabilities, latency of the `debug` call), and the main area switches to the overview.
5. On failure the dialog stays open and shows the error inline: the full Typesense message for `Http`, a plain sentence for `Network` ("Could not reach localhost:8108") and `Timeout`.
6. `health` answering `{"ok": false}` (Typesense sends this with status 503 while starting or lagging, verified against 30.2) is not an error: `health()` returns `Health { ok: false }` and the flow continues to `debug`. If `debug` then fails, its message ("Not Ready or Lagging") is shown as in step 5. If it succeeds, the overview's health card shows "Unhealthy" in red. Any other non-2xx `health` response is an `Http` error.
7. A version below 30.0 still connects, but the overview shows a warning banner ("Tessera supports Typesense 30.0 and later. Some features may not work.").

No error text or `Debug` output ever contains the key; a unit test asserts this.

## 7. UI

The canvas shows the M1 target. M0 builds this subset:

| Canvas screen | In M0 | Later |
|---|---|---|
| 1 · First run | Welcome screen: "Add connection" (⌘N), "Run Typesense locally" with the Docker command and a Copy button, trademark notice | "Import from typesense-dashboard" (M1) |
| 2 · Connection dialog | URL, API key (masked, with show toggle), Connect, inline result | Name, color, environment, nodes, read-only, keychain, advanced (M1) |
| 3 · Overview | Cards for version, Raft role, health; warning banner below 30.0 | Nodes, metrics, API stats, request log (M1) |
| 4 · Production safety | – | M1 (read-only), M8 (all danger levels) |
| 5 · Document browser | – | M3 |

**Shell:** `Root` → `TitleBar` (connection switcher showing "No connection" or `host:port`) → `DockArea` with a left sidebar and a center tab panel → `StatusBar`. Once connected, the sidebar shows the instance with one entry, "Overview". The status bar shows either "Not connected" or `● Connected · host · v30.2.0 · leader · 11 ms`.

**Window:** title "Tessera", minimum size 960×600. The theme follows the system light/dark setting with GPUI Kit's default tokens. M0 adds no colors of its own beyond status green, red and amber.

**Keyboard:** ⌘N / Ctrl+N opens the dialog, focus starts in the first field, Tab moves between fields, Enter connects, Esc closes the dialog, ⌘Q / Ctrl+Q quits. Shortcuts are shown on their controls.

**i18n:** every visible string goes through `t!()`. Keys are English and namespaced (`welcome.title`, `connect.submit`, `status.connected`). `assets/locales/en.yml` and `de.yml` hold the texts. The locale comes from `sys-locale`; anything not German falls back to English. German strings for GPUI Kit's built-in components are added once a component with built-in text appears; M0 uses none.

**Accessibility and testability:** every interactive element gets a `.test_support()` id, which also supplies its accessibility name.

**UX rules** (also in `docs/design/ux.md`, `CLAUDE.md` and product spec §8.5):
1. A connection's environment is visible everywhere: title bar, tabs, sidebar, and for prod the window frame.
2. Errors show where they happen, inline and in full. Toasts are only for background events.
3. Default focus is never on a destructive action. High danger means typing the name, and the dialog states the concrete consequences.
4. No dead controls: navigation entries, panels and buttons appear in the milestone that makes them work.
5. Every action is reachable by keyboard, and its shortcut is shown on the control.

## 8. Testing

| Level | What it checks |
|---|---|
| Client unit (`wiremock`) | `health` and `debug` parse; a `{"message"}` body lands in full in `Error::Http`; timeout maps to `Error::Timeout`; the key appears in neither `Debug` nor `Display`; the hook fires once per request |
| Core unit | Version parsing table: `30.2.0`, `30.2`, `v30.0.rc1`, `29.0` (unsupported), garbage (unknown, supported) |
| Client integration | Against real Typesense. Marked `#[ignore = "needs Typesense, see dev/docker-compose.yml"]` so local runs neither fake a pass nor fail; CI runs them with `--ignored` |
| App bridge | Dropping an `AbortOnDrop` stops the tokio task |
| App UI (`#[gpui_kit::test]`, fake `Connector`) | ⌘N opens the dialog with focus in the URL field; Enter with a successful fake shows the version in the status bar; a fake 401 shows the Typesense message inline |
| i18n | `en.yml` and `de.yml` have exactly the same keys |

## 9. CI

`.github/workflows/ci.yml`, on push and pull request:

- **lint** (ubuntu): `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo deny check`. `deny.toml` allows only licenses compatible with GPL-3.0.
- **test** (ubuntu, macos, windows): `cargo test --workspace`, including the UI tests, which run on GPUI's test platform without a display.
- **integration** (ubuntu): Typesense service container, matrix 30.0 and 30.2, then `cargo test -- --ignored`.
- **build** (macOS arm64, macOS x64, Linux x64, Windows x64): release build, stripped binary size check (< 40 MB, NF5), artifact upload, and a smoke start.

**Smoke start:** with `TESSERA_SMOKE=1` set, the app quits with exit code 0 after its first rendered frame. On Linux it runs under `xvfb-run`. This is how CI proves "the app starts" on every platform.

Linux build dependencies are taken from the GPUI Kit docs, installed by the CI script, and listed in `docs/building.md`.

## 10. Dev environment

`dev/docker-compose.yml` runs one `typesense/typesense:30.2` node on port 8108 with a named data volume. The API key comes from `TYPESENSE_API_KEY` and falls back to `xyz`; a comment marks that value as local-only.

## 11. Repository conventions

- Commits are signed off (`git commit -s`, DCO).
- `CLAUDE.md` contains the working rules from product spec §15, the UX rules, and the language rule (English everywhere except `assets/locales/de.yml`).
- `CHANGELOG.md` starts with an `Unreleased` section; M0 adds its entries there.
- README carries the trademark notice and build instructions pointing to `docs/building.md`.

## 12. Risks

| Risk | Impact | Fallback |
|---|---|---|
| GPUI cannot render on GPU-less CI runners (Linux with software Vulkan, Windows) | Smoke start fails on those platforms | Smoke start on macOS only, one manual start in a Linux and a Windows VM, and the product spec's M0 acceptance note updated accordingly |
| `gpui-kit` 0.9.0 APIs differ from the docs | Slower implementation | Rule: the pinned source wins; the plan starts with the shell and the bridge so surprises show up first |
| `gpui-pre` needs a newer Rust than current stable | Toolchain pin changes | Pin whatever `gpui-pre` requires |
| GitHub's macOS x64 runner label changes | Build job breaks | Check the current label when writing the workflow |
