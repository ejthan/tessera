# M3 Document Browser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A document table that scrolls through 200k rows with 20 cached pages, filter autocompletion, an inspector with validation and diff preview, single and bulk writes with previews, and copy in five formats.

**Architecture:** `tessera-client` gains `documents.rs` and a clone that logs no response bodies. `tessera-core` gains `documents/` (columns, pages, preferences, filter context, merge patch, validation, id batches, CSV). `tessera-widgets` gains `FilterInput` and marks on `JsonEditor`. `tessera-app` gains a `DocumentsBackend` seam and `views/documents/`, and the collection tab gets Documents and Schema views. Three PRs, in order: A (Tasks 1–7), B (Tasks 8–9), C (Tasks 10–13).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11, feature `tree-sitter`), reqwest 0.13.5, tokio 1.54.1, serde_json 1.0.154, wiremock 0.6.5, insta, lsp-types 0.97.0.

**Spec:** [`docs/superpowers/specs/2026-11-05-m3-documents-design.md`](../specs/2026-11-05-m3-documents-design.md). Section numbers below (§4.2 …) refer to it. Read the section a task names before starting it.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI. Direction: `app → widgets → core → client`.
- Direct dependencies are pinned exactly (`=x.y.z`) in the workspace `Cargo.toml`.
- Never guess a GPUI or GPUI Kit API. Read the pinned source under `~/.cargo/registry/src/*/gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0` and `gpui-pre-0.3.11` first. When a task names a file there, it was verified; anything else you need, verify the same way.
- Every UI string goes through `t!()`, same keys in `assets/locales/en.yml` and `de.yml`. The key-parity test stays green. The `tessera-widgets` crate has its own `rust_i18n::i18n!`.
- No blocking I/O on the GPUI thread. Network calls go through the backend seam on the tokio runtime. JSON parsing of pages and building of cell previews happen in the tokio future.
- No key in logs, files, errors, `Debug` or `Display`. Redaction tests are never removed.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. No commit with a failing check.
- Commits: `git commit -s`, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense: `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, key `xyz`, live tests with `TESSERA_IT_URL=http://localhost:18108 TESSERA_IT_KEY=xyz cargo test -p tessera-client --test live -- --ignored`. **Never send anything to port 8108.**
- UX rules: environment visible everywhere; errors inline and in full; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown on the control via `Kbd::binding_for_action(&Action, Some(CONTEXT), window)`.
- Shortcuts are bound in key contexts, never globally: `DOCUMENTS` ("Documents"), `DOCUMENTS_TABLE` ("Documents > DataTable"), `INSPECTOR` ("Inspector"), plus M2's `COLLECTION`. Platform constants follow the `#[cfg(target_os = "macos")]` pairs in `crates/tessera-app/src/actions.rs`.
- All writes go through `ConnectionState::ensure_writable(window, cx, then)`, and the control shows `env_badge::lock(id, false, cx)` while `writes_locked()`.
- NF5: the Linux release binary stays below 41,943,040 bytes. Each PR records the size in its description.
- Each task ends with its CHANGELOG entry under `## [Unreleased]` → `### Added` in `CHANGELOG.md`, in the same commit.

## Review Focus

1. `merge_patch` must remove a nested key with `null`, and an unchanged key must not be in the body. Test in Task 8.
2. An update by query with a value of the wrong type answers `200 {"num_updated":0}`. The dialog must report it as a failure, not a success. Test in Task 12.
3. ⌘⌫ in the documents view must never open the collection delete dialog. Test in Task 11.
4. A late page answer from an old filter must not land in the new result. Test in Task 4.
5. A selected document hidden by a new filter must not be deletable: the selection clears on every query change. Test in Task 11.
6. A page search's log event must have no response body while keeping `size`. Test in Task 1.

---

## PR A · Browse (branch `feat/m3-documents`, already checked out)

### Task 1: Document read endpoints

**Files:**
- Create: `crates/tessera-client/src/documents.rs`
- Modify: `crates/tessera-client/src/lib.rs` (`mod documents; pub use documents::{DocumentQuery, SearchPage};`, new `Client` field), `crates/tessera-client/src/collections.rs` (make `SEGMENT` `pub(crate)`), `crates/tessera-client/src/send.rs`, `crates/tessera-app/src/views/request_detail.rs`, locales
- Test: `crates/tessera-client/tests/documents.rs` (new, wiremock), `crates/tessera-client/tests/live.rs`

**Interfaces:**
- Produces, as an `impl Client` block in `documents.rs` (no `NodeClient` versions; a child module can call the private `self.get_ok(None, path, query)` and `self.call(None, method, path, query, body, timeout)` of `lib.rs`):
  - `pub struct DocumentQuery { pub page: u32, pub per_page: u32, pub filter_by: Option<String>, pub sort_by: Option<String>, pub exclude_fields: Option<String> }`.
  - `pub struct SearchPage { pub found: u64, pub out_of: u64, pub page: u32, pub search_time_ms: u64, pub documents: Vec<Map<String, Value>> }`, decoded from `{found,out_of,page,search_time_ms,hits:[{document:{…}}]}` through private raw structs; every other key ignored.
  - `pub async fn search_documents(&self, collection: &str, query: &DocumentQuery) -> Result<SearchPage, Error>`: `GET collections/{c}/documents/search`, query `q=*&per_page=&page=` plus present options.
  - `pub async fn count_documents(&self, collection: &str, filter_by: Option<&str>) -> Result<u64, Error>`: same with `per_page=0`, returns `found`.
  - `pub async fn facet_values(&self, collection: &str, field: &str, prefix: &str, limit: u32) -> Result<Vec<String>, Error>`: `per_page=0&facet_by={field}&max_facet_values={limit}`, plus `facet_query={field}:{prefix}` when `prefix` is not empty. Reads `facet_counts[0].counts[].value`; a missing `facet_counts` is an empty list.
  - `pub async fn document(&self, collection: &str, id: &str) -> Result<Value, Error>`: path `collection_path(collection, &format!("/documents/{}", segment(id)))`.
  - `pub async fn export_documents(&self, collection: &str, filter_by: &str) -> Result<String, Error>`: `GET …/documents/export?filter_by=`, `LONG_TIMEOUT`, raw JSONL.
  - `pub(crate) fn encode_query(pairs: &[(&str, &str)]) -> String` with `utf8_percent_encode(value, SEGMENT)`, joined `k=v&k=v`. `pub(crate) fn segment(s: &str) -> String`.
  - `pub fn without_response_bodies(&self) -> Self`: clone with new field `keep_response_body: bool` false (default true in `Client::new`). In `send.rs`: `response_body: text.filter(|_| self.keep_response_body).map(|t| Body::new(t))`, leaving `size` as it is.

- [ ] **Step 1: Write the failing wiremock tests** in `tests/documents.rs`:
  - `search_builds_the_query_and_decodes_hits`: mock answers `{"found":2,"out_of":5,"page":3,"search_time_ms":7,"hits":[{"document":{"id":"1","a":1},"highlight":{},"highlights":[]}],"facet_counts":[]}`; matchers `query_param("q","*")`, `per_page=250`, `page=3`, `filter_by=year:>2000 && a:=b`, `sort_by=price:desc`, `exclude_fields=embedding`; asserts all fields. Add a check on the raw query that `&&` arrived as `%26%26` (from `RequestEvent.query`).
  - `count_uses_per_page_zero`, `facet_values_reads_counts` (body `{"found":5,"hits":[],"facet_counts":[{"counts":[{"count":1,"highlighted":"crime","value":"crime"}],"field_name":"genre"}]}`), `facet_values_without_prefix_sends_no_facet_query`.
  - `document_id_is_one_segment`: id `a/b c?x` requests path `/collections/c/documents/a%2Fb%20c%3Fx`.
  - `missing_document_is_http_404`: `{"message":"Could not find a document with id: 1"}` → `typesense_message()` equal to that text.
  - `export_uses_the_long_timeout`: the mock answers after 300 ms, the client has `with_timeout(Duration::from_millis(100))`, and the call still succeeds, as M2's PATCH test does (`tests/collections.rs`).
  - `quiet_client_logs_size_but_no_body`: hook records the event; `client.without_response_bodies().search_documents(…)` gives `response_body == None`, `size == Some(body_len)`, `status == Some(200)`; the plain client gives `Some`.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test documents`. Expected: compile errors.
- [ ] **Step 3: Implement** the module and the field.
- [ ] **Step 4: Live tests** in `tests/live.rs` (`#[ignore]`): make a uniquely named collection with a facet field through `create_collection`, import 3 documents with plain `reqwest`, then `search_documents` (2 per page), `count_documents` with and without a filter, `facet_values` with a prefix, `document`, `export_documents`; assert a bad filter is `Error::Http{status:400,..}` with the server's text; drop the collection. Run them against 18108.
- [ ] **Step 5: Log detail.** In `request_detail.rs`, when `e.response_body` is `None` and `e.size` is `Some(_)`, show `t!("log.detail.body_not_kept")`: "Response body not kept for document pages". German: "Antworttext von Dokumentseiten wird nicht aufbewahrt". Update `ui_log.rs` with one test.
- [ ] **Step 6: Run** the checks. Expected: PASS.
- [ ] **Step 7: Commit.** `feat(client): read documents`.

### Task 2: The 200k products seed set

**Files:**
- Modify: `xtask/src/seed.rs`, `docs/building.md`

**Interfaces:**
- Consumes: the existing `Rng`, `WORDS`, `Server`, `run`.
- Produces: `fn products(n: usize) -> impl Iterator<Item = Value>`; `Server::import` with signature `async fn import(&self, collection: &str, docs: impl Iterator<Item = Value>) -> Result<()>`, filling batches of 1,000 without collecting the iterator.

- [ ] **Step 1: Write the failing tests** in `seed.rs`: `products_are_reproducible` (`products(50).eq(products(50))`), `product_ids_are_unique_and_prices_positive` over 2,000 documents, `schemas_has_products` (finds name `products` with `enable_nested_fields: true` and `default_sorting_field: "created_at"`).
- [ ] **Step 2: Run** `cargo test -p xtask`. Expected: FAIL.
- [ ] **Step 3: Implement.** `COLLECTIONS` becomes `[&str; 6]` with `"products"` added. Schema:

```json
{"name":"products","enable_nested_fields":true,"default_sorting_field":"created_at","fields":[
 {"name":"title","type":"string"},{"name":"description","type":"string"},
 {"name":"brand","type":"string","facet":true},{"name":"category","type":"string","facet":true},
 {"name":"tags","type":"string[]","facet":true},{"name":"price","type":"float","sort":true},
 {"name":"stock","type":"int32"},{"name":"rating","type":"float","sort":true},
 {"name":"active","type":"bool","facet":true},{"name":"created_at","type":"int64"},
 {"name":"dimensions","type":"object"}]}
```

  Document `i`: `id` = `i` as a string; `title` 3 words; `description` 25 words; `brand` = `format!("brand{:03}", below(200))`; `category` one of `WORDS`; `tags` 3 of `WORDS`; `price` = `below(50_000) as f64 / 100.0`; `stock` below 1000; `rating` = `below(50) as f64 / 10.0`; `active` = `below(5) != 0`; `created_at` = `1_700_000_000 + i`; `dimensions` = `{"w": below(100), "h": below(100)}`. Seed `0xC2B2_AE3D_27D4_EB4F`. In `run`, replace `sets: Vec<(&str, Vec<Value>)>` by a helper that imports an iterator and checks the count, so that `products` is not collected. Sets stay in their order, `products` last, with `PRODUCTS: usize = 200_000`.
- [ ] **Step 4: Live check.** `cargo xtask seed --url http://localhost:18108 --reset`, then `curl -s -H 'X-TYPESENSE-API-KEY: xyz' 'http://localhost:18108/collections/products?exclude_fields=fields'` shows `num_documents: 200000`.
- [ ] **Step 5: Docs.** `docs/building.md` line 58 names `products` (200,000, with nested `dimensions`, about 10 s to load). CHANGELOG entry.
- [ ] **Step 6: Run** the checks, then commit. `feat(xtask): seed 200k products`.

### Task 3: Columns, pages and column preferences in core

**Files:**
- Create: `crates/tessera-core/src/documents/mod.rs`, `columns.rs`, `page.rs`, `views.rs`
- Modify: `crates/tessera-core/src/lib.rs` (`pub mod documents;`), `crates/tessera-core/src/store.rs` (extract `write_atomic`)
- Test: inline `#[cfg(test)]` in each file

**Interfaces:**
- Produces exactly the items of §5.1, §5.2 and §5.3, plus in `documents/mod.rs`: `pub const PER_PAGE: u32 = 250; pub const PAGE_CACHE: usize = 20; pub const CELL_LIMIT: usize = 200; pub const CONFIRM_ABOVE: u64 = 1000; pub const MAX_SELECTION: usize = 1000; pub const ID_FILTER_BYTES: usize = 7500;`.
- `views.rs` also: `pub fn load(path: &Path) -> ViewsFile` (missing or unreadable file gives the default, unreadable logs a warning), `pub fn save(path: &Path, file: &ViewsFile) -> Result<(), StoreError>`, `impl ViewsFile { pub fn view(&self, profile: Uuid, collection: &str) -> CollectionView; pub fn set_view(&mut self, profile: Uuid, collection: &str, view: CollectionView); }`. `CollectionView::default()` has `pinned: vec!["id".into()]`. `pub fn views_path() -> Option<PathBuf>`: `ProjectDirs` data dir plus `document_views.json`.
- `store.rs`: `pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError>`, with `save` calling it; behaviour and tests unchanged.

- [ ] **Step 1: Write the failing tests:**
  - `columns.rs`: `id_comes_first_then_schema_order`; `nested_fields_collapse_to_one_column` (`author` object plus `author.name` give one `author`; `author.name` alone gives `author` too); `wildcards_make_no_column`; `vector_fields_make_no_column_and_are_excluded` (`float[]` with `num_dim` and `float[]` with `embed`; `exclude_fields` returns `"embedding,other"`, and `None` without vectors); `plain_float_array_stays` (no `num_dim`); table `sortable_table`: `string` with `sort:true` yes, `string` with `sort:false` no, `int32` server `sort:true` yes, `bool` yes, `geopoint` with `sort:true` no, `string[]` no, `index:false` no, `id` no; `merge_keys_appends_unknown_and_reports` (`true`, then `false` for the same keys); `sort_key` gives `price:desc`.
  - `page.rs`: `cell_preview_is_one_line_and_short` (string with `\n`, 300 chars cut to 200 plus `…`); `cell_preview_of_object_is_compact_json`; `cell_preview_null_is_empty_marker` (`null` gives `null`); `cell_full_keeps_strings_raw`; `from_documents_builds_ids_cells_and_sorted_keys`; `page_cache_evicts_least_recently_touched` (capacity 3, touch page 1, insert 4, page 2 is gone); `page_cache_never_exceeds_capacity` over 100 inserts at capacity 20; `insert_of_existing_page_replaces_it`; `row_position` (`0→(1,0)`, `249→(1,249)`, `250→(2,0)`); `wanted_pages_covers_visible` (`100..130 → [1]`), `wanted_pages_adds_neighbour_near_boundary` (`230..260 → [1,2]`, `220..240 → [1,2]`, `260..290 → [1,2]`), `wanted_pages_stops_at_found`.
  - `views.rs`: `missing_file_is_default`; `round_trip`; `default_view_pins_id`; `corrupt_file_is_default`.
- [ ] **Step 2: Run** `cargo test -p tessera-core documents`. Expected: compile errors.
- [ ] **Step 3: Implement.** `Row` with `id` from `doc["id"]` (a document without a string id gets `""`, which only a broken server would send). `cell_preview`: string as is with `\n`, `\r`, `\t` → space; other values `to_string()`; cut at `CELL_LIMIT` chars on a char boundary with `…`. `wanted_pages` per §5.2 with a lookahead of 50 rows.
- [ ] **Step 4: Run** the checks. Commit. `feat(core): document columns, page cache and column preferences`.

### Task 4: Documents view with paging and sorting

**Files:**
- Create: `crates/tessera-app/src/documents.rs`, `crates/tessera-app/src/views/documents/mod.rs`, `crates/tessera-app/src/views/documents/table.rs`, `crates/tessera-app/tests/ui_documents.rs`
- Modify: `crates/tessera-app/src/lib.rs` (install the global), `crates/tessera-app/src/views/mod.rs`, `views/dock.rs` (`CollectionPanel`), `views/schema_view.rs` (event), `crates/tessera-app/src/actions.rs`, `crates/tessera-app/tests/common/mod.rs`, `tests/ui_collections.rs`, `tests/ui_schema_dialog.rs`, locales, `crates/tessera-app/Cargo.toml`

**Interfaces:**
- Consumes: Task 1 and Task 3.
- Produces:
  - `documents.rs`: `pub trait DocumentsBackend: Send + Sync { fn page(&self, client: Client, collection: String, query: DocumentQuery) -> BoxFuture<Result<PageResult, Error>>; }` (later tasks add methods), `pub struct PageResult { pub found: u64, pub out_of: u64, pub search_time_ms: u64, pub page: DocumentPage }`, `pub struct DocumentsBackendGlobal(pub Arc<dyn DocumentsBackend>)`, `pub fn tokio_backend(handle: Handle) -> Arc<dyn DocumentsBackend>`. Reuse `BoxFuture` from `crate::collections`, and copy `TokioBackend::run`'s shape. `page` calls `client.without_response_bodies().search_documents(…)` and then `DocumentPage::from_documents` inside the tokio future.
  - `views/schema_view.rs`: `pub struct SchemaLoaded(pub Collection); impl EventEmitter<SchemaLoaded> for SchemaView {}`, emitted in `show` with `loaded.raw.clone()`.
  - `views/documents/table.rs`: `pub struct DocRows` implementing `TableDelegate`.
  - `views/documents/mod.rs`: `pub struct DocumentsView`, `DocumentsView::new(state: Entity<AppState>, connection: Entity<ConnectionState>, name: String, window, cx)`, `pub fn set_schema(&mut self, collection: Collection, window, cx)`, `pub fn reload(&mut self, cx)`, `pub fn found(&self, cx: &App) -> u64`. Tests read requested pages from the fake backend.
  - `CollectionPanel`: `view: CollectionView` (`enum CollectionView { Documents, Schema }`, default `Documents`), `documents: Entity<DocumentsView>`, `pub fn documents(&self)`.
  - Actions: `ShowDocuments`, `ShowSchema`, `SortByColumn`; consts `DOCUMENTS = "Documents"`, `DOCUMENTS_TABLE = "Documents > DataTable"`.
  - Bindings: `cmd-1`/`ctrl-1` → `ShowDocuments` and `cmd-2`/`ctrl-2` → `ShowSchema` in `COLLECTION` (reuse consts `SHOW_FORM`, `SHOW_JSON`; dialogs are outside that context). `SORT = cmd-shift-s / ctrl-shift-s` → `SortByColumn` in `DOCUMENTS`. In `DOCUMENTS`: `KeyBinding::new(EDIT, NoAction, …)`, same for `CLONE`, `TRUNCATE`, `DELETE`.
  - Test ids: `documents.table`, `documents.status`, `documents.error`, `documents.empty`, `documents.row.{id}`, `documents.cell.{id}.{key}`, `collection.view.documents`, `collection.view.schema`.
  - i18n keys: `documents.view`, `documents.schema_view`, `documents.status_count` (`%{found} of %{total}`), `documents.status_time` (`%{ms} ms`), `documents.status_sort` (`sorted by %{field} %{arrow}`), `documents.empty`, `documents.empty_hint`, `documents.loading_row` (`…`).

Behaviour, in `table.rs`:

```rust
pub struct DocRows {
    pub columns: Vec<ColumnDef>,            // visible ones; pinned first
    pub widths: HashMap<String, f32>,
    pub sort: Option<(String, bool)>,       // key, ascending
    pub found: u64, pub out_of: u64, pub search_time_ms: u64,
    pub filter: Option<String>,
    pub exclude: Option<String>,
    pub error: Option<SharedString>,
    pub first_load: bool,
    pub query_error: Option<SharedString>,  // failure of a new filter or sort; shown under the filter
    replacing: bool,
    generation: u64,
    pages: PageCache<Arc<DocumentPage>>,
    visible: Range<usize>,
    inflight: HashMap<u32, Task<()>>,
    throttle: Option<Task<()>>,
    target: Target,                         // client and collection name
}
```

  - `rows_count` returns `found as usize`. `columns_count` is `columns.len() + 1`. Column 0 is `__select`, added in Task 11; until then `columns_count` is `columns.len()`, and Task 11 shifts the index.
  - `column(ix)`: `Column::new(key, key).width(px(width)).min_width(px(60.))`, `.text_right()` for numeric, `.fixed_left()` for pinned, `.sortable()`, `.ascending()` or `.descending()` when `sortable`. Default width 160, `id` 120, numeric 100.
  - `perform_sort(col_ix, sort, window, cx)`: `ColumnSort::Default` clears, `Ascending` and `Descending` set `self.sort`, then `self.requery(cx)`.
  - `requery(cx)`: `generation += 1; inflight.clear(); throttle = None; replacing = true;` and load page 1. Old rows stay on screen until the new query answers: `finish` clears `pages` on the first `Ok` of a generation, and only then resets `replacing`. `first_load` is true only until the very first answer of the view.
  - `visible_rows_changed(range, _, cx)`: stores `visible`; for each page under it `pages.touch(page)`; if `throttle.is_none()` then `throttle = Some(cx.spawn(async move |table, cx| { cx.background_executor().timer(Duration::from_millis(80)).await; table.update(cx, |t, cx| { t.delegate_mut().throttle = None; t.delegate_mut().fetch_wanted(cx) }).ok(); }))`.
  - `fetch_wanted(cx)`: `let wanted = wanted_pages(visible, 250, found)`; `inflight.retain(|p, _| wanted.contains(p))`; each wanted page neither cached nor in flight starts `fetch(page, cx)`, up to 4 in flight.
  - `fetch(page, cx)`: builds `DocumentQuery { page, per_page: 250, filter_by: filter.clone(), sort_by: sort.as_ref().map(|(k, a)| sort_key(k, *a)), exclude_fields: exclude.clone() }`; `let load = cx.global::<DocumentsBackendGlobal>().0.page(client, name, query);`, stores `cx.spawn(async move |table, cx| { let r = load.await; table.update(cx, |t, cx| t.delegate_mut().finish(page, generation, r, cx)).ok(); })`.
  - `finish(page, generation, result, cx)`: returns at once if `generation != self.generation`. `Ok`: clears `pages` when `replacing`, removes it from `inflight`, inserts the page, sets `found`, `out_of`, `search_time_ms`, `first_load = false`, both errors to `None`, runs `merge_keys` (a change calls `cx.notify` after `refresh(cx)`; `DocumentsView` recomputes visible columns through `cx.observe(&table, …)`). `Err`: sets `error_text` into `query_error` while `replacing`, else into `error`; `first_load = false`.
  - `render_td`: looks up `pages.get(page)`, then `rows.get(ix)`, then `cells.get(key)`. Missing row: muted `…`. Absent key: empty. Wrap text in `div().id("documents.cell.{id}.{key}").test_support().aria_label(text.clone()).truncate()`, as `schema_view.rs` does.
  - `cell_label(row, col, cx)` returns the same text, `cell_text` too.
  - `render_tr` gives `.id(SharedString::from(format!("documents.row.{id}")))`, or `("documents.placeholder", row_ix)` for a row not loaded.
  - `loading` returns `first_load`. `render_empty` shows the empty state of Task 7's text keys.

`mod.rs`: builds `TableState::new(rows, window, cx).cell_selectable(true).row_header(false).row_selectable(false).col_selectable(false).col_movable(false)`. `set_schema` sets `columns = columns_from_schema`, `exclude = exclude_fields`, calls `refresh`, and on first call loads page 1. Subscribes to `CollectionChanged(name)` and calls `requery`. The status line reads the delegate. `DataTable::new(&table).bordered(true)` with `.key_context(DOCUMENTS)` on the root and `track_focus`.

`dock.rs` `CollectionPanel::new` creates `documents` as well, subscribes `cx.subscribe_in(&schema, window, |this, _, SchemaLoaded(c), window, cx| this.documents.update(cx, |d, cx| d.set_schema(c.clone(), window, cx)))`. `render` draws `TabBar::new("collection.views").underline().selected_index(ix).child(Tab::new().label(...))` (`gpui-component-0.9.0/src/tab/tab_bar.rs`, `tab.rs`) with a `Kbd` per tab, then the active view. `SchemaView` is kept alive and only not drawn. Its `render` needs no change. `Refresh` routes by `view`: `documents.reload(cx)` or the current schema reload.

- [ ] **Step 1: Fake backend.** In `tests/common/mod.rs`: `pub struct FakeDocs { pub found: Mutex<u64>, pub pages: Mutex<Vec<DocumentQuery>>, pub fail: Mutex<Option<(u16, String)>>, pub hold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>> }`, whose `page` synthesises `{"id": n, "title": "t{n}", "price": n}` for the rows of that page (offset `(page-1)*per_page`, stopping at `found`) and builds through `DocumentPage::from_documents`. `pub fn fake_documents(cx) -> Arc<FakeDocs>`. `open_state_at` installs an empty one by default, because `CollectionPanel` reads the global when it opens.
- [ ] **Step 2: Fix the M2 tests that read the schema.** Add `pub fn show_schema(window, cx)` to `common` (presses `SHOW_SCHEMA`: `key("cmd-2", "ctrl-2")`), and call it in `tests/ui_collections.rs` and `tests/ui_schema_dialog.rs` after opening a collection tab wherever `schema.*` ids are read. `gets == ["books"]` assertions stay true: `DocumentsView` issues no `get`.
- [ ] **Step 3: Write the failing UI tests** in `ui_documents.rs`, with a fake collection `products` (fields `title`, `price`, `found` 200,000):
  - `opens_on_documents_and_shows_found`: `documents.status` contains `200,000`, and `documents.cell.0.title` holds `t0` (the fake's first row has id `0`).
  - `command_two_shows_the_schema_and_one_goes_back`.
  - `scrolling_loads_only_the_pages_it_needs`: `table.update(|t, cx| t.scroll_to_row(100_000, cx))`, `cx.run_until_parked()`, `cx.executor().advance_clock(Duration::from_millis(100))`, `cx.run_until_parked()`; requested pages are `[1, 401]` or `[1, 401, 402]`, nothing else.
  - `long_scroll_keeps_twenty_pages`: 40 jumps of 300 rows, parked between; read `delegate.pages.len()` through a `pub fn cached_pages(&self) -> usize` on `DocRows`; ≤ 20.
  - `continuous_scroll_still_fetches`: 10 scrolls 10 ms apart, without parking the timer between, produce a fetch before the last one (the throttle does not restart).
  - `late_answer_of_an_old_query_is_dropped`: hold the fake's page 1 answer, sort by price, release; the rows shown are those of the sorted query.
  - `rows_stay_until_the_new_query_answers`: hold the answer after a sort; the old rows are still drawn.
  - `header_click_sorts`: click id `icon-sort` for the price column (the kit's id is `("icon-sort", col_ix)`); the last query has `sort_by == "price:desc"`, next click `price:asc`, next `None`.
  - `sort_shortcut_uses_the_selected_cell`: select a price cell with `set_selected_cell`, press `SORT`.
  - `unsortable_column_has_no_icon_and_the_shortcut_does_nothing`.
  - `new_document_key_appears_as_a_column`: fake returns a key outside the schema.
  - `vector_field_is_excluded`: schema with `float[]` and `num_dim`; query has `exclude_fields`.
  - `page_error_shows_in_full`.
  - `collection_changed_reloads`.
- [ ] **Step 4: Run** `cargo test -p tessera-app --test ui_documents`. Expected: FAIL.
- [ ] **Step 5: Implement** as above. Sort-key action: `SortByColumn` reads `TableState::selected_cell()`, then `columns[col]`, ignores unsortable, cycles off → desc → asc → off, then `t.refresh(cx)` and `requery`. Add to `crates/tessera-app/Cargo.toml`: `[features] frame-overlay = ["gpui-kit/profiler"]`. `ToggleFrameOverlay` action with `cmd-alt-f` / `ctrl-alt-f`, bound and handled only under `#[cfg(feature = "frame-overlay")]`: `window.cycle_debug_frame_overlay_mode()` (`gpui-pre-0.3.11/src/window.rs:3884`), registered in `RootView`. `cargo check -p tessera-app --features frame-overlay` must pass.
- [ ] **Step 6: Run** the checks and `cargo xtask seed` data by hand: `cargo run -p tessera-app`, open `products`, scroll, click headers.
- [ ] **Step 7: Docs.** CHANGELOG. `docs/building.md` gets a line on `--features frame-overlay`.
- [ ] **Step 8: Commit.** `feat(app): browse documents with paging and sorting`.

### Task 5: Column picker, pinning and resizing, saved per collection

**Files:**
- Create: `crates/tessera-app/src/views/documents/columns.rs`
- Modify: `views/documents/mod.rs`, `views/documents/table.rs`, `crates/tessera-app/src/state.rs` (holds `ViewsFile`), `crates/tessera-app/src/lib.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_documents.rs`

**Interfaces:**
- Consumes: Task 3 `ViewsFile`, `CollectionView`.
- Produces:
  - `AppState` fields `document_views: ViewsFile`, `views_path: Option<PathBuf>`, set up in `AppState::new`'s caller in `lib.rs` from `views::views_path()` and `views::load`. `pub fn document_view(&self, profile: Uuid, collection: &str) -> CollectionView`. `pub fn set_document_view(&mut self, profile: Uuid, collection: &str, view: CollectionView, cx) -> Task<Result<(), String>>`: updates memory; saves via `cx.background_spawn` unless `profile` is the temporary one, as `set_favorite` does. Tests pass `views_path: None`.
  - Actions `ToggleColumns`, `TogglePinColumn`, `NarrowerColumn`, `WiderColumn` with chords `cmd-shift-o`, `cmd-shift-p`, `cmd-shift-left`, `cmd-shift-right` (and `ctrl-shift-…`), all in `DOCUMENTS`.
  - Test ids: `documents.columns` (button), `documents.column.{key}` (checkbox), `documents.pin.{key}`, `documents.columns.reset`.

Behaviour:

  - `DocumentsView` keeps `all_columns: Vec<ColumnDef>` and `prefs: CollectionView`. `visible_columns()` is pinned ones in `prefs.pinned` order that exist, then the rest of `all_columns` in order, without `hidden`. `DocRows.columns` is that list, reassigned then `refresh(cx)`.
  - `id` can be hidden and unpinned. At least one column stays: hiding the last is refused, with its checkbox disabled.
  - `TableEvent::ColumnWidthsChanged(widths)` arrives through `cx.subscribe_in(&table, …)`: zip with the current columns (skip `__select` from Task 11 on), store into `prefs.widths` and `DocRows.widths`, then a debounced save (300 ms, one `Task` slot replaced on each change, so a drag writes once).
  - Keyboard width: `prefs.widths[key] ± 20`, clamped to 60..1200, then `refresh`.
  - Popover: `gpui_kit::component::popover::Popover::new("documents.columns.popover").open(self.columns_open).on_open_change(…).trigger(Button…).content(…)` (`gpui-component-0.9.0/src/popover.rs`). One row per `all_columns`: `Checkbox::new(id).checked(!hidden).label(key).on_click(…)` (`src/checkbox.rs`), and a small `Button` with a pin icon, `.selected(pinned)`. "Reset" clears the view to the default. The trigger shows its `Kbd`.
  - A failed save sets a status-line message `documents.columns_save_failed` with the error text.

- [ ] **Step 1: Write the failing UI tests:** `hidden_column_is_not_drawn_and_stays_hidden_after_reopen` (hide `title` through its checkbox, close the tab, reopen, assert `documents.cell.0.title` absent); `pinned_column_comes_first`; `pin_shortcut_uses_the_selected_cell`; `resized_width_survives_a_refresh` (dispatch `WiderColumn`, assert `TableState::col_width(ix)` grew, make a page bring a new key, assert the width stayed); `last_visible_column_cannot_be_hidden`; `preferences_are_per_collection`; `temporary_connection_writes_no_file` (state test with a temp path); `failed_save_shows_inline`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks. CHANGELOG. Commit. `feat(app): pick, pin and resize document columns`.

### Task 6: Filter bar with autocompletion

**Files:**
- Create: `crates/tessera-core/src/documents/filter.rs`, `crates/tessera-widgets/src/filter_input.rs`, `crates/tessera-widgets/tests/filter_input.rs`
- Modify: `crates/tessera-core/src/documents/mod.rs`, `crates/tessera-widgets/src/lib.rs`, `crates/tessera-widgets/Cargo.toml`, workspace `Cargo.toml` (`lsp-types = "=0.97.0"`), `crates/tessera-app/src/documents.rs`, `views/documents/mod.rs`, `actions.rs`, locales

**Interfaces:**
- Consumes: `Client::facet_values` (Task 1).
- Produces:
  - Core: `FilterContext`, `context_at`, `operators_for`, `quote_value` as in §5.4.
  - Widget: `FilterField`, `FilterInputEvent`, `FacetFetch`, `FilterInput` as in §6.1. Test id `filter.input`. Constants `FACET_DELAY = 150 ms`, `FACET_LIMIT = 20`.
  - Backend: `fn facet_values(&self, client: Client, collection: String, field: String, prefix: String) -> BoxFuture<Result<Vec<String>, Error>>` on `DocumentsBackend`.
  - App: `FocusSearch` (existing action) bound to `FIND` in `DOCUMENTS`. New action `ClearFilter`, chord `cmd-shift-f` / `ctrl-shift-f` in `DOCUMENTS`, shown on the button `documents.filter.clear`. Esc is not used for it, because Esc closes the completion menu.
  - Test ids: `documents.filter`, `documents.filter.error`, `documents.filter.clear`.

- [ ] **Step 1: Write the failing core tests**, a table `(text, offset) → context`:
  - Cursor at the end of the text in each case. `""` → `Field{start:0,prefix:""}`; `pri` → `Field{prefix:"pri"}`; `price:` → `Operator{field:"price",start:5}`; `price:>` → `Operator{field:"price",start:5}`; `price:>1` → `Value{field:"price",prefix:"1"}`; `genre:=cr` → `Value{field:"genre",prefix:"cr",start:7}`; `genre:[a,b` → `Value{prefix:"b"}`; ``genre:`science fi`` → `Value{prefix:"science fi"}`; `a:1 ` → `Join`; `a:1 && pr` → `Field{prefix:"pr",start:7}`; `(a:1 || b` → `Field{prefix:"b"}`; `a:1 && (b:2)` at the end → `Join`.
  - `operators_for`: `string` has 3, `int32` has 7, `bool` has 2, `string[]` as `string`, `float[]` as numbers.
  - `quote_value`: `crime` unchanged; `science fiction` in backticks; `a,b` in backticks.
- [ ] **Step 2: Run** `cargo test -p tessera-core filter`. Expected: FAIL. **Implement.**
- [ ] **Step 3: Write the failing widget tests** in `tests/filter_input.rs` (`#[gpui_kit::test]`, fields `title` string, `genre` string facet, `year` int32 facet, `active` bool facet; a `FacetFetch` that records calls and returns `["crime","fantasy"]`): `typing_offers_fields`; `after_field_offers_operators`; `facet_value_waits_then_fetches` (no call before 150 ms of `advance_clock`, one after, with `("genre", "cr")`); `bool_offers_true_false_without_fetching`; `plain_field_value_offers_nothing`; `enter_with_open_menu_picks_and_does_not_submit`; `enter_with_closed_menu_submits` (`Submitted("genre:=crime")`); `pick_replaces_only_the_prefix` (`year:>2000 && gen` → pick `genre`); `newline_becomes_space`. Read the menu through `state.completion_menu_state()` (`gpui-base-0.9.0/src/input/editor/lsp/overlay.rs`).
- [ ] **Step 4: Implement `FilterInput`:**

```rust
let state = cx.new(|cx| {
    let mut s = EditorState::new(window, cx)
        .line_number(false).folding(false)
        .searchable(false).submit_on_enter(true)
        .scroll_beyond_last_line(None);
    s.lsp_mut().completion_provider = Some(provider.clone());
    s
});
```

  `Provider` implements `CompletionProvider` from `gpui_kit::component::input` (`gpui-base-0.9.0/src/input/editor/lsp/completions.rs:146`): `is_completion_trigger` returns true; `completions(&self, text: &Rope, offset, _, window, cx) -> Task<anyhow::Result<CompletionResponse>>`. Items are `lsp_types::CompletionItem { label, text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text })), ..Default::default() }` with `range` from `text.offset_to_position(start)` to `text.offset_to_position(offset)` through `RopeExt`. For a value, `cx.spawn` waits `FACET_DELAY` on `cx.background_executor().timer`, then awaits `fetch`. `new_text` for a value is `quote_value(value)`. Render: `Editor::new(&state).h(px(32.)).aria_label(...)`. Subscribe to `InputEvent::Change` (emit `Changed`, strip newlines via `set_value` only when one is present) and `InputEvent::PressEnter` (emit `Submitted(value.trim())`). `gpui_kit::component::input::{Rope, RopeExt, Position, CompletionProvider, Editor, EditorState, InputEvent}` are all re-exported; check `crates/tessera-widgets/src/json_editor.rs` for the import style.
- [ ] **Step 5: UI tests** in `ui_documents.rs`: `enter_applies_the_filter` (query's `filter_by`, status count updates); `clearing_the_filter_reloads`; `bad_filter_shows_the_server_message_and_keeps_rows`; `focus_shortcut_moves_to_the_filter`; `facet_values_come_from_the_backend`.
- [ ] **Step 6: Wire into `DocumentsView`.** `fields` from `set_schema`: indexed fields with dotted names, plus `id` as `string`, no wildcards. The `FacetFetch` closure calls `cx.global::<DocumentsBackendGlobal>().0.facet_values(…)`, and a failed fetch returns an empty list, because autocompletion must not surface errors. `Submitted` sets `DocRows.filter` (`None` when empty) and calls `requery`. `DocRows.query_error` renders as `documents.filter.error` under the bar, in full, and the old rows draw dimmed while it is set.
- [ ] **Step 7: Run** the checks, measure the release binary size once (`cargo build --release -p tessera-app`; note it). CHANGELOG. Commit. `feat(app): filter documents with autocompletion`.

### Task 7: Read-only inspector, go to ID, copy cell and row

**Files:**
- Create: `crates/tessera-app/src/views/documents/inspector.rs`, `crates/tessera-app/src/views/documents/copy.rs`
- Modify: `crates/tessera-app/src/documents.rs`, `views/documents/mod.rs`, `views/documents/table.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_documents.rs`

**Interfaces:**
- Produces:
  - Backend: `fn document(&self, client: Client, collection: String, id: String) -> BoxFuture<Result<Value, Error>>`.
  - `pub struct Inspector` (entity): `Inspector::new(connection, name, window, cx)`, `pub fn show(&mut self, id: String, window, cx)` loads and fills, `pub fn is_dirty(&self, cx: &App) -> bool` (false in this task), `pub fn current_id(&self) -> Option<&str>`. Holds `editor: Entity<JsonEditor>` (read-only here), `base: Option<Value>`, `error: Option<SharedString>`, `task: Option<Task<()>>`.
  - Actions and chords, in `DOCUMENTS` unless noted: `ToggleInspector` `cmd-i`; `OpenDocument` `enter` in `DOCUMENTS_TABLE`; `GoToId` `cmd-g`; `CopyCell` `cmd-c` in `DOCUMENTS_TABLE`; `CopyRowJson` `cmd-alt-c`. Linux and Windows: `ctrl-i`, `ctrl-g`, `ctrl-c`, `ctrl-alt-c`.
  - `copy.rs`: `pub fn copy_cell(view, cx)`, `pub fn copy_row(view, window, cx)`.
  - Test ids: `documents.inspector`, `documents.inspector.error`, `documents.goto`, `documents.goto.error`, `documents.copy` (dropdown), `documents.copy.cell`, `documents.copy.row`, `documents.toggle_inspector`.
  - i18n: `documents.inspector.title`, `.empty` ("Select a row"), `.loading`, `documents.goto.placeholder`, `documents.copy.cell`, `.row`, `.copied`.

Behaviour: the view root becomes `h_resizable("documents.split").child(resizable_panel().child(table)).child(resizable_panel().visible(self.inspector_open).size(px(420.)).child(inspector))` (pattern in `views/request_log.rs:617`; `visible` at `gpui-base-0.9.0/src/resizable/panel.rs:276`). `TableEvent::SelectCell(row, _)` while open: if the row has a loaded id different from `current_id` and not dirty, wait 120 ms (one `Task` slot) and `show(id)`. `TableEvent::DoubleClickedCell` and `OpenDocument` open it and show. `GoToId` focuses an `InputState` in the toolbar, whose `PressEnter` calls the same `show` and opens the panel; a 404 goes to `documents.goto.error`, and a success clears it. Copy cell writes `cell_full(row.doc[key])` with `ClipboardItem::new_string`; an absent key copies an empty string; an unloaded row does nothing. Copy row calls `document()` and writes pretty JSON; an error goes to `documents.error`. The Copy button is a `DropdownMenu` (see `views/overview.rs:109` for the `PopupMenuItem` pattern), and each item carries its binding through the menu's action form `menu.menu(label, Box::new(Action))` (as `collections_list.rs` `menu_entries`), so GPUI shows the shortcut. The table's `context_menu` uses the same list. The empty-state text keys from Task 4 now also get the docs link (`https://typesense.org/docs/30.2/api/documents.html`) and, on a local connection, the `cargo xtask seed` hint (mirror `collections_list.rs`'s empty state).

- [ ] **Step 1: Write the failing UI tests:** `enter_opens_the_inspector_with_the_full_document` (the fake's `document` includes a vector field the page lacks; the editor text shows it); `inspector_follows_the_selected_row`; `goto_shows_the_document`; `goto_404_shows_inline`; `copy_cell_copies_the_full_value` (a 500-character string, while the cell preview is cut); `copy_row_is_pretty_json`; `copy_on_unloaded_row_does_nothing`; `shortcuts_are_shown` (`documents.toggle_inspector` and each toolbar control's aria text or a `Kbd` child, as M2's tests do).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, then by hand against 18108: open `products`, ⌘I, arrow through rows. CHANGELOG. Commit. `feat(app): inspect documents, go to an id, copy cells and rows`.
- [ ] **Step 5: Open PR A.** Description records the Linux binary size. Merge before Task 8.

---

## PR B · Edit (branch `feat/m3-edit`, from `main` after PR A)

### Task 8: Document writes, merge patch, validation, template

**Files:**
- Create: `crates/tessera-core/src/documents/edit.rs`, `validate.rs`, `template.rs`
- Modify: `crates/tessera-client/src/documents.rs`, `crates/tessera-core/src/documents/mod.rs`
- Test: `crates/tessera-client/tests/documents.rs`, `tests/live.rs`, inline tests in core

**Interfaces:**
- Produces in the client:
  - `pub async fn create_document(&self, collection: &str, document: &Value) -> Result<Value, Error>`: `POST …/documents`, decodes the 201 body.
  - `pub async fn upsert_document(&self, collection: &str, document: &Value) -> Result<Value, Error>`: same with `action=upsert`.
  - `pub async fn patch_document(&self, collection: &str, id: &str, patch: &Value) -> Result<Value, Error>`.
  - `pub async fn delete_document(&self, collection: &str, id: &str) -> Result<(), Error>`.
  - All through `self.write(None, method, path, query, body, None)`.
- Produces in core: `merge_patch`, `DiffEntry`, `diff_entries`, `template`, `DocIssue`, `IssueSeverity`, `validate_document`, `key_position` as in §5.5. `DocIssue.key` is one of `document.missing_required`, `document.wrong_type`, `document.wrong_length`, `document.id_not_string`, `document.not_in_schema`. `detail` holds the expected type or length.

- [ ] **Step 1: Client tests.** wiremock: method, path, body, decoding, `action=upsert`; read-only client refuses all four before sending (`Error::ReadOnly`). Live (`#[ignore]`, 18108): create → duplicate 409 → PATCH merges nested (`{"author":{"age":4}}` keeps `author.name`) → PATCH `{"author":{"age":null}}` removes it → PATCH `{"title":null}` on a required field is 400 → upsert replaces → upsert without a required field is 400 → delete → delete again is 404.
- [ ] **Step 2: Core tests:**
  - `merge_patch`: changed scalar; added key; removed key gives `null`; nested change only sends the nested key; nested removal gives `{"a":{"b":null}}`; a whole object removed gives `{"a":null}`; arrays are whole; equal documents give `{}`; `id` is never included; object replaced by scalar sends the scalar.
  - `diff_entries`: dotted paths, sorted; added has `old: None`; removed has `new: None`; equal gives empty.
  - `template`: required `string`, `int32`, `int64`, `float`, `bool`, `string[]`, `object`, `geopoint`; `optional` skipped; `embed` skipped; `id` skipped; wildcard skipped; nested `a.b` builds an object; result validates with no `Error`.
  - `validate_document`: missing required; `optional` missing is fine; `int32` with `"x"` is Error; with `"12"` is Warning; `bool` with `"true"` is Error; `int32` out of range is Error; `float` accepts an integer; `string[]` with a number inside is Error; `geopoint` needs two numbers; vector length against `num_dim`; `id: 5` is Error; key outside the schema is Info; none when the schema has a `.*` field; `auto` and `string*` accept anything; nested `author.age` found through `author`; field `embed` not required.
  - `key_position`: top-level key; nested `a.b`; key text inside a string value does not match; escaped quotes; a key after a nested object; multi-line positions; unknown path gives `None`.
- [ ] **Step 3: Run** `cargo test -p tessera-client --test documents && cargo test -p tessera-core documents`. Expected: FAIL.
- [ ] **Step 4: Implement.** `key_position` is one pass over `char_indices` tracking line, column, in-string, escape, and a stack of `Frame { key: Option<String>, is_object: bool }`; on a string followed by optional whitespace and `:` at object level, compare the joined path.
- [ ] **Step 5: Run** the checks. CHANGELOG entry at Task 9. Commit. `feat: document writes, merge patch and validation`.

### Task 9: Editing in the inspector, review, new document

**Files:**
- Create: `crates/tessera-app/src/views/documents/review.rs`
- Modify: `crates/tessera-app/src/documents.rs`, `views/documents/inspector.rs`, `views/documents/mod.rs`, `crates/tessera-widgets/src/json_editor.rs`, `crates/tessera-widgets/src/lib.rs`, `actions.rs`, locales, `tests/common/mod.rs`
- Test: `crates/tessera-app/tests/ui_documents.rs`, `crates/tessera-widgets/tests/json_editor.rs`

**Interfaces:**
- Consumes: Task 8.
- Produces:
  - `JsonEditor::set_marks`, `JsonMark`, `MarkSeverity { Error, Warning, Info }` as in §6.2; a `set_read_only(&mut self, bool, cx)` on `JsonEditor`.
  - Backend methods `create`, `upsert`, `patch`, each `(client, collection, …) -> BoxFuture<Result<Value, Error>>`.
  - `enum SaveMode { Patch, Upsert }`, `enum InspectorMode { Existing, New }` in `inspector.rs`. `pub fn new_document(&mut self, template: Value, window, cx)`.
  - Actions: `NewDocument` `cmd-shift-n` in `DOCUMENTS`; `ReviewDocument` `cmd-enter` in `INSPECTOR`; `ToggleSaveMode` `cmd-shift-m` in `INSPECTOR`; `RevertDocument` `cmd-shift-r` in `INSPECTOR`; `ApplyDocument` `cmd-enter` in `DOCUMENT_REVIEW` (`"DocumentReview"`). Linux and Windows `ctrl-…`.
  - Test ids: `documents.new`, `inspector.save_mode.patch`, `inspector.save_mode.upsert`, `inspector.review`, `inspector.revert`, `inspector.dirty`, `inspector.issues`, `inspector.save_error`, `review.diff.{path}`, `review.body`, `review.back`, `review.apply`, `review.error`, `review.stale`, `review.target`.

Behaviour, per §7.4 and §7.5:

  - `is_dirty`: text parses to something other than `base`, or does not parse and differs from the last loaded text.
  - On every `JsonEditorEvent::Changed`: parse; `validate_document(&schema, &doc)`; map with `key_position` to `JsonMark`, with `t!(issue.key, detail = issue.detail)`; list the issues under the editor in `inspector.issues`. A 150 ms debounce in one `Task` slot keeps typing light.
  - `review_disabled_reason() -> Option<SharedString>`: parse error; not an object; `doc["id"] != base["id"]`; `diff_entries` empty. The button is disabled and the reason shows beside it.
  - `ReviewDocument`: `connection.ensure_writable(window, cx, then)`; `then` fetches `document(id)`. `fresh != base` sets `stale: true` and uses `fresh` as the base of the diff. Opens `review::open(ReviewData { collection, id, mode, entries, body, stale, on_apply }, window, cx)`.
  - `review::open` uses `window.open_dialog` as `crates/tessera-widgets/src/confirm.rs` does, with `first_build` focusing Back. `.on_ok` runs Apply only when the Apply button is focused, or by the `ApplyDocument` action. Body in a read-only `JsonEditor`. Consequence line for upsert: `documents.review.upsert_consequence`. Apply disables itself while running and stays open on `Err`, with `review.error`.
  - On success: `base = response`, editor text = pretty response, `DocumentsView::reload` (drops pages, keeps scroll), `connection.collection_changed(name, cx)` is called for create only (the count changed).
  - Toggling the mode changes only `SaveMode`. Read-only profile: the editor stays editable, and Review shows the lock.
  - Dirty close: `ToggleInspector` while dirty opens `tessera_widgets::confirm::open` with focus on Cancel. Revert restores `base`.
  - New document: `template(&schema)` pretty, `InspectorMode::New`, header `documents.inspector.new`, the Review button reads Create, Save mode hidden. Create is `create_document` directly, no review. On `Ok`, mode becomes `Existing`. On `Err`, `inspector.save_error` shows the message in full, the draft stays.

- [ ] **Step 1: Write the failing tests:**
  - Widget: `marks_reach_the_diagnostics` (`state.diagnostics()` length).
  - UI: `edit_then_review_shows_the_diff_and_patch_body`; `apply_sends_the_merge_patch` (fake records `("patch", id, body)`); `upsert_sends_the_whole_document`; `review_focus_starts_on_back`; `escape_returns_without_sending`; `server_error_stays_in_the_review`; `unchanged_document_disables_review`; `changed_id_disables_review`; `parse_error_disables_review`; `stale_document_warns_and_diffs_against_fresh`; `locked_profile_opens_unlock_and_sends_nothing`, then after typing the name it continues; `wrong_type_is_marked_but_does_not_block`; `dirty_inspector_does_not_follow`; `closing_dirty_asks`; `revert_restores`; `new_document_uses_the_template`; `create_409_keeps_the_draft_inline`; `create_reloads_the_count`; `vector_survives_a_patch_edit` (a field the table lacks is not in the patch body).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: By hand** on 18108: edit a `products` document with PATCH and with upsert, and watch the request log's bodies. CHANGELOG: edit via PATCH or upsert with a diff preview, schema marks, new document from a template.
- [ ] **Step 5: Run** the checks. Commit. `feat(app): edit and create documents with a diff preview`. Open PR B, merge before Task 10.

---

## PR C · Bulk (branch `feat/m3-bulk`, from `main` after PR B)

### Task 10: Bulk endpoints, id batches, CSV and curl

**Files:**
- Create: `crates/tessera-core/src/documents/ids.rs`, `export.rs`
- Modify: `crates/tessera-client/src/documents.rs`, `crates/tessera-core/src/documents/mod.rs`, `crates/tessera-core/src/curl.rs`
- Test: `crates/tessera-client/tests/documents.rs`, `tests/live.rs`, inline tests

**Interfaces:**
- Produces in the client:
  - `pub async fn delete_by_filter(&self, collection: &str, filter_by: &str) -> Result<u64, Error>`: `DELETE …/documents?filter_by=&batch_size=1000`, `LONG_TIMEOUT`, reads `num_deleted`.
  - `pub async fn update_by_filter(&self, collection: &str, filter_by: &str, partial: &Value) -> Result<u64, Error>`: `PATCH …/documents?filter_by=`, `LONG_TIMEOUT`, reads `num_updated`.
- Produces in core: `IdBatches`, `id_batches`, `to_jsonl`, `to_csv`, `Danger { Medium, High }`, `danger_level` as in §5.6, and `curl::request(node: &Url, method: &Method, path: &str, query: Option<&str>, body: Option<&str>) -> String`, with `command` delegating to it.

- [ ] **Step 1: Tests:**
  - wiremock: query and decoding of both; `filter_by` required at the type level; long timeout on both; read-only client refuses both.
  - Live: delete by filter counts; update by filter counts; a bad value answers `0`, status 200; an unknown field in the filter is 400; a filter of 2,000 ids (over 4,000 bytes) passes the delete.
  - Core: `id_batches` splits so every filter's percent-encoded length is ≤ `max_bytes` (use `percent_encoding` as the client does; if `tessera-core` lacks it, add `percent-encoding.workspace = true`); keeps all ids exactly once; quotes with backticks; backtick ids go to `single`; empty input gives none. `to_jsonl`. `to_csv`: comma, quote, newline, absent key, nested compact JSON, number, bool, header order. `danger_level`: `(1000,false)` Medium, `(1001,false)` High, `(1,true)` High.
  - Curl: all existing snapshots unchanged; `request` for a GET has `$TYPESENSE_API_KEY`, and a body with `api_key` is masked.
- [ ] **Step 2: Run.** Expected: FAIL. **Implement.**
- [ ] **Step 3: Run** the checks. Commit. `feat: bulk document endpoints, id batches and CSV`.

### Task 11: Row selection and delete

**Files:**
- Create: `crates/tessera-app/src/views/documents/selection.rs`
- Modify: `crates/tessera-app/src/documents.rs`, `views/documents/table.rs`, `views/documents/mod.rs`, `views/documents/columns.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_documents.rs`

**Interfaces:**
- Consumes: Task 8 `delete_document`, Task 10 `delete_by_filter`, `id_batches`, `danger_level`.
- Produces:
  - Backend: `delete(client, collection, id) -> BoxFuture<Result<(), Error>>`, `delete_by_filter(client, collection, filter) -> BoxFuture<Result<u64, Error>>`.
  - `pub struct Selection { ids: BTreeSet<String>, anchor: Option<usize> }` with `toggle(&mut self, id: &str, row: usize) -> bool`, `extend(&mut self, row: usize, ids_of: impl Fn(usize) -> Option<String>)`, `add_all(...)`, `contains`, `len`, `clear`, `ids(&self) -> Vec<String>`. `toggle` and the others refuse beyond `MAX_SELECTION` and return that.
  - `DocRows` gains `selection: Selection`, and column 0 `__select`: `Column::new("__select", "").width(px(36.)).fixed_left().selectable(false).resizable(false).p_0()`, so every `col_ix` from Task 4 and 5 shifts by one. `ColumnWidthsChanged` skips index 0.
  - Actions: `ToggleRow` `space` in `DOCUMENTS_TABLE`, `ExtendRows` `shift-space`, `SelectLoaded` `cmd-a` / `ctrl-a`, `DeleteDocuments` on `DELETE` in `DOCUMENTS` (replacing the `NoAction` for that chord from Task 4). Space stays text in `Input`, because `DOCUMENTS_TABLE` only matches the table.
  - Test ids: `documents.check.{id}`, `documents.delete`, `documents.selected`, `documents.selection_limit`.
  - i18n: `documents.delete.title_one`, `.title_many`, `.consequence_one` ("Deletes document %{id} from %{collection}."), `.consequence_many` ("Deletes %{count} documents from %{collection}."), `.action`, `.partial` ("Deleted %{done} of %{count}: %{error}"), `documents.selected` ("%{count} selected"), `documents.selection_limit` ("At most %{max} rows can be selected").

Behaviour, per §7.7:

  - The checkbox in `render_td(row, 0)`: `Checkbox::new(SharedString::from(format!("documents.check.{id}"))).checked(selected)`, inside a wrapping `div().on_click(cx.listener(…))` that calls `cx.stop_propagation()`, reads `event.modifiers().shift`, and toggles or extends. Selected rows get a background in `render_tr`. An unloaded row draws nothing there.
  - `requery` (Task 4) calls `selection.clear()`, and so does a successful delete. A `reload` (⌘R) does not, because the query is the same.
  - Delete target: `selection.ids()`, else the id of the selected cell's row. Nothing to delete: do nothing, and the button is disabled.
  - `ensure_writable`, then `danger_level(count, prod)`. `Medium`: `tessera_widgets::confirm::open(title, body, label, …)`, whose focus starts on Cancel. `High`: `confirm_danger::open(DangerOptions { title, consequences, expected: collection name, action_label, prod }, …)`.
  - Run: one id goes through `delete`, several through `id_batches(ids, ID_FILTER_BYTES)`: each `filters` entry with `delete_by_filter`, in sequence, then each `single` with `delete`. On the first `Err` stop: show `documents.delete.partial` in the dialog (`Err` of the `Task<Result<(), SharedString>>`) and reload. A 404 on a single id counts as done.
  - After: `connection.collection_changed(name, cx)`, `reload`, and if the inspector shows a deleted id, close it.

- [ ] **Step 1: Write the failing tests:** `checkbox_toggles_and_does_not_select_the_cell`; `shift_click_selects_the_range`; `space_toggles_the_selected_cell_row`; `command_a_selects_loaded_rows`; `selection_cap_stops_at_one_thousand_with_a_note` (the fake has 2,000 rows on loaded pages); `selection_survives_scrolling_out_of_the_cache`; `new_filter_clears_the_selection`; `delete_key_in_documents_does_not_open_collection_delete`: press `DELETE` with a selected row, and assert the dialog text is the document's and that the collections backend got no `delete`; `single_delete_uses_the_document_endpoint`; `multi_delete_builds_one_id_filter`; `long_selection_is_batched` (1,000 ids of 32 characters give several filters); `backtick_id_goes_alone`; `partial_failure_reports_and_reloads`; `prod_needs_the_collection_name` and default focus is the text field; `locked_profile_opens_unlock_first`; `read_only_sends_nothing`; `deleted_row_closes_the_inspector`; `count_refreshes_after_delete`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** Update Task 5's `ColumnWidthsChanged` zip and Task 4's cell lookups for the shifted index.
- [ ] **Step 4: Run** the checks. CHANGELOG. Commit. `feat(app): select rows and delete documents`.

### Task 12: Delete and update by query

**Files:**
- Create: `crates/tessera-app/src/views/documents/bulk.rs`
- Modify: `crates/tessera-app/src/documents.rs`, `views/documents/mod.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_documents.rs`

**Interfaces:**
- Consumes: `FilterInput` (Task 6), `JsonEditor`, Task 10.
- Produces:
  - Backend: `count(client, collection, filter: Option<String>) -> BoxFuture<Result<u64, Error>>`, `update_by_filter(client, collection, filter, partial: Value) -> BoxFuture<Result<u64, Error>>`.
  - `pub enum BulkMode { Delete, Update }`, `pub fn open(view: WeakEntity<DocumentsView>, mode: BulkMode, filter: String, window, cx)`.
  - Actions: `DeleteByQuery` `cmd-shift-k`, `UpdateByQuery` `cmd-shift-u` in `DOCUMENTS` (`ctrl-shift-…`), `BulkContinue` `cmd-enter` in `BULK_DIALOG` (`"BulkDialog"`).
  - Test ids: `documents.delete_query`, `documents.update_query`, `bulk.filter`, `bulk.count`, `bulk.error`, `bulk.partial`, `bulk.issues`, `bulk.continue`, `bulk.result`.
  - i18n: `documents.bulk.delete_title`, `.update_title`, `.affects` ("Affects %{count} documents"), `.counting`, `.no_filter_delete` ("Enter a filter. To delete everything, use Truncate."), `.no_filter_update`, `.no_match`, `.partial_id` ("`id` cannot be set by a query"), `.consequence_delete` ("Deletes %{count} documents from %{collection} matching: %{filter}"), `.consequence_update`, `.deleted`, `.updated`, `.update_mismatch` ("Updated %{done} of %{count} documents. The server skips documents whose new values fail validation and reports no error, so check the field types.").

Behaviour, per §7.8: the dialog opens through `window.open_dialog`, focus in the filter. `FilterInputEvent::Changed` restarts a 300 ms timer in one `Task` slot, then `count`. Showing the result replaces the older one, and dropping the slot cancels the running count. An `Err` shows the server text in `bulk.error` and disables Continue. Update mode has a `JsonEditor` starting at `{}`: Continue is disabled when it does not parse, is not an object, is empty, or has a top-level `id`. `validate_document` marks are filtered to the keys present, by dropping `document.missing_required`. `BulkContinue` runs `ensure_writable`, counts once more, and asks by `danger_level(count, prod)`. On `High` the `ConfirmDanger` consequences hold count and filter, and its `Task` runs the bulk call and resolves `Err` with the `long_error_text` message. On success the dialog closes with `bulk.result` shown in the view's status line for delete. For update, `num_updated != count` keeps the dialog open with `bulk.partial`, `Ok` closes it otherwise. Afterwards `collection_changed` for delete, `DocumentsView::reload` for both, and the selection clears.

- [ ] **Step 1: Write the failing tests:** `delete_query_starts_with_the_table_filter`; `count_shows_after_the_debounce` (no call at 200 ms, one at 300 ms); `newer_filter_replaces_a_slow_count`; `empty_filter_disables_continue`; `zero_matches_disables_continue`; `bad_filter_shows_the_server_message`; `continue_counts_again`; `at_one_thousand_uses_the_simple_confirm`; `above_one_thousand_needs_the_name`; `prod_needs_the_name`; `type_to_confirm_is_exact`; `delete_reports_the_count_and_reloads`; `update_sends_filter_and_partial`; `update_with_id_is_refused`; `update_mismatch_stays_open_with_the_warning` (count 5, fake answers 0); `update_invalid_json_disables_continue`; `locked_profile_unlocks_first_and_sends_nothing_before`; `timeout_uses_the_long_text`; `dialog_buttons_show_their_shortcuts`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: By hand** on 18108: `category:=ocean` on `products` above 1,000 documents needs the name; then `cargo xtask seed --url http://localhost:18108 --reset`.
- [ ] **Step 5: Run** the checks. CHANGELOG. Commit. `feat(app): delete and update documents by query`.

### Task 13: Copy selection, curl, and NF2 sign-off

**Files:**
- Modify: `crates/tessera-app/src/documents.rs`, `views/documents/copy.rs`, `views/documents/mod.rs`, `views/documents/table.rs`, `actions.rs`, locales, `docs/spec.md` only if a measurement changes a decision, `CHANGELOG.md`
- Test: `crates/tessera-app/tests/ui_documents.rs`

**Interfaces:**
- Consumes: `id_batches`, `to_jsonl`, `to_csv`, `curl::request`, `Client::export_documents`.
- Produces:
  - Backend: `export(client, collection, filter: String) -> BoxFuture<Result<String, Error>>`.
  - Actions: `CopyJsonl` `cmd-alt-j`, `CopyCsv` `cmd-alt-s`, `CopyCurl` `cmd-alt-u` in `DOCUMENTS` (`ctrl-alt-…`).
  - `copy.rs`: `pub fn copy_selection(view, format: CopyFormat, window, cx)` with `enum CopyFormat { Jsonl, Csv }`, `pub fn copy_curl(view, cx)`.
  - Test ids: `documents.copy.jsonl`, `documents.copy.csv`, `documents.copy.curl`, `documents.copy.error`.

Behaviour, per §7.6: `copy_selection` takes `selection.ids()`, or the selected cell's row. `id_batches(ids, ID_FILTER_BYTES)`; `export` per filter, `document` per `single`, in sequence. JSONL writes the export text as it came, joined with `\n`. For CSV, parsing the lines and `to_csv` run in `cx.background_spawn`, and the result goes to the clipboard on the GPUI thread. CSV uses `DocRows.columns` keys, in table order, without `__select`. Empty selection and no selected cell: the entries are disabled. Errors go to `documents.copy.error`. `copy_curl`: with a selected row, `curl::request(node, GET, "/collections/{c}/documents/{id}", None, None)`; without one, the current search with its `q=*`, filter, sort and `per_page=250`, using `client.base_url()`. Paths are percent-encoded like the client's: expose `tessera_client::collection_path` as `pub` with `pub fn document_path(collection, id)`. The copy dropdown and the context menu gain the three entries, disabled when they have no target.

- [ ] **Step 1: Write the failing tests:** `jsonl_copies_the_selection`; `csv_uses_the_visible_columns_in_order`; `csv_quotes_values`; `selection_is_batched_for_export`; `backtick_id_uses_the_document_endpoint`; `copy_error_shows_inline`; `curl_for_a_row_has_the_placeholder_key`; `curl_without_a_row_repeats_the_search`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks.
- [ ] **Step 5: Measure NF2** by the steps of §12 of the design, on M1 hardware, against `http://localhost:18108`. This step needs a person at the machine, or a run driving the real app; an implementer without one stops here and says so in the PR instead of claiming the result. Record draw time p95 and RSS after a 5-minute scroll in the PR description. If p95 exceeds 16.6 ms, profile before merging: first suspects are `render_td`'s allocations and the number of visible columns.
- [ ] **Step 6: Docs.** CHANGELOG entries for the copy formats. The §14 M3 row in `docs/spec.md` stays linked. Add the `products` scroll and the bulk dialogs to the QA notes in the PR.
- [ ] **Step 7: Commit.** `feat(app): copy selections as JSONL, CSV and curl`. Open PR C.
