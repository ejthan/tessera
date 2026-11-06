# M8 Operations and Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Server operations with safe confirmations, a backup and restore assistant, the §9 security audit, settings, migrations and layout persistence, on Typesense 30.0 and 30.2.

**Architecture:** `tessera-client` gains `operations` on `NodeClient` and `update`. `tessera-core` gains `operations`, `backup/` (format, archive, run, restore), `settings`, `layout`, migrations in `store`, `redact::line`. `tessera-app` gains the Ops and Backup resource kinds, two job kinds, `views/settings/`, `views/transport.rs` and a log setup. Three PRs: A (Tasks 1–4), B (Tasks 5–8), C (Tasks 9–14).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11), reqwest 0.13.5, tokio 1.54.1, tracing-subscriber 0.3.23, plus tracing-appender 0.2.5, tar 0.4.46, zstd 0.14.3, zeroize 1.9.1.

**Spec:** [`docs/superpowers/specs/2026-11-06-m8-operations-design.md`](../specs/2026-11-06-m8-operations-design.md). Section numbers below refer to it. Read the section a task names first. **§4 was not probed on a server:** Task 1 does that, and PR A stops if a result disagrees with the design.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI. Direction: `app → widgets → core → client`.
- Direct dependencies are pinned exactly in the workspace `Cargo.toml`. Only the four of design §11 are new. Check licenses against `deny.toml`.
- Never guess a GPUI or GPUI Kit API. Read the pinned source under `~/.cargo/registry/src/*/` (`gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0`, `gpui-pre-0.3.11`) first. For a new crate read its pinned source after `cargo fetch`.
- Typesense truth is `spec/typesense-openapi.yml`, the v30.2 docs, and design §4 once Task 1 confirmed it. Never guess a request shape.
- Every UI string goes through `t!()`, same keys in `assets/locales/en.yml` and `de.yml`, flat keys, under `ops.*`, `backup.*`, `restore.*`, `settings.*`, `transport.*`. The key-parity test stays green. Core words nothing: it returns enums, and the app translates them.
- No blocking I/O on the GPUI thread and none on a tokio worker: `spawn_blocking` for tar, hashing and file reads.
- **Secrets.** No key in a log, file, backup, error, `Debug`, `Display` or persisted state. Redaction tests are never removed. `blank_secrets` runs on every schema before it is written to a backup.
- **Untrusted input.** A backup comes from anywhere. A path in a manifest or archive passes `allowed_path`, a name goes into a URL only through `collection_path` or `resource_path`, and a server path passes `check_snapshot_path` and `encode_query`.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. No commit with a failing check.
- Commits: `git commit -s`, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense on port 18108, key `xyz`. **Never send anything to port 8108.** Destructive probes and live tests of vote, snapshot and compact never run on 18108. Use `docker run --rm -d --name t302 -p 18208:8108 --tmpfs /data typesense/typesense:30.2 --data-dir /data --api-key=xyz --enable-cors=false`, the same with `typesense:30.0` afterwards, and for the second server of a round trip the same on 18209. Stop them afterwards. The cluster: `docker compose -f dev/docker-compose.yml --profile cluster up -d ts1 ts2 ts3` on 18111–18113, then `--profile cluster down -v`.
- If Docker does not answer, stop and report. Do not restart OrbStack: it holds another project's server.
- UX rules: environment visible everywhere (`dialog_title`, `env_badge::render`); errors inline and in full; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown via `Kbd::binding_for_action`.
- Shortcuts are bound in key contexts, platform constants in `#[cfg(target_os = "macos")]` pairs in `crates/tessera-app/src/actions.rs`. New actions go into `ACTION_LABELS` with a `palette.action.<Name>` key, and the two `alt-cmd-N` and `ctrl-alt-N` lists gain `0`.
- Writes go through `ConnectionState::ensure_writable(window, cx, then)`, and the control shows `env_badge::lock(id, false, cx)` while `writes_locked()`. Destructive flows confirm first through `confirm_write`; cancelling sends nothing. A write runs detached.
- File saves are chained through a `WriteChain` in `AppState` and flushed on quit.
- NF5: the Linux release binary stays below 41,943,040 bytes. Each PR records the size in its description.
- Each task ends with its CHANGELOG entry under `## [Unreleased]` → `### Added`, in the same commit, plain prose, in the style of the entries there.

## Review Focus

1. `POST /config` must send one key per Apply and only keys of `CONFIG_KEYS`; `skip-writes` must not be offered. Tasks 2 and 3.
2. Vote and snapshot must be typed on prod, and each operation must go to the node on screen, never to the failover. Tasks 1 and 3.
3. `unpack` must never call `Entry::unpack`, must refuse every crafted archive of Task 5, and must stop at the declared size.
4. A backup must hold no provider key, including through `extra`. Task 6.
5. A restore must recompute its plan before the first write, and must never write to an existing collection. Task 6.
6. Aliases must come last in a restore. Task 6.
7. A job must survive closing its tab, and a read-only connection must refuse a restore until unlocked. Tasks 7 and 8.
8. The log writer must mask both layers, and no call site may log around it. Task 9.
9. A failed migration must leave the file and write nothing over it. Task 10.
10. An explicit theme must survive a system appearance change. Task 11.
11. The update check must send no key and show no URL from the answer. Task 12.

---

## PR A · Operations and safety (branch `feat/m8-operations`, from the tip of `feat/m7-ai`)

### Task 1: Probes, then the operations client

**Files:**
- Create: `crates/tessera-client/src/operations.rs`, `crates/tessera-client/tests/operations.rs`, `crates/tessera-client/tests/live_operations.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-client/tests/cluster.rs`, `.github/workflows/ci.yml`, `.github/workflows/nightly.yml`, the design

**First:** run P1 to P12 of design §4.6 with curl on the servers named in Global Constraints, on 30.2 and on 30.0. Write the results into design §4 as measured, replacing **source** and **expected**, and remove the probe table's open status from the header. If P2, P4, P8, P9, P10 or P12 differs from what the design assumes, change the design and this plan first and say so in the report. Remove every probe resource and stop the servers.

**Interfaces (on `NodeClient`, through `self.client.write(Some(self.index), …)`):**

```rust
pub async fn snapshot(&self, path: &str) -> Result<(), Error>;
pub async fn vote(&self) -> Result<bool, Error>;
pub async fn compact_db(&self) -> Result<(), Error>;
pub async fn clear_cache(&self) -> Result<(), Error>;
pub async fn set_config(&self, key: &str, value: &Value) -> Result<(), Error>;
```

**Behaviour:**
- `snapshot`: `POST operations/snapshot`, query `snapshot_path=` through the crate's `encode_query`, timeout `LONG_TIMEOUT`.
- `compact_db`: `POST operations/db/compact`, `LONG_TIMEOUT`.
- `set_config`: `POST config`, body `{key: value}`, exactly one key.
- Any 2xx passes, because the source answers 201 on `/config` and the OpenAPI file says 200.
- A 2xx whose JSON has `"success": false` is `Error::http(status, body)`, except `vote`, which returns `false`. A body without `success` passes.
- All five use the read-only guard already in `call`. None fails over: the call is pinned and `POST` is not in `FAILOVER_METHODS`.

**Tests:** wiremock for the method, path, encoded query, body, 201, `success:false`, 409 with the message kept in `Error::Http`, read-only refusal. Live (`#[ignore]`), on `TESSERA_IT_URL`: snapshot into `/tmp/tessera-it-<pid>`, compact, clear cache, each config key set back to its start value afterwards where the start value is known. `live_operations` must refuse to run when the URL's port is 18108 or 8108. Cluster: vote on a follower moves the leader within 30 s, then the test votes the first leader back.

**CI:** add `live_operations` to `integration` and the vote test runs already through `cluster`.

**Verify:** `TESSERA_IT_URL=http://localhost:18208 TESSERA_IT_KEY=xyz cargo test -p tessera-client --test live_operations -- --ignored`, on 30.2 and 30.0. Cluster: `cargo test -p tessera-client --test cluster -- --ignored --test-threads=1`.

### Task 2: Core operations and the console danger

**Files:**
- Create: `crates/tessera-core/src/operations.rs`
- Modify: `crates/tessera-core/src/lib.rs`, `crates/tessera-core/src/raw.rs`

**Interfaces:** design §5.1, exactly.

**Exact values:**
- `check_snapshot_path`: empty → `Empty`; no leading `/` → `NotAbsolute`; only slashes → `Root`; a segment `..` → `Climbs`; any `char::is_control` → `Control`; over 1,024 bytes → `TooLong`. Spaces and non-ASCII pass.
- `CONFIG_KEYS`, in this order: `log-slow-requests-time-ms` (min −1), `log-slow-searches-time-ms` (min −1), `enable-search-logging` (bool), `cache-num-entries` (min 1), `embedding-cache-num-entries` (min 1), `healthy-read-lag` (min 1), `healthy-write-lag` (min 1). Upper bound `i32::MAX`.
- `parse_config`: an integer must parse whole, `1e3` and `1.0` are refused; bools are exactly `true` and `false`.
- `confirm_for`: Snapshot and Vote: Plain, on prod Typed. Compact and ClearCache: Plain. Config: None, on prod Plain.
- `raw_danger`: `POST config` is `by_env(RawDanger::Medium, high("config"))`. Extend the table test at `raw.rs:282`.

**Tests:** table tests for each function. A test asserts `skip-writes` is not in `CONFIG_KEYS`.

**Verify:** `cargo test -p tessera-core operations raw`.

### Task 3: Ops tab

**Files:**
- Create: `crates/tessera-app/src/views/resources/operations.rs`, `crates/tessera-app/tests/ui_operations.rs`
- Modify: `crates/tessera-app/src/admin.rs`, `crates/tessera-app/src/state.rs` (`ResourceKind`, `ALL` gets size 10 here, 11 in Task 7), `crates/tessera-app/src/actions.rs`, `crates/tessera-app/src/views/root.rs`, `crates/tessera-app/src/views/sidebar.rs`, `crates/tessera-app/src/views/resources/mod.rs`, `crates/tessera-app/tests/common/mod.rs`, locales, `CHANGELOG.md`

**Behaviour:** design §7.1 to §7.3.
- `AdminBackend` gains `snapshot`, `vote`, `compact`, `clear_cache`, `set_config`, each taking `client: Client, node: usize`, run with `client.node(node)`, and `schema_changes` for polling through `client.polling()`.
- `ResourceKind::Operations`: `key()` is `"operations"`, `title_key()` gives `sidebar.operations` ("Ops"), `shown` is true on every server. `OpenOperations` chord: `alt-cmd-0`, `ctrl-alt-0`, global, in `resource_shortcut`, and in `root.rs` as the other kinds.
- Node picker: a `Select` of the profile's nodes with host and role from `ConnectionState.nodes[i].debug`, default `client.active_node()`. Picking does not change the active node.
- Rows as in design §7.2, each with explanation text, its button with `.danger()` for Snapshot, Compact and Vote, its result under it with node and time.
- Snapshot: `check_snapshot_path` runs as the user types, the problem shows under the input, the button is disabled with it. The dialog states the path and node.
- Confirmation by `confirm_for`: Typed through `confirm_write` with `typed: true`, `expected` the profile name; Plain with `typed: false`; None runs directly.
- Vote disabled with a reason when the node's state is leader or the profile has one node. Adjust to what P4 measured.
- Runtime config: a `Select` over `CONFIG_KEYS`, a value input or a switch by kind, Apply, and always the line that the value cannot be read back and lasts until that node restarts (as P7 measured).
- Schema changes: a table of collection, validated and altered docs, polled every 2 s while the tab shows, by a `Task` on the view, stopped with it. An empty list says nothing runs.
- Each action sits behind `ensure_writable`, shows the lock while `writes_locked()`, and runs detached. A stale result after a node change still names its own node.

**UI tests (with a fake backend):** typed on prod for snapshot and vote, plain elsewhere; cancel sends nothing; a read-only connection unlocks first; bad path disables the button; config sends one key; the node sent is the picked one; Esc leaves focus sane; default focus is not on the red button.

**Verify:** `cargo test -p tessera-app --test ui_operations`.

### Task 4: Safety UX audit and the transport banner

**Files:**
- Create: `crates/tessera-app/src/views/transport.rs`, `crates/tessera-app/tests/ui_safety.rs`
- Modify: `crates/tessera-core/src/profile.rs`, `crates/tessera-app/src/views/dock.rs`, `crates/tessera-app/src/views/mod.rs`, views whose destructive button is not red, locales, `CHANGELOG.md`

**Interfaces:**

```rust
pub enum TransportRisk { PlainHttp { host: String }, InsecureCertificates }
impl Profile { pub fn transport_risks(&self) -> Vec<TransportRisk>; }
```

`PlainHttp` for every node with `Protocol::Http` and not `is_local()`, the nearest node too. `InsecureCertificates` for `tls.allow_insecure`.

**Behaviour:**
- `transport::banner(conn, context, window, cx) -> Option<impl IntoElement>`, one line per risk, id `transport.banner`, at each of the seven `unlock::banner` call sites in `dock.rs`, above it. Edit connection… opens `ConnectDialog` for the profile. A temporary connection has no such button. No close control.
- Audit: list every `Button::new` whose handler deletes, truncates or removes server data, or deletes a saved query. Each gets `.danger()`. First candidates: `search.saved.delete` (`views/search/queries.rs:931`), `documents.delete` (`views/documents/mod.rs:1102`). Removal from a local draft stays as it is: `search.remove`, `console.file.remove`.
- Confirm that each hand-built dialog with a destructive action uses `confirm` or `confirm_danger`. If one does not, move it.

**Tests:** `transport_risks` unit table (localhost, `::1`, remote http, https, insecure). UI: a banner for each case, none for a local or https connection; each destructive button found has the danger style, by reading the element in a window test as `ui_collections.rs` does.

**Verify:** `cargo test -p tessera-core profile && cargo test -p tessera-app --test ui_safety`.

---

## PR B · Backup (branch `feat/m8-backup`, from `feat/m8-operations`)

### Task 5: Backup format and archive

**Files:**
- Create: `crates/tessera-core/src/backup/mod.rs`, `format.rs`, `archive.rs`, `crates/tessera-core/tests/backup_archive.rs`
- Modify: workspace `Cargo.toml`, `crates/tessera-core/Cargo.toml`, `Cargo.lock`, `crates/tessera-core/src/lib.rs`

**Dependencies:** `tar = { version = "=0.4.46", default-features = false }`, `zstd = { version = "=0.14.3", default-features = false }`. Read both pinned sources first and write here, in the commit message, what you verified: `tar::Builder::append_data` or `append`, `tar::Archive::entries`, `Entry::header().entry_type()`, `Entry::path_bytes`, and that reading `Entry` as `Read` writes nothing to disk; `zstd::stream::write::Encoder::new`, `.finish()`, `zstd::stream::read::Decoder::new`, and which feature bundles the C sources. If `tar` without default features loses what is needed, keep the defaults and note why.

**Interfaces:** design §5.2 and §5.3, exactly. `JobControl` from `transfer::import`.

**Behaviour:**
- `allowed_path(path)`: exactly `aliases.json`, `stopwords.json`, `synonym_sets.json`, `curation_sets.json`, or `collections/NNNN.schema.json` or `collections/NNNN.jsonl`, with four to nine ASCII digits. `manifest.json` is handled apart.
- `Manifest::check`: `format` 1 to `FORMAT`; each `files[].path` passes `allowed_path`, unique; each collection's `file` unique, with both its files listed; at most 100,000 collections and 200,010 files; `sha256` is 64 lowercase hex.
- `pack` and `unpack` as in design §5.3. `pack` checks `cancel` between files and every 16 MiB. A target that is a symbolic link is refused with `symlink_metadata`. Permissions 0600 and 0700 under `#[cfg(unix)]`.
- `unpack`: refuse in this order: first entry not `manifest.json`, entry over 64 MiB for the manifest, `Manifest::check`, entry type not regular, path not listed, path seen before, bytes over declared, fewer bytes than declared, files listed but absent at the end.
- `verify`: size, then sha256 by a 64 KiB buffer, checking `cancel` between reads, on the blocking thread.

**Tests:** round trip of a folder through `pack`, `unpack`, `verify`. Crafted archives built with `tar::Header` raw bytes: `../x`, `/etc/x`, a symlink, a hard link, a character device, a FIFO, a duplicate, an unlisted file, more bytes than declared, manifest second, manifest over the limit, `format: 2`. In each case assert the error and that nothing appears outside `into`. `verify` with one flipped byte, a truncated file, an extra file ignored. A stop leaves no part file.

**Verify:** `cargo test -p tessera-core --test backup_archive`; `cargo deny check` if installed.

### Task 6: Backup and restore engines

**Files:**
- Create: `crates/tessera-core/src/backup/run.rs`, `restore.rs`, `crates/tessera-core/tests/backup_run.rs`, `crates/tessera-core/tests/live_backup.rs`
- Modify: `crates/tessera-core/src/backup/mod.rs`, `.github/workflows/ci.yml`

**Interfaces:** design §5.4 and §5.5. `run_backup` and `run_restore` own their blocking work and reuse `run_export` and `run_import` without changing them.

**Behaviour:** the numbered steps of design §5.4 and §5.5, in order. Also:
- Dictionary names come from the helper `tessera_core::stemming` already has for naming users of a dictionary, on the field `stem_dictionary`.
- `Mask` block: `redact::text_has_mask` on a schema's JSON text.
- `BlankSecrets`: `SchemaDraft::from_value(..).empty_secrets() > 0`.
- `RequiredUnstored`: fields with `store: false` and without `optional`, as in `relevance::reindex::preflight`, listed in the manifest's `unstored` and looked up in the schema.
- Set comparison reuses `relevance::changes`, as D60's review does. Aliases compare `collection_name`; stopwords compare the sorted words and the locale, since the server reorders them.
- Replace on: `upsert_set`, `upsert_stopwords`, `upsert_alias` for `Differs` rows. Off: named as kept.
- `synonym_sets` in a schema naming a set neither in the backup nor on the server: drop it with a warning, as `reindex::without_missing` does. Same for `curation_sets`.
- `RestoreOutcome` holds a row per resource: done, kept, skipped, failed with text, and `to_delete`, the collections created by this run.
- Before the first write: fresh `RestoreView`, fresh `plan`; a new `CollectionExists` for a ticked collection ends the run with no write.
- A stop finishes the current batch, as `run_import` does, and reports what stands.

**Tests:** with wiremock: order of writes; a refresh that finds a new collection writes nothing; a failed import stops before the next collection and before aliases; aliases last; Replace off writes nothing to a `Differs` row; `plan` for every `RowState`; reference order, alias reference, cycle; a schema with an `api_key` in `embed.model_config` and one under an unknown key of `model_config` written to a backup holds neither. Live (`#[ignore]`, `TESSERA_IT_URL`, `TESSERA_IT_URL2`, default keys `xyz`): `cargo xtask seed` data on the first, backup of `books`, `authors`, `books_ref`, `products` as a folder and an archive, restore into the empty second, compare `num_documents`, sorted exports, sets, stopwords, aliases.

**CI:** `integration` gets a second service `typesense2` on host port 8109, `TESSERA_IT_URL2`, and the `live_backup` run.

**Verify:** `TESSERA_IT_URL=http://localhost:18208 TESSERA_IT_URL2=http://localhost:18209 cargo test -p tessera-core --test live_backup -- --ignored`, on 30.2 and 30.0. Seed the first with `cargo xtask seed --url http://localhost:18208 --key xyz`.

### Task 7: Jobs and the backup view

**Files:**
- Create: `crates/tessera-app/src/views/resources/backup.rs`, `crates/tessera-app/tests/ui_backup.rs`
- Modify: `crates/tessera-app/src/jobs.rs`, `crates/tessera-app/src/transfer.rs`, `crates/tessera-app/src/relevance.rs`, `crates/tessera-app/src/views/jobs/panel.rs`, `crates/tessera-app/src/views/root.rs`, `crates/tessera-app/src/state.rs`, `crates/tessera-app/src/actions.rs`, `crates/tessera-app/src/views/sidebar.rs`, `crates/tessera-app/src/views/resources/mod.rs`, `crates/tessera-app/tests/common/mod.rs`, locales, `CHANGELOG.md`

**Behaviour:** design §7.1, §7.6 Backup, §7.7.
- `TransferBackend` gains `backup` and `restore`, like `export`, on `TokioBackend`, on a task that does not die with a view.
- `Jobs::start_backup` and `start_restore` through `start`. The panel row shows step, collection, `done` of `total`, and for a running export or import its numbers. Stop works; the second stop (abort) acts as for imports.
- `reload_after` also runs for `JobKind::Restore`: reload the collection list on the job's profile.
- `COPY_PREFIX` stays; add `BACKUP_PREFIX = "tessera-backup-"`, and `remove_stale_copies` takes both. Extend its test.
- `ResourceKind::Backup`, key `backup`, `OpenBackup`, `alt-cmd-b`, `ctrl-alt-b`. `ALL` has size 11.
- This task shows the Backup view alone. The ⌘1 and ⌘2 switch, in the pattern of `views/resources/ai.rs`, arrives with Task 8, so that no dead control ships.
- Backup view: checkboxes, All, count per collection, format radio, notes as in design §7.6, Start (⌘↩), `cx.prompt_for_new_path` in the pattern of `views/jobs/export_dialog.rs`. A target that exists is refused inline. A symbolic link is refused.

**UI tests (fakes):** Start without a selection is disabled; the warnings for an unstored field, blanked key and an outside reference; a job appears and goes on with the tab closed; no unlock is asked on a read-only connection.

**Verify:** `cargo test -p tessera-app --test ui_backup`, and `relevance` unit tests.

### Task 8: Restore view

**Files:**
- Create: `crates/tessera-app/src/views/resources/restore.rs`
- Modify: `crates/tessera-app/src/views/resources/backup.rs`, `crates/tessera-app/tests/ui_backup.rs`, `docs/building.md`, locales, `CHANGELOG.md`

**Behaviour:** design §7.6 Restore.
- Choose… (⌘O): `cx.prompt_for_paths` with files and directories allowed, in the pattern of the import dialog's file pick. A file goes through `unpack` into a `tessera-backup-` temp directory held by the view; a folder must hold `manifest.json`. Then `verify`, then load, then `plan`. Each stage shows, Stop works, and every failure shows inline in full.
- A table of rows with kind, name, state and detail. Blocked rows state the reason and carry no checkbox. Others are ticked by default, except `Differs`, which follows Replace.
- Confirmation through `ensure_writable` then `confirm_write`. `typed` when prod, or when Replace is on and at least one row would change. `expected` is the profile name. Lines state: collections and documents to create, sets, stopwords and aliases to create and to replace, and that existing collections stay untouched.
- After the job ends, the tab shows `RestoreOutcome` rows and the list to delete before a retry. The temp directory goes when the view lets go of it or a new file is chosen; a running job holds its own `Arc<TempDir>`.

**UI tests:** a blocked collection cannot be ticked; Replace off keeps a differing row; typed on prod and with Replace; cancel writes nothing; read-only unlocks first; a bad manifest shows inline; the job survives closing the tab.

**Docs:** `docs/building.md` gains "Manual checks of operations and backup (M8)": each check on screen, vote on the cluster, and a restore into a second server.

**Verify:** `cargo test -p tessera-app --test ui_backup`.

---

## PR C · Hardening and settings (branch `feat/m8-settings`, from `feat/m8-backup`)

### Task 9: Log files and redaction

**Files:**
- Create: `crates/tessera-app/src/logging.rs`, `crates/tessera-app/tests/logging.rs`
- Modify: `crates/tessera-core/src/redact.rs`, `crates/tessera-core/src/store.rs`, `crates/tessera-app/src/lib.rs`, workspace `Cargo.toml`, `crates/tessera-app/Cargo.toml`, `Cargo.lock`, `docs/building.md`, `CHANGELOG.md`

**Dependencies:** `tracing-appender = "=0.2.5"`. Read its pinned source first: `RollingFileAppender::builder`, `Rotation::DAILY`, `filename_prefix`, `max_log_files`, `non_blocking`, `WorkerGuard`, including when old files are pruned. Spec §11 says 7 days, and the crate counts files, which is the same at one file a day; a day without a start writes no file, so state in the code comment what you find.

**Behaviour:**
- `store::logs_dir()`: `ProjectDirs::data_local_dir()` plus `logs`.
- `redact::line`, design §5.9.
- `logging::init(dir: Option<&Path>) -> Option<WorkerGuard>`: a `registry()` with `EnvFilter` (`info` by default) and `fmt::layer()` to stderr and, if `dir` works, a second `fmt::layer()` with `.with_ansi(false)` to the roller. Both through `Redacted<M>`, a `MakeWriter` whose writer holds the inner writer and in `write` masks `from_utf8_lossy(buf)` and writes the result, reporting `buf.len()` as written. Verified in `tracing-subscriber-0.3.23/src/fmt/fmt_layer.rs`: one event is one `write_all`.
- Replaces the `tracing_subscriber::fmt()` block in `run`. The guard lives until `run` returns.
- A failed log directory: a `warn` on stderr, the app goes on.
- NF1: `let started = Instant::now()` first in `run`, and in the existing `window.on_next_frame` path, one for every start, an `info` line `ready in {ms} ms`.

**Tests:** an event with `http://h/x?x-typesense-api-key=abc&a=1`, with `"api_key":"abc"`, and with a header line leaves no `abc` in the file. A file appears with the prefix `tessera.log`. Newlines stay. A test proves `send.rs`' debug line never carries a key, with `RUST_LOG=debug` semantics through a local subscriber.

**Docs:** `docs/building.md` Run section: where logs live.

**Verify:** `cargo test -p tessera-app --test logging && cargo test -p tessera-core redact`.

### Task 10: Migrations, settings file, layout file

**Files:**
- Create: `crates/tessera-core/src/settings.rs`, `crates/tessera-core/src/layout.rs`
- Modify: `crates/tessera-core/src/store.rs`, `crates/tessera-core/src/search/saved.rs`, `crates/tessera-core/src/documents/views.rs`, `crates/tessera-core/src/documents/export.rs`, `crates/tessera-core/src/lib.rs`, the three callers of `danger_level`, `CHANGELOG.md`

**Interfaces:** design §5.6 to §5.8.

**Behaviour:**
- `load_migrated` as in design §5.7. The `.bak` name is the file name plus `.bak`. Order: read, check newer, copy to a temp beside it and rename to `.bak`, migrate, write through `write_atomic`. Any failure before the write leaves the file. Each file passes `&[]` today and has a one-line comment.
- `documents/views.rs` moves onto it and so refuses a newer file: `load` keeps its shape of logging and returning the default, and sets a flag that stops saving, as `saved.rs` does through `AppState`.
- `write_atomic`'s temp name stays. `settings::save` uses `toml::to_string_pretty` then `write_atomic`.
- `danger_level(count, threshold, prod)`, with `threshold` from settings. `prod` is `High` at any count; `count > threshold` is `High`.

**Tests:** migration with a fake list of two steps, from level 1 and level 2; `.bak` holds the old bytes; a failing step leaves both alone; a newer file is refused for all three. Settings: default on a missing file, unknown keys ignored, a bad file is an error and is not written, clamping at both ends of every field. Layout: clamping, a bad file gives the default.

**Verify:** `cargo test -p tessera-core store settings layout views`.

### Task 11: Settings dialog: appearance and defaults

**Files:**
- Create: `crates/tessera-app/src/views/settings/mod.rs`, `crates/tessera-app/src/settings.rs`, `crates/tessera-app/tests/ui_settings.rs`
- Modify: `crates/tessera-app/src/state.rs`, `crates/tessera-app/src/lib.rs`, `crates/tessera-app/src/i18n.rs`, `crates/tessera-app/src/actions.rs`, `crates/tessera-app/src/views/root.rs`, `crates/tessera-app/src/views/palette.rs`, `crates/tessera-app/src/poller.rs`, the document page size and import default sites, data table sites, locales, `CHANGELOG.md`

**Behaviour:** design §7.8 Appearance and Defaults.
- `AppState` holds `settings: Settings`, `settings_path`, `settings_error: Option<String>`, and a `settings_chain: WriteChain`. Load once in `run`, before the window.
- `OpenSettings`, `cmd-,`, `ctrl-,`, in the title bar as a ghost button with `IconName::Settings` on the right of `TitleBar`, with its tooltip and shortcut, and in `ACTION_LABELS`, so the palette lists it (D44).
- Dialog pattern from `views/connect_dialog/mod.rs`: `window.open_dialog`. Read `gpui-component-0.9.0/src/setting/` and use it if a keyboard reaches each field; otherwise plain rows. Say which in the commit.
- Theme apply: `system` runs `Theme::sync_system_appearance(Some(window), cx)`; `light` and `dark` run `Theme::change(ThemeMode::…, Some(window), cx)`; a name sets the registry theme, read in `theme/registry.rs` and `theme/mod.rs`. The `observe_window_appearance` handler in `RootView::new` runs sync only for `system`.
- Font size: `Theme::update(cx, |t| t.font_size = px(n))`, and `None` keeps the kit's default, read at start before any change. Read how `Root` picks it up at `root.rs:714`.
- Density: a global `Density` read at each data table: `DataTable … .with_size(size)`, `Size::XSmall` for compact and the present size for comfortable (`table/data_table.rs:183`).
- Language: `locale_for` takes the setting first. Runtime change: `gpui_kit::component::set_locale` and `cx.refresh_windows()`.
- `per_page`: `tessera_core::documents::PER_PAGE` stays the default and the cap; find every use and pass the setting where a page is requested. Read it when a documents view opens.
- `batch_size`: the import dialog's start value in place of `DEFAULT_BATCH` (`views/jobs/import_dialog.rs:274`).
- `poll_seconds`: the start value for a new connection's interval in place of `DEFAULT_INTERVAL`, if the overview's picker offers it.
- `confirm_above`: the three `danger_level` callers read it.

**UI tests:** each control changes the state and survives a restart of `AppState` from the file; an explicit theme survives an appearance change; a bad `settings.toml` shows inline and saves nothing; the palette lists Settings; `confirm_above` of 5 makes a delete of 6 typed. Font size 20 at the minimum window: the title bar and status bar stay inside.

**Verify:** `cargo test -p tessera-app --test ui_settings`.

### Task 12: Settings: update check, storage, about

**Files:**
- Create: `crates/tessera-client/src/update.rs`, tests in `crates/tessera-client/tests/update.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-core/src/settings.rs`, `crates/tessera-app/src/views/settings/mod.rs`, `crates/tessera-app/src/relevance.rs`, `crates/tessera-app/src/admin.rs`, `crates/tessera-app/Cargo.toml`, locales, `CHANGELOG.md`

**Behaviour:** design §4.4, §7.8 Privacy, Storage and About.
- `latest_release(api_url)` as in design §4.4. Constant `RELEASES_API = "https://api.github.com/repos/ejthan/tessera/releases/latest"` in core. Core: `pub fn is_newer(tag: &str, current: &str) -> Option<bool>`, stripping one leading `v`, by `semver`; a tag that does not parse is `None` and no prompt.
- Off by default. On, it runs once at start, on the runtime, and by Check now. A result shows in the section; a newer version also shows one toast, a background event. A failure shows only in the section.
- Storage: config, data and logs paths with Copy; Open logs (⌥⌘L / Ctrl+Alt+L, in the settings key context) through `cx.reveal_path`; Clear cache asks plainly, removes `tessera-reindex-` and `tessera-backup-` directories not held by a running job, via `spawn_blocking`, by a new `relevance::clear_copies(held: &[PathBuf]) -> usize`, and calls `invalidate_collections` on each open connection, then says how many went.
- About: `env!("CARGO_PKG_VERSION")`, GPL-3.0-or-later, the §2.3 notice (the text is in `README.md`; `en.yml` has a key to reuse if the grep finds it), link to the releases page by `cx.open_url`.

**Tests:** wiremock: 200, 404, a body over 1 MiB, no `X-TYPESENSE-API-KEY` header, a `User-Agent`. `is_newer` table. UI: off by default sends nothing; Check now with a fake shows the version; Clear cache keeps a held directory.

**Verify:** `cargo test -p tessera-client --test update && cargo test -p tessera-app --test ui_settings`.

### Task 13: Layout and empty states

**Files:**
- Modify: `crates/tessera-app/src/views/root.rs`, `crates/tessera-app/src/state.rs`, `crates/tessera-app/src/lib.rs`, `crates/tessera-app/src/views/jobs/panel.rs`, `crates/tessera-app/src/views/stats_table.rs`, `crates/tessera-app/src/views/search/queries.rs`, `crates/tessera-app/src/views/request_log.rs`, `crates/tessera-app/tests/ui_shell.rs`, locales, `docs/spec.md` if a row closes differently, `CHANGELOG.md`

**Behaviour:** design §5.8, §7.9, §7.10.
- `run` loads the layout, `RootView::new` takes it: `SIDEBAR_WIDTH` and `LOG_HEIGHT` become fallbacks; `bottom_open` and `bottom_tab` replace the closed start.
- On `DockEvent::LayoutChanged` read `area.dock_size` and `is_dock_open`, compare, and save after 500 ms through a `WriteChain`, dropping a pending timer on a newer change. Confirm in `gpui-base-0.9.0/src/dock/dock_area.rs` that a resize emits `LayoutChanged`. If only the end of a drag does, that is enough; if none does, save on quit through `cx.on_app_quit`.
- Empty states: the jobs panel adds a link to the import docs, API stats to the cluster operations docs, saved queries a sentence, the request log a sentence if it lacks one. Use `resources.docs` and `cx.open_url`. Check the §8.3 entries in `welcome.rs` hold, and write a test if none covers Run locally.

**UI tests:** sizes and bottom tab survive a rebuilt root; a bad file starts with the defaults; each empty state shows its link.

**Verify:** `cargo test -p tessera-app --test ui_shell`.

### Task 14: Zeroization, security checklist, close-out

**Files:**
- Create: `docs/security-checklist.md`
- Modify: `crates/tessera-app/src/views/connect_dialog/mod.rs`, `crates/tessera-app/src/views/resources/key_dialog.rs`, workspace `Cargo.toml`, `crates/tessera-app/Cargo.toml`, `xtask/src/seed.rs` if needed, `docs/building.md`, `docs/architecture/async.md`, `docs/spec.md`, `CHANGELOG.md`

**Behaviour:**
- `zeroize = "=1.9.1"` in `tessera-app`. The temporaries at `connect_dialog/mod.rs:185`, `key_dialog.rs:459` and `:502` become `Zeroizing<String>`. Search `expose_secret` again in non-test code for any other copy and treat it the same.
- `docs/security-checklist.md`: one row per point of §9, §7.1's AC, §7.19 redaction, §7.14's one-time value, §7.16's provider keys, read-only guard, the danger table (D28, D40, D49, D62, D81, D85), backups, with its proof: a test name, a CI job or a file. Exceptions of design §8.2 are written out. A point without proof gets a test here.
- `cargo deny check` passes, or the result is written down when the tool is not installed, with CI as the proof.
- `docs/architecture/async.md`: how a backup and restore run as one job.
- `docs/spec.md` §14: the M8 row's acceptance states what passed and what stays manual, as the M7 row does.

**Full check:** the whole of `cargo test --workspace`, then every `live_*` test on 30.0 and 30.2 and the cluster test, then the Linux or macOS release binary size. Stop all servers.

**Verify:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`.
