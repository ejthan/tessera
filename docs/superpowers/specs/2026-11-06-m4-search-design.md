# M4 Search and raw console: design

- **Date:** 2026-11-06
- **Status:** written autonomously (maintainer takes the recommendations), pending final review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M4 row in §14; §7.5, §7.17, §7.19, §7.20, §5, §8, §10, §11, §12, §16)
- **Previous milestone:** [M3 document browser design](2026-11-05-m3-documents-design.md)
- **Base:** the M3 work on branch `feat/m3-bulk`, which is not merged yet

All behaviour marked "confirmed" was checked on 2026-11-06 with curl against Typesense 30.2 on `localhost:18108` and against a throwaway 30.0 container (`typesense/typesense:30.0`, port 18208, with search analytics switched on), on resources that are gone again. The two versions gave the same answers everywhere both were probed. Analytics was probed on 30.0 only, because the dev server runs without it. Every GPUI Kit API named here was read in the pinned source (`gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-pre-0.3.11`).

## 1. Goal

M4 lets a user ask a Typesense server anything: a search workbench with a form and a JSON mode, results with highlights and facets, multi and union search, history, saved queries, presets and code export. Beside it a raw console reaches every endpoint, a command palette reaches every action, and the request log shows truncated bodies in full.

**Acceptance (product spec §14, §7.5, §7.17, §7.19, §7.20):**

- An exported `curl` command returns the same response when run, `search_time_ms` aside (live test, 30.0 and 30.2).
- Every request run from the workbench appears 1:1 in the request log.
- In a read-only profile the workbench searches, and the console sends `GET` and `HEAD` only until the user unlocks.
- A raw console path cannot leave the configured node, whatever is typed.
- ⌘K / Ctrl+K reaches every action available at the current focus; ⌘P / Ctrl+P switches collections.
- Integration tests for every new endpoint are green against 30.0 and 30.2.

## 2. Delivery

One spec, one plan, four pull requests in this order, each with green CI and its own CHANGELOG entries.

| PR | Branch | Content | Why this order |
|---|---|---|---|
| **A · Search** | `feat/m4-search` | Search and multi-search endpoints, the read-only exception for `POST /multi_search`, the parameter model, results, the workbench tab with form, JSON mode, instant search, hits, facets and groups | The core of the milestone; nothing here writes |
| **B · Queries** | `feat/m4-queries` | Multi and union search in the UI, history, saved queries, presets, code export with the curl acceptance test | Builds on A's draft model |
| **C · Console** | `feat/m4-console` | Raw requests, path validation, console tab, full bodies in the request log | The first writes of M4, and it needs B's history file |
| **D · Palette** | `feat/m4-palette` | Fuzzy matching, command palette, quick switch, `keymap.toml` | Lists what A to C added, so it comes last |

`feat/m4-search` starts from `main` once M3 is merged. If M3 is still open, it starts from `feat/m3-bulk`.

## 3. Scope

### In M4

Product spec §7.5 without its P2 parts, §7.17 and §7.20, the §7.19 full view, and D30's long filters in the document browser, because the same client path solves them.

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Compare mode, Elasticsearch helper | v1.x | §7.5 marks them P2, §14 lists them there |
| Preset list, delete and "preset mode" | M6 | §14 gives §7.12 to M6. M4 has what §7.5 itself names: load and save from the workbench (D37) |
| Settings entries in the palette | with §7.21 | No settings view exists, and an entry without one would be a dead control (D44). §14 names no milestone for §7.21; D44 asks |
| Saving a response to a file | M5 | Copy covers it; the jobs panel owns file output |
| Reverse sync from the filter text to builder rows | not planned | The builder appends to the text and holds no state (D38) |
| Reordering search tabs | v1.x | Nobody asked |
| Rebuilding the keymap while running | v1.x | `keymap.toml` loads once at start, like `connections.json` |

## 4. tessera-client

### 4.1 New endpoints

New module `crates/tessera-client/src/search.rs`. Parameters arrive as validated pairs from core (§5.1); the client does not interpret them. Responses stay text, because the Raw tab shows what the server said and core parses it on the tokio worker.

```rust
pub const GET_QUERY_LIMIT: usize = 4000;   // an encoded query of this length or more goes by POST (GET at 4000 is a 400)
pub const MULTI_SEARCH_LIMIT: usize = 50;  // the server's default limit_multi_searches

pub enum SearchRoute { Get { path: String, query: String }, Post { query: Option<String>, body: Value } }
pub fn search_route(collection: &str, params: &[(String, String)]) -> Result<SearchRoute, Error>;

pub struct MultiSearch {
    pub union: bool,
    pub searches: Vec<Map<String, Value>>,  // each with its "collection"
    pub page: Option<u32>,                  // union only, sent in the URL
    pub per_page: Option<u32>,              // union only, sent in the URL
}
pub fn multi_route(request: &MultiSearch) -> SearchRoute;

pub struct Preset { pub name: String, pub value: Value }
```

| Method | Endpoint | Confirmed behaviour |
|---|---|---|
| `search(c, params) -> String` | `GET …/documents/search`, or `POST /multi_search` with one search when the encoded query is `GET_QUERY_LIMIT` bytes or more (3,999 passes by GET, 4,000 is a 400) | The answer of a one-search `multi_search` equals the `GET` answer, `search_time_ms` aside. An 11 KB filter passes. String values such as `"per_page":"1"` and `"enable_analytics":"false"` are honoured, so pairs convert without typing |
| `multi_search(&MultiSearch) -> String` | `POST /multi_search[?page=&per_page=]` | See §4.3. A form-encoded content type is accepted too, so a curl heredoc without `-H content-type` works. Body stays `application/json` here |
| `preset(name) -> Value` | `GET /presets/{name}` | 404 `Not found.` |
| `presets() -> Vec<Preset>` | `GET /presets` | `{"presets":[{"name","value"}]}` |
| `upsert_preset(name, &Value)` | `PUT /presets/{name}` with `{"value": …}` | Overwrites silently; a non-object `value` is accepted (the UI never sends one) |
| `raw(RawRequest) -> RawResponse` | anything | §4.5 |

`search_documents`, `count_documents` and `facet_values` go through `search_raw`, which now uses the same route, so a browser filter over 4,000 bytes works and D30 is closed. Pages keep `without_response_bodies`.

`preset_path(name)` calls `addressable` and encodes one segment, like `document_path`.

### 4.2 Read-only and failover for a read sent as POST

D34. `Client` gains a private `read_post: bool`, false everywhere except a clone made inside `search`, `search_raw` and `multi_search`. In `send`, the guard also passes `POST` when `read_post` is set. In `call`, such a request fails over like a `GET`. The flag has no public setter, so `raw()` cannot reach it and a console `POST /multi_search` on a read-only profile is refused, which keeps §7.17's "GET only" literal.

`Filter::writes_only` in `tessera-core/src/log.rs` stops counting a `POST` to a path ending in `/multi_search` as a write. Other methods stay writes, so `DELETE /collections/multi_search` is not hidden.

### 4.3 Traps found while probing

- **`/multi_search` answers 200 for failures.** Federated: `{"results":[{"code":404,"error":"…"},…]}`, per search. Union: a top-level `{"code":400,"error":"…"}`. `multi_search` turns the top-level form into `Error::Http` through a new `Error::embedded(code, error, body)`. A one-search fallback does the same for `results[0]`. Federated items stay in the text, and core reports each on its own tab (§5.2).
- **An empty `searches` is a 500** with `Embedded params parsing error.`; more than 50 is a 400. The UI keeps the count in `1..=MULTI_SEARCH_LIMIT`. `limit_multi_searches` is not raised: a scoped key may embed it, and 50 tabs are plenty.
- **Union pagination lives in the URL.** `page` and `per_page` inside a search or at the top of the body are ignored; `?per_page=2&page=3` works. Without `page` the answer says `"page":0`, so the client always sends it.
- **Union `found` counts duplicates.** Two overlapping searches over 10,000 documents gave `found` 19,750 while the hits are unique (`remove_duplicates` defaults to true). The status line says "19,750 before duplicates" for union.
- **Union demands uniform searches.** `sort_by` must match in count and type, and that includes the implicit sort: `q=*` next to `q=dune` is a 400. All searches group or none does. A search of `books` with one of `products` fails on their default sort types. The server's message shows inline.
- **Union adds `collection` and `search_index` to each hit** and `union_request_params` at the top.
- **A search can have side effects.** With `enable_search_analytics` on the server, a `multi_search` was recorded in a `popular_queries` destination collection, and with `enable_analytics=false` (bool or string) it was not (30.0). So a workbench would pollute production analytics while someone tunes relevance. `conversation=true` with a model writes conversation history and calls an LLM, `nl_query=true` calls an LLM, and `query_by` on an `embed` field sends `q` to a remote embedding API. §5.1 handles all four.
- **A missing preset is ignored silently.** `preset=nope` answers 200 with the unfiltered search. A preset can also supply `q`, so `request_params.q` comes back empty. The Presets list therefore comes from the server (§7.6); a name typed into the form's `preset` field is sent as it is.
- **An empty `q` without `query_by` is a 400;** `q=*` needs none. An unknown parameter is ignored silently, so a typo in JSON mode changes nothing and says nothing. The JSON editor marks keys unknown to the vendored spec as `Info`.
- **Facet values can break a filter.** `` genre:=`*` `` and `genre:=*` are 400s. A value ending in a backslash works alone and silently breaks a list: `` genre:[`sci fi`,`back\`,`(p)`] `` counts 2 instead of 4. A numeric value in backticks is a 400. §5.3 uses an allowlist.
- **`group_by` on a value with a backtick is a 400** `Could not parse the filter query.`, on a search with no filter at all. Shown inline; nothing to work around.
- **Highlights have two shapes.** In `highlight`, a string field is `{"snippet","matched_tokens"}`, an array field is a list of those, one per element, and a nested object nests. `highlights` lists only some of it. Core walks `highlight` (§5.2).
- **The response has no useful headers:** `content-type`, `connection`, `transfer-encoding` only, on both versions. The console shows what arrives.

### 4.4 Numbers the app uses

| Constant | Value | Why |
|---|---|---|
| `INSTANT_DEBOUNCE` | 150 ms | §7.5 |
| `HISTORY_CAP` | 500 per kind | §7.5 |
| `HISTORY_SETTLE` | 2 s | An instant search enters history only when it stands that long (D39) |
| `RAW_BODY_KEEP` | 64 KiB | Larger console bodies are not kept in history (D39) |
| `RAW_FILE_MAX` | 64 MiB | A console body from a file is held in memory |
| `FULL_BODY_MAX` | 32 MiB | Largest body kept whole (D42) |
| `FULL_BODY_BUDGET` | 64 MiB | All full bodies together (D42) |
| `HIGHLIGHT_MAX` | 32 KiB | Above this `BodyView` shows plain text (§6.2). Measured in Task 12, below |
| `VIEW_MAX` | 512 KiB | Above this `BodyView` pages in chunks of at most this, with Copy all (§6.2). Measured in Task 12, below |
| `PALETTE_LIMIT` | 100 rows | One render of the list |

Task 12 lowered `HIGHLIGHT_MAX` from 1 MiB and `VIEW_MAX` from 8 MiB. The kit does its layout and highlighting on the UI thread before the first frame. Measured with a release build on an Apple M3 Max, pretty-printed JSON at about 18 bytes a line, calling `Window::draw` in a real window that was not on screen, so without GPU present: time to build the editor plus its first draw, warm.

| Bytes | Plain | Highlighted | Scroll p95, plain / highlighted |
|---|---|---|---|
| 32 KiB | 3.0 ms | 11.2 ms | 1.5 / 2.4 ms |
| 64 KiB | 3.7 ms | 20.5 ms | 1.5 / 2.4 ms |
| 128 KiB | 4.8 ms | 30.7 ms | 1.8 / 2.4 ms |
| 256 KiB | 10.1 ms | 32.2 ms | 1.8 / 2.2 ms |
| 512 KiB | 11.6 ms | 72.6 ms | 1.9 / 1.9 ms |
| 1 MiB | 21.1 ms | 109.4 ms | 1.6 / 1.1 ms |
| 2 MiB | 28.4 ms | 210.6 ms | 2.9 / 1.8 ms |
| 4 MiB | 52.1 ms | not run | 4.3 ms |
| 8 MiB | 91.3 ms | not run | 3.9 ms |

With the new limits, an 8 MiB body through `BodyView` has a first draw of 6.6 ms, and draws stay at 1.7 ms p95 while scrolling. The first editor of a process costs about 10 ms more, cold. A single line of 512 KiB that is not JSON takes 21 ms once. These runs are not the reference machine; NF4's manual check with the overlay still stands.

### 4.5 Raw requests

```rust
pub enum RawBody { Json(String), Text(String) }     // application/json, text/plain
pub struct RawRequest { pub method: Method, pub path: String, pub query: Option<String>, pub body: Option<RawBody> }
pub struct RawResponse { pub status: u16, pub headers: Vec<(String, String)>, pub body: String, pub duration: Duration }
impl Client { pub async fn raw(&self, request: RawRequest) -> Result<RawResponse, Error>; }
```

- `send` splits: the part up to the response moves into `send_with(node, method, path, query, body: Option<(String, &'static str)>, timeout) -> Result<(u16, Vec<(String, String)>, String), Error>`, and `send` serializes its `Value`, calls it and drops the headers. So the guard, the event and the redaction stay on the one path.
- `raw` sends once to the active node with `LONG_TIMEOUT`. No failover: a console request goes where the title bar says. Any status is `Ok`, because the status is what the console shows.
- After `base.join(path)`, `raw` compares origin and path prefix with the node's and answers `Error::InvalidUrl` when they differ. Core validates first (§5.7); this check keeps the promise if core misses a spelling. `Url::join` resolves `//host/x`, `http://host/` and `http:foo` against nothing of ours, and each of those would carry the API key to another host.

## 5. tessera-core

No GPUI dependency. New module tree `crates/tessera-core/src/search/`, plus `raw.rs`, `openapi.rs` and `keymap.rs`. Everything here is unit tested.

### 5.1 The draft (`search/params.rs`)

A search is a JSON object, because JSON mode can set anything and the form is a view onto it.

```rust
pub struct SearchDraft { pub collection: String, pub params: Map<String, Value> }
impl SearchDraft { pub fn new(collection: &str) -> Self; }  // q="*", enable_analytics=false

pub enum ParamError { BadKey(String), Reserved(String), BadValue(String), NotAnObject }
pub fn parse_params(text: &str) -> Result<Map<String, Value>, ParamError>;
pub fn params_text(params: &Map<String, Value>) -> String;                  // pretty
pub fn to_pairs(params: &Map<String, Value>) -> Result<Vec<(String, String)>, ParamError>;
pub fn to_search(draft: &SearchDraft) -> Result<Map<String, Value>, ParamError>;  // for multi_search

pub enum ParamKind { Text, Filter, Sort, Fields, Int, Bool, Choice(&'static [&'static str]) }
pub struct ParamDef { pub key: &'static str, pub kind: ParamKind, pub common: bool }
pub const FORM_PARAMS: &[ParamDef];

pub enum SideEffect { Analytics, Conversation, NaturalLanguage, RemoteEmbedding }
pub fn side_effects(params: &Map<String, Value>, schema: Option<&Collection>) -> Vec<SideEffect>;
```

- **Allowlist.** A key must match `[a-z][a-z0-9_]*`, which also keeps `encode_query`'s unencoded keys safe. `collection`, `searches` and `union` are `Reserved`: the picker and the mode own them. `x-typesense-api-key` fails the pattern, so a key never lands in a draft or in history. A value must be a string, number or bool; `null`, arrays and objects are `BadValue`.
- **Defaults.** `SearchDraft::new` writes `enable_analytics: false` into the map, where JSON mode shows it and the form has a checkbox for it (D35). Curations stay at the server's default here: the workbench shows what clients get, unlike the browser, which shows what is stored.
- **`FORM_PARAMS`** lists §7.5's parameters in §7.5's order plus `enable_analytics`. `common` marks `q`, `query_by`, `query_by_weights`, `filter_by`, `sort_by`, `facet_by`, `page`, `per_page`; the rest fold under "More". `text_match_type` is `Choice(["max_score", "max_weight"])`, the vendored spec's values, and `infix` is `Choice(["off", "always", "fallback"])`.
- **`side_effects`:** `enable_analytics` true or absent gives `Analytics`; `conversation` true gives `Conversation`; `nl_query` true gives `NaturalLanguage`; a `query_by` entry whose schema field has `embed` gives `RemoteEmbedding`. A truthy value is `true` or the string `"true"`. §7.3 turns these into UI.

`to_pairs` stringifies numbers and bools as JSON does. `q` is always sent, as `*` when blank, so a blank draft is a browse and not a 400.

### 5.2 Results (`search/results.rs`)

```rust
pub struct SearchResult {
    pub found: u64, pub out_of: u64, pub page: u32, pub search_time_ms: u64,
    pub found_docs: Option<u64>,         // grouped answers
    pub hits: Vec<Hit>,
    pub groups: Vec<HitGroup>,           // group_key, found, hits
    pub facets: Vec<Facet>,              // field, counts: (value, count), sampled
}
pub struct Hit {
    pub id: Arc<str>, pub doc: Arc<Map<String, Value>>,
    pub lines: Vec<HitLine>,             // one per highlighted field, schema order not known here: as sent
    pub text_match: Option<u64>, pub text_match_info: Option<Value>,
    pub curated: bool, pub collection: Option<String>, pub search_index: Option<usize>,
}
pub struct HitLine { pub field: String, pub text: String, pub marks: Vec<Range<usize>> }
pub enum MultiResult { Federated(Vec<Result<SearchResult, ItemError>>), Union(SearchResult) }
pub struct ItemError { pub code: u16, pub message: String }

pub fn parse_result(text: &str, tags: (&str, &str)) -> Result<SearchResult, serde_json::Error>;
pub fn parse_multi(text: &str, union: bool, tags: (&str, &str)) -> Result<MultiResult, serde_json::Error>;
pub fn marks(snippet: &str, tags: (&str, &str)) -> (String, Vec<Range<usize>>);
```

- `tags` comes from the draft's `highlight_start_tag` and `highlight_end_tag`, default `<mark>`, `</mark>`. A literal `<mark>` in a document is drawn as a mark and nothing worse, since no markup is interpreted.
- `lines` walks `highlight` recursively with dotted paths. An array field joins its elements that have matched tokens with ` · `. A hit with no highlight (`q=*`) falls back to the first three string values of the document.
- Parsing runs in the backend future, like `DocumentPage::from_documents`, so 250 hits cost the UI thread nothing.

### 5.3 Facets into filters (`search/facet.rs`)

The filter text is the only filter state (D38). A click rewrites it.

```rust
pub fn facet_clause(field: &str, field_type: &str, values: &[&str]) -> Option<String>;
pub fn toggle_facet(filter: &str, field: &str, field_type: &str, value: &str) -> Option<String>;
pub fn facet_active(filter: &str, field: &str, value: &str) -> bool;
fn split_and(filter: &str) -> Option<Vec<&str>>;
```

- **Allowlist on the value** (`facet_value_ok`): numeric fields take what parses as `i64` or `f64`, and go unquoted. `bool` takes `true` and `false`. Strings refuse a backtick, a backslash, `*` alone, a blank, and leading or trailing spaces; they go in backticks. A refused value has its row disabled with the note "This value cannot be written in filter_by". `quote_value` from M3 is not enough here, since it lets a backslash through.
- **`toggle_facet`** splits at top-level `&&` with the scanner rules of `context_at`. It finds this field's clause in the forms it writes itself, `` field:=`a` `` and `` field:=[`a`,`b`] ``, then adds or removes the value. A clause left empty goes away. With a top-level `||` it writes `(old) && clause`. Anything it cannot read stays untouched and the new clause is appended.

### 5.4 Sort rows (`search/sort.rs`)

```rust
pub struct SortRow { pub expr: String, pub desc: bool }
pub fn parse_sort(text: &str) -> Option<Vec<SortRow>>;   // None: more than 3, or not rows
pub fn sort_text(rows: &[SortRow]) -> String;
```

Commas inside `(…)` and `[…]` do not split, so `_eval([(a:1):2]):desc` is one row. When `parse_sort` answers `None` the form shows the text read-only with "Edit in JSON".

### 5.5 Code export (`search/export.rs`)

```rust
pub enum ExportLang { Curl, JavaScript, Python, Rust, RawUrl }
pub fn export(lang: ExportLang, node: &Url, route: &SearchRoute, target: &ExportTarget) -> Option<String>;
```

`ExportTarget` carries the collection and the params, or the `MultiSearch`, for the client libraries. `None` means `RawUrl` of a `POST` route, and the menu entry is disabled with "This search is sent as POST". The route is the one the client uses, built by `search_route` and `multi_route`, so a curl command is the request and not a re-creation of it.

| Language | Shape | Key | Escaping |
|---|---|---|---|
| curl | `curl::request` | `$TYPESENSE_API_KEY` | existing shell quoting, the encoded query keeps `[` and `]` away from curl's URL globbing |
| JavaScript | `new Typesense.Client({…})`, `client.collections('c').documents().search(p)` or `client.multiSearch.perform(req, common)` | `process.env.TYPESENSE_API_KEY` | `serde_json` string |
| Python | `typesense.Client({…})`, `client.collections['c'].documents.search(p)` or `client.multi_search.perform(req, common)` | `os.environ["TYPESENSE_API_KEY"]` | own printer: `True`, `False`, `None`, `serde_json` strings |
| Rust (reqwest) | `reqwest::Client::new().get(url)` with the encoded URL, or `.post(url).body(r#"…"#)` | `std::env::var("TYPESENSE_API_KEY")?` | raw string with enough `#`; `{:?}` for short strings. No `.query()`, which is an optional feature in reqwest 0.13 |
| Raw URL | the URL, no key | none | encoded |

A node with a path prefix passes `path` in the client libraries' node config. Generated code gets `insta` snapshots, including a `q` with a quote, a backslash and a newline.

### 5.6 History and saved queries (`search/history.rs`, `search/saved.rs`)

D39. `data/history.jsonl` and `data/saved_queries.json`.

```rust
pub struct HistoryItem { pub at: i64, pub profile: Uuid, pub entry: HistoryEntry }
pub enum HistoryEntry {
    Search { union: bool, searches: Vec<SearchDraft> },
    Raw { method: String, path: String, query: Option<String>, body: Option<String>, body_dropped: bool },
}
pub struct History { /* two VecDeques, newest last */ }
impl History {
    pub fn push(&mut self, item: HistoryItem) -> bool;  // false: equal to the newest of its kind
    pub fn searches(&self, profile: Uuid, needle: &str) -> Vec<&HistoryItem>;
    pub fn raw(&self, profile: Uuid, needle: &str) -> Vec<&HistoryItem>;
    pub fn clear(&mut self, profile: Uuid);
}
pub fn load(path: &Path) -> (History, usize);          // the line count; bad lines are skipped with a warning
pub fn append(path: &Path, item: &HistoryItem) -> Result<(), StoreError>;
pub fn rewrite(path: &Path, history: &History) -> Result<(), StoreError>;  // atomic
pub fn raw_entry(method: &str, path: &str, query: Option<&str>, body: Option<&str>) -> HistoryEntry;
```

- `raw_entry` passes the query through `redact::query` and the body through `redact::json`, and drops a body over `RAW_BODY_KEEP`. Loading such an entry refuses to send a masked value through `redact::contains_mask`, as M2's schema dialog does.
- `append` opens with `OpenOptions::append`, writes one line and syncs. `AppState` calls `rewrite` when `load` counted more than 2 × `HISTORY_CAP` lines, and for `clear`.
- A line with an unknown shape is skipped, so a future format does not lose the file.
- A temporary connection (D13) records nothing to disk.

```rust
pub struct SavedFile { pub schema_version: u32, pub queries: Vec<SavedQuery>, pub favorites: Vec<HistoryItem> }
pub struct SavedQuery {
    pub id: Uuid, pub profile: Uuid, pub name: String, pub tags: Vec<String>,
    pub union: bool, pub searches: Vec<SearchDraft>,
}
pub fn load(path: &Path) -> Result<SavedFile, StoreError>;   // missing: empty; bad or newer: Err
pub fn save(path: &Path, file: &SavedFile) -> Result<(), StoreError>;
pub fn saved_path() -> Option<PathBuf>;
```

Saved queries are the user's work, so a corrupt or newer file behaves like `connections.json`: it is left alone, nothing is written, and the Saved panel says why. "Per connection and collection" filters on `profile` and on the drafts' `collection`.

### 5.7 Raw console logic (`raw.rs`)

```rust
pub enum PathError { Empty, Character(char), Segment(String), BadEscape, Scheme }
pub fn parse_path(input: &str) -> Result<String, PathError>;
pub fn param_key_ok(key: &str) -> bool;                      // [A-Za-z0-9_.-]+, not the key parameter
pub fn raw_query(pairs: &[(String, String)]) -> Result<Option<String>, String>;

pub enum RawDanger { None, Medium, High { expected: String } }
pub fn raw_danger(method: &Method, path: &str, prod: bool) -> RawDanger;
pub fn outcome_note(path: &str, status: u16, body: &str) -> Option<OutcomeNote>;
```

**`parse_path`** is an allowlist.

- One leading `/` is dropped.
- Every character must be ASCII alphanumeric or one of `-._~%/:@+,=`. A `?`, `#`, `\`, space, control or non-ASCII character is `Character`: the query has its own table, and a segment with a space is typed `%20`.
- `%` must be followed by two hex digits.
- No segment is empty, so `//host` fails.
- No segment decodes to `.` or `..`, which covers `%2e%2E`. Decoding runs twice over.
- The first segment holds only alphanumerics, `-`, `_` and `.`, so `http:x` cannot read as a scheme.

Everything else stays reachable: §6's "Raw: anything".

**`raw_danger`** (D40). Reads answer `None`, and `POST multi_search` is a read.

| Request | Not prod | Prod |
|---|---|---|
| `DELETE collections/{c}`, `DELETE collections/{c}/documents` with or without a query | High, type `c` | High, type `c` |
| Any other `DELETE` | Medium | High, type the last segment |
| `POST operations/*` | Medium | High, type the last segment |
| Other `POST`, `PUT`, `PATCH` | None | Medium |

The expected text is percent-decoded. The dialog states method, full path and connection.

**`outcome_note`** covers the 200s that hide a failure: `…/documents/import` counts lines with `"success":false` and gives "2 of 40 lines failed"; `multi_search` counts `results` carrying `error`, or reads the union form. The console shows it above the body, as a warning.

### 5.8 OpenAPI paths (`openapi.rs`)

```rust
pub struct Route { pub method: &'static str, pub template: &'static str }
pub fn routes() -> &'static [Route];     // scanned once from include_str!("../../../spec/typesense-openapi.yml")
pub fn path_candidates(typed: &str, collections: &[String]) -> Vec<String>;
```

A line scanner, no YAML dependency: two-space `/…:` lines open a path, four-space `get:`, `post:`, `put:`, `patch:`, `delete:` lines add a method. The Ruby YAML parser rejects this file at line 4304 anyway, so no strict parser is assumed. `path_candidates` fills `{collectionName}` with the connection's collections and keeps other placeholders. A test pins the count at 46 paths and fails on a re-vendored file, so the change gets looked at.

### 5.9 Fuzzy and keymap (`search/fuzzy.rs`, `keymap.rs`)

```rust
pub fn score(query: &str, text: &str) -> Option<u32>;
pub fn rank<T>(query: &str, items: Vec<T>, text: impl Fn(&T) -> &str, limit: usize) -> Vec<T>;

pub struct UserBinding { pub key: String, pub action: Option<String>, pub context: Option<String> }
pub fn parse_keymap(text: &str) -> Result<Vec<UserBinding>, String>;
pub fn keymap_path() -> Option<PathBuf>;
```

`score` is a case-insensitive subsequence match with a bonus for word starts and consecutive characters, and an empty query matches all. New dependency for TOML: `toml = "=1.2.0"`, already in `Cargo.lock` through the kit, in `tessera-core` only.

`config/keymap.toml`:

```toml
[[binding]]
key = "cmd-shift-p"
action = "tessera::OpenPalette"

[[binding]]
key = "cmd-k"        # no action: the chord does nothing
context = "Workbench"
```

### 5.10 The request log keeps full bodies

D42. The console holds at most `FULL_BODY_MAX` of an answer as well, and says inline when it cut one. `Body` gains `pub full: Option<Arc<str>>`, set by `Body::new` when the text is over `BODY_LIMIT` and at most `FULL_BODY_MAX`. Clones stay cheap, `Debug` keeps printing lengths. A client built with `without_response_bodies` builds no `Body`, so document pages keep nothing (D24).

`RequestLog::push` takes `full` out of the entry's bodies into its own `VecDeque<(u64, BodyPart, Arc<str>)>`, evicting the oldest past `FULL_BODY_BUDGET`, and releases an entry's full bodies when the ring drops it. `pub fn full_body(&self, id: u64, part: BodyPart) -> Option<Arc<str>>` reads it. So the budget bounds this memory, and an evicted entry says so.

Refetching on demand was rejected: a second `GET` is a different answer and a write cannot be repeated. A file on disk was rejected too: §9 keeps bodies in memory only.

## 6. tessera-widgets

### 6.1 One-line inputs

`FilterInput` is reused in the workbench as it is: fields from the schema, facet values through `FacetFetch`. A new `CompletionInput` (`crates/tessera-widgets/src/completion_input.rs`) copies its `EditorState` setup and `Enter` rule with a plain candidate list, `set_candidates(Vec<String>)`, for the console's path. Both keep `FilterInput`'s rule that a menu never opens where Enter should submit.

### 6.2 BodyView

`crates/tessera-widgets/src/body_view.rs`, used by the workbench's Raw tab, the console's response and the log's full view.

```rust
pub struct BodyView { /* JsonEditor, full text, state */ }
BodyView::new(window, cx);
fn set_body(&mut self, text: Arc<str>, window, cx);
fn clear(&mut self, window, cx);
fn shown(&self) -> Shown;   // Highlighted, Plain, Cut { shown, total }
```

- Formatting runs once through `cx.background_spawn`: `serde_json` pretty print up to 32 MiB, so a compact body pages by lines, then the chunks. Turning a page rebuilds only the editor.
- Up to `HIGHLIGHT_MAX` it shows highlighted. Up to `VIEW_MAX` it shows plain, without a language, which the kit draws unhighlighted. Above that it pages: chunks of at most `VIEW_MAX` bytes that end on a line end where the chunk holds one, else on a char boundary, plain, with "Chunk 3 of 14, 6.8 MB in all", Previous and Next (`alt-cmd-[`, `alt-cmd-]`, Ctrl+Alt+[ and ]) and Copy all. The whole body is viewable.
- Pinned facts: `gpui-base-0.9.0/src/input/base/state.rs` says the editor handles "large text up to about 50K lines", and `default_value` and `set_value` build the rope on the UI thread. Hence the limits, and Task 12 measures them (§12).
- `JsonEditor` gains `JsonEditor::plain(text, window, cx)`, without `.language("json")`, read-only.

## 7. tessera-app

### 7.1 Tabs

D33. Two new center tabs per connection, **Search** and **Console**, opened from the sidebar, the palette and the shortcuts below. `Kind` and `ConnectionTab` in `views/root.rs` gain `Search` and `Console`, with `SearchPanel` and `ConsolePanel` in `views/dock.rs` built like `CollectionPanel`: colored edge, unlock banner, title with environment badge, lock. `AppState` gains `open_search(id, draft: Option<SearchDraft>, cx)` and `open_console(id, cx)`, emitting `OpenSearch` and `OpenConsole`. The sidebar adds two `link_row`s under Collections.

§8.1 draws the console in the bottom dock. It moves to a tab because a bottom panel would follow the active tab, and a `DELETE` typed against staging would go to prod after a tab switch. A tab belongs to one connection and carries its color. §8.1 is updated.

### 7.2 Backend seam

`crates/tessera-app/src/search.rs` holds `SearchBackend` with `SearchBackendGlobal` and `tokio_backend`, built like `DocumentsBackend`.

```rust
pub struct SearchAnswer { pub text: Arc<str>, pub result: SearchResult }
pub struct MultiAnswer { pub text: Arc<str>, pub result: MultiResult }
trait SearchBackend {
    fn search(&self, client: Client, collection: String, params: Vec<(String, String)>, tags: (String, String)) -> BoxFuture<Result<SearchAnswer, Error>>;
    fn multi(&self, client: Client, request: MultiSearch, tags: Vec<(String, String)>) -> BoxFuture<Result<MultiAnswer, Error>>;
    fn presets(&self, client: Client) -> BoxFuture<Result<Vec<Preset>, Error>>;
    fn save_preset(&self, client: Client, name: String, value: Value) -> BoxFuture<Result<(), Error>>;
    fn raw(&self, client: Client, request: RawRequest) -> BoxFuture<Result<RawResponse, Error>>;
}
```

The real backend does not use `without_response_bodies`: the log entry must be the request (§7.5 AC). `fn raw` runs through `TokioBackend::run` for a read. For a write the view spawns the future detached, as `bulk.rs` does at line 417, so closing the tab cannot abort it, and the hook logs the result either way.

### 7.3 Workbench

`views/search/mod.rs` (`SearchWorkbench`), `form.rs`, `results.rs`, `hits.rs`, `multi.rs`, `queries.rs`, `export.rs`.

```
┌ tabs ───────────────────────────────────────────────────────────────────┐
│ [Search 1 ✕] [Search 2 ✕] [+ ⌘T]   Federated|Union ⇧⌘M                  │
├ toolbar ────────────────────────────────────────────────────────────────┤
│ Collection ▾  [Run ⌘↩] ☑ Instant ⌥⌘L  History ⇧⌘H  Saved ⌘O  Save ⌘S     │
│ Presets ⌥⌘P  Export ⌥⌘E                                                 │
├ Form ⌘1 | JSON ⌘2 ───┬ status: 12 found of 200,000 · 3 ms ───────────────┤
│ q                    │ Hits ⌘3 | Raw ⌘4                     │ Facets     │
│ query_by  [a][b] +   │ ┌ hits ─────────────────┬ detail ───┐│ genre      │
│ filter_by            │ │ …                     │ JSON      ││ ☐ crime 12 │
│ sort_by  rows        │ └───────────────────────┴───────────┘│            │
│ ▸ More               │                                      │            │
└──────────────────────┴──────────────────────────────────────┴────────────┘
```

**State.** `drafts: Vec<SearchDraft>`, `active: usize`, `union: bool`, `mode: Form | Json`. The draft map is the truth. A form control writes its key, and an empty control removes it. The JSON editor parses on change: a good parse replaces the map and refreshes the form, a bad one keeps the map and disables Run with the parser's position. Unknown keys stay, and the form's footer says "2 parameters only in JSON".

**Form.** A generic renderer over `FORM_PARAMS`:

| Kind | Control |
|---|---|
| `Text`, `Int` | `Input`. `Int` stores a number when the text parses and shows an inline error when it does not |
| `Bool` | three states as a `Select` of default, true, false, because absent differs from false on the server |
| `Choice` | `Select` |
| `Filter` | `FilterInput`, plus "Add condition" |
| `Fields` | chips for `query_by`, text otherwise |
| `Sort` | up to 3 rows |

- **`query_by`.** A `ComboboxState` with `.multiple(true)` adds fields, and chips keep the order. A chip drags with `.on_drag` and `.on_drop` (`gpui-pre-0.3.11/src/elements/div.rs`, used in `gpui-component-0.9.0/src/table/state.rs:1913`). On keyboard, ⌥← / ⌥→ on a focused chip move it and Delete removes it. Candidates are indexed `string` and `string[]` fields, dotted nested names, and vector fields.
- **Add condition.** A row with a field `Select`, an operator `Select` from `operators_for`, a value input and Add (⌘↩ in the row, in its own `CONDITION_ROW` context, deeper than Run). It appends ` && clause` to the filter text through `facet_clause`; a refused value shows inline (D38).
- **Sort rows.** A field `Select` of sortable fields, `_text_match`, `_vector_distance` and `_eval`, a text input for an `_eval` expression, and a direction toggle.

**Running.** Run (⌘↩) validates with `to_pairs` and shows `ParamError` inline. A new run bumps a generation and drops the running read, whose `AbortOnDrop` cancels it. With one draft it calls `search`; with several, `multi`. Errors show in full over the results, which stay dimmed, as M3's filter bar does.

**Instant search.** On by default, off by default on a prod profile (D36). 150 ms after a change, one run. `side_effects` other than `Analytics` pause it and show "Instant search paused: this query calls a model. Run with ⌘↩." `Analytics` shows a notice on the checkbox and does not pause. JSON mode with a parse error waits.

**Side effects and read-only.** A locked profile may still run `Analytics`, `NaturalLanguage` and `RemoteEmbedding`, which store nothing in a collection. `Conversation` writes, so Run goes through `ConnectionState::ensure_writable` first.

### 7.4 Results

- **Status line:** `found`, `out_of`, `search_time_ms`; `found_docs` with groups; "before duplicates" for union.
- **Hits** (`hits.rs`). A `uniform_list` of fixed 72 px rows (`gpui-pre-0.3.11/src/elements/uniform_list.rs`): id, `text_match`, up to two `HitLine`s drawn as `StyledText` with `HighlightStyle` on `marks`, a curated marker. Selecting a row, by arrow keys or click, fills a resizable detail: the document in a read-only `JsonEditor` and `text_match_info` under it. Union rows show their collection.
- **Groups.** `grouped_hits` draw as heading rows (`group_key`, `found`) inside the same list.
- **Facet panel.** One section per `facet_counts` entry, a `Checkbox` per value with its count, checked by `facet_active`, disabled with the note when refused. A toggle writes the filter and runs. `sampled: true` shows "sampled".
- **Raw tab.** `BodyView` with the response text.
- **Pagination.** Previous and Next under the list (⌥⌘↑ / ⌥⌘↓) write `page` and run; there is no infinite scroll, because the page is a parameter the user is studying.
- **Multi.** With several searches, the results area has one tab per search, each with its status and its own error (§5.2). Union shows one list.

### 7.5 Multi and union

Tabs above the toolbar edit `drafts`. ⌘T adds a copy of the active draft, up to `MULTI_SEARCH_LIMIT`, ⇧⌘W removes, ⌥⌘← / ⌥⌘→ switch. Federated|Union (⇧⌘M) needs at least two drafts. In union, `page` and `per_page` are read from the first draft and sent in the URL, and their fields on other tabs are disabled with "Union uses the first search's page". The server's sort and grouping rules show inline (§4.3).

### 7.6 History, saved queries, presets

`views/search/queries.rs`, and `AppState` owns the writes.

- `AppState` gains `history`, `history_path`, `saved`, `saved_path`, `saved_error`. Both files use one small `WriteChain` (previous task awaited, flushed through `cx.on_app_quit`, on the background executor), copying the `views_save_task` pattern. A view never owns a save.
- A run enters history when it was started by Run, or when an instant run stood for `HISTORY_SETTLE`. `History::push` drops an equal neighbour.
- **History** (⇧⌘H) is a popover with a search input and a list, newest first. Enter loads the entry into the drafts and runs nothing. Clear history asks a medium `Confirm`.
- **Saved** (⌘O) lists this connection's queries, filtered to the active collection, with a toggle for all, and searchable by name and tag. Enter loads, ⌘⌫ deletes after `Confirm`.
- **Save** (⌘S) opens a dialog with name and comma-separated tags. An existing name asks before replacing.
- **Presets** (⌥⌘P) lists `presets()`. A pick replaces the drafts: an object with `searches` becomes one draft per search, anything else one draft on the current collection. A `value` that is not an object shows inline; §4.3's silent `preset=nope` is why the list comes from the server. Save as preset takes a name, confirms a replacement with `Confirm`, and goes through `ensure_writable`. The body is `to_search`'s output, or `{"searches":[…]}` for several.

### 7.7 Export

⌥⌘E opens a menu with the five formats, each writing to the clipboard, and ⌥⌘U copies the curl directly. The inline status says "Copied". Export never includes a key.

### 7.8 Raw console

`views/console/mod.rs`.

```
┌ Method ▾ [ path …                              ] [Send ⌘↩]  ⌥⌘U  ⇧⌘H  ⌘D ┐
├ Query ──────────────────────────────┬ 200 OK · 12 ms · 2.1 KB ──────────┤
│ key | value  + ⇧⌘A                  │ Body | Headers                    │
├ Body (JSON ▾) ⌘O from file          │ warning: 2 of 40 lines failed     │
│ JSON editor                         │ BodyView (masked)  Reveal ⌥⌘K     │
└─────────────────────────────────────┴───────────────────────────────────┘
```

- **Method** is a `Select` of GET, HEAD, POST, PUT, PATCH, DELETE. On a locked profile the non-read methods show the lock.
- **Path** is a `CompletionInput` fed by `path_candidates`. A `parse_path` error shows under it and disables Send.
- **Query** is rows of key and value, with `param_key_ok` per row. ⇧⌘A adds a row. **Body** is a `JsonEditor`, hidden for GET and HEAD. A parse error warns and does not block, because JSONL is valid here: the Body kind `Select` picks JSON or Text. **From file** reads through `spawn_blocking`, refuses above `RAW_FILE_MAX`, and keeps the text outside the editor, showing name and size with Remove.
- **Send** (⌘↩): masked body check (`contains_mask`), then `ensure_writable` for anything but GET and HEAD, then `raw_danger`: `None` sends, `Medium` opens `Confirm`, `High` opens `ConfirmDanger`. Cancelling sends nothing. A read can be replaced by a new Send. A write runs detached (§7.2), Send stays busy, and "Stop waiting" lets go of the result without cancelling the request.
- **Response:** status, duration, size, a Body and a Headers tab, `outcome_note`. A status of 400 or above colors the status line and the body still shows, in full.
- **Masking** (D41): the body goes through `redact::json` first. Reveal (⌥⌘K) shows the raw text until the next Send, because `POST /keys` answers with the only copy of a key's `value`.
- **History** (⇧⌘H) and **favorites** (⌘D toggles the current request, a star in the list). Favorites are `HistoryEntry::Raw` items in `SavedFile::favorites`, so they survive the 500 cap. Loading an entry fills the form and sends nothing.
- **Copy as curl** (⌥⌘U) uses `curl::request`, masking the body. From a file it adds nothing to the command and says "body from file: `--data-binary @name`" in the text.
- A request here is logged like any other. The log's "Copy as curl" works on it.

### 7.9 Full view in the request log

`request_detail::Detail` learns whether a cut body has its whole text (`full_body`). The section then has "Show full body" (⇧⌘B in a new `RequestLog` key context on the panel). It opens a dialog with a `BodyView`, redacted through `redact::json` on the background executor, with the environment named in the title. The dialog owns the view, so closing it frees the text. When the text was evicted, the same place says "Full body no longer kept. The log keeps the last 64 MiB."; a body over `FULL_BODY_MAX` says it was too large to keep instead.

### 7.10 Command palette

`views/palette.rs`. ⌘K / Ctrl+K and ⌘P / Ctrl+P are global bindings handled on `RootView`. An open dialog stops them (`has_active_dialog`).

- **Kit.** `gpui-component-0.9.0/src/command/`: `CommandState`, `Command`, `CommandGroup`, `CommandItem`. The kit's filter is a substring match, so `.filterable(false)` hands the query to `.on_query` and `search::fuzzy::rank` orders the items, at most `PALETTE_LIMIT`.
- **Why items carry no `.action`.** `CommandState::confirm` dispatches an item's action from the palette's own focus, where `DOCUMENTS` and the other contexts are out of reach. So `.on_confirm(IndexPath)` closes the dialog and then dispatches with `window.dispatch_action` in `window.defer`, after focus returned. The shortcut is drawn by `.child`, which the kit documents as owning the hint, with `Kbd::binding_for_action_in(action, &previous_focus, window)`.
- **Actions.** `window.available_actions(cx)` is read before the dialog takes focus, so it lists what the focused view can do, and the list has no dead control. Names keep the `tessera::` prefix and need an entry in `actions::action_label(name) -> Option<&'static str>`, a key under `palette.action.*`. The table is a test: every action in `gpui_kit::actions!` is either labelled or listed in `PALETTE_HIDDEN` (selection steps, dialog internals).
- **Other groups.** Collections of open connections (opens the tab), saved queries of the selected connection (opens Search with it), connections (Enter connects or selects). Each row names its connection, and prod carries the badge in the row (UX rule 1).
- **⌘P** is the same palette with collections only. Enter opens the tab.
- Destructive actions open their usual dialogs; the palette adds no way around a confirmation.
- `Esc` closes through `.on_cancel`.

### 7.11 keymap.toml

`main` reads the file before the window opens, like the profiles. `actions::apply_keymap(bindings, cx) -> Vec<String>` runs after `bind_keys`:

- `cx.build_action(name, None)` builds the action.
- `KeyBindingContextPredicate::parse` reads the context.
- `KeyBinding::load` builds the binding, and `cx.bind_keys` adds it. Later bindings win at the same depth, and `Kbd::binding_for_action` picks the highest precedence, so every control shows the user's chord.
- No action binds `NoAction`, which unbinds.

Each bad entry is skipped and named in a notification after the window opens: a background event, so a toast. `docs/building.md` documents the file.

### 7.12 Shortcuts

Global unless a context is named. The new contexts are `WORKBENCH` ("Workbench"), `WORKBENCH_INPUT` ("Workbench > Input"), `CONSOLE` ("Console"), `CONSOLE_INPUT` ("Console > Input") and `REQUEST_LOG` ("RequestLog"). All show with `Kbd` on the control.

| Action | macOS | Linux, Windows |
|---|---|---|
| Command palette | ⌘K | Ctrl+K |
| Quick switch | ⌘P | Ctrl+P |
| Open Search | ⇧⌘E | Ctrl+Shift+E |
| Open Console | ⇧⌘Y | Ctrl+Shift+Y |
| Run (`WORKBENCH`, input) | ⌘↩ | Ctrl+Enter |
| Form / JSON | ⌘1 / ⌘2 | Ctrl+1 / Ctrl+2 |
| Hits / Raw | ⌘3 / ⌘4 | Ctrl+3 / Ctrl+4 |
| Instant | ⌥⌘L | Ctrl+Alt+L |
| Add / remove search | ⌘T / ⇧⌘W | Ctrl+T / Ctrl+Shift+W |
| Previous / next search | ⌥⌘← / ⌥⌘→ | Ctrl+Alt+Left / Right |
| Federated or union | ⇧⌘M | Ctrl+Shift+M |
| Previous / next page | ⌥⌘↑ / ⌥⌘↓ | Ctrl+Alt+Up / Down |
| History | ⇧⌘H | Ctrl+Shift+H |
| Saved | ⌘O | Ctrl+O |
| Save query | ⌘S | Ctrl+S |
| Presets | ⌥⌘P | Ctrl+Alt+P |
| Export | ⌥⌘E | Ctrl+Alt+E |
| Copy as curl | ⌥⌘U | Ctrl+Alt+U |
| Focus `q` | ⌘F | Ctrl+F |
| Send (`CONSOLE`, input) | ⌘↩ | Ctrl+Enter |
| Body from file (`CONSOLE`) | ⌘O | Ctrl+O |
| Favorite (`CONSOLE`) | ⌘D | Ctrl+D |
| Add query row (`CONSOLE`) | ⇧⌘A | Ctrl+Shift+A |
| Reveal (`CONSOLE`) | ⌥⌘K | Ctrl+Alt+K |
| Response body / headers (`CONSOLE`) | ⌘3 / ⌘4 | Ctrl+3 / Ctrl+4 |
| Stop waiting (`CONSOLE`) | ⌥⌘. | Ctrl+Alt+. |
| Remove body file (`CONSOLE`) | ⌥⌘O | Ctrl+Alt+O |
| Previous / next chunk (`BodyView`, `CONSOLE`, `WORKBENCH`) | ⌥⌘[ / ⌥⌘] | Ctrl+Alt+[ / ] |
| Full body (`REQUEST_LOG`) | ⇧⌘B | Ctrl+Shift+B |

Checked against `gpui-base-0.9.0/src/input/base/state.rs`: Ctrl+Y is the kit's redo in an input, which is why history is not ⌘Y. ⌥⌘I is the kit's inspector, which is why Instant is ⌥⌘L. `ctrl-shift-e` is bound there on macOS only, and that is a different chord from ⇧⌘E.

### 7.13 Empty states, errors, i18n

- No collection on the connection: one sentence and the `cargo xtask seed` hint on a local connection. No hits: "No documents match" with the search's `q` and `filter_by`.
- All strings through `t!()` under `search.*`, `console.*`, `palette.*`, with the same keys in `en.yml` and `de.yml`.
- The `documents.*` copy strings are not reused in the workbench, since their wording names documents and rows.

## 8. Testing

| Level | What it checks |
|---|---|
| Client unit (`wiremock`) | Route at 3,999 and 4,000 bytes; `read_post` passes a read-only client and fails over; `raw` refuses a foreign origin; `raw` keeps response headers; embedded error mapping for union and for a one-search fallback; union page in the URL; preset paths |
| Client integration (`#[ignore]`, 30.0 and 30.2) | `search`, long filter over POST with the same answer, federated with one failing search, union, union uniform error, presets create, read, list, delete, `raw` GET and POST, read-only refuses `raw` POST |
| Core unit | Param allowlist table; `to_pairs`; `side_effects`; `marks`; highlight walk with arrays and nesting; `facet_value_ok`, `toggle_facet` table including `\|\|`; `parse_sort` with `_eval`; export snapshots with hostile strings; history skip, redaction, rotation; saved load errors; `parse_path` table with every spelling from §5.7; `raw_danger` table; `outcome_note`; OpenAPI scanner; fuzzy order; keymap parse; `RequestLog` budget and eviction |
| Acceptance (`#[ignore]`, 30.0 and 30.2) | `tests/live.rs`: for a `q` with quotes and a filter with `&&`, spaces and backticks, run the exported curl through `sh -c` with `TYPESENSE_API_KEY` set, compare JSON with the client's answer minus `search_time_ms`, for a GET route, a POST route and a multi-search |
| Widgets (`#[gpui_kit::test]`) | `CompletionInput` Enter rule; `BodyView` three states |
| App UI (fake backends) | Tab opens per connection; Form and JSON sync, a bad JSON keeps the map; stale answers dropped; instant debounce fires once; instant off on prod; side effects pause it; facet click writes the filter and runs; refused value disabled; ⌘T up to 50; union moves page to the URL; history records after settle only; saved round trip; a corrupt saved file is not overwritten; preset save locked until unlock; console: read-only refuses POST before sending; `High` needs the name; cancel sends nothing; a write survives closing the tab; reveal resets; palette lists only available actions, dispatches in the old focus, ⌘⌫ from documents via palette opens the documents dialog; keymap override shows on the control |
| Secrets | History never holds a key parameter or an `api_key` value; export contains no key; `Body::full` stays out of `Debug`; console reveal resets on Send; full view is redacted |
| Manual (PR C) | `BodyView` on an 8 MiB body against NF4, §12 |

## 9. CI

- The `integration` job already runs on 30.0 and 30.2. The acceptance test needs `curl` and `sh` on the runner, which both have.
- The analytics trap (§4.3) stays a manual note: neither CI job starts the server with `--enable-search-analytics`. Add the flag to `dev/docker-compose.yml`? No: seeding should not depend on it, and the flag only proves the server's own promise.
- NF5 stays; each PR records the Linux binary size.

## 10. New dependencies

| Crate | Used by | Why |
|---|---|---|
| `toml = "=1.2.0"` (already in the tree) | core | `keymap.toml` |

Nothing else. No fuzzy matcher, no YAML parser, no code generator: the scanner and scorer are a few dozen lines each, tested.

## 11. Changes to the product spec

Recorded in `docs/spec.md` together with this design:

- §16 gains D33 to D44. D30 notes that D34 closes it. The §14 M4 row links this design.
- §7.5 gains the side effects, §7.17 the path and danger rules, §7.19 the budget, §7.20 the groups that exist and the keymap shape.
- §8.1's bottom panel loses the raw console, §11 gains the favorites and the keymap format, §6 notes the `multi_search` read route.

## 12. Risks

| Risk | Impact | Fallback |
|---|---|---|
| The kit's `Editor` is slow on large bodies: the rope builds on the UI thread and "about 50K lines" is its own stated limit | NF4 breaks on a big response | `VIEW_MAX` is one constant. Task 12 measures an 8 MiB pretty-printed body and lowers it if a frame takes over 16 ms. A line-count check joins it if needed |
| Instant search keeps a body per run in the log, up to 1 MiB each for 5,000 entries (the D24 problem again, since §7.5's AC needs the entry) | Memory during long typing sessions | A typical result is small and 150 ms bounds the rate. If measured high, drop bodies for instant runs only, keeping size. Noted, not built |
| `window.available_actions` depends on handlers registered in the last frame | An action missing from the palette for one frame after a view change | Read at open, which follows a frame. Accept |
| Palette dispatch after `window.defer` races focus return | Action lands on the wrong context | UI test from `DOCUMENTS` and from the workbench. If it fails, focus the stored handle explicitly first |
| Drag and drop of chips inside a scroll view | Awkward on trackpads | ⌥← / ⌥→ is the full path; drag is a convenience |
| Saved or history files hold query text with personal data | Local privacy | Same trust as `connections.json`; Clear history exists; temporary connections write nothing |
| `keymap.toml` can unbind safety chords | User can remove a shortcut | The controls stay, and a chord never skips a dialog |
| Union `found` semantics change between versions | Status line misleading | The wording says "before duplicates" and the integration job covers the shape only |
| Analytics was probed on 30.0 only | 30.2 may differ | The docs say the same (`enable_analytics: false`), and sending the flag is harmless either way |
| `POST` reads hide in dashboards that count writes | The log's Writes filter would mislead | Fixed in §4.2 |
