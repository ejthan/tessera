# M3 Document browser: design

- **Date:** 2026-11-05
- **Status:** written autonomously (maintainer takes the recommendations), pending final review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M3 row in §14; §7.4, §5, §8, §10 NF2, §12, §16)
- **Previous milestone:** [M2 collections design](2026-11-05-m2-collections-design.md)
- **UX mockups:** [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d), screen 1 (main)

All behaviour marked "confirmed" was checked on 2026-11-05 with curl against Typesense 30.2 on `localhost:18108`, on throwaway collections that are gone again. Every GPUI Kit API named here was read in the pinned source (`gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-pre-0.3.11`).

## 1. Goal

M3 opens the documents of a collection: a table over 200,000 rows that scrolls smoothly and holds only 20 pages in memory, a filter bar with autocompletion, an inspector with a JSON editor, single and bulk edits with previews, and copying in the formats people paste elsewhere.

**Acceptance (product spec §14, §7.4, NF2):**

- NF2: the seeded 200k `products` collection scrolls at ≥ 60 fps on M1 or comparable hardware, read from the frame overlay (D29).
- Memory stays bounded: at most 20 pages of 250 documents are held, and scrolling does not grow the request log (D24).
- Integration tests for every new endpoint are green against Typesense 30.0 and 30.2.
- In a read-only profile no document write leaves the client until the user unlocks it.
- Delete by query above 1,000 documents, or on a prod profile, needs the collection name typed.

## 2. Delivery

One spec, one plan, three pull requests in this order, each with green CI and its own CHANGELOG entries.

| PR | Branch | Content | Why this order |
|---|---|---|---|
| **A · Browse** | `feat/m3-documents` | Read endpoints, 200k seed, column model, page cache, documents view with sorting, column picker, filter bar, read-only inspector, go to ID, copy cell and row, frame overlay | Everything that cannot damage data, and the part NF2 depends on |
| **B · Edit** | `feat/m3-edit` | Single-document writes, merge patch, schema validation marks, review dialog, new document from template | The first document writes, one at a time |
| **C · Bulk** | `feat/m3-bulk` | Row selection, single and multi delete, delete and update by query, copy selection (JSONL, CSV) and curl, NF2 measurement | Builds on B's review and on the selection model |

## 3. Scope

### In M3

Product spec §7.4 except the card view: everything the M3 brief lists, plus the frame overlay needed to measure NF2 and the `products` seed set.

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Card view | v1.x | §14 lists it there |
| Text search (`q`) in the browser | M4 | The search workbench owns `q` and `query_by`; the browser is `q=*` as §7.4 says |
| Sorting by a nested field such as `author.age` | v1.x | Columns are top-level keys (§5.1). A nested sort needs a sort picker, not a header. `sort_by` on such a field is confirmed to work, so nothing blocks it later |
| Filter builder with form rows | M4 | §7.5 needs it; M3's filter bar is text with completion |
| Export to a file | M5 | Copy covers the selection; a whole collection goes through the jobs panel |
| Reordering columns | v1.x | §7.4 asks for pin, resize, show and hide. `TableState::col_movable(false)` keeps it off |
| Inline cell editing | not planned | The inspector is the one edit path, so the diff preview cannot be bypassed |
| Configurable confirmation thresholds (§7.21) | settings milestone | M3 uses the constant from §4.3 |
| Long filters over `POST /multi_search` | M4 | D30 |

## 4. tessera-client

### 4.1 Models

New module `crates/tessera-client/src/documents.rs`. Documents stay `serde_json` values (D5).

```rust
pub struct DocumentQuery {
    pub page: u32,                     // 1-based
    pub per_page: u32,                 // at most 250; 251 answers 422
    pub filter_by: Option<String>,
    pub sort_by: Option<String>,       // "price:desc"
    pub exclude_fields: Option<String>,
}

pub struct SearchPage {
    pub found: u64,
    pub out_of: u64,
    pub page: u32,
    pub search_time_ms: u64,
    pub documents: Vec<Map<String, Value>>, // hits[].document
}
```

### 4.2 Endpoints

| Method | Endpoint | Confirmed behaviour |
|---|---|---|
| `search_documents(c, &DocumentQuery)` | `GET /collections/{c}/documents/search?q=*&…` | No `query_by` needed for `q=*`. A page past the end answers 200 with no hits. Bad filter: 400 with message. Sorting by a non-sortable field: 404 `Could not find a field named … for sorting` |
| `count_documents(c, filter)` | same with `per_page=0` | `found` is the count |
| `facet_values(c, field, prefix, limit)` | same with `per_page=0&facet_by=f&facet_query=f:prefix&max_facet_values=limit` | Works for string, numeric and nested facet fields. A field that is not a facet answers 404 |
| `document(c, id)` | `GET …/documents/{id}` | 404 `Could not find a document with id: x` |
| `export_documents(c, filter)` | `GET …/documents/export?filter_by=` | JSONL. No 4,000-byte limit on this query (9 KB tested). Long timeout |
| `create_document(c, &Value)` | `POST …/documents` | 201 with the stored document. 409 on a duplicate id. Keys outside the schema are stored |
| `upsert_document(c, &Value)` | `POST …/documents?action=upsert` | Replaces the whole document; a missing required field answers 400 |
| `patch_document(c, id, &Value)` | `PATCH …/documents/{id}` | Deep merge, see §4.4. An `id` in the body that differs answers 400 |
| `delete_document(c, id)` | `DELETE …/documents/{id}` | 404 when gone |
| `delete_by_filter(c, filter)` | `DELETE …/documents?filter_by=&batch_size=1000` | `{"num_deleted":n}`. Without `filter_by`: 400. No 4,000-byte limit (205 KB tested). 12,513 documents took 1.4 s. Long timeout |
| `update_by_filter(c, filter, &Value)` | `PATCH …/documents?filter_by=` | `{"num_updated":n}`, see the trap in §4.4. Long timeout |

- The collection and the document id are each one percent-encoded path segment through `collection_path`. An id of `a/b c?x` is confirmed to work encoded.
- Query values are percent-encoded with the same `SEGMENT` set, because `Url::set_query` leaves `&` and `+` as they are. A helper `encode_query(&[(&str, &str)]) -> String` does this for all of the above.
- The read-only guard, failover rules and request events of M1 apply unchanged. POST and PATCH are not retried. DELETE by filter fails over as M2's deletes do.

### 4.3 Numbers the app uses

| Constant | Value | Why |
|---|---|---|
| `PER_PAGE` | 250 | Server maximum and §7.4 |
| `PAGE_CACHE` | 20 pages | §7.4 |
| `CONFIRM_ABOVE` | 1,000 | §7.4 |
| `MAX_SELECTION` | 1,000 rows | D26 |
| `ID_FILTER_BYTES` | 7,500 | Below the 8 KB URL limit of common proxies (D26) |

Measured on a 200k collection with 11 fields: page 1 takes 11 ms, page 800 (the last) 55 ms, sorted by price 69 ms, filtered 40 ms, and a page of 250 documents is 113 KB. Deep paging has no server limit: all 800 pages load. Facet and count queries take 7–25 ms.

### 4.4 Traps found while probing

- **A PATCH is a deep JSON merge.** `{"author":{"age":4}}` keeps `author.name`. `null` removes a key at any depth (`{"author":{"age":null}}`), and a `null` on a required field answers 400. Arrays are replaced whole. So the PATCH body is an RFC 7386 merge patch (§5.5), and removals need no upsert.
- **Update by query answers 200 when it did nothing.** A body that fails validation (`{"year":"x"}` on an `int32`) gives `{"num_updated":0}` with status 200, and no message. The UI compares `num_updated` with the previewed count (§7.8).
- **Update by query ignores `id`.** `{"id":"z","genre":"x"}` updates `genre` everywhere and says nothing about `id`. The dialog refuses a partial with `id` (§7.8).
- **Backticks in ids cannot be quoted.** ``id:[`p,q]`]`` handles commas, brackets, spaces and slashes, but an id containing a backtick matches nothing. Such ids go through the single-document endpoints (§5.6).
- **`GET` search refuses a query string over 4,000 bytes** (400 `Query string exceeds max allowed length of 4000`). Delete and update by filter do not. See D30.
- **`/multi_search` wraps errors in a 200** (`{"results":[{"code":400,"error":…}]}`). Not used in M3; noted for M4.
- **The server returns document keys alphabetically,** so `preserve_order` keeps its order, not the order the document was written in. Column order comes from the schema (§5.1).

### 4.5 Keeping the request log bounded

Each logged event copies up to 1 MiB of response body, and the user buffer holds 5,000 events. Scrolling through 200k rows would pin about 90 MB per pass, and up to 565 MB over time, which breaks the memory AC while the page cache behaves perfectly (D24).

- `Client::without_response_bodies(&self) -> Self` returns a clone, in the style of `polling()`, whose events carry `response_body: None`. `size`, status and timing stay.
- The documents backend uses it for `search_documents` only. Every other call logs bodies as before, so a request stays visible in the log and "Copy as curl" still repeats it.
- The log's detail view says "Response body not kept for document pages" instead of showing nothing.

## 5. tessera-core

No GPUI dependency. New module tree `crates/tessera-core/src/documents/`. Everything here is unit tested.

### 5.1 Columns (`columns.rs`)

```rust
pub struct ColumnDef {
    pub key: String,          // top-level document key
    pub sortable: bool,
    pub numeric: bool,        // right-aligned
    pub in_schema: bool,
}

pub fn columns_from_schema(c: &Collection) -> Vec<ColumnDef>;
pub fn merge_keys(columns: &mut Vec<ColumnDef>, keys: &[String]) -> bool; // true when one was added
pub fn exclude_fields(c: &Collection) -> Option<String>;
pub fn sort_key(column: &str, ascending: bool) -> String;
```

- Order: `id`, then the schema's fields in schema order, then keys seen in documents (keys outside the schema are confirmed to be stored), appended as pages arrive.
- One column per top-level key. A nested field `author.name` collapses into the column `author`, which shows the compact JSON preview. An `object` field and its nested fields give one column.
- Wildcard names (`.*`, anything with `*`) make no column.
- **Vector fields** (`float[]` with `num_dim`, or with `embed`) make no column and are excluded from pages through `exclude_fields`, so a 1,536-dimension embedding never reaches the table. The inspector loads the full document.
- **Sortable:** type is `int32`, `int64`, `float`, `bool` or `string`, `Field::flag("sort")` is true and `index` is true. The server reports `sort: true` for numbers and bools. `geopoint` reports `sort: true` too but needs coordinates in `sort_by`, so the type check excludes it. `index: false` fields are not sortable (confirmed 404). `id` is not sortable (confirmed 404).

### 5.2 Pages (`page.rs`)

```rust
pub struct Row {
    pub id: Arc<str>,
    pub doc: Arc<Map<String, Value>>,
    pub cells: HashMap<String, Arc<str>>,   // preview text per top-level key
}
pub struct DocumentPage { pub rows: Vec<Row>, pub keys: Vec<String> }
impl DocumentPage { pub fn from_documents(docs: Vec<Map<String, Value>>) -> Self; }

pub fn cell_preview(v: &Value) -> Arc<str>;  // one line, at most CELL_LIMIT = 200 chars
pub fn cell_full(v: &Value) -> String;       // strings raw, the rest compact JSON

pub struct PageCache<T> { /* capacity, least recently touched evicted */ }
impl<T> PageCache<T> {
    pub fn new(capacity: usize) -> Self;
    pub fn get(&self, page: u32) -> Option<&T>;   // no touch: this runs per cell
    pub fn touch(&mut self, page: u32);
    pub fn insert(&mut self, page: u32, value: T);
    pub fn clear(&mut self);
    pub fn len(&self) -> usize;
}

pub fn row_position(row: usize, per_page: usize) -> (u32, usize);
pub fn wanted_pages(visible: Range<usize>, per_page: usize, found: u64) -> Vec<u32>;
```

- `from_documents` runs on the tokio worker, so previews are built off the GPUI thread and render only does a hash lookup. Long values cost nothing in the render path, which is what NF2 needs.
- `PageCache` is a `VecDeque` of `(page, value)`, oldest first. No LRU crate: it holds 20 entries.
- `wanted_pages` returns the pages under `visible`, plus the neighbouring page when `visible` comes within 50 rows of a page boundary, clamped to `found`.

### 5.3 Column preferences (`views.rs`)

D25. `data/document_views.json`, written atomically like `connections.json`.

```rust
pub struct ViewsFile { pub schema_version: u32, pub profiles: BTreeMap<Uuid, BTreeMap<String, CollectionView>> }
pub struct CollectionView {
    pub hidden: Vec<String>,
    pub pinned: Vec<String>,          // missing file or entry: ["id"]
    pub widths: BTreeMap<String, f32>,
}
```

Keys that no longer exist are kept, so dropping and re-adding a field keeps its width. `store::save` gains a generic `write_atomic(path, bytes)`, which `save` then calls. A temporary connection (D13) keeps preferences in memory only.

### 5.4 Filter completion (`filter.rs`)

`filter_by` is a small grammar, and completion only needs to know where the cursor is.

```rust
pub enum FilterContext {
    Field { start: usize, prefix: String },
    Operator { start: usize, field: String },
    Value { start: usize, field: String, prefix: String },
    Join { start: usize },                // after a finished clause: && ||
}
pub fn context_at(text: &str, offset: usize) -> FilterContext;
pub fn operators_for(field_type: &str) -> &'static [&'static str];
pub fn quote_value(value: &str) -> String;
```

- A forward scan tracks parentheses, `&&`, `||`, a backtick string and a `[…]` list. Inside a list, the context is `Value` for the current element.
- Operators: strings `:`, `:=`, `:!=`; numbers `:=`, `:>`, `:>=`, `:<`, `:<=`, `:!=`, `:[`; bools `:=`, `:!=`.
- `quote_value` wraps a value in backticks when it has a space, comma, bracket, parenthesis or `&`.
- Field candidates are the schema's indexed fields plus `id` (filtering a field with `index: false` is confirmed 400). Nested fields appear with their dotted name.

### 5.5 Document edits (`edit.rs`, `validate.rs`, `template.rs`)

```rust
pub fn merge_patch(old: &Map<String, Value>, new: &Map<String, Value>) -> Map<String, Value>;
pub struct DiffEntry { pub path: String, pub old: Option<Value>, pub new: Option<Value> }
pub fn diff_entries(old: &Map<String, Value>, new: &Map<String, Value>) -> Vec<DiffEntry>;
pub fn template(c: &Collection) -> Value;
pub struct DocIssue { pub path: String, pub key: &'static str, pub severity: IssueSeverity, pub detail: String }
pub enum IssueSeverity { Error, Warning, Info }
pub fn validate_document(c: &Collection, doc: &Value) -> Vec<DocIssue>;
pub fn key_position(text: &str, path: &str) -> Option<(u32, u32, u32)>; // line, start and end column
```

- **`merge_patch`** recurses into objects on both sides, gives `null` for a removed key, and replaces arrays and scalars whole. `id` is never in the result.
- **`diff_entries`** uses the same walk and yields one entry per changed leaf, with dotted paths, sorted. An empty list means there is nothing to save.
- **`template`:** one key per required field, skipping `id`, `optional`, `embed` and wildcard fields, with placeholders `""`, `0`, `0.0`, `false`, `[]`, `{}` and `[0.0, 0.0]` for `geopoint`. A nested required field `a.b` writes `{"a": {"b": …}}`.
- **`validate_document`** is advisory (D27). It reports a missing required field, a value of the wrong type, a wrong vector length against `num_dim`, a non-string `id`, and keys outside the schema as `Info` (they are stored, not indexed; skipped when the schema has a wildcard field). A numeric string for a numeric field is `Warning`, because the default `dirty_values` coerces it (`"12"` for an `int32`). `"true"` for a `bool` is `Error`: 30.2 rejects it, confirmed.
- **`key_position`** walks the JSON text with a string-aware scanner and a path stack, and returns where a key sits, so the editor can underline it. `serde_json::Value` has no positions.

### 5.6 Id filters and exports (`ids.rs`, `export.rs`)

```rust
pub struct IdBatches { pub filters: Vec<String>, pub single: Vec<String> }
pub fn id_batches(ids: &[String], max_bytes: usize) -> IdBatches;
pub fn to_jsonl(docs: &[Value]) -> String;
pub fn to_csv(columns: &[String], docs: &[Map<String, Value>]) -> String;
pub fn danger_level(count: u64, prod: bool) -> Danger;     // Medium, High
```

- `id_batches` writes ``id:[`a`,`b`]``, each filter at most `max_bytes` after percent-encoding. Ids with a backtick land in `single`.
- CSV follows RFC 4180: quotes around a field with a comma, quote or newline, doubled quotes, `\n` line ends. The header is the visible columns. Nested values are compact JSON. No `csv` dependency for a dozen lines; M5 may replace it.
- `danger_level` is `High` when `count > CONFIRM_ABOVE` or `prod`, else `Medium` (D28).

### 5.7 Curl

`curl::command(entry)` becomes a thin wrapper over a new `curl::request(node: &Url, method, path, query, body: Option<&str>) -> String`, so "Copy as curl" in the table builds the same masked text without a log entry. All snapshots stay as they are.

## 6. tessera-widgets

### 6.1 FilterInput

A one-line `EditorState` with a `CompletionProvider`, in `crates/tessera-widgets/src/filter_input.rs`. Reused by the bulk dialogs here and by the search workbench in M4.

```rust
pub struct FilterField { pub name: String, pub r#type: String, pub facet: bool }
pub enum FilterInputEvent { Submitted(String), Changed(String) }
pub type FacetFetch = Rc<dyn Fn(String, String, &mut App) -> Task<Vec<String>>>;
FilterInput::new(fields: Vec<FilterField>, fetch: FacetFetch, window, cx)
fn value(&self, cx: &App) -> String
fn set_value(&mut self, text: &str, window, cx)
fn set_fields(&mut self, fields: Vec<FilterField>, cx)
fn focus(&self, window, cx)
```

- Pinned facts (`gpui-base-0.9.0/src/input/base/state.rs`, `input/editor/mod.rs`, `input/editor/lsp/completions.rs`): the completion provider exists on `EditorState` only (`lsp_mut().completion_provider`); `InputState` has none. So the input is `EditorState::new(window, cx).line_number(false).folding(false).searchable(false).submit_on_enter(true).scroll_beyond_last_line(None)` drawn with `Editor::new(&state).h(px(32.))`.
- `enter` hands the action to an open completion menu first and only then submits, so Enter picks a suggestion when the menu is open and applies the filter when it is not. `submit_on_enter` emits `InputEvent::PressEnter`.
- `Shift+Enter` would insert a newline, which `filter_by` cannot use: the widget replaces newlines with spaces on `Change`.
- The provider answers from `context_at`. Field and operator candidates are synchronous. A `Value` on a `bool` field offers `true` and `false`. A `Value` on a facet field waits 150 ms, then calls `fetch`. Candidates carry a `CompletionTextEdit` over `start..offset`, with positions from `RopeExt::offset_to_position`, so a pick replaces the prefix and keeps the rest of the text.
- Direct dependencies added for the trait's types: `lsp-types = "=0.97.0"` (same as `Cargo.lock`), `anyhow`. `Rope` and `Position` are re-exported by `gpui_kit::component::input`.

### 6.2 JsonEditor marks

`JsonEditor::set_marks(&mut self, marks: Vec<JsonMark>, cx)` with `JsonMark { line: u32, start: u32, end: u32, message: String, severity: MarkSeverity }`. It clears and fills `state.diagnostics_mut()` with `Diagnostic::new(start..end, message).with_severity(…)` and notifies, as `gpui-component-0.9.0/src/inspector.rs` does at line 211. `MarkSeverity` keeps the widget free of `tessera-core` types.

## 7. tessera-app

### 7.1 Collection tab

- D23: a collection tab has two views, **Documents** (default) and **Schema**, in a `TabBar` (`gpui-component-0.9.0/src/tab/tab_bar.rs`) under the unlock banner. ⌘1 / ⌘2 (Ctrl+1 / Ctrl+2) switch, bound in `COLLECTION`, and each tab shows its shortcut.
- `CollectionPanel` keeps `SchemaView` as the one loader of the collection, so opening a tab stays one `GET`. `SchemaView` emits `SchemaLoaded(Collection)` with the unmasked collection, and the panel hands it to `DocumentsView::set_schema`.
- `CollectionChanged(name)` (truncate, schema change) reaches `DocumentsView` through its own subscription. It clears selection and pages and reloads.
- `Refresh` routes to the active view. Documents: pages are dropped and reloaded, the scroll position stays.
- ⌘E, ⌘⇧D, ⌘⇧⌫ act on the schema view only. `DOCUMENTS`, a deeper key context on the documents view, binds them to `NoAction` so that a hidden control can never fire. ⌘⌫ in `DOCUMENTS` deletes documents, never the collection.

### 7.2 Backend seam

`crates/tessera-app/src/documents.rs` holds `DocumentsBackend` (trait object, `DocumentsBackendGlobal`, `tokio_backend`) with the same structure as `CollectionsBackend`, because GPUI's test executor does not wait for network I/O. `page()` builds the `DocumentPage` inside the future, on the tokio worker. UI tests install a fake that synthesises documents for any page number.

### 7.3 Documents view

```
┌ toolbar ────────────────────────────────────────────────────────────────┐
│ [filter_by …                       ⌘F]  [Go to ID ⌘G]  [Columns ⇧⌘O]    │
│ [New ⇧⌘N] [Delete ⌘⌫] [Copy ▾] [Delete by query ⇧⌘K] [Update ⇧⌘U]       │
├ status ─────────────────────────────────────────────────────────────────┤
│ 159,784 of 200,000 · 12 ms · sorted by price ↓ (⇧⌘S) · 3 selected        │
├ table ───────────────────────────────────┬ inspector ───────────────────┤
│ ☐ id     title    brand   price  stock   │ JSON editor                  │
└──────────────────────────────────────────┴──────────────────────────────┘
```

**Table** (`views/documents/table.rs`, `DocRows: TableDelegate`). `gpui-component-0.9.0/src/table/`:

- `rows_count` is `found`, not the loaded count, so the scrollbar spans the whole result and a drag can land anywhere. Infinite scroll therefore means fetching what becomes visible. `has_more` and `load_more` stay unused.
- `uniform_list` renders only the visible rows, and the horizontal `virtual_list` only the visible columns. The cost per frame is about 30 rows times the visible columns, each one a hash lookup and a `SharedString::from(&Arc<str>)` clone.
- `TableState::new(...).cell_selectable(true).row_header(false).row_selectable(false).col_selectable(false).col_movable(false)`.
- Unloaded rows render a muted `…` in `render_td`. No shimmer, which would animate 30 rows at scrollbar-drag speed.
- A first column, key `__select`, `fixed_left()`, `selectable(false)`, `resizable(false)`, width 36, draws a `Checkbox` (§7.7).
- Pinned columns: `Column::fixed_left()`. `fixed_left_cols_count` counts all fixed columns, so `column(ix)` must return them first. Default pin: `id`.
- `TableState::refresh` rebuilds columns from `column()`, which resets resize widths and sort marks to what the delegate returns. So the delegate keeps both: widths from `TableEvent::ColumnWidthsChanged`, and the sort as `(key, ascending)`, returned through `.ascending()`, `.descending()` or `.sortable()`.
- Cell text goes through `cell_label` as well, because plain text drawn in a cell is announced as nothing otherwise.

**Paging.**

1. `visible_rows_changed` must stay fast: it stores the range and touches visible pages. If no throttle timer runs, it starts one of 80 ms. A running timer is left alone, so continuous scrolling still fetches (a debounce would starve).
2. When the timer fires, `wanted_pages` is compared with the cache and `inflight`. Tasks for pages no longer wanted are dropped, which aborts the tokio task through `AbortOnDrop`. Missing pages start, at most 4.
3. A result carries the `generation` it was requested under. Any query change bumps the generation, so a late answer is dropped. `found` from each answer updates `rows_count`. New `keys` go through `merge_keys`, then `refresh`.
4. `visible_rows_changed` does not fire while `rows_count` is 0, so the view loads page 1 itself on open and on every query change. `loading()` is true until it arrives, which shows the kit's skeleton.

**Sort.** Header click uses the kit's `perform_sort` with its cycle (off → descending → ascending → off). ⌘⇧S / Ctrl+Shift+S does the same for the selected cell's column, because `TableState::perform_sort` is private and Tab inside a cell-selecting table moves between cells, so M2's focusable headers would not be reachable. The status line shows the sort with the shortcut. A column that is not sortable has no icon, and the shortcut does nothing there.

**Filter bar.** `FilterInput`. Enter applies. ⌘F / Ctrl+F focuses it. Selection is cleared by any query change, so a delete cannot hit rows the new filter hides. A server error (bad field, bad syntax) shows under the bar in full, while the previous rows stay on screen dimmed until the filter is fixed.

**Column picker.** ⌘⇧O / Ctrl+Shift+O opens a `Popover` with one `Checkbox` per column, a pin toggle per row, and "Reset". On keyboard: Tab between entries, Space toggles, Esc closes. ⌘⇧P / Ctrl+Shift+P pins or unpins the selected cell's column. ⌘⇧← / ⌘⇧→ (Ctrl+Shift+Left/Right) make it 20 px narrower or wider, because a mouse drag is not a keyboard path. Changes go to `ViewsFile` through a background save. A failed save shows inline in the status line.

### 7.4 Inspector

An `h_resizable` split as in the request log (D22). ⌘I / Ctrl+I toggles. Enter or a double click on a row opens it. While open it follows the selected cell's row.

- Always loads the row with `document()`: pages may lack vector fields and may be stale.
- Editor: `JsonEditor` with folding, highlighting and parse errors from M2. `validate_document` runs on every change and is mapped to marks by `key_position`. Issues also list under the editor, because marks alone are not readable by keyboard.
- A second selection of the same row does nothing. Selecting another row while the editor is dirty does not follow. The header shows "Unsaved changes" with Revert (⌘⇧R / Ctrl+Shift+R).
- Closing a dirty inspector asks through `Confirm`, with focus on Cancel.
- PR A ships it read-only. PR B makes it editable.

**Go to ID.** ⌘G / Ctrl+G focuses an input in the toolbar. Enter calls `document(id)` and opens the inspector with the result. A 404 shows inline next to the input. The table does not move, because a document's row number is unknown without a search.

### 7.5 Edit (PR B)

- Header control **Save as**: PATCH (default) or Upsert, two selected-style buttons as in the schema dialog, ⌘⇧M / Ctrl+Shift+M flips. **Review changes…** ⌘↩ / Ctrl+↩.
- Review disabled when: the JSON does not parse, the text is no object, `id` changed (inline: "The id cannot change. To copy a document, use New document."), or `diff_entries` is empty.
- Before the dialog the document is fetched again. If it changed since it was loaded, the diff is computed against the fresh copy and a warning says so. This is what stops an upsert from silently overwriting someone else's edit.
- The review dialog is medium danger: target line (`PATCH /collections/products/documents/42`), `diff_entries` (added green, removed red, changed with old and new), the exact body in a read-only `JsonEditor`, consequence for upsert ("Replaces the whole document. Fields missing here are removed."). Focus starts on Back. Apply is ⌘↩ / Ctrl+↩ with focus on it, Back is Esc.
- PATCH sends `merge_patch`. Upsert sends the editor's document.
- Success closes the dialog, replaces the editor text with the response, drops pages and reloads visible ones. Failure shows the message inline and stays.
- Save goes through `ConnectionState::ensure_writable`, so a locked profile opens the unlock dialog first, and the save button shows the lock.

**New document.** ⌘⇧N / Ctrl+Shift+N opens the inspector with `template()` and the header "New document". Save is ⌘↩ / Ctrl+↩: `create_document`, no review (there is nothing to overwrite). A 409 or 400 shows inline with the draft kept. On success the view reloads and the inspector switches to the stored document.

### 7.6 Copy

| Action | Shortcut | Source |
|---|---|---|
| Cell | ⌘C / Ctrl+C | `cell_full` of the cached row |
| Row as JSON | ⌥⌘C / Ctrl+Alt+C | `document()`, pretty, because pages lack vectors |
| Selection as JSONL | ⌥⌘J / Ctrl+Alt+J | `export_documents` per id batch, `document()` for `single` |
| Selection as CSV | ⌥⌘S / Ctrl+Alt+S | same documents, visible columns |
| Copy as curl | ⌥⌘U / Ctrl+Alt+U | `curl::request` for the selected row's GET, or for the current search when no row is selected, key as `$TYPESENSE_API_KEY` |

The toolbar's Copy dropdown lists all five with their shortcuts, and the table's context menu lists them too. ⌘⇧C stays the global "copy log entry as curl" from M1. Copying never masks document content: the user asked for their data. "Copy as curl" of a body goes through `redact` as everywhere else.

### 7.7 Selection and delete (PR C)

- Checkbox click toggles. Shift+click selects the range from the last anchor. Space toggles the selected cell's row, Shift+Space extends, ⌘A / Ctrl+A selects all rows of the loaded pages, Esc on the table clears (the kit's `action_cancel` clears the cell first, then propagates).
- The selection is a set of ids, so it survives scrolling out of the cache. Rows on unloaded pages cannot join a range: the range covers loaded rows and the status line says so. The cap is `MAX_SELECTION` (D26); going over shows an inline note and adds nothing more.
- **Delete** ⌘⌫ / Ctrl+Backspace: the selection, or the selected cell's row when nothing is ticked. `danger_level` picks `Confirm` or `ConfirmDanger` (D28). `id_batches` splits the request. Batches run in sequence and stop at the first failure, and the message says "Deleted N of M". On success selection clears and the view reloads.

### 7.8 Delete and update by query (PR C)

A dialog in `views/documents/bulk.rs`, one mode each. ⌘⇧K / Ctrl+Shift+K and ⌘⇧U / Ctrl+Shift+U.

- A `FilterInput` starts with the table's current filter. An empty filter disables Continue: "Use Truncate to delete everything" for delete.
- 300 ms after the last change `count_documents` runs and the dialog shows "Affects N documents" or the server's message. Zero disables Continue. Stale counts are dropped through the task handle.
- Update mode adds a `JsonEditor` for the partial, with `validate_document` marks limited to the keys present. `id` in the partial disables Continue, §4.4.
- Continue counts once more, so the consequence text is current, then asks: `Confirm` or `ConfirmDanger` by `danger_level`, with the collection name to type at high. Consequences name the count and the filter.
- The result: "Deleted N documents". For update, "Updated N of M documents" as an inline warning, not a success, when the numbers differ, with the note that the server rejects non-matching values silently (§4.4). The dialog then stays open.
- The long timeout applies, and a timeout uses `long_error_text`. "Stop waiting" comes from `ConfirmDanger`; the plain `Confirm` path uses the dialog's own busy state.
- Read-only: `ensure_writable` before Continue's request.

### 7.9 Shortcuts

All bound in `DOCUMENTS` or the table's `Documents > DataTable` unless noted, and all shown with `Kbd` on the control.

| Action | macOS | Linux, Windows |
|---|---|---|
| Documents view / Schema view (`COLLECTION`) | ⌘1 / ⌘2 | Ctrl+1 / Ctrl+2 |
| Focus filter | ⌘F | Ctrl+F |
| Clear filter | ⇧⌘F | Ctrl+Shift+F |
| Go to ID | ⌘G | Ctrl+G |
| Columns | ⇧⌘O | Ctrl+Shift+O |
| Pin column | ⇧⌘P | Ctrl+Shift+P |
| Narrower / wider column | ⇧⌘← / ⇧⌘→ | Ctrl+Shift+Left / Right |
| Sort by column | ⇧⌘S | Ctrl+Shift+S |
| Inspector | ⌘I | Ctrl+I |
| Open row | Enter | Enter |
| New document | ⇧⌘N | Ctrl+Shift+N |
| Review or save (`INSPECTOR`) | ⌘↩ | Ctrl+Enter |
| PATCH or upsert (`INSPECTOR`) | ⇧⌘M | Ctrl+Shift+M |
| Revert (`INSPECTOR`) | ⇧⌘R | Ctrl+Shift+R |
| Delete | ⌘⌫ | Ctrl+Backspace |
| Delete by query | ⇧⌘K | Ctrl+Shift+K |
| Update by query | ⇧⌘U | Ctrl+Shift+U |
| Toggle row / extend | Space / ⇧Space | Space / Shift+Space |
| Select loaded rows | ⌘A | Ctrl+A |
| Copy cell, row, JSONL, CSV, curl | §7.6 | §7.6 |
| Reload (`COLLECTION`, existing) | ⌘R | Ctrl+R |
| Frame overlay (feature only) | ⌥⌘F | Ctrl+Alt+F |

⌘N stays "new connection" globally, which is why New document uses the chord M2 chose for New collection. Ctrl+Alt+Backspace is avoided on purpose: X11 may kill the session on it.

### 7.10 Empty states, errors, i18n

- Empty collection: one sentence on documents, the docs link, New document and, on a local connection, the `cargo xtask seed` hint. No filter match: "No documents match this filter" with Clear filter.
- A failed page shows as a line under the status bar in full, with Retry (⌘R). Its rows keep the `…` placeholder.
- All strings go through `t!()` under `documents.*`, with the same keys in `en.yml` and `de.yml`.
- UX rule 1: the panel's title, environment badge and colored edge are unchanged. The bulk dialogs carry the PROD badge through `ConfirmDanger` and name the connection in `Confirm`'s body.

## 8. Seed data

`cargo xtask seed` gains a sixth set, always loaded:

| Collection | Documents | Shows |
|---|---|---|
| `products` | 200,000 | NF2; `default_sorting_field: created_at`; facets `brand`, `category`, `tags`, `active`; sortable `price`, `rating`, `stock`; nested `dimensions`; a 25-word `description` |

- `enable_nested_fields: true`. The generator uses the same fixed-seed `Rng` and yields documents lazily, so 200k `Value`s are never held at once. `Server::import` takes an iterator and fills batches of 1,000.
- `COLLECTIONS` gains `products`, so `--reset` drops it.
- A Python equivalent took 8 s on the dev server. The integration job keeps seeding on both versions.

## 9. Testing

| Level | What it checks |
|---|---|
| Client unit (`wiremock`) | Each endpoint's method, path, encoded query and decoding; `filter_by` with `&&`, spaces and backticks; ids with `/`, `?`, space; long timeout on export and both bulk calls; `without_response_bodies` drops the body and keeps the size |
| Client integration (`#[ignore]`, 30.0 and 30.2) | create → get → patch (deep merge, null removal) → upsert → search → count → facet values → export → delete; duplicate 409; bad filter 400; delete and update by filter counts; update with a bad value answers 0 |
| Core unit | Columns (nested collapse, vector skip, wildcard skip, sortable table); `cell_preview` limit and newlines; `PageCache` eviction order and 20-entry bound; `wanted_pages` at boundaries and past the end; `context_at` table; `merge_patch` and `diff_entries`; validation table; `key_position` with escapes and nesting; `id_batches` (byte limit, backticks); CSV quoting; `danger_level` |
| Widgets (`#[gpui_kit::test]`) | `FilterInput`: field, operator and value suggestions, Enter picks when open and submits when closed; `JsonEditor` marks |
| App UI (fake `DocumentsBackend`) | Opens on Documents; ⌘2 shows the schema; rows and `found`; scrolling to row 100,000 asks only for its pages; at most 20 pages cached after a long scroll; stale answers dropped; sort click and shortcut change the query; filter error inline; column hide, pin, width survive a reopen; inspector follows and stops when dirty; review focuses Back; stale-document warning; locked profile sends nothing before the unlock; selection cap; multi-delete batches and partial failure; preview count; threshold picks the typed confirmation; `num_updated` mismatch stays open; ⌘⌫ in documents never reaches `DeleteCollection` |
| Secrets | A page request's log event has no response body; the log detail says so; a body with `api_key` stays masked in Copy as curl |
| Manual (PR C) | NF2 and RSS on M1 hardware, §12 |

## 10. CI

- The `integration` job runs the document tests on 30.0 and 30.2. This design's confirmed behaviour comes from 30.2; a difference on 30.0 fails there.
- The seed step now loads 200k documents.
- The NF5 size check stays. PR A adds `lsp-types` to a direct dependency only (it is already linked) and PR descriptions record the Linux binary size. The `frame-overlay` feature is off in release builds.

## 11. New dependencies

| Crate or feature | Used by | Why |
|---|---|---|
| `lsp-types = "=0.97.0"` (already in the tree through GPUI Kit) | widgets | Types of `CompletionProvider` |
| `anyhow` (workspace) | widgets | The trait's `Task<Result<…>>` |
| `gpui-kit` feature `profiler`, behind `tessera-app` feature `frame-overlay` | app, off by default | `Window::cycle_debug_frame_overlay_mode` is `#[cfg(feature = "profiler")]` |

## 12. Measuring NF2

`gpui-kit` 0.9.0 has no FPS HUD; §7.4 and NF2 name one that does not exist (D29). GPUI's `DebugFrameOverlay` (`gpui-pre-0.3.11/src/debug_overlay.rs`) paints frame-time statistics without layout or text, so it does not disturb what it measures.

1. `cargo xtask seed --url http://localhost:18108 --reset`.
2. `cargo run --release -p tessera-app --features frame-overlay`, open `products`, ⌥⌘F twice for the detailed overlay.
3. Wheel-scroll for 30 s, then drag the scrollbar end to end for 30 s, with the default columns and with all columns.
4. Pass: draw time p95 ≤ 16.6 ms. Record it in PR C's description with the machine.
5. Watch `ps -o rss= -p <pid>` during a 5-minute scroll. Pass: flat after the first pass. Record it.

## 13. Changes to the product spec

Recorded in `docs/spec.md` together with this design:

- §16 gains D22 to D30. The §14 M3 row links this design.
- §7.4 notes the frame overlay, §7.19 the kept-back page bodies, §11 the new file, and NF2's measurement names the overlay.

| # | Question | Decision |
|---|---|---|
| D22 | Inspector: right dock panel or inside the tab? | A resizable split inside the documents view, like the request log's detail. A dock panel would be shared by all tabs, while the document, its unsaved edits and its schema belong to one tab |
| D23 | What does a collection tab show? | Two views, Documents (default) and Schema, ⌘1 / ⌘2. One tab per collection stays; the sidebar and the list keep opening it |
| D24 | Request log vs. page cache memory | Document page responses are logged without their body (status, size and timing stay). Otherwise the log, not the cache, holds the scrolled documents, up to 565 MB |
| D25 | Where column preferences live | `data/document_views.json`, per profile and collection, not in `Profile`: profiles are exported as configuration, and a large cluster would bloat them |
| D26 | Multi-select size and URL length | At most 1,000 selected rows. Filters are batched at 7,500 encoded bytes, which passes common proxies. The `http` crate refuses URLs above 64 KB in any case. Ids with a backtick go one by one |
| D27 | Does schema validation block saving? | No. Marks are advisory and the server decides, as in M2. Only a parse error, a changed `id` or an empty diff stop a save. The server's rules (coercion, wildcards, references) are wider than a client check should claim |
| D28 | Which confirmation for document deletes and bulk updates? | Medium dialog; type the collection name above 1,000 documents or on prod. The §7.4 rule for delete by query also covers multi delete, single delete on prod, and update by query |
| D29 | NF2 names a GPUI Kit FPS HUD that does not exist | Measured with GPUI's debug frame overlay behind a `frame-overlay` cargo feature, by hand, p95 draw time ≤ 16.6 ms |
| D30 | `filter_by` over 4,000 bytes in the browser | Not supported in M3: the server's message shows inline. `POST /multi_search` is a read, but the read-only guard refuses POST and failover skips it. M4 needs it anyway and should allow that one path in the guard |

## 14. Risks

| Risk | Impact | Fallback |
|---|---|---|
| `f32` pixel precision at 6.4 million px (200k × 32 px): the offset's step is 0.5 px at the far end | Slight jitter when scrolling slowly near the end | Accept: it is under a pixel. If visible, a smaller row height (`Size::Small`, 30 px) does not help much; a windowed `rows_count` around the visible page would, at the cost of the true scrollbar |
| `visible_rows_changed` runs inside the render processor and starts a timer | A slow path in it costs frames | It only stores a range and touches a `VecDeque` of 20, and the timer starts at most once per 80 ms |
| `EditorState` in one line is not a designed use: `Enter` semantics or scrolling may surprise | Filter bar feels wrong | Pinned code reviewed (§6.1). Fallback: plain `Input` plus an own `Popover` list, same `FilterInput` API |
| Checkbox click inside a cell also reaches `on_cell_click` | Click selects the cell too | A stop in the checkbox handler, tested. Harmless if it fails |
| Clicks on the kit's sort icon only, not the header cell | Smaller target than M2's headers | Accept; the shortcut and the status line cover it |
| Scrollbar drag issues many aborted searches | Load on a production cluster | Throttle of 80 ms bounds it at about 12 requests per second, at most 4 at a time, and only while dragging |
| Pages stay cached while others write to the cluster | User sees old rows | ⌘R; every mutation in the view reloads; the inspector always refetches |
| 30.0 behaves differently from the probed 30.2 | Integration job fails | The job is the check. Differences go into this design before the UI depends on them |
| Selection across pages, then a concurrent delete elsewhere | Fewer deleted than asked | `num_deleted` is reported as "Deleted N of M" |
