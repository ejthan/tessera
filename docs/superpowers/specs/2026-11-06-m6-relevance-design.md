# M6 Relevance resources: design

- **Date:** 2026-11-06
- **Status:** written autonomously (maintainer takes the recommendations), pending final review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M6 row in §14; §7.8 to §7.12, §5, §6, §8, §10, §12, §16)
- **Previous milestone:** [M5 import and export design](2026-11-06-m5-import-export-design.md)
- **UX mockups:** none for these screens. The [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d) has Main, Connect, Overview, Browser and Safety boards; the sketches below stand in

All behaviour marked "both" was checked on 2026-11-06 with curl on `localhost:18108` (Typesense 30.2) and on a throwaway 30.0 container on port 18208, on `m6p_*` resources that are gone again. The two versions answered the same except where §4.4 says so (spacing in error JSON aside). Port 8108 was never touched. Every GPUI Kit API named here was read in the pinned source (`gpui-pre-0.3.11`, `gpui-base-0.9.0`, `gpui-component-0.9.0`, `gpui-kit-0.9.0`).

## 1. Goal

M6 makes the resources that shape search results manageable: aliases with a reindex assistant that switches a collection without downtime, synonym sets and curation sets with item editors, attachment and side-by-side test panels, stopword sets, and presets as a listed resource edited in the workbench. Nothing here can silently damage search: a delete that would make a collection unsearchable says so and prevents it, and a 200 that stored something else is compared and shown.

**Acceptance (product spec §14, §7.8 to §7.12):**

- CRUD for aliases, synonym sets, curation sets, stopwords and presets works against 30.0 and 30.2, with an integration test for every endpoint in §4 on both.
- The test panels run the same two searches on both versions and show them side by side, with what each side means stated on screen (§7.6).
- The reindex assistant takes an alias from an old collection to a new one in five steps: nothing on the server changes before the first Start, and the alias never moves before the counts were compared.
- In a read-only profile no write of this milestone leaves the client until the user unlocks it.
- Every "⚠ verify" of §6 for these resources is resolved in `docs/spec.md`.

## 2. Delivery

One spec, one plan, three pull requests in this order, each with green CI and its own CHANGELOG entries. PRs stack on `feat/m5-export`, because merging is the maintainer's step.

| PR | Branch | Content | Why this order |
|---|---|---|---|
| **A · Simple resources** | `feat/m6-resources` | Client for aliases, stopwords, preset delete; core names; navigation and the resources tab; aliases view; stopwords view; presets view and preset mode | Proves the navigation on the three resources that have no items |
| **B · Sets** | `feat/m6-sets` | Client for both set kinds, core models and validation, synonym editor, attachment, missing sets, delete with detach, import and export, shared test panel, curation editor with document picker | The largest slice, and it needs the shell of A |
| **C · Reindex** | `feat/m6-reindex` | Core preflight and compare, the assistant, seed additions, docs | Uses aliases from A and the attachment rules from B |

## 3. Scope

### In M6

§7.8 including the reindex assistant, §7.9, §7.10, §7.11 and §7.12, with the sidebar rows, palette entries and shortcuts that reach them. Presets: listing, delete and preset mode. M4 already loads and saves (D37).

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Legacy synonyms and overrides below v30 | v1.x | §14, §7.22 |
| Stemming dictionaries | M7 | §14 |
| Key actions `synonym_sets:*`, `curation_sets:*` | M7 | §7.14 |
| Rank difference between the two sides of a test panel | v1.x | §7.5 keeps it as compare mode. Two lists side by side is what §7.9 asks |
| Catching up writes made during a reindex | not planned | Export is not a snapshot and the API has no change feed. The count comparison shows the gap, and the assistant says to stop writers first |
| Resuming a stopped reindex copy | not planned | Same as M5: start again into a new collection |
| A test panel for stopwords | not planned | §7.11 names none, and the workbench takes `stopwords` in its JSON mode |
| CSV for curation sets | not planned | §7.10 does not list it; a rule does not fit one row |
| Renaming a preset or set | not planned | The server has no rename. Save under a new name and delete the old one |
| Rule marks for the search error `Synonym index not found` in the documents view | v1.x | The Synonyms tab names the cause and the fix (§7.5). The browser shows the server's message inline, as any other |
| Analytics rules that name a collection, in the reindex preflight | M7 | The endpoints arrive there; M7 extends the preflight |

## 4. tessera-client

No new dependency. Models stay `serde_json` values where they round-trip server data (D5).

### 4.1 Aliases

Into `lib.rs` beside `aliases()`, because `Alias` already lives in `collections.rs`.

```rust
impl Client {
    pub async fn alias(&self, name: &str) -> Result<Alias, Error>;
    pub async fn upsert_alias(&self, name: &str, collection: &str) -> Result<Alias, Error>;
    pub async fn delete_alias(&self, name: &str) -> Result<(), Error>;
}
```

`PUT /aliases/{name}` with `{"collection_name":…}`. Paths go through a private `alias_path` that uses `addressable` and `SEGMENT`, like `preset_path`. `upsert_alias` decodes the answer into `Alias`. The caller compares it (§4.4).

### 4.2 Synonym and curation sets

New module `crates/tessera-client/src/sets.rs`. The two kinds have the same seven endpoints (both, including every 404), and only their items differ, so they share one implementation. Items stay `Value`: a rule has fields this client does not know, and the editors keep them (§7.10 of the product spec).

```rust
pub enum SetKind { Synonym, Curation }            // Debug, Clone, Copy, PartialEq, Eq
pub struct ItemSet { pub name: String, pub items: Vec<Value> } // Debug, Clone, PartialEq, Deserialize, Serialize

impl Client {
    pub async fn sets(&self, kind: SetKind) -> Result<Vec<ItemSet>, Error>;          // GET /{kind}
    pub async fn set(&self, kind: SetKind, name: &str) -> Result<ItemSet, Error>;    // GET /{kind}/{name}
    pub async fn upsert_set(&self, kind: SetKind, name: &str, items: &[Value]) -> Result<ItemSet, Error>; // PUT
    pub async fn delete_set(&self, kind: SetKind, name: &str) -> Result<(), Error>;  // DELETE
    pub async fn set_item(&self, kind: SetKind, name: &str, id: &str) -> Result<Value, Error>;
    pub async fn upsert_set_item(&self, kind: SetKind, name: &str, id: &str, item: &Value) -> Result<(), Error>;
    pub async fn delete_set_item(&self, kind: SetKind, name: &str, id: &str) -> Result<(), Error>;
}
```

`upsert_set_item` removes `id` from a copy of the body, because the path decides (§4.4), and returns nothing, because the answer echoes what was sent rather than what was stored: callers read the item back with `set_item`. `GET /{kind}/{name}/items` exists on both and is not used: the set answer holds the same array.

### 4.3 Stopwords and presets

Stopwords go into `crates/tessera-client/src/stopwords.rs`.

```rust
pub struct StopwordSet { pub id: String, pub stopwords: Vec<String>, pub locale: Option<String> }
impl Client {
    pub async fn stopwords(&self) -> Result<Vec<StopwordSet>, Error>;  // {"stopwords":[…]}
    pub async fn stopword_set(&self, id: &str) -> Result<StopwordSet, Error>; // {"stopwords":{…}}: wrapped
    pub async fn upsert_stopwords(&self, id: &str, words: &[String], locale: Option<&str>) -> Result<(), Error>;
    pub async fn delete_stopwords(&self, id: &str) -> Result<(), Error>;
}
```

`upsert_stopwords` returns nothing, because the `PUT` answer is unwrapped and echoes the input (§4.4), and the view must read the stored form. In `search.rs`: `pub async fn delete_preset(&self, name: &str) -> Result<(), Error>`.

Every call above runs through `get_ok` or `write`, so the read-only guard, the log and failover apply with no new path. `PUT` and `DELETE` are in `FAILOVER_METHODS`; all of these are idempotent.

### 4.4 Traps found while probing

Marked (both) unless a version is named.

**Aliases**

- **An alias takes anything as its target.** `PUT` with a missing collection, with `""`, or with another alias answers 200. An alias on an alias is stored and resolves to 404, so chains do not work. The view offers only real collections and marks broken aliases in the list (§7.4).
- **`PUT` on an existing alias repoints it silently.** So "new" and "repoint" are different flows (§7.4).
- **An alias cannot take the name of a collection** (500, `Name … conflicts with an existing collection name.`), but a collection can take the name of an alias: `POST /collections` answers 201, and the collection then shadows the alias until it is dropped. The list marks such a shadowed alias.
- **`DELETE` of a missing alias answers 404 `Collection not found`,** which misleads. The view says the alias is gone and reloads.
- **Dropping a collection leaves its aliases** pointing at nothing.
- **A `PATCH /collections/{alias}` works on the target,** so an alias may be used as a target of any collection call, as M5 found for import and export.

**Attachment (resolves §6 "attachment attribute")**

- **The attribute is `synonym_sets` and `curation_sets`, arrays of names, on the collection.** `GET /collections/{name}` shows them. `PATCH` replaces the whole array and takes effect at once, without a schema change job. `POST /collections` takes them too. So there is no attach endpoint, and two writers can overwrite each other (§5.3).
- **`PATCH` or `POST` with a name that does not exist answers 404** (`Synonym set … not found.`) and changes nothing.
- **Deleting an attached synonym set is allowed and breaks every search on the collection.** `DELETE` answers 200, the collection keeps the name, and from then on every search there, `q=*` and `enable_synonyms=false` and `multi_search` included, answers 404 `Synonym index not found`. Export still works. Creating a set of that name heals it, and so does a `PATCH` without the name. This decides §7.7.
- **Deleting an attached curation set is harmless to search,** which keeps working, but the name stays on the collection. Both kinds then block a later `PATCH` that still lists the missing name, and a `POST /collections` with it, which is what a clone or reindex sends.

**Sets**

- **`PUT` on a set replaces it,** and a `PUT` of a new name creates it, with no difference in the answer. `{"items":[]}` is accepted and empties a set. `{}` is a 400.
- **Duplicate ids in `items` are silently reduced to the last one:** two sent, one stored, 200. Core refuses them first (§5.2).
- **An item `PUT` takes its id from the path;** an `id` in the body is ignored. Its answer echoes the input with any unknown keys, while the stored item has them dropped and the defaults added, so a `PUT` answer proves nothing.
- **Unknown item attributes are dropped silently** on both kinds (`unknown_attr`, `typo`). "Unknown fields are kept as a raw JSON block" can only keep what the server returned.
- **Item ids and set names may hold anything.** `/`, `,`, `?`, `#`, a backtick, a space and `é` all round-trip percent-encoded. A comma in a set name cannot be used in `synonym_sets=`, which splits at commas.
- **Item endpoints resolved:** `GET/PUT/DELETE /{kind}/{name}/items/{id}` and `GET /{kind}/{name}/items` exist on both. `PUT` on an item of a missing set is 404 `Synonym index not found` or `Curation index not found`. `DELETE` of a missing item is 404 ``Could not find that `id`.``; `GET` is 404 `Not Found`. `DELETE` of an item or set answers `{"id":…}` or `{"name":…}`.
- **Synonym items:** `root` comes back as `""` for a multi-way item. One synonym alone is accepted and does nothing. `symbols_to_index` and `locale` round-trip.

**Curation rules**

- **The server accepts a rule that can never work.** `rule.match: "wrong"` is stored and never fires. `filter_by: "nosuchfield:=1 &&"` is stored, and the bad filter is silently ignored when the rule fires. `effective_from_ts` after `effective_to_ts` is stored. `position: 0` is stored. `rule: {"tags":[]}` is stored. Core checks all of these (§5.2).
- **A rule with a bad `sort_by` breaks the searches it matches:** it fires and the search answers 404 ``Could not find a field named `nosuch` in the schema for sorting.`` The editor warns (§5.2).
- **The server's errors:** no `id` is a 400, `rule` without `tags` or `query` plus `match` is a 400, an item with no action is a 400 naming the actions, an include without `position` is a 400, and a string in `effective_from_ts` is `Bad request.`
- **Defaults depend on the content:** `stop_processing` is `true`; `remove_matched_tokens` is `true` without includes or excludes and `false` with them. A form must not hide that: it sends both explicitly (§5.2).
- **`rule.stem` and `rule.synonyms` exist on 30.2 only.** 30.2 stores and returns them; 30.0 accepts the keys and drops them. The same key at the top of an item is dropped on both. The form hides these two below 30.2.
- **`diversity` differs.** 30.2 takes `"weight":1`; 30.0 refuses it with ``Invalid `weight` format: `1`. Expected a float number.`` and takes `1.0`. Neither `Value` nor the raw JSON block changes a number's spelling, so the server's message shows inline. `diversity` is in the raw block, not the form.
- **`curation_sets` is not a search parameter.** It is absent from the OpenAPI search parameters, `curation_sets=name` changes nothing, and a missing name is no error, on `GET` and on `multi_search`. The product spec's "or passed as a search parameter" holds for synonym sets only. `enable_overrides` and `override_tags` are ignored too. Only `enable_curations=false` and `curation_tags` steer curation at search time. This decides §7.6.
- **Tags switch rules on and off in both directions.** A rule with `tags` fires only when `curation_tags` names one of them. With `curation_tags` set, rules without tags do not fire. A rule with `tags`, `query` and `match` needs both.

**Search parameters**

- **`synonym_sets=` adds to the attached sets.** It cannot leave one out, and `synonym_sets=` empty changes nothing. A missing name is 404. `enable_synonyms=false` beats every set, attached or named. So "without this set" exists only as "without any synonyms" when the set is attached (§7.6).
- **`stopwords=name` with a missing set is 404** ``Could not find the stopword set named …``. **`preset=name` with a missing preset is ignored silently,** 200. These go into the delete dialogs.

**Stopwords and presets**

- **`GET /stopwords/{id}` wraps its answer** in `{"stopwords":{…}}`, the `PUT` does not, and the list is an object with an array.
- **The stored words are not the sent words.** The server lowercases them, splits them at spaces, removes duplicates and changes the order: `["The","the","  x y "]` is stored as `["y","x","the"]`, while the `PUT` answer echoes the input. An empty list is a 400. `locale` is unchecked.
- **A preset's `value` is any JSON,** `"str"` included, and the list hands it back. `{}` is a 400. `DELETE` answers the deleted preset. A `multi_search?preset=` with `searches` works.

**Reindex**

- **`store: false` fields do not survive.** Export omits them. An optional one is lost from the index without any error: a search that found the document on the old collection finds nothing on the new one. A required one fails every document with ``Field `req` must be a string.``
- **Fields outside the schema survive,** in export and in import, and so do nested objects, geopoints and vectors.
- **`GET /collections/{name}` lists fields the server added for patterns** (`x_s`, `o.a.b`) next to the pattern. Sent back in a `POST`, without `num_documents` and `created_at`, it creates an equal collection.
- **Joins do not survive a switch.** A `reference` may name an alias, and the join works until the alias is repointed. Then it fails with ``Failed to join on `…`: No reference field found.``, and upserting the child's documents does not repair it. Searches without the join keep working, and so do the child's documents. A dropped parent gives ``Referenced collection … not found.`` on joins only.
- **`PUT` on the alias is atomic from a client's view,** and there is no compare-and-swap.

### 4.5 An export ends when its reader stalls

Measured with a socket that read one buffer of an export of about 81 MB and then stopped reading, on `products` (30.2) and a collection of the same size (30.0):

| Stall | 30.2 | 30.0 |
|---|---|---|
| 5 s | complete, ends with the chunked terminator | complete, same |
| 25 s, 50 s | complete | not measured |
| 70 s | connection closed at 5.2 MB, no terminator | closed at 5.2 MB, no terminator |
| 100 s, 130 s | closed at 5.2 MB, then 3.6 MB | not measured |

So the server gives up on a reader that makes no progress for somewhere between 50 and 70 s. The close comes without a terminator, so `reqwest` reports it and `ExportStream::next_line` gives `Error::Stream`: M5's export job fails rather than writing a short file, and it never stalls that long anyway, because a file takes bytes faster than the server sends them (largest gap 80 ms, M5 §4.5). But an export piped straight into an import stalls for as long as one batch takes, and a batch of 1,000 documents with a remote embedding model can take minutes. This decides D63.

## 5. tessera-core

New module tree `crates/tessera-core/src/relevance/`. No GPUI dependency.

### 5.1 Names (`mod.rs`)

```rust
pub fn name_ok(name: &str) -> bool; // [A-Za-z0-9_][A-Za-z0-9_.-]{0,127}
pub fn param_safe(name: &str) -> bool; // no comma, control character or edge whitespace
```

`name_ok` is the allowlist for a name this client creates (D61). A name that exists on the server may hold anything: it stays addressable through `SEGMENT` encoding, editable and deletable. Only `param_safe` limits it: a set whose name has a comma cannot be sent in `synonym_sets=`, so its test side is disabled with the reason.

### 5.2 Models and validation (`synonyms.rs`, `curations.rs`, `changes.rs`)

```rust
pub struct SynonymDraft { pub id: String, pub root: String, pub synonyms: Vec<String>,
    pub locale: String, pub symbols_to_index: Vec<String>, pub extra: Map<String, Value> }
impl SynonymDraft { pub fn from_value(&Value) -> Result<Self, ModelError>; pub fn to_value(&self) -> Value; }

pub struct CurationDraft { pub id: String, pub rule: RuleDraft, pub includes: Vec<(String, i64)>, pub excludes: Vec<String>,
    pub filter_by: String, pub sort_by: String, pub replace_query: String, pub remove_matched_tokens: bool,
    pub filter_curated_hits: bool, pub stop_processing: bool, pub effective_from_ts: Option<i64>,
    pub effective_to_ts: Option<i64>, pub metadata: Option<Map<String, Value>>, pub extra: Map<String, Value> }
pub struct RuleDraft { pub query: String, pub r#match: String, pub filter_by: String, pub tags: Vec<String>,
    pub synonyms: bool, pub stem: bool, pub extra: Map<String, Value> }

pub enum ItemIssue { /* §5.2 table */ }
pub fn check_synonym(draft: &SynonymDraft) -> Vec<ItemIssue>;
pub fn check_curation(draft: &CurationDraft) -> Vec<ItemIssue>;
pub fn check_items(items: &[Value]) -> Vec<ItemIssue>;            // duplicates, no id
pub struct Changes { pub added: Vec<String>, pub changed: Vec<String>, pub removed: Vec<String> }
pub fn changes(old: &[Value], new: &[Value]) -> Changes;          // by `id`
pub fn sort_fields(sort_by: &str) -> Vec<String>;
```

`extra` keeps what the form has no control for, and the form shows it as a raw JSON block, so nothing a server returned is lost on a save. Unset text fields are left out of `to_value`, `rule.synonyms` and `rule.stem` are written only when true, and `remove_matched_tokens`, `stop_processing` and `filter_curated_hits` are always written (§4.4). A new draft starts with `stop_processing: true`, and `remove_matched_tokens` follows the server's rule until the user touches it.

Errors stop a save, because the server would store them without complaint. Warnings do not.

| Issue | Level | Checks |
|---|---|---|
| Empty or duplicate `id` | error | Both kinds; duplicates are lost silently |
| Fewer than 2 synonyms without a `root`, no synonym with one | error | Synonyms |
| `match` other than `exact` or `contains` | error | Rule with a query |
| Rule with neither query plus match, non-empty tags, nor `filter_by` | error | Rule |
| No action: all of includes, excludes, `metadata`, `filter_by`, `sort_by`, `replace_query` empty, and `remove_matched_tokens` false | error | Same list as the server's message |
| An include without an id, or with a `position` below 1 | error | Rule |
| The same document in includes and excludes | error | Rule |
| `effective_from_ts` after `effective_to_ts` | error | Rule |
| `sort_by` names a field missing from an attached collection | warning | `sort_fields` against the schemas of attached collections, skipping `_text_match`, `_eval(…)`, `_vector_distance`, `_seq_id`, `_group_found` and `$` joins. Such a rule fails every search it matches |

`changes` drives the review before a whole-set `PUT`: it says "adds 3, changes 1, removes 12" rather than a diff, because a set is a list of independent rows.

### 5.3 Attachment and missing sets (`attachment.rs`)

```rust
pub fn attached<'a>(collection: &'a Collection, kind: SetKind) -> &'a [String];
pub fn users<'a>(collections: &'a [Collection], kind: SetKind, name: &str) -> Vec<&'a str>;
pub struct Missing { pub set: String, pub collections: Vec<String> }
pub fn missing(collections: &[Collection], kind: SetKind, known: &[String]) -> Vec<Missing>;
pub struct NewList { pub list: Vec<String>, pub dropped: Vec<String> }
pub fn with_set(current: &[String], known: &[String], name: &str) -> NewList;
pub fn without_set(current: &[String], known: &[String], name: &str) -> NewList;
pub fn attachment_patch(kind: SetKind, list: &[String]) -> Value; // {"synonym_sets":[…]}
```

`with_set` and `without_set` keep the order, and put names that are not in `known` into `dropped`, because a `PATCH` that lists one fails with 404 (§4.4). The caller reads the collection and the set list fresh right before: a stale `known` would detach someone else's new set, and a stale `current` would undo their attachment.

### 5.4 CSV and JSON files of a synonym set (`synonym_file.rs`)

```rust
pub fn synonyms_to_json(items: &[Value]) -> String;   // {"items":[…]}, pretty
pub fn synonyms_to_csv(items: &[SynonymDraft]) -> String;
pub fn parse_synonyms(text: &str, format: SetFormat) -> Result<Vec<Value>, SetFileError>;
pub fn merged(current: &[Value], incoming: &[Value]) -> Vec<Value>; // by id, incoming wins, order kept
```

D67. The CSV has the header `id,root,synonyms,locale,symbols_to_index`, written with the `csv` crate. List cells join with `;`, the separator business tools use after `,`. A value that holds a `;`, or a list that starts with `[`, is written as a JSON array in the cell, the way M5 treats list cells (D51), and the reader takes such a cell as JSON, so the round trip is lossless. A blank `id` becomes `row_{n}`. JSON accepts `{"items":[…]}` or a bare array. Cells stay verbatim, with the existing hint of D55. Files are limited to `CLIPBOARD_LIMIT` (16 MiB).

`parse_synonyms` runs `check_items`, so a file with duplicate ids is refused with both row numbers.

### 5.5 Test sides (`test.rs`)

```rust
pub enum SideMeaning { AsConfigured, WithSet, NoSynonyms, NoCurations }
pub struct TestSides { pub left: (SideMeaning, Vec<(String, String)>), pub right: (SideMeaning, Vec<(String, String)>), pub broad: bool }
pub enum TestBlocked { NotAttached, NameNotSendable }
pub fn test_sides(kind: SetKind, set: &str, attached: &[String], tags: &str) -> Result<TestSides, TestBlocked>;
```

D56, D57. The returned pairs are what differs; the panel appends them to the common `q`, `query_by`, `enable_analytics=false`. Table in §7.6.

### 5.6 Reindex (`reindex.rs`)

```rust
pub fn reindex_name(source: &str, now: jiff::Timestamp) -> String;
pub struct Preflight { pub other_aliases: Vec<String>, pub unstored: Vec<String>, pub required_unstored: Vec<String>,
    pub referenced_by: Vec<(String, String)>, pub missing_sets: Vec<String>, pub embedded: Vec<String>, pub blank_secrets: usize }
pub fn preflight(alias: &str, source: &Collection, draft: &SchemaDraft, collections: &[Collection],
    aliases: &[Alias], synonym_sets: &[String], curation_sets: &[String]) -> Preflight;
pub fn without_missing(draft: &mut SchemaDraft, preflight: &Preflight);
pub struct Compared { pub expected: u64, pub target: u64, pub failed: u64, pub unconfirmed: u64 }
impl Compared { pub fn equal(&self) -> bool; } // expected == target && failed == 0 && unconfirmed == 0
```

- **`reindex_name`** is `{base}_{YYYYMMDD_HHMMSS}` in UTC, and `base` is `source` without a trailing `_\d{8}_\d{6}`, so a third reindex does not grow the name. It is a proposal in an input and passes `name_ok`.
- **`unstored`** lists the source's fields with `store: false`. **`required_unstored`** are those that stay in the draft without `optional`: every document would fail, so copying from the old collection is refused until the schema changes.
- **`referenced_by`** lists `(collection, field)` for any field whose `reference` names the source or any alias on it, before the `.`.
- **`missing_sets`** are the draft's attached names that no longer exist; `without_missing` removes them, since `POST` would answer 404.
- **`embedded`** lists fields with `embed`, whose copy was not verifiable here (§13).
- **`blank_secrets`** comes from `SchemaDraft::blank_secrets`: a clone blanks embedding keys, so the user must enter them.

### 5.7 Constants

| Constant | Value | Why |
|---|---|---|
| `MAX_NAME` | 128 | `name_ok`; the server has no limit, a name in a URL and a tab should |
| `SET_FILE_LIMIT` | 16 MiB (`CLIPBOARD_LIMIT`) | A set is read whole |
| `TEST_PER_PAGE` | 10 | Two columns of a window |
| `PICKER_PER_PAGE` | 20 | Document picker |

## 6. tessera-widgets

No new widget. `Confirm`, `ConfirmDanger`, `JsonEditor` and `FilterInput` serve as they are. The picker for `effective_*_ts` is the kit's `date_picker::{DatePicker, DatePickerState}` (`gpui-component-0.9.0/src/time/date_picker.rs`, re-exported in `lib.rs:119`), with `.time_precision(TimePrecision::Minute)` from `time_field`.

## 7. tessera-app

### 7.1 Backend seam

`crates/tessera-app/src/relevance.rs`, same structure as `DocumentsBackend`.

```rust
pub trait RelevanceBackend: Send + Sync {
    fn sets(&self, client: Client, kind: SetKind) -> BoxFuture<Result<Vec<ItemSet>, Error>>;
    fn set(&self, client: Client, kind: SetKind, name: String) -> BoxFuture<Result<ItemSet, Error>>;
    fn save_set(&self, client: Client, kind: SetKind, name: String, items: Vec<Value>) -> BoxFuture<Result<ItemSet, Error>>;
    fn delete_set(&self, client: Client, kind: SetKind, name: String) -> BoxFuture<Result<(), Error>>;
    fn save_item(&self, client: Client, kind: SetKind, name: String, id: String, item: Value) -> BoxFuture<Result<Value, Error>>; // PUT, then GET
    fn delete_item(&self, client: Client, kind: SetKind, name: String, id: String) -> BoxFuture<Result<(), Error>>;
    fn stopwords(&self, client: Client) -> BoxFuture<Result<Vec<StopwordSet>, Error>>;
    fn save_stopwords(&self, client: Client, id: String, words: Vec<String>, locale: Option<String>) -> BoxFuture<Result<StopwordSet, Error>>; // PUT, then GET
    fn delete_stopwords(&self, client: Client, id: String) -> BoxFuture<Result<(), Error>>;
    fn save_alias(&self, client: Client, name: String, collection: String) -> BoxFuture<Result<Alias, Error>>;
    fn alias(&self, client: Client, name: String) -> BoxFuture<Result<Alias, Error>>;
    fn delete_alias(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;
    fn delete_preset(&self, client: Client, name: String) -> BoxFuture<Result<(), Error>>;
    fn attach(&self, client: Client, kind: SetKind, collection: String, set: String, attach: bool) -> BoxFuture<Result<AttachOutcome, Error>>;
    fn read_set_file(&self, path: PathBuf) -> BoxFuture<Result<String, SetFileError>>;
    fn write_set_file(&self, path: PathBuf, text: String) -> BoxFuture<Result<(), String>>;
    fn copy_dir(&self) -> BoxFuture<Result<Arc<TempDir>, String>>;
}
```

- Writes are detached in the views (`cx.spawn(...).detach()`, as `send_preset`), so closing a tab never aborts one.
- `attach` is the whole §5.3 sequence on the tokio side: read the collection and the set names, build the list, `PATCH`, and read the collection again. `AttachOutcome { list, dropped, mismatch }`: the list read back is compared with the list sent, and `mismatch` shows inline.
- `save_item` and `save_stopwords` read back, since the `PUT` echo proves nothing (§4.4).

Aliases and collections stay in `CollectionsBackend::list`, which already loads both. Test panels and the picker use `SearchBackend::search`.

### 7.2 Navigation and the resources tab

Five rows in the sidebar under a connected entry, after Console, through the same `link_row`: Aliases, Synonyms, Curations, Stopwords, Presets. The product spec's §8.1 lists three, and §7.11 and §7.12 need a home too (D68). `Synonyms` and `Curations` show only with `capabilities.supported`, since below v30 those endpoints do not exist (§5.6; UX rule 4).

```rust
pub enum ResourceKind { Aliases, Synonyms, Curations, Stopwords, Presets }
pub struct OpenResource(pub Uuid, pub ResourceKind);        // state.rs, like OpenConsole
pub struct ResourcePanel { /* connection, kind, view */ }   // views/dock.rs
```

One panel type with an enum of views, because `SearchPanel` and `ConsolePanel` show a panel is 90 lines of the same code. `Kind::Resource(ResourceKind)` joins `Kind` and `ConnectionTab` in `root.rs`, with one tab per connection and kind. Title: dot, name, environment badge, lock (UX rule 1). The edge color and `unlock::banner` come along. `AppState::open_resource(id, kind, cx)` follows `open_console`.

The list half of each view is a `DataTable` with a `TableDelegate`, as `collections_list.rs` does, with a filter `Input`, ⌘F focus, ⌘R reload, and an empty state that explains the resource in one sentence and links its Typesense docs (§8.3). Lists load on open and on ⌘R. An error shows inline in full (`long_error_text`). A `401` or `403` says the key may not read this resource, inline.

### 7.3 Shared behaviour of all writes

1. `ConnectionState::ensure_writable(window, cx, then)` before any dialog that leads to a write; the control shows `env_badge::lock` while locked.
2. Confirmation by D62. Cancelling sends nothing.
3. Detached send. Errors inline in the dialog or view that started it, in full.
4. Reload the list after success. Compare where §7.4 to §7.9 say so.

**New checks first.** `PUT` overwrites silently, so New loads the fresh list and refuses a name that exists, inline, with the existing target or size (D60). The list may still race another client, which costs a replaced resource; nothing better exists without compare-and-swap.

### 7.4 Aliases

`views/resources/aliases.rs`.

```
┌ Aliases  [New ⇧⌘N] [Reload ⌘R]  [filter ⌘F]──────────────────────────────┐
│ Name           Target                Documents   State                    │
│ books_current  books                    10,000                            │
│ shop           products_20261106_…     200,000                            │
│ old            archive                       –   ⚠ target does not exist  │
│ chain          shop                          –   ⚠ target is an alias     │
│ orders         orders_v2                 5,120   ⚠ hidden by a collection │
└ [Repoint ⌘E] [Reindex ⇧⌘R] [Delete ⌘⌫]──────────────────────────────────────┘
```

- **State** is computed from the loaded list, in core's `alias_state(alias, collections, aliases)`: `Ok`, `TargetMissing`, `TargetIsAlias`, `TargetEmpty`, `Shadowed`. Rule 2: these show in the row, in full.
- **New:** name input and a `SelectState` over real collection names. `name_ok`, not a collection's name, and not an existing alias; for the last, the message offers Repoint. After the `PUT`, the answer's `collection_name` must equal the target (a 200 can have done something else), then the list reloads and `invalidate_collections` runs so the collections tab shows the alias.
- **Repoint:** the same select, on the current target. D62. Consequences: alias, old target with its count, new target with its count, and a line when the counts differ by more than nothing, since a client would see different results at once.
- **Delete:** D62. Consequences: searches through the alias answer 404. A 404 on delete says the alias is gone, and reloads.
- **Reindex:** opens the assistant (§7.9). Disabled for a broken alias, with the reason on the control.

### 7.5 Synonym sets

`views/resources/synonyms.rs`, on top of `views/resources/sets.rs`, which the two kinds share: the split, the set table, tabs, attachment, JSON tab, delete, missing sets.

```
┌ Synonyms [New ⇧⌘N] [Import ⇧⌘I] [filter]  ┬ books_synonyms   ●prod              ┐
│ ⚠ `old_set` does not exist but is attached │ [Items ⌘1][JSON ⌘2][Collections ⌘3] │
│   to shop. Every search on shop fails.     │ [Test ⌘4]                           │
│   [Detach ⌘⌫]                              ├─────────────────────────────────────┤
│ Name             Items  Collections        │ Id     Root   Synonyms       Locale │
│ books_synonyms       2  books              │ novel         novel, book, … en     │
│ colors              41  –                  │ sci           science, sci          │
│                                            │ [Add ⇧⌘N] [Edit ⌘E] [Delete ⌘⌫]      │
└────────────────────────────────────────────┴─────────────────────────────────────┘
```

- **Set table:** name, item count, using collections. New asks a name, by `NameDialog`'s shape, and creates with `{"items":[]}` after the fresh check of §7.3.
- **Items tab:** a `DataTable`. Add and Edit open `SynonymItemDialog`: id, kind (multi-way or one-way, which shows `root`), one synonym per line, locale, `symbols_to_index`, and the raw block when `extra` is not empty. It saves that one item with `save_item`, because a table row is one change and must not overwrite a colleague's other rows (D60). Changing the id of an existing item saves the new one, then deletes the old. Delete asks through `Confirm`.
- **JSON tab:** `JsonEditor` over `{"items":[…]}`. Save parses, runs `check_items`, shows `changes` in the review, and sends the set `PUT`. A stored count unlike the sent count shows inline. With removals on prod, D62 applies.
- **Collections tab:** §7.7.
- **Test tab:** §7.6.
- **Missing sets** (from `missing`) stand above the set table, red, with the collections named and `Detach`, which runs `attach(…, false)` for each. Creating a set of that name is the other way out, and New prefills it from the row. D58.
- **Delete set:** §7.7.
- **Import ⇧⌘I** in the Synonyms view: pick a file (JSON or CSV by extension, overridable), read off the GPUI thread, `parse_synonyms`. A dialog shows the count and the mode: into a new set (name input), or into the selected set as Merge or Replace. Review by `changes`, then `save_set`. **Export ⇧⌘X:** JSON or CSV by `ButtonGroup`, `prompt_for_new_path`, written through a new `transfer::write_text` on M5's `PartFile`, on the tokio side, with a note for CSV on D55's hint.

### 7.6 Test panel

`views/resources/test_panel.rs`, in both set views.

```
┌ Collection [books ▾]  q [novel        ⌘F]  query_by [title    ]  Tags [     ]  [Run ⌘↩] ┐
│ books_synonyms is attached here, with other sets. Typesense cannot leave out one         │
│ attached set, so the left side runs without any synonyms.                                │
├ No synonyms · 12 found · 3 ms ──────────┬ As configured · 41 found · 4 ms ────────────────┤
│ …HitsView…                              │ …HitsView…                                      │
└─────────────────────────────────────────┴─────────────────────────────────────────────────┘
```

Two `HitsView` entities (`views/search/hits.rs`, which already marks curated hits), filled through `SearchBackend::search`, so both requests are in the request log 1:1. Common parameters: `q`, `query_by`, `per_page=10`, `enable_analytics=false` (D35). `query_by` starts from `query_fields(schema)`, joined by commas, in a plain input. An empty `query_by` with `q=*` is allowed. Each side has its own error, inline, so a failed side leaves the other. Both requests start together and either result shows when it arrives; a new Run drops the old tasks, and a stale result cannot arrive. Tests are reads, so they run on a locked profile.

| Kind | Set on the collection? | Left | Right | The panel says |
|---|---|---|---|---|
| Synonym | No | As configured | `synonym_sets={set}` | The set is added for this search only |
| Synonym | Yes, alone | `enable_synonyms=false` | As configured | – |
| Synonym | Yes, with others | `enable_synonyms=false` | As configured | Typesense cannot leave out one attached set, so the left side runs without any synonyms |
| Curation | Yes, alone | `enable_curations=false` | As configured, with `curation_tags` if typed | – |
| Curation | Yes, with others | `enable_curations=false` | Same | Same sentence for curations |
| Curation | No | blocked | blocked | Curation sets work only when attached; a search cannot name one. The Collections tab attaches it |
| Either | Name holds a comma (`param_safe`) | blocked for the unattached row | – | The name cannot be sent |

Curation tags: the input shows for curation only, and a note under it states the two-way rule of §4.4. `curation_tags` is validated by `param_safe` per tag.

The left side's meaning is decided by fresh data: the collection is read when picked (`CollectionsBackend::get`), so a recent attach counts.

A missing attached set makes both sides fail with `Synonym index not found`; the panel then shows that the Synonyms tab can fix it. Attaching from here is deliberately not offered: a test must not change production search with one click.

### 7.7 Attachment and deleting a set

The Collections tab lists every collection with a `Checkbox`, and a filter. The attached ones come first. Toggling runs `ensure_writable`, D62, then `attach`. The box shows the fresh answer afterwards, not the click. `dropped` names shows inline: "Also removed `x`, which no longer exists." An `Error` shows inline and the box returns. Aliases are not offered: attachment belongs to the collection.

`invalidate_collections` runs afterwards, so the collections tab's counts and open schema views follow.

**Delete set** (D58, D62): reloads collections and sets, then asks. With users, the consequences say which collections lose the set and that a set cannot stay attached, because every search on a collection fails while it does (synonyms) or its later schema changes fail (curations). The action first detaches from each user with `attach(…, false)`, in order, then deletes, then reads the collection list again and checks that no collection names the set. If a detach fails, it stops: the set stays, the dialog shows the error, and the collections done are named. If deletion fails after the detaches, the dialog says the set still exists and is no longer attached, and which ones to re-attach. Running this inside `ConfirmDanger`'s task keeps the dialog busy until it ends, and "Stop waiting" leaves it running.

### 7.8 Curation sets

`views/resources/curations.rs`, on `sets.rs`. Items tab columns: id, query or tags, match, includes, excludes, active (from `effective_*_ts` against now), stops. `CurationItemDialog`, width 760, form and JSON tabs (⌘1, ⌘2) over one `CurationDraft`:

```
┌ Rule  sale_chairs   shop  PROD ──────────────────────────────────────────┐
│ Id [sale_chairs]                                                         │
│ When  Query [chair]  Match [exact|contains]  Tags [        ]             │
│       Filter [                ]  [ ] Synonyms  [ ] Stem   (30.2 only)    │
│ Pin   ⋮ 4  Lamp "Arc"          position [1] [↑⌥↑] [↓⌥↓] [×]              │
│       ⋮ 7  Chair "Eames"       position [2]                              │
│ Hide  3  "Stool"  [×]                                                    │
│ Find  Collection [shop ▾] [search ⌘F]   results: + Pin  + Hide           │
│ Then  Filter [      ] Sort [      ] Replace query [      ]               │
│       [x] Remove matched tokens  [ ] Filter curated hits  [x] Stop       │
│ From  [date, UTC]  To [date, UTC]   Metadata {…}   Other fields {…}      │
├──────────────────────────────────────────────────────────────────────────┤
│ `sort_by` field `nosuch` is not in shop: every match would fail          │
│                                                   [Cancel] [Save ⌘↩]     │
└──────────────────────────────────────────────────────────────────────────┘
```

- **Tabs** convert through the draft, as the schema dialog's form and JSON do. Invalid JSON stays in the editor, with the form unchanged and Save disabled.
- **Includes** are `(id, position)`. Rows reorder by drag, with the `DragChip` pattern of `views/search/form.rs:600-612` (`on_drag`, `on_drop`), and by ⌥↑ ⌥↓ on the focused row, the keyboard way UX rule 5 needs. Reordering renumbers positions 1 to n. A typed position stays, and order follows it.
- **Document picker:** `SearchBackend::search` with `q` (or `*`), `query_by` from `query_fields`, `per_page=20`, `enable_curations=false`, `enable_analytics=false`, `exclude_fields` of vector fields, so the picker shows stored documents. A row shows the id and the first string field, with `+ Pin` and `+ Hide`. The collection select lists attached collections first, and any other. A manual id input stays, because a document may be in several collections.
- **Dates:** two `DatePickerState`s with minute precision and a label saying UTC: `effective_*_ts` are absolute, and showing the viewer's zone would make a rule read differently between colleagues. Conversion uses `chrono::NaiveDateTime::and_utc().timestamp()`, and `chrono = "=0.4.45"` becomes a direct dependency of `tessera-app`, the same version `Cargo.lock` has, since the kit's picker exposes that type and `gpui_kit` does not re-export it. Clearable, because both are optional.
- **Version gate:** `rule.synonyms` and `rule.stem` show at ≥ 30.2 by a new `ServerCapabilities::rule_flags()`. On an older server, set values in `extra` would be silently dropped, so a draft that has them shows a note.
- **Checks:** `check_curation` errors disable Save with the text inline. The `sort_by` warning reads the schemas of the set's attached collections from the collection cache.
- **Save:** `save_item`, read back. A stored item that lost a key that was sent shows inline: "The server did not keep: …". This is how a future or older server's dropped fields show, instead of disappearing.

### 7.9 Reindex assistant

`views/resources/reindex.rs`. A center tab, `Kind::Reindex(alias)`, one per alias, opened from the aliases view.

```
┌ Reindex shop   ●prod ───────────────────────────────────────────────────────┐
│ ① Schema   ② Copy   ③ Compare   ④ Switch   ⑤ Clean up                        │
├──────────────────────────────────────────────────────────────────────────────┤
│ shop → products (200,000 documents)                                          │
│ New collection [products_20261106_142233]     [Edit schema… ⌘E]              │
│ Source (●) products  ( ) A file…                                             │
│                                                                              │
│ ⚠ `notes` is not stored, so export cannot copy it. It stays empty.           │
│ ⚠ orders.product_id joins this collection. Those joins fail after the switch │
│   and need orders rebuilt; a re-import of orders into it does not help.      │
│ ⚠ Stop writers first: documents written during the copy are not in it.       │
│ ⓘ Also pointing at products: shop_v1 (stays).                                │
│                                                               [Start ⌘↩]     │
└──────────────────────────────────────────────────────────────────────────────┘
```

**State.** `ReindexState` is an entity in a new `ConnectionState::reindex: HashMap<String, Entity<ReindexState>>`, keyed by alias, so closing the tab loses nothing and opening it again continues (D66). The copy is two normal jobs in `Jobs`, an export and then an import, so it outlives the tab and shows in the jobs panel with its report. `run_export` and `run_import` stay unchanged. A disconnect drops the state. That is safe: the alias only moves in step 4, and the job's own rules (D48) apply.

```rust
pub enum CopySource { Collection, File(PathBuf) }
pub enum Step { Schema, Exporting(JobId), Importing(JobId), Compare(Compared), Switched(Compared), Done }
```

**① Schema.** On open: fresh `CollectionsBackend::list` and both set lists, `SchemaDraft::from_collection(source)`, `blank_secrets`, `preflight`, `without_missing`. Warnings list as above. `Edit schema…` opens `SchemaDialog::open_pick`, as M5 did, so nothing is sent; the picked draft reruns `preflight`. `Start` is disabled, with the reason inline, while `required_unstored` is non-empty for the collection source, while `blank_secrets > 0`, while the name fails `name_ok`, or while it exists.

Source: the old collection, or a file picked with `prompt_for_paths`, read with `sniff` and checked with `preview`, whose `ReadError` shows inline. No other import option is offered: `action=create`, `dirty_values=coerce_or_reject`, batch `DEFAULT_BATCH`. Anything else is the import dialog's job on the new collection.

Start: `ensure_writable`, `Confirm` on prod (D49), `CollectionsBackend::create`. A failed create stays on step 1, inline. Then the copy starts, and the bottom dock opens on Jobs without moving focus.

**② Copy** (D63). From the old collection: `RelevanceBackend::copy_dir` makes a private temp directory (`tempfile::Builder`, mode 0700 on Unix), and `Jobs::start_export` writes all documents as JSONL to `documents.jsonl` in it, with `expected` the source count. `ReindexState` subscribes to `JobFinished`, so this works with the tab closed. A finished export starts `Jobs::start_import` on that file with `action=create`. From a file: only the import. The step shows the job's numbers, with Cancel ⌘. and Stop waiting ⌥⌘. acting on it. The directory goes when the import ends in any way, when the export stops or fails, on `Start over`, on disconnect and on quit. A job that ended `Stopped` or `Failed` stays here, with its text in full, and offers `Delete {new}` (typed, as the collection has data) and `Start over`. Nothing moved, which it says. Step 1 says that the copy needs free disk of about the collection's export size. A full disk is an ordinary failed export (`TransferError::Io`).

**③ Compare.** Reads both collections fresh into `Compared`, with `failed` and `unconfirmed` from the job. `expected` is the source's count, or for a file the job's `imported`, with the old count shown for information, because a file may rightly differ. The panel states the numbers and the job's `count` check. Equal: Next. Different: `Save report` as in the panel, `Compare again` ⌘R, and Next stays enabled, with the danger raised in step 4.

**④ Switch.** Reads the alias fresh. If it no longer points to the source, it stops and says where it points: someone else moved it, and overwriting that would lose their change. Consequences: alias, old and new targets with counts, joins that fail from `referenced_by`. D62. `save_alias`, then a fresh `GET`; a different target is an inline error. `invalidate_collections`. Afterwards the step offers `Switch back` (a plain `Confirm`, re-checking that the alias points to the new collection) and Next.

**⑤ Clean up.** `Delete {old}` calls `collection_danger::delete`, whose typed dialog lists every alias that still points there. `Keep it` ends the assistant. After this, the state is removed. A note says a dropped old collection breaks nothing but the joins listed before.

### 7.10 Stopwords

`views/resources/stopwords.rs`. A split: table (id, words, locale) and an editor with a plain multi-line `JsonEditor::plain` of one word per line, and a locale input. Save splits lines, trims, drops empties, refuses an empty result inline, and runs `save_stopwords`, which reads back. The text is replaced by the stored words, and a note appears when they differ from the sent ones: "Stored 3 words: the server lowercases, splits at spaces and removes duplicates." The stored order is the server's, so the note compares sets, not sequences. Delete: D62, with the consequence that searches naming this set fail with 404 until they stop naming it.

### 7.11 Presets

`views/resources/presets.rs`. A table (name, kind, summary). `Kind` from core's `preset_kind(value)`: `Search`, `Multi(n)`, `Union(n)`, `Invalid`. Summary: `q` and `query_by`, or the collections. An `Invalid` value stays listed, with the reason and Delete; Open is disabled.

- **Open ⌘E** opens the workbench in preset mode. `OpenSearch` gains a third field, the preset name: `OpenSearch(Uuid, Option<(Vec<SearchDraft>, bool)>, Option<String>)`. The root's handler calls `workbench.load(drafts, union, window, cx)` then `workbench.set_preset(name, cx)`. `preset_drafts` and `preset_value` move from `views/search/queries.rs` to `tessera_core::search::presets`, so the list and the workbench read values the same way and core tests them. `preset_drafts`' text errors become a `PresetError` enum that the app translates (rule 7).
- **Preset mode** is one field, `SearchWorkbench::preset: Option<String>`. A band above the toolbar shows `Preset {name}`, with `Save ⌘S` and `Leave`. `SaveQuery` (⌘S) in this mode saves to that preset through `send_preset`, with `Confirm` on prod and `ensure_writable`. `load` clears the field when it comes from elsewhere. Runs behave as always, against the chosen collection; a preset keeps no collection unless it has several searches.
- **New ⇧⌘N** asks a name (`name_ok`, fresh check), then opens the workbench in preset mode on an empty draft of the current collection. The preset exists only after the first Save, so cancelling costs nothing.
- **Delete:** D62, with the consequence that a search naming it keeps working but silently loses its parameters (§4.4).

### 7.12 Shortcuts

All shown on the control with `Kbd`. New contexts: `RESOURCES` ("Resources"), `RESOURCES_TABLE` ("Resources > DataTable"), `SET_ITEM_DIALOG` ("SetItem"), `TEST_PANEL` ("TestPanel"), `INCLUDE_ROW` ("IncludeRow"), `REINDEX` ("Reindex"), each with an `… > Input` twin where ⌘↩ must work inside an input.

| Action | macOS | Linux, Windows | Context |
|---|---|---|---|
| Aliases, Synonyms, Curations, Stopwords, Presets | ⌥⌘1 to ⌥⌘5 | Ctrl+Alt+1 to 5 | global, with a selected connection |
| New | ⇧⌘N | Ctrl+Shift+N | `RESOURCES` |
| Edit, Repoint, Open | ⌘E | Ctrl+E | `RESOURCES` |
| Delete | ⌘⌫ | Ctrl+Backspace | `RESOURCES` |
| Reload | ⌘R | Ctrl+R | `RESOURCES` |
| Filter | ⌘F | Ctrl+F | `RESOURCES` |
| Import set, Export set | ⇧⌘I, ⇧⌘X | Ctrl+Shift+I, X | `RESOURCES` |
| Reindex | ⇧⌘R | Ctrl+Shift+R | `RESOURCES` |
| Items, JSON, Collections, Test | ⌘1 to ⌘4 | Ctrl+1 to 4 | `RESOURCES` |
| Run test | ⌘↩ | Ctrl+Enter | `TEST_PANEL` |
| Save item | ⌘↩ | Ctrl+Enter | `SET_ITEM_DIALOG` |
| Move include | ⌥↑, ⌥↓ | Alt+Up, Alt+Down | `INCLUDE_ROW` |
| Next step, Start | ⌘↩ | Ctrl+Enter | `REINDEX` |
| Save preset | ⌘S | Ctrl+S | `WORKBENCH` (existing binding) |

All chords were checked against `actions.rs`. ⌥⌘1 to 5 are free, and context-bound chords reuse their constants. New actions go into `ACTION_LABELS` with a `palette.action.*` key; the palette also lists the five tabs for the selected connection.

### 7.13 i18n

Flat keys under `resources.*`, `aliases.*`, `synonyms.*`, `curations.*`, `sets.*`, `stopwords.*`, `presets.*`, `test.*`, `reindex.*`, same keys in `en.yml` and `de.yml`. `search.preset.*` stays. Core words nothing (`ItemIssue`, `AliasState`, `SideMeaning`, `PresetError` are enums the app translates).

## 8. Testing

| Level | What it checks |
|---|---|
| Client unit (`wiremock`) | Paths with encoded names, ids with `/` and `,`, `.` and `..` refused; `upsert_set_item` removes `id` from the body; `stopword_set` unwraps, `stopwords` does not; read-only refusal on every write, `expect(0)`; 404 keeps `typesense_message()` |
| Client integration (`#[ignore]`, 30.0 and 30.2), in a new `tests/live_relevance.rs` | Create, read, update, delete of every endpoint in §4 on throwaway `tessera_it_*` names; the §4.4 traps as assertions: alias to a missing target, repoint, name conflict, duplicate ids reduced, echo versus stored, unknown attributes dropped, attachment by `PATCH`, `PATCH` with a missing name is 404, deleted attached synonym set fails search and export still works, deleted curation set leaves search working, `curation_sets` parameter changes nothing, `synonym_sets` adds, `enable_synonyms=false` beats it, stopwords normalised, preset of any JSON, unstored fields lost on copy, joins fail after repoint. `rule.stem` asserted by version from `/debug` |
| Core unit | `name_ok`, `param_safe`; draft round trips including `extra`; every row of the issue table; `changes`; `with_set`, `without_set` with dangling names and order; `missing`; CSV and JSON round trips with `;`, `[`, quotes and blanks; duplicate ids refused with row numbers; `test_sides` every row of §7.6; `reindex_name` with an existing suffix; `preflight` for each field; `alias_state`; `preset_kind`; `sort_fields` |
| Core live (`tests/live_transfer.rs`) | A collection of 2,000 documents with nested, geopoint, vector and out-of-schema data copies through `run_export` and `run_import` into a clone, with equal counts and equal export bytes after sorting by id; an optional unstored field is lost, and a required one fails every line |
| App UI (fake `RelevanceBackend`) | Sidebar rows and hidden rows below v30; tab per kind; every write sends nothing when locked until unlocked; D62 table including focus; New refuses an existing name; alias states; `save_alias` mismatch inline; item dialog validation; JSON tab review; attach shows the fresh answer and dropped names; delete set detaches in order and stops at a failure; test panel: every row of §7.6, one side failing leaves the other, a new Run drops the old answers, both requests in the log; curation form to JSON round trip keeps `extra`; include reorder by key and drag; picker adds; preset mode saves to its name, New sends nothing until Save, `Invalid` cannot open; stopwords note; reindex: nothing on the server before Start, the tab closes and reopens with its state, the import starts after the export with the tab closed, Cancel in step 2 leaves the alias and removes the directory, unequal counts raise the danger, an alias moved by someone else stops the switch, Switch back |
| Secrets | A preset saved in preset mode goes through `without_secrets`. A log entry of a test search holds no key |

## 9. CI

The integration job runs `live_relevance` and the extended `live_transfer` on 30.0 and 30.2, against the existing matrix. No new job. NF5: `chrono` is already in the binary through the kit, so the size does not move by more than the new views.

## 10. New dependencies

`chrono = "=0.4.45"` as a direct dependency of `tessera-app` only, already in `Cargo.lock`. `jiff` joins `tessera-core` from the workspace for `reindex_name`. `tempfile` must be a normal dependency of `tessera-app` for `copy_dir`, if it is only a dev-dependency there today. No new crate enters the tree.

## 11. Architecture notes

- `docs/architecture/async.md` gains a paragraph on the reindex copy: export to a file, then import, and why one stream would break (§4.5).
- Nothing new persists, so §11 of the product spec gains no file.

## 12. Changes to the product spec

Recorded in `docs/spec.md` together with this design: §5.2 (`sets` replaces two module names), §5.6, §6 (all "⚠ verify" of these rows, the attachment note, the wrong search-parameter claim), §7.8 to §7.12, §8.1, §12.2, §14's M6 row, and §16 gains D56 to D68.

| # | Question | Decision |
|---|---|---|
| D56 | What does "with and without the set" mean in a synonym test panel? | Without it is the plain search and with it adds `synonym_sets=`, for a set not attached. For an attached set the left side is `enable_synonyms=false`, because a search cannot leave out one attached set; with other sets attached the panel says that side has no synonyms at all |
| D57 | How is a curation set tested? | Only on a collection it is attached to, as `enable_curations=false` against the plain search, plus `curation_tags`. `curation_sets` is not a search parameter on 30.0 or 30.2. A test never attaches anything |
| D58 | What happens to attachments when a set is deleted? | The delete detaches it from every collection first, then deletes, and checks afterwards. A missing attached synonym set makes every search on the collection fail. Missing sets show in the view with Detach |
| D59 | How is an attachment written? | `PATCH` of the whole list, right after reading the collection and the set names fresh. Names that no longer exist are dropped and named, because the `PATCH` would fail on them |
| D60 | Item endpoints or whole-set `PUT`? | The table saves item by item, so a row edit cannot overwrite a colleague's other rows. JSON, import and Replace use the whole-set `PUT` with a review of adds, changes and removals. New refuses an existing name after a fresh list, because every `PUT` is an upsert |
| D61 | Which names can Tessera create? | `[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}`. Existing names stay usable whatever they hold |
| D62 | Confirmations in M6 | New, item save, attach, detach: none, `Confirm` on prod. Item delete: `Confirm`. Whole-set save that removes items: `Confirm`, and typed on prod. Alias repoint or delete, stopwords delete, preset delete: `Confirm`, typed on prod. Set delete: `Confirm`, typed when attached or on prod. Reindex switch: `Confirm`, typed on prod or when the counts differ or joins fail |
| D63 | Does the reindex copy go through a temp file? | Yes: export to a private temp directory, then import that file, as two ordinary jobs, with no change to either pipeline. One stream was the first design, but the server closes an export whose reader stalls for 50 to 70 s (both versions), and one slow batch does that. The cost is free disk of about the export's size, for as long as the copy runs |
| D64 | What blocks a reindex? | A required `store: false` field for a copy from the old collection, a blank embedding key, or a bad or taken name. Optional unstored fields, joins from other collections and other aliases are warnings: the operator may well accept them |
| D65 | What guards the switch? | Counts are compared and shown; a difference raises the confirmation to typing the alias. The alias is read right before the `PUT` and the assistant stops if it moved. The `PUT` is read back. `Switch back` stays until Clean up |
| D66 | Where does the assistant live? | A center tab per alias, with state in `ConnectionState`, so closing the tab keeps it, and the copy in `Jobs`. Nothing can be half-switched, so losing the state on disconnect loses only the wizard |
| D67 | CSV of a synonym set | `id,root,synonyms,locale,symbols_to_index`, lists joined by `;`, or a JSON array in the cell when a value holds `;` or the list starts with `[`. Lossless, readable for business teams |
| D68 | Where do stopwords and presets go in the sidebar? | Their own rows after Curations. `Synonyms` and `Curations` hide below v30. One tab per kind and connection, one panel type |

## 13. Risks

| Risk | Impact | Fallback |
|---|---|---|
| Embedding fields were not probed: it takes a model | A copy may call the model again, or keep the exported vector. Cost, or different vectors | Preflight warns for every `embed` field. A vector comes from export either way, and the count check runs |
| Export is not a snapshot | Documents written during the copy are missing | Warning in step 1; counts in step 3 show the gap |
| The temp directory holds a full copy of the documents | A hard kill leaves it in the OS temp directory until the OS clears it; a full disk stops the export | Mode 0700, removed in every normal ending. Same class as a `.part` file (M5) |
| The server's stall limit lies between 50 and 70 s and may be a setting | Export to a very slow disk could hit it | Not reachable in practice (80 ms largest gap); the job fails loudly, without a short file |
| Two operators attach at the same moment | One change lost, no compare-and-swap | The fresh read keeps the window to one round trip; the box shows the fresh answer after |
| The curation editor is the largest dialog in the app | Implementation size, focus bugs | One dialog, one draft, the JSON tab as an escape hatch for all of it |
| The kit's `DatePicker` inside a stacked dialog | Popup clipping | `deferred` is used by the picker. A text input with `jiff` parsing is the fallback, and the JSON tab always works |
| `rule.stem` on 30.1 is unknown | The gate could hide or show them wrongly there | The gate is ≥ 30.2, the verified version. The read-back note catches a dropped key on anything else |
| `alias` reindex of a collection without any alias | The assistant cannot help, because a name cannot be both | Says so in the empty state: create an alias first and point clients at it |
| Report memory for a copy with many failures | None new | Same `Report` as M5 |
| No on-screen check from this session | Layout bugs found late, as in M3 to M5 | Steps listed in `docs/building.md`, as before |
