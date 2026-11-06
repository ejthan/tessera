# Tessera product specification

> **Name:** Tessera
> **Status:** v0.2 · 2026-11-04 (translated from the German v0.1 draft; open decisions resolved, see §16)
> **Stack:** 100% Rust · GPUI + GPUI Kit · no WebView, no Electron, no JavaScript
> **Target server:** Typesense ≥ 30.0 (reference: 30.2)
> **License:** GPL-3.0-or-later (see §2)

This document is the binding specification for the full feature set of version 1.0 and the roadmap after it. It is written for humans **and** coding agents (Claude Code). §15 holds explicit working rules for agents.

Each milestone gets its own design spec and implementation plan under `docs/superpowers/`. Where a milestone design deviates from this document, the milestone design records why, and this document is updated.

---

## Contents

1. Vision, goals, non-goals
2. License, governance, legal notes
3. Target platforms
4. Tech stack and dependencies
5. Architecture
6. Typesense API coverage (endpoint matrix)
7. Functional specification (all modules)
8. UI/UX specification
9. Security and privacy
10. Non-functional requirements
11. Persistence and configuration
12. Testing and quality assurance
13. CI/CD, packaging, releases
14. Milestones
15. Working rules for Claude Code and other coding agents
16. Decisions
17. References

---

## 1. Vision, goals, non-goals

### 1.1 Vision

A fast, native, secure open source desktop client for Typesense, comparable to Elasticvue for Elasticsearch. It covers **the entire Typesense admin API**, stays smooth with hundreds of thousands of documents, and guards against dangerous actions on production clusters.

The functional reference is [bfritscher/typesense-dashboard](https://github.com/bfritscher/typesense-dashboard). Tessera matches its feature set and improves on it in specific places (see 1.3).

### 1.2 Goals

- **G1 – Completeness:** Every admin resource of Typesense 30.x can be managed through the UI. Anything without its own UI yet is reachable through the raw API console.
- **G2 – Performance:** Cold start under 1 s. Smooth scrolling (≥ 60 fps, target 120 fps) in tables with 100,000+ rows. Import and export with constant memory (streaming).
- **G3 – Security:** API keys live only in the OS keychain. Production protection mode. No telemetry.
- **G4 – Native:** Small binaries (target < 40 MB), native Apple Silicon, no CORS needed.
- **G5 – Transparency:** Every HTTP request is visible in the request log and can be copied as `curl`.
- **G6 – Keyboard-first:** Command palette, full keyboard control, configurable keymap.

### 1.3 Improvements over the reference (bfritscher)

| Area | bfritscher (Electron/Vue) | Tessera |
|---|---|---|
| Runtime | Electron, about 130–360 MB, macOS x64 only | Native Rust, native arm64 and x64 |
| CORS | Web version needs `--enable-cors`, Instant Search needs it on desktop too | never needed, all requests are native |
| Large collections | Card view, limited | virtual DataTable, 100k+ rows |
| Import/export | Browser timeouts on large files | Streaming, progress, cancel, per-line error report |
| Snapshots | missing | ✔ |
| Delete by query | missing | ✔ with a preview of the match count |
| Scoped search keys | missing | ✔ local generator and decoder |
| Key storage | localStorage / config.json | OS keychain |
| Production protection | – | Environment tags, read-only mode, type-to-confirm |
| Request transparency | – | Request log with "Copy as curl" |
| Raw API | – | Raw API console |
| JSON editing | Text area | Code editor (Tree-sitter JSON, folding, validation) |
| Relevance tuning | – | Side-by-side comparison of two parameter sets (v1.x) |

### 1.4 Non-goals (v1.0)

- No web or WASM version. GPUI Kit supports WASM, but in a browser CORS would be needed again. Possible later.
- No support for the Typesense Cloud Management API (provisioning clusters, billing). Cluster API only.
- No server deployment, no hosting.
- No replacement for monitoring systems such as Prometheus. Only live metrics with a short history.
- No mobile apps.

---

## 2. License, governance, legal notes

### 2.1 License: GPL-3.0-or-later

The goal was "open source, not commercial". A license that forbids commercial use (for example CC BY-NC or PolyForm Noncommercial) is **not** open source under the OSI definition. The OSI-compliant choice that comes closest is a **copyleft license**:

- **GPL-3.0-or-later.** Anyone may use the app, companies included. Anyone who modifies and distributes it must publish the source under the same license. Proprietary closed-source forks are ruled out.
- Compatible with the dependencies: GPUI and GPUI Kit are Apache-2.0, which is compatible with GPL-3.0.
- Compatible with the reference bfritscher/typesense-dashboard (GPL-3.0). Ideas and, where useful, logic may be reused. Still: **rewrite, don't copy** (Rust is not Vue).

### 2.2 Governance files

- `LICENSE` (full GPL-3.0 text), SPDX header `// SPDX-License-Identifier: GPL-3.0-or-later` in every source file
- `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `SECURITY.md` (how to report vulnerabilities)
- `CHANGELOG.md` following *Keep a Changelog*, versioning per SemVer
- **DCO** (`Signed-off-by`) instead of a CLA
- Issue and PR templates; labels `good first issue`, `help wanted`, `area/*`, `typesense-api`

### 2.3 Trademark notice

"Typesense" is a trademark of Typesense, Inc. The app name must **not** contain "Typesense". The README and the About dialog carry the notice: *"Not affiliated with or endorsed by Typesense, Inc."*

### 2.4 Language

Everything in the repository is written in English: code, comments, docs, commit messages, i18n keys. The one exception is `assets/locales/de.yml`, which holds the German UI translation.

---

## 3. Target platforms

| Platform | Architecture | Priority | Notes |
|---|---|---|---|
| macOS 13+ | arm64, x86_64 (separate builds or universal) | P0 | Metal rendering; Xcode and Command Line Tools needed to build |
| Linux | x86_64 (arm64 P2) | P0 | Wayland **and** X11 (gpui_platform features), AppImage + .deb, Flatpak P2 |
| Windows 10/11 | x86_64 (arm64 P2) | P1 | Win32 + DirectWrite |

---

## 4. Tech stack and dependencies

> **Rule:** Every direct dependency is pinned exactly in `Cargo.toml` (`=x.y.z`) and updated only on purpose. `Cargo.lock` is committed. GPUI and GPUI Kit are **pre-1.0** and ship breaking changes regularly.

| Purpose | Crate | Notes |
|---|---|---|
| UI framework | `gpui-kit` (0.9.x) | Facade over `gpui-base`, `gpui-component` and default assets. It pins the GPUI version (`gpui-pre`, a snapshot of Zed's GPUI); **never choose it ourselves** |
| HTTP | `reqwest` (features `rustls`, `json`, `stream`, `http2`, `charset`) | no OpenSSL needed. `stream` arrives with M5, for exports; `gzip` is not needed so far |
| Async runtime for I/O | `tokio` (rt-multi-thread, fs, io-util, time, sync, macros) | bridge to GPUI, see §5.4 |
| Streaming | `tokio-util` (codec, io), `futures` | export lines, cancellation tokens (M5). Import sends one body per batch (D45) |
| Serialization | `serde`, `serde_json` (feature `preserve_order`) | document field order is preserved |
| CSV | `csv` | CSV import, on a blocking reader task like the JSON array reader (D51) |
| Secrets | `keyring`, `secrecy` | macOS Keychain, Windows Credential Manager, Secret Service |
| Paths | `directories` | config and data directories |
| Config | `toml`, `serde` | settings |
| Errors | `thiserror` (libraries), `anyhow` (app) | |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` | rotating log files, key redaction |
| Versions | `semver` | feature gating by server version |
| Scoped keys | `hmac`, `sha2`, `base64` | §7.14 |
| Time | `jiff` or `time` | `expires_at`, timestamps |
| i18n | `rust-i18n`, `sys-locale` | the same crate GPUI Kit's components use; EN and DE at launch |
| Temporary files | `tempfile` | error report spool (D47); tests |
| Tests | `wiremock`, `insta`, `testcontainers` (optional), `gpui_kit::test` | §12 |

**Typesense client:** The community crate `typesense` (typesense/typesense-rust, generated from the OpenAPI spec) exists. An admin tool needs **every** endpoint, raw JSON and full control over streaming. Tessera therefore has its **own lean crate `tessera-client`** on top of `reqwest`:
- Typed models where the UI needs them (collections, schema fields, keys …)
- `serde_json::Value` passthrough for documents and unknown fields (forward compatible)
- The official OpenAPI spec is vendored as a reference (`spec/typesense-openapi.yml`, with its commit hash noted). Models from `typesense_codegen` are not reused (D5).

---

## 5. Architecture

### 5.1 Cargo workspace

```
tessera/
├── Cargo.toml                  # [workspace], resolver = "3", edition = "2024"
├── crates/
│   ├── tessera-client/         # Typesense HTTP client, NO UI dependency
│   ├── tessera-core/           # Domain: profiles, settings, history, feature gating, import/export engine
│   ├── tessera-widgets/        # Reusable widgets (JsonEditor, FilterBuilder, FieldEditor, ConfirmDanger …)
│   └── tessera-app/            # Binary: GPUI app, windows, dock, views, actions, keymap
├── assets/
│   ├── icons/                  # Lucide SVGs (GPUI Kit ships none)
│   ├── locales/                # en.yml, de.yml
│   └── themes/
├── spec/typesense-openapi.yml  # vendored reference spec
├── dev/
│   ├── docker-compose.yml      # Typesense 30.2 (+ 3-node cluster profile)
│   └── seed/                   # sample data (books.jsonl, products.jsonl, joins)
├── xtask/                      # cargo xtask: dist, icons, seed, spec-fetch
└── .github/workflows/
```

All crates set `publish = false`. Tessera is an application, and the names `tessera` and `tessera-ui` are taken on crates.io by unrelated projects. The widget crate is called `tessera-widgets` so it cannot be confused with the unrelated `tessera-ui` framework. Crates and directories are created by the first milestone that puts something in them.

**Dependency direction (strict):** `app → widgets → core → client`. `client` and `core` build and test **without** GPUI.

### 5.2 Layers

1. **tessera-client.** One `TypesenseClient` per connection, with a node list, round robin or failover, timeouts, retries (exponential backoff, idempotent requests only) and request hooks for the request log. Each resource has its own module: `collections`, `documents`, `search`, `multi_search`, `aliases`, `sets` (synonym and curation sets, same endpoints, D60), `stopwords`, `presets`, `stemming`, `keys`, `analytics`, `conversations`, `nl_search_models`, `operations`, `metrics`, `debug`, `raw`.
2. **tessera-core.** Connection profiles, `ServerCapabilities` (version → feature flags), query history, saved queries, import/export pipelines (UI independent, progress events over channels), schema inference from sample data, scoped key generator.
3. **tessera-widgets.** Reusable GPUI Kit compositions without app state.
4. **tessera-app.** Views as GPUI entities, global `AppState`, actions and keybindings, dock layout, window management.

### 5.3 State management

- A global `AppState` entity holds the list of connections, the active connection, settings and the request log (two ring buffers: 5,000 entries for user requests, 500 for polling, D12).
- Each connection has a `ConnectionState` entity with its client, capabilities, cached collection list and health status.
- Views hold only their own view state and read everything else through entity handles. Changes flow through `cx.notify()` or events (`EventEmitter`).
- Resource caches are invalidated after every mutation. A manual refresh is available everywhere (⌘R / Ctrl+R).

### 5.4 Async bridge between GPUI and tokio (critical)

GPUI has its own executor, integrated into the platform event loop. `reqwest` needs a tokio runtime.

**Requirements:**
- At startup the app creates **one** `tokio::runtime::Runtime` (multi-thread, 2–4 workers) and registers its handle as a GPUI global.
- UI code starts I/O through a helper that spawns the future on tokio and returns a handle that aborts the tokio task when dropped. A tokio `JoinHandle` is an ordinary future and can be awaited from the GPUI executor.
- **Cancellation:** When the view task is dropped (for example, the tab is closed), the tokio task stops too.
- Long operations (import, export) report progress over `tokio::sync::watch`/`mpsc`. A GPUI task polls that channel and updates the UI, throttled to at most 30 updates per second.
- No blocking code on the main thread. CPU-heavy work (parsing large JSON responses, schema inference) runs on GPUI's background executor or via `spawn_blocking`.
- The exact pattern is documented in `docs/architecture/async.md` (M0).

### 5.5 Error model

- `tessera_client::Error` distinguishes `Network`, `Timeout`, `Tls`, `Http { status, message, body }`, `Decode`, `Cancelled`, `Unsupported { min_version }`. Variants are added when the first code path produces them.
- Typesense errors (`{"message": "..."}`) are parsed and shown in plain text, with a link to the request log entry.
- Display: inline errors where the action happened (forms, panels), an error panel for batch operations, toasts only for background events.

### 5.6 Feature gating by server version

- On connect, Tessera calls `GET /debug`; the response contains `version` and the Raft `state`. This produces `ServerCapabilities`.
- Features that need a newer version are hidden or disabled with the note "requires Typesense ≥ x.y".
- **v1.0 requires Typesense ≥ 30.0.** Older versions get a warning, and the app runs in a limited mode: collections, documents, search, aliases and keys work; the legacy endpoints for synonyms and overrides arrive in v1.x (§14). Sidebar rows for synonyms and curations hide there.
- Curation rule flags `rule.stem` and `rule.synonyms` need 30.2: 30.0 accepts and drops them, so the form hides them below 30.2.

---

## 6. Typesense API coverage (endpoint matrix)

> The vendored OpenAPI spec in `spec/typesense-openapi.yml` and the official v30.2 docs are authoritative. **Before implementing any endpoint, check** its path, parameters and response there. Details marked "⚠ verify" are inferred from the docs but not confirmed in detail.

| Area | Method + path | Module |
|---|---|---|
| Health | `GET /health` | §7.2 |
| Version / Raft state | `GET /debug` | §7.2 |
| Metrics | `GET /metrics.json` | §7.2 |
| API stats | `GET /stats.json` | §7.2 |
| Snapshot | `POST /operations/snapshot?snapshot_path=…` | §7.18 |
| Compact DB | `POST /operations/db/compact` | §7.18 |
| Leader election | `POST /operations/vote` | §7.18 |
| Clear cache | `POST /operations/cache/clear` | §7.18 |
| Schema change status | `GET /operations/schema_changes` ⚠ verify | §7.18 |
| Runtime config (e.g. slow request log) | `POST /config` | §7.18 |
| Collections | `GET/POST /collections`, `GET/PATCH/DELETE /collections/{name}` | §7.3 |
| Document CRUD | `POST /collections/{c}/documents[?action=upsert]`, `GET/PATCH/DELETE …/documents/{id}` | §7.4 |
| Update by query | `PATCH /collections/{c}/documents?filter_by=…` | §7.4 |
| Delete by query / truncate | `DELETE /collections/{c}/documents?filter_by=…&batch_size=…` (truncate parameter ⚠ verify) | §7.4 |
| Import | `POST /collections/{c}/documents/import?action=&batch_size=&dirty_values=&return_id=&return_doc=` | §7.6 |
| Export | `GET /collections/{c}/documents/export?filter_by=&include_fields=&exclude_fields=` | §7.7 |
| Search | `GET /collections/{c}/documents/search` (or POST via multi_search for long parameters) | §7.5 |
| Multi/union search | `POST /multi_search` (a read: allowed on read-only clients and fails over, D34; `page` and `per_page` of a union go in the URL) | §7.5 |
| Aliases | `GET /aliases`, `PUT/GET/DELETE /aliases/{name}` (`PUT` is an upsert and takes any target unchecked) | §7.8 |
| Synonym sets | `GET /synonym_sets`, `PUT/GET/DELETE /synonym_sets/{name}`, `GET …/{name}/items`, `PUT/GET/DELETE …/{name}/items/{id}` (verified on 30.0 and 30.2) | §7.9 |
| Curation sets | `GET /curation_sets`, `PUT/GET/DELETE /curation_sets/{name}`, `GET …/{name}/items`, `PUT/GET/DELETE …/{name}/items/{id}` (verified on 30.0 and 30.2) | §7.10 |
| Set attachment | `synonym_sets` and `curation_sets` arrays on the collection: `POST /collections`, `PATCH /collections/{name}` replaces the array (verified on 30.0 and 30.2) | §7.9, §7.10 |
| Stopwords | `GET /stopwords`, `PUT/GET/DELETE /stopwords/{id}` (`GET` of one wraps the answer in `stopwords`) | §7.11 |
| Presets | `GET /presets`, `PUT/GET/DELETE /presets/{name}` | §7.12 |
| Stemming dictionaries | `GET /stemming/dictionaries`, `GET …/{id}`, `POST /stemming/dictionaries/import?id=…` | §7.13 |
| API keys | `GET/POST /keys`, `GET/DELETE /keys/{id}` | §7.14 |
| Analytics rules | `GET/POST /analytics/rules`, `PUT/GET/DELETE /analytics/rules/{name}` (v30 format ⚠ verify) | §7.15 |
| Analytics events | `POST /analytics/events` | §7.15 |
| Conversation models (RAG) | `GET/POST /conversations/models`, `GET/PUT/DELETE …/{id}` | §7.16 |
| NL search models | `GET/POST /nl_search_models`, `GET/PUT/DELETE …/{id}` | §7.16 |
| Raw | anything | §7.17 |

**v30 specifics (important):** From v30 on, synonyms and curation rules (formerly "overrides") are **global resources** (`/synonym_sets`, `/curation_sets`). They can be attached to several collections. A synonym set can also be passed as the search parameter `synonym_sets`, which adds to the attached sets; a curation set cannot: `curation_sets` is not a search parameter on 30.0 or 30.2 (D57). Deleting an attached synonym set makes every search on that collection answer 404 (D58). API keys with the old `overrides:*` actions do not grant access to the new endpoints. The key editor must offer the new actions (`curation_sets:*`, `synonym_sets:*`).

---

## 7. Functional specification

Priorities: **P0** = required for v1.0 · **P1** = wanted for v1.0 · **P2** = v1.x.
Each module lists acceptance criteria (AC).

### 7.1 Connection management (P0)

- **Profiles:** name, color, environment tag (`local` / `dev` / `staging` / `prod` / custom), node list (protocol, host, port, path prefix), timeout, retry count, optional nearest node, TLS options (system CAs, custom CA file, "allow insecure certificates" only when set explicitly per profile, with a warning banner).
- **API key** only in the OS keychain (`keyring`, service `tessera`, account = profile UUID). The profile itself holds no key.
- **Connection test** before saving: `/health`, `/debug`, and a permission probe with `GET /collections` and `GET /keys`. The result is shown as "admin key" or "restricted key".
- **Cluster profiles:** several nodes in one profile. Per-node view (leader/follower) in §7.2.
- **Read-only mode** per profile, **on** by default for tag `prod`: all write actions are disabled. Unlocking is a temporary session grant, confirmed by typing the profile name. The unlock is delivered in M2 with the first write action (D10).
- **Import:** `config.json` from bfritscher's dashboard (fields `apiKey`, `node`, `history[]`, `clusterTag`), environment variables (`TYPESENSE_HOST` …), command line (`tessera --host … --port …`, key via stdin).
- **Export** of profiles as JSON, without keys.
- The most recently used connection can optionally open at startup.
- **Several connections open at once**, each in its own tabs. The tab color matches the profile color.

**AC:** A key never appears in plain text in logs, config files or crash reports. In a read-only profile, no UI path sends a write request (tested through a client hook that rejects write methods).

### 7.2 Server and cluster overview (P0)

- Cards for version, Raft role (leader/follower/…), health, uptime (if available), number of collections and documents.
- **Live metrics** from `/metrics.json`: CPU, RAM (system and `typesense_memory_*`), disk, network rx/tx. Configurable polling interval (default 5 s, can be paused). Line charts with 15 minutes of in-memory history.
- **API stats** (`/stats.json`): requests per second and latency per endpoint as a sortable table.
- **Cluster view:** all nodes of a profile side by side, polled in parallel, leader highlighted. Clicking a node makes it the active node.
- **Memory estimate per collection** (P2): rough heuristic, clearly labeled as an estimate.

**AC:** A failed node is marked red while the other nodes keep working. Polling stops while the window is hidden.

### 7.3 Collections and schema (P0)

- **List:** name, document count, field count, `created_at`, aliases pointing at it, attached synonym and curation sets. Search, sorting, favorites.
- **Schema view:** field table with `name`, `type`, `facet`, `optional`, `index`, `sort`, `infix`, `locale`, `stem`, `store`, `range_index`, `reference`, `num_dim`, `vec_dist`, `embed` (model and source fields), `truncate_len` and other attributes. Unknown attributes are shown raw (forward compatible). Collection options: `default_sorting_field`, `token_separators`, `symbols_to_index`, `enable_nested_fields`, synonym sets, metadata.
- **Create:**
  - Form mode: field editor with a type dropdown (all types including `auto`, `string*`, `object`, `object[]`, `geopoint`, `geopolygon`, `image`, `float[]`), live validation.
  - JSON mode: code editor, synchronized with the form in both directions.
  - **Infer schema from a file:** JSONL/JSON/CSV sample (first N = 1,000 rows) → suggested type per field, including `optional` when fields are missing. Arrives in M5 with import (D19). Rules in D50: integers are `int64`, and coordinate pairs stay numeric with a suggestion.
  - **Clone:** use the schema of an existing collection as a template.
- **Modify (PATCH):** add and drop fields (`drop: true`); changing a field is a drop plus an add. A diff preview is shown before sending. Progress comes from the schema change status (§7.18).
- **Delete:** confirmed by typing the collection name. A warning appears if aliases still point to the collection.
- **Truncate** (delete all documents, keep the schema): confirmed by typing the name.
- **JOIN relationships** (P2): graph view of all `reference` fields between collections.

**AC:** A schema built in the form is byte-equivalent (apart from field order) to the JSON mode. Schema inference distinguishes `int64` from `float` and detects `bool`, `string[]`, nested objects and geo coordinate pairs (as a suggestion).

### 7.4 Document browser (P0)

- **DataTable** (GPUI Kit) with virtual scrolling, pinnable and resizable columns, and cell selection.
  - Columns are generated from the schema and the documents. A column picker shows and hides columns; the choice is saved per collection.
  - Nested objects show as a compact JSON preview; clicking opens the inspector panel.
  - Data is loaded through search with `q=*`, `per_page=250`, page by page (infinite scroll). The total comes from `found`.
  - Clicking a column header sorts, only for sortable fields (numeric or `sort: true`).
  - The filter bar takes `filter_by` with autocompletion of field names, operators and facet values.
  - **Card view** as an alternative to the table (P1), with configurable title and image fields.
- **Inspector panel on the right:** the full document in the JSON editor, with syntax highlighting, folding and validation against the schema (type errors are marked).
- **Edit:** save changes from the editor via `PATCH` or upsert (selectable), with a diff preview.
- **New document:** template from the schema (required fields prefilled with type placeholders).
- **Delete:** single or multi-select. Multi-delete combines the IDs into one `filter_by: id:[…]`.
- **Delete by query:** enter a filter; Tessera first shows a preview ("affects N documents", via a search with `per_page=0`) and asks for confirmation. Above 1,000 documents, or on a prod profile, the collection name must be typed.
- **Update by query** (P1): filter plus partial document, also with a preview of the match count.
- **Copy:** cell, row as JSON, selection as JSONL or CSV, "Copy as curl".
- **Go to ID:** quick fetch via `GET …/documents/{id}`.

**AC:** 200,000 documents scroll without stutter (measured with GPUI's debug frame overlay, D29, ≥ 60 fps on an M1 or comparable hardware). Memory stays bounded through LRU page caching (default: 20 pages).

### 7.5 Search workbench (P0)

- **Two modes:**
  - **Form** for all common parameters: `q`, `query_by` (multi-select from schema fields, reorder by drag and drop), `query_by_weights`, `filter_by` (builder and text), `sort_by` (up to 3 criteria, including `_text_match`, `_vector_distance`, `_eval(...)`), `facet_by`, `max_facet_values`, `facet_query`, `group_by`/`group_limit`, `page`/`per_page`, `include_fields`/`exclude_fields`, `highlight_fields`, `num_typos`, `prefix`, `infix`, `drop_tokens_threshold`, `typo_tokens_threshold`, `text_match_type`, `prioritize_exact_match`, `exhaustive_search`, `vector_query`, `synonym_sets`, `curation_tags`, `preset`, `use_cache`, `search_cutoff_ms`, `limit_hits`.
  - **JSON/raw** in the code editor, able to set any parameter, including unknown ones.
  - Both modes stay in sync as far as the form can express the parameters.
- **Results:** hit list with rendered highlights, `text_match` score and `text_match_info` per hit, facet panel (clickable, filters directly), group view, search time (`search_time_ms`) and `found`. The raw JSON response has its own tab.
- **Instant search** (search while typing, 150 ms debounce), can be turned off.
- **Multi-search:** several searches in one request, federated (separate results) or `union` (merged).
- **Load and save presets** directly from the workbench (§7.12).
- **Saved queries** per connection and collection, with name and tags.
- **History:** the last 500 queries, searchable.
- **Export the query** as `curl`, JavaScript (typesense-js), Python, Rust (reqwest) and as a raw URL.
- **Compare mode** (P2): two parameter sets side by side, with rank difference per document (↑↓), for relevance tuning.
- **Elasticsearch helper** (P2): pasting an ES `_search` query produces a suggested translation (match/multi_match → q/query_by, bool.filter/term/range → filter_by, sort, terms agg → facet_by). Parts that cannot be translated are clearly marked.

- **Server-side effects:** the workbench sends `enable_analytics=false` unless the user changes it, so tuning does not feed production analytics. `conversation`, `nl_query` and `query_by` on an embedding field call a model, so they pause instant search; `conversation` also writes, so a read-only profile unlocks first (D35).

**AC:** Every request run from the workbench appears 1:1 in the request log. An exported `curl` command returns the same response when run, `search_time_ms` aside.

### 7.6 Import (P0)

- **Sources:** JSONL, JSON array, CSV (delimiter, header, type conversion based on the schema). Files via file dialog or drag and drop. Clipboard for small amounts.
- **Options:** `action` (`create` / `upsert` / `update` / `emplace`), `batch_size` (default 1,000), `dirty_values` (default `coerce_or_reject`, always sent, D46), target collection (existing, or new with an inferred schema, see §7.3, created only at Start, D54). `return_id` is always on so that the report has ids (D46). Confirmation in D49.
- **Streaming:** the file is read record by record, sent as one `POST` per batch (D45), and the response is evaluated line by line. Memory stays constant regardless of file size. A batch also ends at 16 MiB, and a record over 32 MiB is refused.
- **Progress:** bytes, documents, rate per second, ETA, error count. Can be cancelled at any time; batches already imported stay imported (the dialog says so). Jobs run in the jobs panel and outlive the dialog and the tab (D48).
- **Error report:** table with line number, error message and original document, the first 1,000 rows in memory and all of them in a temporary file (D47). Exportable as JSONL. The failed lines can be saved as a new file and imported again.
- **Preview:** the first 20 lines before starting, including schema validation. It is advisory (D27): it checks the values as the server would coerce them under the default `dirty_values`, so with `drop` or `reject` a numeric string is flagged although the server decides.

**AC:** Importing 1 million documents (≈ 1 GB JSONL) completes with less than 300 MB RSS for the app. Cancelling stops within one batch round trip. Measured headlessly (see §14); the figure for the window is a lower bound, since the smoke start holds no connection or jobs panel.

### 7.7 Export (P0)

- Everything, by `filter_by`, or only the current selection. `include_fields` and `exclude_fields` are selectable, as top-level schema names (D52).
- Formats: JSONL (native, streamed) and JSON array. CSV (P1, in M5) with nested fields flattened (`a.b.c`), the header taken from the first 1,000 documents.
- Progress with cancel; the target is chosen in a file dialog and replaced only after a complete export. A target that is a symbolic link is refused, and an export that finds no document keeps an existing target. Export ignores curations, so the count matches what is written, and a finished export says when it wrote fewer documents than counted. Reading needs no unlock, so the dialog works on a read-only connection.
- CSV cells are written as they are (D55). A column that first appears after the first 1,000 documents is left out and counted in the job, not added to the header.
- **Backup assistant** (P1, M8): exports schema, all documents, aliases, attached synonym and curation sets, and stopwords for one or more collections into a folder or a `.tar.zst` archive. The **restore assistant** loads it into any cluster.

### 7.8 Aliases (P0)

- List with target collection and a state for broken aliases (target missing or empty, target is an alias, hidden by a collection of the same name); create, repoint (upsert), delete.
- **Reindex assistant** (P1) for switching without downtime, started from an alias. A collection without an alias must get one first, since a name cannot be both:
  1. New collection from the existing or a changed schema (name with timestamp suffix). A preflight names unstored fields, joins from other collections, other aliases, missing sets, embedding fields and blanked keys (D64)
  2. Import data (from a file, or export to a private temp directory and import from there, D63)
  3. Compare document counts
  4. Switch the alias, after reading it again; a difference in the counts raises the confirmation (D65). A switch back stays available until step 5
  5. Optionally delete the old collection after confirmation

The assistant is a tab per alias, and closing it keeps its state and jobs (D66). Joins from other collections fail after a switch, and upserting their documents does not repair them, so the confirmation names them.

### 7.9 Synonym sets (P0, v30+)

- List of all sets, CRUD.
- **Item editor:** multi-way (`synonyms[]`) and one-way (`root` + `synonyms[]`), `locale`, `symbols_to_index`. As a table and as JSON.
- Table rows save through the item endpoints, the JSON view and imports through the whole set, with a review of adds, changes and removals (D60). Duplicate ids are refused before sending: the server keeps the last and answers 200.
- **Attachment** to collections (the `synonym_sets` array of the collection, D59), and a view of which collections use a set. Sets attached but missing are shown with a way to detach them. Deleting a set detaches it first (D58).
- **Test panel:** a search against a chosen collection with and without the set, results side by side. For an attached set "without" means no synonyms at all, and the panel says so, because a search cannot leave out one attached set (D56).
- Import and export of a set as JSON and as CSV (for business teams, D67).

### 7.10 Curation sets (P0, v30+)

- List of all sets, CRUD.
- **Rule editor** with all rule attributes: `rule.query`, `rule.match` (`exact`/`contains`), `rule.filter_by`, `rule.tags`, `includes[]` (`id` + `position`, reorder by drag and drop), `excludes[]`, `filter_by`, `sort_by`, `replace_query`, `remove_matched_tokens`, `filter_curated_hits`, `effective_from_ts` / `effective_to_ts` (date picker), `stop_processing`, `synonyms`, `stem`, diversification (MMR) and other fields new in v30. Fields that came from the server and have no control are kept as a raw JSON block; the server itself drops keys it does not know, and the editor reports that after saving. Rules the server would store but that can never work are refused first (bad `match`, reversed dates, position below 1), and a `sort_by` field missing from an attached collection warns, because such a rule fails every search it matches.
- **Document picker** for `includes`/`excludes`: search the collection, add hits with a click.
- **Attachment** to collections as in §7.9, and a **test panel** with `enable_curations=false` against the plain search, plus testing `curation_tags`. It needs an attached set, because a search cannot name a curation set (D57). Dates are shown in UTC.

### 7.11 Stopwords (P1)

CRUD for stopword sets (`id`, `stopwords[]`, `locale`). Bulk editing in a text area (one word per line). The server lowercases, splits at spaces and removes duplicates, so the editor shows the stored words after saving and says when they differ.

### 7.12 Presets (P1)

CRUD for search presets (`value` = search parameters or a multi-search body). The editor is the search workbench (§7.5) in preset mode. A preset whose `value` the workbench cannot read stays listed and can be deleted. A search naming a deleted preset keeps working without its parameters, which the delete dialog says.

### 7.13 Stemming dictionaries (P2)

List, detail view and import (JSONL with `word`/`root`) via `POST /stemming/dictionaries/import?id=…`.

### 7.14 API keys and scoped search keys (P0)

- **List:** `id`, description, `value_prefix`, actions, collections, `expires_at` (flagging expired keys and keys about to expire). Delete.
- **Create:**
  - Actions as a checkbox matrix grouped by resource, with every action listed in the docs, including `curation_sets:*`, `synonym_sets:*`, `documents:*`, `collections:*`, `analytics:*` …
  - Wildcard `*` with a warning.
  - Collections: a list or a regex pattern.
  - `expires_at` with a date picker, a "never expires" option, and templates (search only, import only, admin).
  - The new key value is shown **only once**, with a copy button and a warning. Then it is discarded, unless the user explicitly saves it into a connection profile.
- **Scoped search key generator** (missing in the reference):
  - Inputs: parent key (search-only), embedded parameters in a JSON editor (`filter_by`, `expires_at`, `limit_multi_searches` …).
  - Algorithm (per the Typesense docs, ⚠ verify): `digest = base64(HMAC-SHA256(key = parent_key, msg = params_json))`, `scoped_key = base64(digest + parent_key[0..4] + params_json)`.
  - Computed locally, without a server request.
  - **Decoder:** paste a scoped key to see its embedded parameters. With the parent key known, the signature is verified.
  - **Test:** run a search with the scoped key and show which documents are visible.
- **Rotate keys** (P2): create a new key with the same permissions, then delete the old one.

**AC:** Scoped keys generated by Tessera are identical to those from `typesense-js` `generateScopedSearchKey` (tested with fixed vectors).

### 7.15 Analytics (P1)

- Rules in the v30 format: CRUD for popular queries, no-hits queries, counter events and other rule types. Source and destination collections are picked from dropdowns.
- View of the destination collections (top queries, no hits) as a sorted table, with a jump into the search workbench.
- **Send a test event** (`POST /analytics/events`) from a form.

### 7.16 AI features (P1)

- **Conversation models (RAG):** CRUD (model name, provider API key field masked, `history_collection`, `system_prompt`, `max_bytes`). Test chat with `conversation=true` against a collection, showing the source documents.
- **NL search models:** CRUD and a test panel showing which `q`/`filter_by`/`sort_by` the model produced from a natural-language query.
- **Embedding fields:** built-in models and external providers selectable in the schema editor; provider keys are masked.
- Provider keys entered in forms are never stored locally, only sent to the server.

### 7.17 Raw API console (P0)

- Method, path (autocompleted from the vendored OpenAPI spec), query parameter table, and body in the JSON editor or from a file (for JSONL).
- Response with status, headers, time and body (JSON formatted; large bodies virtualized or folded).
- History, favorites, "Copy as curl".
- Read-only profiles allow `GET` only (and `HEAD`, which reads nothing more) until unlocked.
- The path is validated by allowlist and cannot leave the configured node. Writes confirm by danger (D40), and the response body is masked until the user reveals it (D41).
- The console is a center tab of one connection, not a bottom panel (D33).

### 7.18 Server operations (P1)

- **Snapshot:** enter a server path (note: the path is **on the server**, not local), confirm, show the result. Type-to-confirm on prod profiles.
- **Compact DB**, **clear cache**, **leader election (vote)**, each with an explanation and a confirmation.
- **Runtime config** (`POST /config`): for example, set the slow request logging threshold.
- **Running schema changes** with progress (polling).

### 7.19 Request log (P0)

- Bottom dock panel listing every request: time, connection, method, path, status, duration, size.
- Detail view with request and response bodies (truncated above 1 MB, full view on demand from M4 on, kept in memory up to 32 MiB per body and 64 MiB in all, D42). Document page responses keep no body (D24).
- **Redaction:** the `X-TYPESENSE-API-KEY` header always shows as `abcd••••`. Known secret fields in bodies (`api_key`, `value` on keys) are masked.
- Filter by status, method and connection. "Copy as curl" uses the placeholder `$TYPESENSE_API_KEY` instead of the real key.

### 7.20 Command palette and navigation (P0)

- ⌘K / Ctrl+K: fuzzy search over the actions available at the current focus, collections, saved queries and connections, and settings once §7.21 exists (D44).
- ⌘P / Ctrl+P: quick switch to a collection.
- Full keyboard control, configurable keymap (`keymap.toml`, `[[binding]]` entries with `key`, `action`, `context`; no action unbinds; loaded at start, D43).

### 7.21 Settings (P0)

- Theme: all GPUI Kit themes plus system light/dark. Font size, UI density (compact/comfortable).
- Language: English and German (English fallback).
- Defaults: `per_page`, import `batch_size`, polling interval, confirmation thresholds.
- **Update check** against GitHub Releases: opt-in, off by default. No telemetry, no crash upload.
- Show storage locations (config, logs), open logs, clear cache.

### 7.22 Legacy servers < 30 (P2)

Collection-bound synonyms (`/collections/{c}/synonyms`) and overrides (`/collections/{c}/overrides`) for Typesense 27–29, enabled through feature gating (§5.6).

---

## 8. UI/UX specification

Mockups: [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d). The rules in 8.5 apply to every milestone.

### 8.1 Main layout (GPUI Kit `DockArea`)

```
┌───────────────────────────────────────────────────────────────────────┐
│ Title bar: [Connection ▾ ●prod]  Breadcrumb   ⌘K search    ⚙         │
├───────────────┬───────────────────────────────────────┬──────────────┤
│ Sidebar       │ Tabs: [products ✕] [Search ✕] [Keys ✕]│ Inspector    │
│ ▸ Connections │                                       │ (document    │
│   ▾ prod-ch   │  Main area (active tab)               │  JSON, field │
│     Overview  │                                       │  details,    │
│     Collections│                                      │  help)       │
│       products│                                       │              │
│       users   │                                       │              │
│     Aliases   │                                       │              │
│     Synonyms  │                                       │              │
│     Curations │                                       │              │
│     Stopwords │                                       │              │
│     Presets   │                                       │              │
│     Keys      │                                       │              │
│     Analytics │                                       │              │
│     AI        │                                       │              │
│     Ops       │                                       │              │
├───────────────┴───────────────────────────────────────┴──────────────┤
│ Bottom panel: Request log | Import/export jobs                        │
├───────────────────────────────────────────────────────────────────────┤
│ Status bar: ● connected · v30.2 · leader · 12 ms · 3 jobs             │
└───────────────────────────────────────────────────────────────────────┘
```

- Search and Raw console are center tabs of one connection (D33).
- Panels can be moved, closed and zoomed. The layout is serialized per window and restored on the next start.
- Several windows are possible (for example prod and staging side by side).

### 8.2 Safety UX

- **Environment color:** for `prod`, a colored band (red by default) runs across the top of the window, the window gets a red frame, and every tab of that connection carries the color.
- **Danger levels** for confirmations:
  - low: no dialog, but an undo hint where possible
  - medium: a simple confirmation dialog
  - high: type the name (delete collection, delete by query > 1,000, snapshot or vote on prod)
- Destructive buttons are always red and never the default focus.

### 8.3 Empty states and onboarding

- First start: a welcome screen with "Add connection", "Import from typesense-dashboard" and "Run Typesense locally" (shows the Docker command, runs nothing itself).
- Every empty list briefly explains the resource and links the matching Typesense docs.

### 8.4 Accessibility

- AccessKit roles and names for every interactive element (GPUI Kit provides the basics).
- Full keyboard operation, visible focus ring, WCAG AA contrast in the default themes.

### 8.5 UX rules

1. A connection's environment is visible everywhere: title bar, tabs, sidebar, and for prod the window frame.
2. Errors show where they happen, inline and in full. Toasts are only for background events.
3. Default focus is never on a destructive action. High danger means typing the name, and the dialog states the concrete consequences (counts, affected aliases).
4. No dead controls: navigation entries, panels and buttons appear in the milestone that makes them work.
5. Every action is reachable by keyboard, and its shortcut is shown on the control.

---

## 9. Security and privacy

- Keys only in the OS keychain. All secrets in memory as `secrecy::SecretString` (or equivalent), zeroized on drop.
- **TLS by default** for remote hosts. Tessera allows `http://` without a warning only for `localhost`/`127.0.0.1`/`::1`; anything else shows a warning banner.
- Logs: a redaction layer in `tracing` masks keys and known secret fields. Default log level `info`. Request bodies are never written to the log file, only to the in-memory request log.
- No telemetry. Network connections go only to configured Typesense nodes and (opt-in) to GitHub for the update check.
- `cargo deny` in CI (advisories, licenses, duplicates).
- `SECURITY.md` with the reporting process; target: response within 7 days.

---

## 10. Non-functional requirements

| ID | Requirement | Measurement |
|---|---|---|
| NF1 | Cold start to interactive window < 1 s (release build, M1) | timestamp logging |
| NF2 | Scrolling a DataTable with 200k rows ≥ 60 fps | GPUI debug frame overlay (D29) |
| NF3 | Import of 1 million documents with RSS < 300 MB | `cargo xtask nf3` plus the app's baseline, nightly (D53) |
| NF4 | App code never blocks the UI for more than 16 ms | tracing spans, debug assertion |
| NF5 | Binary size < 40 MB (stripped, per platform) | CI artifact check |
| NF6 | Idle CPU < 1% without active polling | manual |
| NF7 | Every Typesense error message is shown in full | tests |
| NF8 | English and German UI complete, no hardcoded strings in views | lint in xtask |

---

## 11. Persistence and configuration

Location via `directories::ProjectDirs::from("org", "tessera", "Tessera")`:

| File | Content | Format |
|---|---|---|
| `config/settings.toml` | settings | TOML |
| `config/connections.json` | profiles **without** keys | JSON, versioned (`schema_version`) |
| `config/keymap.toml` | key bindings, on top of the defaults (D43) | TOML |
| `data/history.jsonl` | search and raw console history | JSONL, rotated |
| `data/saved_queries.json` | saved queries and console favorites | JSON, versioned (`schema_version`) |
| `data/document_views.json` | column choice, pins and widths per profile and collection (D25) | JSON |
| `data/layout/*.json` | dock layouts | JSON (GPUI Kit format) |
| `logs/tessera.log*` | logs | text, rotated (7 days) |

- Migrations between `schema_version` levels run automatically; a `*.bak` backup is written first.
- All writes are atomic (temporary file, then rename).

---

## 12. Testing and quality assurance

### 12.1 Test levels

1. **Unit (`tessera-client`, `tessera-core`):** request construction, parameter serialization, error parsing, scoped key generation (fixed test vectors), schema inference, CSV/JSONL parsing, feature gating. Mock server with `wiremock`.
2. **Integration against real Typesense:** `dev/docker-compose.yml` or a GitHub Actions service container. Version matrix **30.0 and 30.2** (minimum and current), plus a 3-node cluster job (nightly). Covers every endpoint from §6 with create → read → update → delete.
3. **UI integration (`#[gpui_kit::test]`):** headless window, simulated keyboard and mouse input. Checks focus, state and accessibility. Required for: connection dialog, read-only lock, type-to-confirm, filter autocompletion, import cancel.
4. **Snapshot tests (`insta`):** generated curl commands, schema JSON from the form, ES → Typesense translation.
5. **Performance tests (nightly):** import of 1 million documents through `xtask nf3` (D53), scroll benchmark (GPUI benchmarks, if available).
6. **Manual QA checklist** before every release in `docs/qa-checklist.md`.

### 12.2 Seed data

`cargo xtask seed` loads sample collections into the local dev server:
- `books` (≈ 10k, facets, sorting)
- `products` (≈ 200k, performance)
- `authors` + `books_ref` (JOINs)
- `places` (geo)
- `articles_vec` (vector field)
- plus synonym sets, curation sets, stopwords, presets, aliases, keys

### 12.3 Code quality

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
- No `unwrap()`/`expect()` outside tests and clearly justified invariants (`clippy::unwrap_used` set to `deny` in the libraries)
- Public APIs of `tessera-client` and `tessera-core` fully documented (`#![warn(missing_docs)]`)

---

## 13. CI/CD, packaging, releases

### 13.1 CI (GitHub Actions)

| Job | Platform | Content |
|---|---|---|
| `lint` | ubuntu | fmt, clippy, cargo-deny, i18n check |
| `test` | ubuntu, macos, windows | `cargo test --workspace`, including headless UI tests |
| `integration` | ubuntu | Typesense service container (30.0, 30.2) |
| `build` | macos-arm64, macos-x64, ubuntu, windows | release build, size check (NF5), smoke start |
| `nightly` | ubuntu | cluster tests, performance |

Linux build dependencies (Wayland, X11, xkbcommon, Fontconfig, Vulkan or similar) are documented in `docs/building.md` and installed by the CI script. The exact list comes from the GPUI and GPUI Kit docs.

### 13.2 Packaging

- **macOS:** `.app` in a `.dmg`, for arm64 and x64 (or as a universal binary). Code signing and notarization once an Apple Developer account is available; until then ad-hoc signed, with instructions (`xattr -cr`) in the README. Homebrew cask (P1).
- **Windows:** `.msi` (WiX) and `.zip`. winget (P2). Signing optional (P2).
- **Linux:** AppImage, `.deb`, `.rpm` (P1). Flatpak/Flathub (P2), AUR (community).
- Tool: `cargo-packager` (D3), set up in M9.

### 13.3 Releases

- Tag `vX.Y.Z` → CI builds all artifacts and creates a GitHub release with the changelog excerpt and SHA256 checksums.
- Channels: `stable` and `nightly` (nightly as a pre-release).

---

## 14. Milestones

Each milestone ends with green CI, an updated CHANGELOG and a short demo (GIF in the PR).

| # | Content | Acceptance |
|---|---|---|
| **M0 Foundation** | Workspace with `tessera-client`, `tessera-core`, `tessera-app`; CI (lint, test, integration, build, smoke start); license and governance files; GPUI Kit window with root, theme, dock skeleton, sidebar, status bar; tokio bridge (§5.4); `tessera-client` with `health` and `debug`; welcome screen and connect dialog; `dev/docker-compose.yml`. Details: `docs/superpowers/specs/2026-11-04-m0-foundation-design.md` | App starts on macOS, Linux and Windows. "Connect" against localhost shows the version |
| **M1 Connections and overview** | §7.1 complete (keychain, profiles, tags, read-only, import from bfritscher config), §7.2 (metrics, stats, cluster view), §7.19 request log. Details: `docs/superpowers/specs/2026-11-04-m1-connections-design.md` | AC from §7.1 and §7.2 met |
| **M2 Collections** | §7.3 without the JOIN graph and schema inference (M5, D19): list, schema view, create (form and JSON), clone, PATCH with diff, delete, truncate, read-only unlock (D10). Creates `tessera-widgets`; `xtask seed` with small data sets. Details: `docs/superpowers/specs/2026-11-05-m2-collections-design.md` | Integration tests for all collection endpoints green |
| **M3 Document browser** | §7.4: DataTable, infinite scroll, filter with autocompletion, inspector with JSON editor, CRUD, multi-select, delete by query with preview. `xtask seed` gains the 200k `products` set. Details: `docs/superpowers/specs/2026-11-05-m3-documents-design.md` | NF2 met (200k rows); measurement still open, steps in `docs/building.md` |
| **M4 Search and raw console** | §7.5 (without P2 parts), multi-search, history, saved queries, code export. §7.17 raw console. §7.20 command palette. §7.19 full view of truncated bodies. Details: `docs/superpowers/specs/2026-11-06-m4-search-design.md`, decisions D33–D44 | Exported curl reproduces the response (live test, 30.0 and 30.2); the checks on screen are still open, steps in `docs/building.md` |
| **M5 Import/export** | §7.6, §7.7 including schema inference, error report, cancel. Jobs panel. The backup and restore assistant is M8. Details: `docs/superpowers/specs/2026-11-06-m5-import-export-design.md`, decisions D45–D54 | NF3 met headlessly: 1,000,000 documents in JSONL, JSON and CSV add at most 25 MB to the process and total at most 118 MB with the app's baseline, limit 300 MB (`cargo xtask nf3`, D53, record in `docs/building.md`); the run through the window and the Linux binary size (NF5) are still open |
| **M6 Relevance resources** | §7.8 aliases including the reindex assistant, §7.9 synonym sets, §7.10 curation sets, §7.11 stopwords, §7.12 presets. Details: `docs/superpowers/specs/2026-11-06-m6-relevance-design.md`, decisions D56–D68 | CRUD and test panels work against 30.0 and 30.2 |
| **M7 Keys, analytics, AI** | §7.14 including the scoped key generator and decoder, §7.15, §7.16, §7.13 | Scoped key test vectors identical to typesense-js |
| **M8 Operations and hardening** | §7.18, safety UX §8.2 complete, backup/restore assistant, §9 audit (redaction tests) | Security checklist done |
| **M9 Release 1.0** | i18n EN/DE complete, accessibility pass, onboarding, packaging for all platforms (`cargo-packager`, `xtask dist`), docs website (mdBook), QA checklist | v1.0.0 published |
| **v1.x** | Legacy < 30 (§7.22), compare mode and ES translator (§7.5), JOIN graph (§7.3), key rotation, card view, Flatpak/winget, optional WASM build | – |

---

## 15. Working rules for Claude Code and other coding agents

These rules are repeated in `CLAUDE.md` at the repository root.

1. **Never guess APIs.** GPUI and GPUI Kit are pre-1.0 and change often. Before using an API:
   - Read the GPUI Kit docs: https://gpui-kit.com/docs, machine-readable at https://gpui-kit.com/llms-full.txt, plus the official GPUI Kit skills (https://gpui-kit.com/skills).
   - Check the source of the **pinned** version (`~/.cargo/registry/src/…/gpui-kit-*`, `docs.rs/gpui-kit/<version>`) and the examples in the GPUI Kit repo.
   - When docs and code disagree, the code of the pinned version wins.
2. **Typesense truth** is `spec/typesense-openapi.yml` plus the v30.2 docs (index: https://typesense.org/docs/llms.txt). Every new endpoint gets an integration test against the Docker server first, then the UI.
3. **Order per feature:** client function with test → core logic with test → UI view → UI test → docs and CHANGELOG.
4. **Small steps:** run `cargo check` / `cargo clippy` / the affected tests after every change. No commits with red CI.
5. **bfritscher reference:** use it only as a **feature and UX reference**. Do not translate Vue or TypeScript code; write idiomatic Rust from scratch.
6. **No blocking code in the render path.** All I/O goes through the bridge from §5.4.
7. **Never hardcode strings.** All UI text goes through i18n keys (`t!("collections.delete.confirm")`).
8. **Secrets:** never log keys, never commit them in test fixtures (the dev key `xyz` is for local use only). Redaction tests must not be removed.
9. **Versions:** update dependencies only in a dedicated PR, with a note on breaking changes.
10. **English only** in the repository, except `assets/locales/de.yml`.
11. **Follow the UX rules** in §8.5.
12. **When unsure,** add an entry to §16 and ask instead of deciding silently.

---

## 16. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | License | GPL-3.0-or-later |
| D2 | App name | Tessera. All crates `publish = false`; widget crate named `tessera-widgets` |
| D3 | Packaging tool | `cargo-packager`, set up in M9 |
| D4 | i18n crate | `rust-i18n` (GPUI Kit's components use it); locale from `sys-locale`, English fallback |
| D5 | Reuse models from `typesense_codegen`? | No, for v1.0 (own models, `Value` passthrough) |
| D6 | Minimum Typesense version for v1.0 | 30.0 |
| D7 | Apple signing | Ad-hoc until the first stable version |
| D8 | Repository hosting | Public GitHub repository, CI on GitHub Actions |
| D9 | Repository language | English, except the German UI translation `de.yml` |
| D10 | Read-only unlock in M1? | No. M1 has the profile flag, the client guard and the badges; the unlock flow arrives in M2 with the first write action |
| D11 | Prod frame with several connections in one window | Red band and frame while the active tab belongs to a prod connection |
| D12 | Polling traffic in the request log | Logged in its own 500-entry buffer next to the 5,000-entry buffer for user requests; hidden by default |
| D13 | Connection from env vars or CLI flags | Temporary, never saved, key in memory only; "Save as profile" opens the prefilled dialog |
| D14 | Request routing in cluster profiles | Active node with failover for idempotent methods; no round robin |
| D15 | Keyboard shortcut for "Disconnect" in the sidebar (UX rule 5) | ⌘⇧D / Ctrl+Shift+D on the focused sidebar entry, shown in the context menu |
| D16 | Connection switcher in the title bar? | No dropdown in M1: the title bar shows the selected connection. Switching happens in the sidebar (PR A) and in the tabs (PR B) |
| D17 | What the overview shows while the active node is down | Its last figures and curves stay, dimmed, with "last sample N s ago" under the node cards; they are not cleared |
| D18 | Collections in the sidebar | Favorites only, under "Collections"; the full list is the collections tab |
| D19 | Schema inference from a file: M2 or M5? | M5, with import, which shares the file readers |
| D20 | Clone shortcut | ⌘⇧D / Ctrl+Shift+D in the collection views; ⌥⌘D is taken by macOS |
| D21 | Truncate/delete dialog when the fresh reload of the collection and aliases fails | Opens with the cached data and says it may be out of date |
| D22 | Inspector: right dock panel or inside the tab? | A resizable split inside the documents view, like the request log's detail: the document, its unsaved edits and its schema belong to one tab |
| D23 | What does a collection tab show? | Two views, Documents (default) and Schema, ⌘1 / ⌘2 |
| D24 | Request log vs. page cache memory | Document page responses are logged without their body; status, size and timing stay. Otherwise the log would hold the scrolled documents, up to 565 MB |
| D25 | Where column preferences live | `data/document_views.json`, per profile and collection, not in `Profile`, which is exported as configuration |
| D26 | Multi-select size and URL length | At most 1,000 selected rows. Id filters are batched at 7,500 encoded bytes. Ids with a backtick cannot be quoted in `filter_by` and go one by one |
| D27 | Does schema validation block saving? | No. Marks are advisory and the server decides. Only a parse error, a changed `id` or an empty diff stop a save |
| D28 | Which confirmation for document deletes and bulk updates? | Medium dialog; type the collection name above 1,000 documents or on prod, for single and multi delete, delete by query and update by query |
| D29 | NF2 names a GPUI Kit FPS HUD that does not exist | Measured with GPUI's debug frame overlay behind a `frame-overlay` cargo feature, by hand, p95 draw time ≤ 16.6 ms |
| D30 | `filter_by` over 4,000 bytes in the browser | Not supported in M3: the server's message shows inline. `POST /multi_search` is a read, but the read-only guard refuses POST and failover skips it. M4 needs it and should allow that one path in the guard. Closed by D34: long filters go through `POST /multi_search` |
| D31 | Unsaved changes in the inspector when a tab, connection or app closes | ⌘W / Ctrl+W asks, with focus in the tab. The kit's other closes cannot ask, so `closable` vetoes them and the tab hides its close button until the changes are saved or reverted. Disconnect and quit have no veto in the pinned kit, and they discard without asking |
| D32 | Delete or update by query with a filter that matches every document | Refused only when the text itself names everything: blank, `*`, or `*` among the values of `id`. A predicate that happens to match all, such as `n:>=0`, keeps its count and the danger level, because an update of a whole collection has no other way in. Open for the maintainer: refuse those too? |
| D33 | Where do the search workbench and the raw console live? | Center tabs of one connection, with its color and badges. A bottom panel would follow the active tab, and a `DELETE` typed against staging would go to prod after a tab switch |
| D34 | `POST /multi_search` on a read-only client | Allowed through a private client flag set only inside the search methods, which also makes it fail over. The raw console cannot set it, so a console POST stays refused. A search whose query exceeds 4,000 bytes takes this route, in the workbench and in the document browser |
| D35 | Server-side effects of a search | The workbench writes `enable_analytics: false` into a new draft, visible and changeable. Curations stay on, to show what clients get, unlike the browser. A query that calls a model pauses instant search, and `conversation` needs an unlocked profile |
| D36 | Instant search default | On, except on prod profiles, where each keystroke would load the production cluster |
| D37 | Presets: M4 or M6? | M4 loads and saves from the workbench, as §7.5 says. Listing as a resource, delete and preset mode stay in M6 with §7.12 |
| D38 | Filter builder and facet clicks | The `filter_by` text is the only filter state. The builder and facet clicks rewrite it, so what runs is what shows. Facet values pass an allowlist first; one that `filter_by` cannot hold is disabled |
| D39 | What enters history | A run started by the user, or an instant run that stood for 2 s. 500 per kind (search, console). Console queries and bodies are redacted, bodies over 64 KiB are not kept, temporary connections write nothing, Clear history exists |
| D40 | Confirmations in the raw console | `DELETE` of a collection or of its documents by filter or truncate: type the name, everywhere. Other `DELETE` and `POST operations/*`: confirm, and type the last path segment on prod. Other writes: nothing, and confirm on prod. `POST /multi_search` is a read |
| D41 | Secrets in console responses | Masked by default, with Reveal until the next Send, because `POST /keys` answers with the only copy of a key. Workbench results are never masked: `value` is an ordinary document field there |
| D42 | Keeping full bodies for §7.19 | In memory, at most 32 MiB per body and 64 MiB in all, oldest evicted, released with their entry. The console holds at most 32 MiB of an answer and says when it cut one. Refetching would show a different answer and cannot repeat a write. Disk is ruled out by §9 |
| D43 | `keymap.toml` shape | `[[binding]]` with `key`, `action` (`tessera::Name`), optional `context`. No action unbinds. Loaded once at start, on top of the defaults. A bad entry is skipped and named above the tabs, a file that does not parse changes nothing and says so there, and `escape` cannot be bound |
| D44 | Settings in the palette, and §7.21's milestone | No entry until a settings view exists (UX rule 4). §14 names no milestone for §7.21. Recommendation: M8. Open for the maintainer |
| D45 | One streamed `POST` or one `POST` per batch? | One per batch, in sequence. The cancel AC speaks of a batch round trip, an abort of a streamed body showed nothing stored after 20 MB of 64 MB, and the answer stays bound. A second press drops the request in flight, and its documents count as unconfirmed |
| D46 | Import defaults | `dirty_values` is `coerce_or_reject`, always sent, because the server's own default rejects on 30.0 and 30.2. `return_id=true` always, so the report has ids. `return_doc` never: the client holds what it sent |
| D47 | Where the error report lives | The first 1,000 rows in memory, all rows in an unnamed private temp file that disappears with the job or process. A million failures must not grow the app. §9 concerns keys and the log file; this holds the user's own documents for as long as the job row exists |
| D48 | Who owns a job? | A global `Jobs` entity. Closing a dialog or tab or disconnecting does not stop it, except that a disconnect locks a read-only connection again (§7.1), which stops a write job on it at its next batch as locked by disconnect, with the line it stopped at. Lock again stops an import at its next batch. Quit stops all jobs; there is no veto (D31) |
| D49 | Confirmation for imports | `create` or a new collection: none, or a plain confirmation on prod. `upsert`, `update`, `emplace`: `danger_level` on the estimated count, so typing the name above 1,000 or on prod. Export needs none: it is a read |
| D50 | Schema inference rules | Integers are `int64`, since a sample cannot prove `int32`. Mixed types are `string` with a note. A 2-number array in lat/lng range stays numeric with a "Use geopoint" suggestion. Keys Typesense would misread (`*`, `id`) make no field |
| D51 | CSV conversion | Cells follow the target's types. Arrays and objects are JSON in the cell, the form CSV export writes. An empty cell leaves the key out. `a.b` headers nest. The sync `csv` crate, not `csv-async`, because the JSON array reader is sync anyway |
| D52 | Which fields can an export pick? | Top-level names from the schema, by checkbox, each through an allowlist, because `include_fields=nope` answers 200 with `{}` per document and a comma cannot be written. No free text. Keys outside the schema export under All and Exclude |
| D53 | How is NF3 measured? | `cargo xtask nf3` runs the real pipeline with a real request log against a real server and adds the app's baseline from the `TESSERA_SMOKE` run. Nightly on Linux, and one run through the UI by hand in M5's last PR |
| D54 | When is a new collection created? | At Start, after the confirmation. The schema dialog gets a pick mode that sends nothing, so cancelling the import leaves the server untouched |
| D55 | Are CSV cells neutralised against spreadsheet formulas? | No, cells stay verbatim. Tessera is an admin tool, and prefixing a cell would break the round trip: a re-import would store changed text. The risk stays: a cell starting with `=`, `+`, `-` or `@` runs as a formula when the file opens in a spreadsheet, and documents can hold what strangers typed. So the export dialog says so for CSV |
| D56 | What does "with and without the set" mean in a synonym test panel? | For a set not attached, without is the plain search and with adds `synonym_sets=`. For an attached set the left side is `enable_synonyms=false`, because the parameter only adds; with other sets attached the panel says that side has no synonyms at all |
| D57 | How is a curation set tested? | Only on a collection it is attached to, as `enable_curations=false` against the plain search, plus `curation_tags`. `curation_sets` is not a search parameter on 30.0 or 30.2. A test never attaches anything |
| D58 | What happens to attachments when a set is deleted? | The delete detaches it from every collection first, stops at a failure, deletes, and checks afterwards. A missing attached synonym set makes every search on the collection fail, while a missing curation set only blocks later `PATCH`es that list it. Missing sets show in the view with Detach |
| D59 | How is an attachment written? | `PATCH` of the whole array, right after reading the collection and the set names fresh. Names that no longer exist are dropped and named, because the `PATCH` would fail on them. There is no compare-and-swap |
| D60 | Item endpoints or whole-set `PUT`? | The table saves item by item, so a row edit cannot overwrite a colleague's other rows. JSON, import and Replace use the whole-set `PUT` with a review of adds, changes and removals. New refuses an existing name after a fresh list, because every `PUT` is an upsert. Both set kinds share one client module |
| D61 | Which names can Tessera create? | `[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}` for aliases, sets, stopwords and presets. Existing names stay usable whatever they hold; one with a comma cannot go into `synonym_sets=` |
| D62 | Confirmations in M6 | New, item save, attach, detach: none, a plain confirmation on prod. Item delete: a plain confirmation. Whole-set save that removes items: a plain confirmation, typed on prod. Alias repoint or delete, stopwords delete, preset delete: a plain confirmation, typed on prod. Set delete: a plain confirmation, typed when attached or on prod. Reindex switch: refused while the counts differ, otherwise a plain confirmation, typed on prod or when joins would fail |
| D63 | Does the reindex copy go through a temp file? | Yes: export to a private temp directory, then import that file, as two ordinary jobs, with no change to either pipeline. One stream was the first design, but the server closes an export whose reader stalls for 50 to 70 s (30.0 and 30.2), and one slow batch does that. The cost is free disk of about the export's size while the copy runs |
| D64 | What blocks a reindex? | A required `store: false` field for a copy from the old collection, a blank embedding key, or a bad or taken name. Optional unstored fields, joins from other collections and other aliases are warnings: the operator may accept them |
| D65 | What guards the switch? | Counts are compared and shown; a difference raises the confirmation to typing the alias. The alias is read right before the `PUT`, and the assistant stops if it moved. The `PUT` is read back. A switch back stays until clean-up |
| D66 | Where does the assistant live? | A center tab per alias, with its state on the connection, so closing the tab keeps it, and the copy in the jobs panel. Nothing can be half-switched, so a disconnect loses only the wizard |
| D67 | CSV of a synonym set | `id,root,synonyms,locale,symbols_to_index`, lists joined by `;`, or a JSON array in the cell when a value holds `;` or the list starts with `[`. Lossless and readable for business teams. D55 applies |
| D68 | Where do stopwords and presets go in the sidebar? | Their own rows after Curations. One tab per kind and connection, one panel type |

---

## 17. References

- Typesense docs v30.2 and index for agents: https://typesense.org/docs/llms.txt
- Typesense OpenAPI spec: https://github.com/typesense/typesense-api-spec
- Typesense v30 breaking changes (synonym sets, curation sets, analytics format): https://typesense.org/docs/30.0/api/
- GPUI Kit: https://gpui-kit.com · repo: https://github.com/longbridge/gpui-kit · API: https://docs.rs/gpui-kit
- GPUI (Zed): https://gpui.rs
- Functional reference: https://github.com/bfritscher/typesense-dashboard (GPL-3.0)
- Community Rust client (reference): https://github.com/typesense/typesense-rust
- UX mockups: https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d
