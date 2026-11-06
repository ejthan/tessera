# M8 Operations and hardening: design

- **Date:** 2026-11-06
- **Status:** written autonomously (maintainer takes the recommendations), pending final review. §4 is measured: the probes of §4.6 ran on 30.0 and 30.2 on 2026-11-06
- **Product spec:** [`docs/spec.md`](../../spec.md) (M8 row in §14; §7.7, §7.18, §7.21, §8.1 to §8.3, §9, §10, §11, §12, §16)
- **Previous milestone:** [M7 keys, analytics, AI design](2026-11-06-m7-keys-analytics-ai-design.md)
- **UX mockups:** none for these screens; the layouts follow M6's resource tabs

**How this was verified.** Server behaviour in §4 was measured with curl on throwaway 30.0 and 30.2 servers and on the compose cluster (§4.6). Statements still marked **source** or **docs** could not be provoked on a small server. Three results differ from what this design first assumed: a missing dictionary does not fail a create (P8), a missing parent directory does not fail a snapshot (P3), and a second snapshot was refused with a 500, not a 409 (P1). GPUI Kit APIs were read in the pinned source. `tracing-appender`, `tar` and `zstd` are not in the registry cache, so their APIs are unread: Tasks 5 and 9 read them after adding them.

## 1. Goal

M8 makes Tessera safe to point at production: server operations with the right confirmations, a backup and restore assistant, the §9 security audit, settings, and the persistence and layout work no earlier milestone took.

**Acceptance (product spec §14):**

- Every §7.18 operation works live on 30.0 and 30.2, vote on the 3-node cluster, each with a confirmation, snapshot and vote typed on prod, and each refused in a read-only profile until unlocked.
- A backup of `books`, `authors`, `books_ref` and `products` restored into an empty server gives equal document counts and equal exports, line for line after sorting, and the sets, stopwords and aliases match. This holds for a folder and for a `.tar.zst`.
- A restore never changes an existing collection, and changes an existing set, stopword set or alias only after the user chose to.
- `docs/security-checklist.md` has every §9 and §7.1 security point with the test or check that proves it, and none open.
- Settings cover §7.21 and survive a restart. Dock sizes survive a restart. A file of an older `schema_version` is migrated with a `.bak` beside it.

## 2. Delivery

One spec, one plan, three pull requests, stacked on `feat/m7-ai`, because merging is the maintainer's step.

| PR | Branch | Content | Why this order |
|---|---|---|---|
| **A · Operations and safety** | `feat/m8-operations` | Client and core operations, Ops tab, §8.2 audit, transport banner | Probes come first, and they can overturn this design |
| **B · Backup** | `feat/m8-backup` | Backup format, engine, jobs, Backup tab | Needs the new dependencies and the longest test |
| **C · Hardening and settings** | `feat/m8-settings` | Logs, migrations, settings, layout, empty states, checklist | Touches startup and many views; the checklist closes the milestone |

## 3. Scope

### In M8

§7.18, §8.2, the backup and restore assistant of §7.7, §9, §7.21 (D44), §11 migrations, §8.1 layout, §8.3 empty states, and the M8 rows of the sweep below.

### Spec sweep

§7, §8, §9, §10 and §11 were read against the code by search and against the CHANGELOG. Points already delivered are not listed. A search can miss a partial delivery, so a row found done during implementation is simply closed.

| Point | State | Decision |
|---|---|---|
| §7.1 banner for "allow insecure certificates" | Only in the connect dialog | M8, Task 4 |
| §9 banner for `http://` to a remote host | Only in the connect dialog | M8, Task 4 |
| §7.20 settings in the palette (D44) | Waits for a settings view | M8, Task 11 |
| §8.1 ⚙ in the title bar | Missing | M8, Task 11 |
| §8.1 layout restored | Missing | M8, Task 13, reduced by D94 |
| §2.3 About with the trademark notice | Text in the README only, no About | M8, Task 12, as a settings section |
| §8.2 destructive buttons red | `search.saved.delete` and `documents.delete` show no `.danger()` next to them in a first search | M8, Task 4 audits every one |
| §8.3 empty lists | Resource tabs are done. Jobs, API stats and saved queries have no docs link, and the request log was not checked | M8, Task 13 |
| §10 NF1 timestamp logging | Missing | M8, Task 9: one log line |
| §11 migrations with `.bak` | Missing | M8, Task 10 |
| `document_views.json` ignores a newer `schema_version` (deferred from M3) | Open | M8, Task 10 |
| D40: `POST /config` from the console rates as a plain write | Open, and it can set `skip-writes` | M8, Task 2 |
| §8.1 several windows | One `open_window`. `OpenOverview` and its kind go to every root view, so each would open the tab | M9 |
| §8.1 breadcrumb and ⌘K field in the title bar | Missing. The palette has its shortcut | M9 |
| §5.5 link from an error to its request log entry | Missing, and `Error` holds no request id | M9 |
| §2.2 issue and PR templates, labels | Missing | M9 |
| §10 NF4 debug assertion and spans | Missing | M9 |
| §10 NF8 string lint in xtask | Missing | M9 |
| §10 NF6, §12.1 `docs/qa-checklist.md` | Missing | M9, with the row's QA checklist |
| §8.4 accessibility, §13.2 packaging, Homebrew and `.rpm` | Not started | M9, as the row says |
| §10 NF2, NF5 Linux, on-screen checks | Open, need a screen | M9, maintainer's step |
| §7.2 memory estimate per collection (P2) | No milestone | v1.x, as all P2 |

`D32` stays open for the maintainer.

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Presets, analytics rules, keys, AI models, stemming dictionaries in a backup | v1.x | §7.7 lists what a backup holds. The dialog says what it leaves out, and a restore warns about a missing dictionary |
| Restoring a collection under another name | v1.x | References and aliases name it. Restore goes to a cluster without it |
| Restoring over an existing collection | not planned | Delete it first, by its own typed flow |
| A provider key typed during a restore | v1.x | Such a collection is blocked with the reason, and `schema.json` is plain JSON for the schema dialog |
| Editing `skip-writes` | not planned | Stops all writes on a node. The console reaches it, typed on prod |
| Restoring connection tabs at start | not planned | D94 |
| Window position and size | v1.x | Not asked by §8.1 |

## 4. tessera-client

New module `operations.rs`, and `update.rs` for the opt-in check.

### 4.1 Operations

All of them run on one node, so they live on `NodeClient`. Snapshot, compact, cache clear and config answer on a follower as on the leader, and nothing replicates them (**source**: the handlers call local objects). `write(Some(index), …)` pins the node, and a `POST` is not in `FAILOVER_METHODS`.

```rust
impl NodeClient {
    pub async fn snapshot(&self, path: &str) -> Result<(), Error>;   // LONG_TIMEOUT
    pub async fn vote(&self) -> Result<bool, Error>;
    pub async fn compact_db(&self) -> Result<(), Error>;             // LONG_TIMEOUT
    pub async fn clear_cache(&self) -> Result<(), Error>;
    pub async fn set_config(&self, key: &str, value: &Value) -> Result<(), Error>;
}
```

| Call | Request | Behaviour |
|---|---|---|
| `snapshot` | `POST operations/snapshot?snapshot_path=` | 201 `{"success":true}` when the copy is done, and `<path>/state` holds `meta` and `snapshot`. Missing parents are created. A path that cannot be written, or that is a file: 500 `{"error":"Copy failed.","success":false}`. A snapshot already saving: 500 `{"error":"Is saving another snapshot","success":false}`. **source:** also 409 `Another snapshot is in progress.`, which three parallel requests did not reach. The directory is not checked: it merges, its files stay, and a second snapshot adds a `snapshot_N` beside the first. Without a path, with an empty one and with a relative one the answer is 201 too. **docs:** without a path the server compacts its Raft log and exports nothing, while the OpenAPI file calls the path required. So the client takes a path and core refuses an empty or relative one (D87) |
| `vote` | `POST operations/vote` | 200 `{"success":false}` on a single node and on a leader. 200 `{"success":true}` on a follower, and the role moves seconds after the answer (P4) |
| `compact_db` | `POST operations/db/compact` | 200 `{"success":true}`. **source:** 500 with `success:false` and `error`. **docs:** does not block, off-peak advised |
| `clear_cache` | `POST operations/cache/clear` | 200 `{"success":true}`. Only the search response cache |
| `set_config` | `POST config` | 201 `{"success":true}`, the OpenAPI file says 200. 400 `Bad JSON.` or the messages of §4.2 |

A `documents:search` key gets 401 on all five.

Any 2xx is accepted. A 2xx whose body says `success:false` is an error, except on `vote`, which returns it: a refused vote is a result, not a failure. `schema_changes` exists and stays; a `polling()` client keeps its requests in the polling buffer (D12).

**source, a trap:** `get_schema_changes` calls `res->set_200` after its error path, so a failure can read as 200 with an odd body. The existing decode reports that as `Error::Decode`, which is fine.

### 4.2 What `POST /config` does (measured, same on 30.0 and 30.2)

Eight keys, each checked only if present. An unknown key is ignored and answers 201, and so does `{}`. Keys apply in order, so a later bad value leaves the earlier ones applied (**source**). There is no `GET /config`, and a value does not survive a restart (P7).

| Key | Type | Offered in Tessera |
|---|---|---|
| `log-slow-requests-time-ms` | integer, `-1` disables, `0` logs every request, the server takes lower values too | yes |
| `log-slow-searches-time-ms` | integer, as above | yes |
| `enable-search-logging` | boolean | yes |
| `cache-num-entries` | positive 32-bit integer | yes |
| `embedding-cache-num-entries` | positive 32-bit integer | yes |
| `healthy-read-lag` | positive 32-bit integer | yes |
| `healthy-write-lag` | positive 32-bit integer | yes |
| `skip-writes` | boolean | **no** (D86) |

A wrong type answers 400 `Configuration `<key>` must be an integer.` or `must be a boolean.`, and 0, a negative or 2,147,483,648 on a positive key answers `must be a positive integer.` So a "verify" by reading back is impossible, and a 201 for a misspelled key proves nothing. Hence: a closed list of keys, one key per Apply so no partial state exists, the node named, and the form says the value cannot be read back and lasts until that node restarts.

### 4.3 What a backup reads

`collections()`, `aliases()`, `sets(kind)`, `stopwords()`, `export_stream` through `run_export`: all exist. The restore writes through `create_collection`, `upsert_set`, `upsert_stopwords`, `upsert_alias` and `run_import`: all exist. No new endpoint.

### 4.4 Update check (`update.rs`)

```rust
/// `GET https://api.github.com/repos/ejthan/tessera/releases/latest`.
pub async fn latest_release(api_url: &str) -> Result<String, Error>;
```

A one-off `reqwest::Client` with a 10 s timeout, a `User-Agent` of `tessera/<version>` (GitHub refuses a request without one), no key header, no retry, a body limit of 1 MiB. It reads `tag_name` only. A 404 means no release yet and answers an error the view words as such. The link shown is the constant `https://github.com/ejthan/tessera/releases`, never a URL from the answer. `api_url` is a parameter for wiremock.

### 4.5 Node choice

`Client::node(index)` exists. The Ops tab reads roles from `ConnectionState.nodes[i].debug`, as the overview does.

### 4.6 Probes (run on 2026-11-06)

On throwaway 30.0 and 30.2 servers. Vote by curl on the 30.2 compose cluster on 18111-18113, and the `cluster` test on both versions. Never on 18108 or 8108. Both versions answered alike.

| # | Question | Measured | Consequence |
|---|---|---|---|
| P1 | Snapshot into a new directory | 201, `<path>/state/{meta,snapshot}`. Three at once: one 201, two 500 `Is saving another snapshot` | The refusal shows inline, whatever its status |
| P2 | Snapshot into a non-empty directory, twice into one path | 201 both. It merges: other files stay, and the second snapshot lands beside the first in `state/snapshot` | The dialog says a used directory keeps its files and collects snapshots (§7.3). Nothing is refused |
| P3 | Snapshot into an unwritable path, into a missing parent | Unwritable (`/proc/…`) or a file: 500 `Copy failed.` A missing parent is created: 201 | The error stays inline. The dialog does not say the directory must exist |
| P4 | Vote on a single node, on a leader, on a follower | Single and leader: 200 `success:false`. Follower: 200 `success:true`, and it led between 4 and 8 s later | The button rule of D88 stands. The result says the election started, not that the role moved |
| P5 | Operations with a `documents:search` key | 401 on all five | None |
| P6 | Each key of §4.2, a bad value, an unknown key | §4.2 | The catalog stands, and `i32::MAX` is the upper bound |
| P7 | `log-slow-requests-time-ms` after a container restart with a kept data dir | Back to its start value: at `0` every request logged `event=slow_request`, after the restart none did until it was set again | The note says "until that node restarts" |
| P8 | Does a `stem_dictionary` naming a missing dictionary fail a create? | **No.** 201, the field reads `stem:true`, and an import passes | The warning stays and nothing blocks. §5.5 is unchanged |
| P9 | Does a `reference` to a missing collection fail a create, and an import into it? | Create passes. An import line fails with `Referenced collection … not found.`, also on an optional field, and then with `Referenced document having id: … not found` until the target document exists. With `async_reference: true` the line passes | Order of §5.5 stands. A cycle fails line by line unless its fields are async |
| P10 | Import of documents that hold their embedding, for a field with `embed` | No provider call, the vector stays. A create calls the provider once, a document without the vector calls it too. A vector of the wrong size: `Field … contains an invalid embedding.` A `GET` shows the key's first five characters and `*` | As designed. `blank_secrets` covers the shortened key |
| P11 | A schema round trip through `SchemaDraft::from_collection` for nested fields, `.*`, `num_.*`, `token_separators`, `symbols_to_index`, `metadata`, sets, `reference`, `hnsw_params`, `vec_dist` | The clone equals the source in every attribute | None |
| P12 | Create with `synonym_sets` or `curation_sets` naming a missing set | 404 `Synonym set … not found.`, `Curation set … not found.`, nothing created | Order already avoids it |

## 5. tessera-core

### 5.1 `operations.rs`

```rust
pub enum PathProblem { Empty, NotAbsolute, Root, Climbs, Control, TooLong }
/// At most 1,024 bytes, starts with `/`, is not `/`, no segment `..`, no control character.
pub fn check_snapshot_path(path: &str) -> Result<(), PathProblem>;

pub enum ConfigKind { Integer { min: Option<i64> }, Bool }
pub struct ConfigKey { pub name: &'static str, pub kind: ConfigKind }
pub const CONFIG_KEYS: [ConfigKey; 7];
pub fn parse_config(key: &ConfigKey, text: &str) -> Result<Value, ConfigProblem>;

pub enum Operation { Snapshot, Vote, Compact, ClearCache, Config }
pub enum Confirm { None, Plain, Typed }
pub fn confirm_for(op: Operation, prod: bool) -> Confirm;
```

`confirm_for` is D85's table. Integers allow `min: Some(-1)` for the two log keys and `Some(1)` for the others.

`raw::raw_danger` gains `POST config`: `by_env(Medium, high("config"))`, because `skip-writes` is reachable there.

### 5.2 Backup format (`backup/format.rs`, D89)

```
<backup>/
  manifest.json
  aliases.json          [{ "name", "collection_name" }]
  stopwords.json        [{ "id", "stopwords", "locale" }]
  synonym_sets.json     [ the set as read ]
  curation_sets.json    [ the set as read ]
  collections/0001.schema.json
  collections/0001.jsonl
```

A collection's name can hold anything the server accepts, so it never becomes a path: files are numbered and the manifest maps them.

```rust
pub const FORMAT: u32 = 1;
pub struct Manifest {
    pub format: u32,
    pub created_at: i64,
    pub tessera_version: String,
    pub server_version: String,
    pub collections: Vec<ManifestCollection>,
    pub files: Vec<ManifestFile>,   // every file but the manifest
}
pub struct ManifestCollection {
    pub file: u32, pub name: String, pub documents: u64,
    pub unstored: Vec<String>, pub blanked_secrets: usize, pub dictionaries: Vec<String>,
}
pub struct ManifestFile { pub path: String, pub bytes: u64, pub sha256: String }
```

The manifest is **untrusted input**, because a backup comes from anywhere. `Manifest::check` refuses a `format` above `FORMAT`, a path outside the allowlist, a duplicate `file`, a `documents.jsonl` missing from `files`, and more than 100,000 entries. Names reach the server only through `collection_path` and `resource_path`, which encode them.

`allowed_path` is exact: the five top-level names, or `collections/` plus four or more digits plus `.schema.json` or `.jsonl`.

A manifest is written last in a folder and first in an archive. So a folder without one is incomplete, and an archive is checked before anything is unpacked.

### 5.3 Archive (`backup/archive.rs`)

Blocking, called through `spawn_blocking`.

```rust
pub fn pack(folder: &Path, target: &Path, cancel: &JobControl) -> Result<(), ArchiveError>;
pub fn unpack(archive: &Path, into: &Path, cancel: &JobControl) -> Result<Manifest, ArchiveError>;
pub fn verify(folder: &Path, manifest: &Manifest, cancel: &JobControl) -> Result<(), ArchiveError>;
```

- `pack` writes `manifest.json` first, then the files in manifest order, as regular entries with mode 0600 and mtime 0, through `zstd` level 3 into `<target>.part` with mode 0600, then syncs and renames. A symbolic link as target is refused, as for exports. Stopping removes the part file.
- `unpack` reads entry by entry and never calls `Entry::unpack`. The first entry must be `manifest.json`, at most 64 MiB. Every further entry must be a regular file, its exact path must be in `manifest.files`, and each path comes once. It copies at most the declared `bytes` plus one to a file created in `into` with `create_new`, and more than declared is an error. So traversal, links, device entries and a decompression bomb all end before they cost disk beyond the declared total. A check of free disk space is not done: the write fails with the system's message.
- `verify` hashes each file with `sha2` through a 64 KiB buffer and compares size and digest.

### 5.4 Backup (`backup/run.rs`)

```rust
pub struct BackupPlan { pub collections: Vec<String>, pub target: BackupTarget }
pub enum BackupTarget { Folder(PathBuf), Archive(PathBuf) }
pub struct BackupProgress { pub step: BackupStep, pub done: u64, pub total: u64, pub export: ExportProgress, pub collection: String }
pub struct BackupOutcome { pub end: JobEnd<BackupError>, pub collections: Vec<BackupRow>, pub warnings: Vec<BackupWarning> }
pub async fn run_backup(client: Client, plan: BackupPlan, control: JobControl, progress: watch::Sender<BackupProgress>) -> BackupOutcome;
```

Steps, in order:

1. Read `collections()`, `aliases()`, `sets` of both kinds, `stopwords()`, `debug()`. A read that fails stops the run: a backup silently without its aliases is worse than none.
2. For a folder, write into `<target>.part/`, created with mode 0700; a target that exists is refused. For an archive, write into a private temp directory through `tempfile`, so that a crash is swept like a reindex copy (§7.6).
3. For each collection: `SchemaDraft::from_collection`, `blank_secrets`, write `to_json`. Then `run_export` with `ExportScope::All`, JSON Lines, `FieldPick::All`, `expected: Some(num_documents)`.
4. An export that stopped or failed ends the run. One that wrote a different count records a warning: a backup of a live collection may differ.
5. Write the four JSON files. Attached sets only: the names from the selected collections, a missing one recorded as a warning. Aliases only where the target is selected. All stopword sets, because nothing ties one to a collection.
6. Hash the files, write the manifest, then rename the folder or `pack`.

Warnings: unstored fields (lost, as in D64), blanked secrets, dictionaries used and not included, a reference to a collection not selected, a count that moved, a missing set.

Stopping removes the partial folder or archive. Nothing on the server changes, so a backup needs no unlock.

### 5.5 Restore (`backup/restore.rs`, D91)

```rust
pub struct RestoreView { /* the current server */ collections, aliases, synonym_sets, curation_sets, stopwords, dictionaries }
pub enum RowState { New, Same, Differs, Blocked(RestoreBlock) }
pub enum RestoreBlock { CollectionExists, BlankSecrets(usize), RequiredUnstored(Vec<String>), Mask }
pub struct RestorePlan { pub rows: Vec<PlanRow>, pub warnings: Vec<RestoreWarning>, pub order: Vec<String> }
pub fn plan(manifest: &Manifest, loaded: &Loaded, server: &RestoreView) -> RestorePlan;
pub async fn run_restore(client: Client, job: RestoreJob, control: JobControl, progress: watch::Sender<RestoreProgress>) -> RestoreOutcome;
```

`plan` is pure. It compares sets with `relevance::changes` so that an order or a server-filled key is no difference.

Order of writes:

1. Stopword sets, then synonym sets, then curation sets. `New` is written. `Differs` is written only with Replace on; otherwise it stays and is named.
2. Collections by `order`: referenced ones first (Kahn over `reference` targets within the backup, by collection name and by aliases in the backup). A cycle keeps its members in manifest order and adds a warning, since an import with a missing target fails line by line and shows in the report.
3. A collection's documents with `run_import`: `ImportAction::Create`, `DirtyValues::CoerceOrReject`, `batch_size` from settings, `ReadOptions::new(Format::Jsonl)`.
4. Aliases, last, so a client moves only when the data is there. An alias whose target was not restored and does not exist is left out and named. A name that is a collection on the server is left out and named, since the collection would hide it.

After each import, `ImportOutcome.count` is compared with the manifest's `documents`, not with the file's line count alone. A difference or any failed line marks the row and ends the run after that collection, because later collections may reference it. What was done stays and is listed. The run never deletes anything. Before a retry, the list names what to delete.

Right before the first write, the server view is read again and the plan recomputed. A collection that now exists stops the run with no write at all: `POST /collections` would answer 409, but sets would already have changed.

### 5.6 Settings (`settings.rs`)

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: String,          // "system" (default), "light", "dark", or a theme name
    pub font_size: Option<u8>,  // None: the kit's default. 12..=20
    pub density: Density,       // comfortable (default) or compact
    pub language: Language,     // system (default), en, de
    pub per_page: u32,          // 250, 10..=250
    pub batch_size: u32,        // DEFAULT_BATCH, 1..=MAX_BATCH
    pub poll_seconds: u32,      // 5, one of 2, 5, 10, 30
    pub confirm_above: u64,     // 1000, 1..=100_000
    pub update_check: bool,     // false
}
pub fn settings_path() -> Option<PathBuf>;        // config_dir()/settings.toml
pub fn load(path: &Path) -> Result<Settings, SettingsError>;
pub fn save(path: &Path, settings: &Settings) -> Result<(), StoreError>;
pub fn clamped(self) -> Settings;
```

A file that does not parse changes nothing and is not overwritten, as with `keymap.toml`: defaults run, and the settings dialog says why and offers no save until the file is fixed. An out-of-range value is clamped when loaded. `danger_level(count, prod)` gains a third argument `threshold`; prod stays typed at any count.

### 5.7 Migrations (`store.rs`)

```rust
pub(crate) type Migration = fn(Value) -> Result<Value, String>;
pub(crate) fn load_migrated<T: Default + Serialize + DeserializeOwned>(
    path: &Path, version: u32, migrations: &[Migration],
) -> Result<T, StoreError>;
```

`migrations[n]` lifts level `n + 1` to `n + 2`. A file below `version` is first copied to `<name>.bak`, then each step runs, then the new version is written atomically. A failed step leaves the file and the backup alone and is a `StoreError`, so the caller does not write. A later migration replaces the `.bak`, so it always holds the file from before the last migration. All three lists start empty, and `connections.json`, `saved_queries.json` and `document_views.json` move to it, the last gaining the newer-version refusal. A test uses a fake list.

`write_atomic` names its temp file `path.with_extension("json.tmp")`, which suits TOML too, as only one writer runs per file.

### 5.8 Layout (`layout.rs`, D94)

```rust
pub struct Layout { pub schema_version: u32, pub left_width: f32, pub bottom_height: f32, pub bottom_open: bool, pub bottom_tab: BottomTab }
pub fn layout_path() -> Option<PathBuf>;   // data dir/layout/main.json
```

Numbers are clamped when loaded: left 160..=600, bottom 120..=800.

### 5.9 Log redaction (`redact.rs`)

```rust
/// One log line with secrets masked: URL query parameters, `"api_key":"…"`-style pairs, and `x-typesense-api-key` headers in any case.
pub fn line(text: &str) -> String;
```

Built from `text_query_params` and `mask_text`.

## 6. tessera-widgets

No new widget. `confirm_danger` and `confirm` already start focus on the field and on Cancel.

## 7. tessera-app

### 7.1 Navigation

`ResourceKind::Operations` and `ResourceKind::Backup` join `ALL` after AI: rows Ops (⌥⌘0 / Ctrl+Alt+0) and Backup (⌥⌘B / Ctrl+Alt+B), both shown on every server, as restore and operations do not depend on v30. Backup on a server below 30 leaves out sets it cannot read, and says so. Actions `OpenOperations`, `OpenBackup`.

`ResourceView` gains two variants. `admin.rs` gains the operations; the backup calls go through `jobs.rs`.

### 7.2 Operations tab (`views/resources/operations.rs`)

```
[Node: ts1:8108 · follower ▾]
Snapshot   explanation   [/var/backups/ts-2026-11-06] [Snapshot…]
Compact    explanation   [Compact…]
Clear cache explanation  [Clear cache…]
Leader     explanation   [Make this node leader…]
Runtime    [key ▾] [value] [Apply]   note: cannot be read back
Running schema changes   table: collection, validated, altered   (polling every 2 s)
```

The node picker defaults to the active node and shows role and host. Each result shows under its row, inline, with the time and node. `success:false` and errors show in full.

### 7.3 Confirmations and rules (D85)

| Operation | Confirmation | Consequences stated |
|---|---|---|
| Snapshot | Plain; typed on prod | The path is on the server, on this node; the disk needs room for the data; a used directory keeps its files and collects snapshots; may take long |
| Vote | Plain; typed on prod | The node named becomes leader within seconds; writes pause during the election; clients of the old leader retry |
| Compact | Plain | Disk and CPU load on this node; off-peak advised |
| Clear cache | Plain | Search latency rises until the cache fills |
| Config | None; plain on prod | Key, value, node; cannot be read back |

Typed means the profile name, as for unlock. Everything goes through `ensure_writable` and `confirm_write`, and writes run detached.

Vote is disabled with the reason when the node is the leader, or when the profile has one node: both answer `success:false` (P4). Its result says the election started, because the role moves seconds after the answer. A snapshot with a bad path shows the problem inline and sends nothing. While a snapshot runs its button shows busy, and the server's refusal of a second one shows inline.

Schema change polling runs only while the tab is the visible one and the window is visible, through `client.polling()`. The handle lives on the view, so closing stops it.

### 7.4 Safety UX audit (§8.2)

Every button whose action deletes, truncates or removes server data is `.danger()`. A test walks them by id. Local removals, such as a row in a draft, keep their look. A saved query is deleted locally with a confirmation; it counts, as §8.2 says "destructive buttons". Default focus: `confirm` and `confirm_danger` are covered by tests, and a test asserts the app has no other confirmation dialog built by hand.

The three levels are `tessera_core::documents::export::Danger` plus D62, D81 and D85. Their table goes into `docs/security-checklist.md`.

### 7.5 Transport banner (`views/transport.rs`, D98)

`transport::banner(conn, cx)` renders above the tab content next to `unlock::banner`, at the seven call sites in `dock.rs`, when a node of the profile is `Http` and not `is_local()`, or when `tls.allow_insecure`. It names the case, always shows, and has no close button: the fix is in the profile. Edit connection… opens the connect dialog. Text only, no toast. A pure `core::profile::Transport` check is unit tested: `Profile::transport_risks() -> Vec<TransportRisk>`.

### 7.6 Backup tab (`views/resources/backup.rs`)

Two views, ⌘1 Backup and ⌘2 Restore, as the AI tab has.

**Backup:** a checkbox list of collections with counts and an All box, the format as a radio, folder or `.tar.zst`, a note of what is included and left out, the warnings computed from the list (unstored, blanked keys, dictionaries, references outside the selection), and Start (⌘↩) which opens the system save dialog. It is a read, so no unlock.

**Restore:** Choose… (⌘O) takes a `.tar.zst` or a folder holding `manifest.json`. An archive unpacks into a private temp directory, shown as a running step with Stop. Then `verify`. Then the plan: one row per collection and each set, stopword set and alias, with its state; blocked rows with their reason; warnings; a box Replace what differs, off. Start confirms through `ensure_writable` and `confirm_write`:

- plain with counts, on any non-prod server;
- typed on prod or when Replace is on and something would change, naming how many sets, stopword sets and aliases change.

The run shows in the jobs panel and goes on with the tab closed. Afterwards the tab shows each row's result and what to delete before a retry.

`relevance::sweep_copies` also covers the prefix `tessera-backup-`, beside `tessera-reindex-`, so a temp directory left by a crash goes after a day.

### 7.7 Jobs

`JobKind::Backup`, `JobKind::Restore`, `Live::Backup(BackupProgress)`, `Live::Restore(RestoreProgress)`, `JobOutcome` to match, `Jobs::start_backup`, `Jobs::start_restore` through the private `start`. One job per run (D90). `collection` holds the first name and the label says the count. A restore is a write job, so D48's stop on disconnect applies through the read-only flag, and `reload_after` also reloads the connection's collection list after a restore.

Jobs quit behaviour stays: quit stops all jobs. A folder backup leaves `<target>.part`, which a later backup to that name refuses and names.

### 7.8 Settings dialog (`views/settings/`, D93)

⌘, / Ctrl+, the ⚙ button in the title bar and the palette open a dialog, like the connect dialog, with sections: Appearance, Defaults, Privacy, Storage, About.

The kit's `setting::Settings` was read and is an option for the body; Task 11 uses it if its fields take focus by keyboard, and plain rows otherwise.

- **Appearance.** Theme: System, Light, Dark, plus `ThemeRegistry::sorted_themes()`. System uses `Theme::sync_system_appearance`, which `RootView::new` and its appearance observer call today; an explicit choice must stop that observer from changing it. Font size sets `theme.font_size` through `Theme::update`, and `Root` applies it as the rem size (`root.rs:714`). Density picks `Size` for data tables. Language calls `set_locale` and `cx.refresh_windows()`; text held in inputs keeps its language until reopened, and the row says so.
- **Defaults.** `per_page`, `batch_size`, `poll_seconds`, `confirm_above`. They apply to new tabs, dialogs and connections. A changed `confirm_above` says that prod is always typed.
- **Privacy.** Update check, off, with what it sends. On, it runs once at start and by Check now, and shows the result in the section; a newer version shows there and as one toast, as a background event. No telemetry line.
- **Storage.** Config, data and log paths, each with Copy. Open logs through `cx.reveal_path`. Clear cache removes `tessera-reindex-` and `tessera-backup-` temp directories that no running job holds, and calls `invalidate_collections` on open connections (D93). History and saved queries have their own controls and are not touched.
- **About.** Version, GPL-3.0-or-later, the notice of §2.3, links.

Saves go through a new `WriteChain` in `AppState`, flushed on quit.

### 7.9 Layout (D94)

`RootView::new` applies a loaded `Layout` where it sets `SIDEBAR_WIDTH` and `LOG_HEIGHT`, and opens the bottom dock on its tab when asked. A change saves after 500 ms through a `WriteChain`. `DockArea::dump` and `load` are not used.

### 7.10 Empty states

Jobs, API stats and saved queries get a docs link where a page exists, in the pattern of `resources.docs`. The request log gets a line if it has none. `welcome.rs` already has all three §8.3 entries.

### 7.11 Shortcuts

| Chord | Action | Context |
|---|---|---|
| ⌥⌘0 / Ctrl+Alt+0 | Ops tab | global |
| ⌥⌘B / Ctrl+Alt+B | Backup tab | global |
| ⌘1, ⌘2, ⌘3, ⌘4, ⌘S | Snapshot, Compact, Clear cache, Vote, Apply | Ops tab |
| ⌘, / Ctrl+, | Settings | global |
| ⌘1, ⌘2 | Backup, Restore | Backup tab |
| ⌘O | Choose a backup | Backup tab |
| ⌘↩ | Start | Backup tab |
| ⌥⌘L / Ctrl+Alt+L | Open logs | Settings dialog |

## 8. Logging and secrets

### 8.1 Logs (D96)

`run` builds a registry with a stderr layer and a file layer. The file layer writes to `<data dir>/logs/tessera.log.YYYY-MM-DD` through a daily `tracing-appender` roller with at most 7 files, and through `non_blocking`, since a log call may run on the GPUI thread. Both layers go through `Redacted<M>`, a `MakeWriter` whose writer passes each `write_all` (one formatted event, `fmt_layer.rs`) through `redact::line`. The default level stays `info`. If the directory cannot be made, the app runs on stderr alone and says so once. The `WorkerGuard` lives until `run` returns, so a normal quit flushes. With `panic = "abort"` the last lines can be lost, which is accepted.

`tracing-subscriber` needs no new feature: `registry()` and `fmt::layer()` are default features of the crate as pinned, to be checked in Task 9.

The start-up time (NF1) is one `info` line, from the first line of `run` to the first frame, through `window.on_next_frame`.

### 8.2 Zeroization (D97)

`secrecy::SecretString` zeroizes on drop. Audit result so far:

| Place | State | Action |
|---|---|---|
| `Client.key` | `SecretString` | none |
| `HeaderValue` per request | Copy, sensitive, not zeroized | Accepted: in `reqwest`, freed after the request |
| `session_keys`, `temporary`, keychain reads | `SecretString` | none |
| `connect_dialog/mod.rs:185`, `key_dialog.rs:459`, `:502` | `expose_secret().to_string()` into an input or the clipboard | Wrap the temporaries in `zeroize::Zeroizing` |
| `InputState` of masked inputs | Rope in the kit, not zeroized | Accepted, documented, as the pinned kit gives no hook |
| Clipboard | Outside the app, by the user's Copy | Accepted, documented |

So "zeroized on drop" holds for what Tessera owns, and the checklist names the two exceptions. `zeroize = "=1.9.1"`, already in `Cargo.lock`, becomes a direct dependency of `tessera-app`.

### 8.3 `cargo deny`

Already in CI with `deny.toml`. `multiple-versions = "warn"` stays, since GPUI pulls duplicates we cannot fix, and §9 says "duplicates" are checked, which a warning does. `tar` and `zstd` licenses are on the allow list; `tracing-appender` is MIT.

## 9. Testing

- **Unit:** `check_snapshot_path`, config parsing, `confirm_for`, `raw_danger`, `Manifest::check`, `allowed_path`, `unpack` against crafted archives (`../x`, an absolute path, a symbolic link, a hard link, a device, a duplicate, an unlisted file, more bytes than declared, manifest not first), `verify` with one flipped byte, `plan` for each `RowState`, reference order with a cycle, settings parsing and clamping, migrations with a fake list, `redact::line`, `transport_risks`.
- **Wiremock:** each operation's path, query and status handling, `success:false`, 409, 500; `latest_release` with 200, 404 and a 2 MiB body.
- **Live, both versions:** operations, a round trip of a backup into a second throwaway server, as a folder and as an archive, comparing sorted exports. **Cluster, nightly:** vote moves the leader.
- **UI with fakes:** confirmations per D85, read-only guard, cancelling sends nothing, a blocked row, Replace, the banner, settings persistence, layout restore.
- **Log test:** a subscriber built like `run`'s writes to a temp dir, an event holding a key in a URL and in JSON leaves no key in the file, and a filename matches `tessera.log.*`.

## 10. CI

`ci.yml` adds the new live tests to `integration`. `nightly.yml` adds the vote test to `cluster`. `integration` needs a second server for the round trip: a second service container on another host port, so the test reads `TESSERA_IT_URL2`.

## 11. New dependencies (D92)

| Crate | Version | Crate using it | Why |
|---|---|---|---|
| `tracing-appender` | `=0.2.5` | `tessera-app` | Named in spec §4. Adds `crossbeam-channel` |
| `tar` | `=0.4.46` | `tessera-core` | `.tar.zst` |
| `zstd` | `=0.14.3` | `tessera-core`, `default-features = false` | Older than two weeks, while `0.13.4` is from 2026-11-01. Adds C code through `zstd-sys`, built by `cc`. NF5 must be measured |
| `zeroize` | `=1.9.1` | `tessera-app` | Already in the tree |

Task 5 reads whether `zstd` without default features still bundles its C sources. `sha2` needs a look too: it is `default-features = false` at 0.11.1 and is used for one-shot digests, while `verify` uses `Digest::update`.

## 12. Changes to the product spec

§4 versions, §6 rows, §7.7, §7.18, §7.21, §8.1, §9, §11, §14 and §16 (D85 to D99) are updated together with this design.

## 13. Risks

1. **A server can change what was measured.** §4 holds for 30.0 and 30.2. `live_operations` and `cluster` pin it, and CI runs them.
2. **Restore into production.** A wrong plan writes to a live cluster. The plan is recomputed before the first write, a collection is never overwritten, and Replace is off and typed.
3. **A backup is not a consistent snapshot.** Collections export one after another while writes go on. The dialog says so and points to Snapshot for a consistent copy.
4. **Backups hold the documents in plain text,** and a reader of the file reads them all. Files are private to the user (0600, 0700), and the dialog says so. Provider keys are never in it: `blank_secrets` runs on every schema and a test proves it.
5. **`zstd-sys` adds C.** The Windows and Linux builds need `cc`, which they have. Size is measured in the PR.
6. **`non_blocking` drops lines under load** by default. That is acceptable for an `info` log.
7. **Font size scales everything by rem.** A size of 20 at the minimum window may clip. The range stops there, and Task 11 checks the window minimum.
8. **`tar` was read from no pinned source yet.** The rule of `unpack` depends on iterating entries and reading `Entry` as `Read` without `unpack`; Task 5 confirms that in the pinned version.
