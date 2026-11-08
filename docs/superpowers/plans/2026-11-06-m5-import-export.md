# M5 Import and Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Import a million documents at constant memory with progress, cancel and an error report that can be re-imported; create a collection from an inferred schema; export a collection, filter or selection to a file; run all of it as jobs in a panel that outlives the views.

**Architecture:** `tessera-client` gains `import_batch` and a streaming `ExportStream`. `tessera-core` gains `transfer/`: synchronous readers on a blocking thread, a bounded channel, a sequential sender, a report with a disk spool, and an export writer with part files. `tessera-app` gains a `TransferBackend` seam, a global `Jobs` entity, a jobs tab in the bottom dock, and import and export dialogs. `xtask nf3` measures NF3 headlessly. Three PRs, in order: A (Tasks 1–7), B (Tasks 8–11), C (Tasks 12–14).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11), reqwest 0.13.5 with `stream`, tokio 1.54.1, tokio-util 0.7.20, futures 0.3.34, csv 1.4.0, tempfile 3.27.0, serde_json 1.0.154, wiremock 0.6.5.

**Spec:** [`docs/superpowers/specs/2026-11-06-m5-import-export-design.md`](../specs/2026-11-06-m5-import-export-design.md). Section numbers below (§4.2 …) refer to it. Read the section a task names before starting it.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI. Direction: `app → widgets → core → client`. `xtask` may use core and client.
- Direct dependencies are pinned exactly (`=x.y.z`) in the workspace `Cargo.toml`. Only Task 1 adds dependencies.
- Never guess a GPUI, GPUI Kit, reqwest, tokio-util or csv API. Read the pinned source under `~/.cargo/registry/src/*/` first (`gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0`, `gpui-pre-0.3.11`, `reqwest-0.13.5`, `tokio-util-0.7.20`, `csv-1.4.0`). When a task names a file there, it was verified, except anything in `csv`, which Task 3 verifies.
- Every UI string goes through `t!()`, same keys in `assets/locales/en.yml` and `de.yml`, flat keys, under `transfer.import.*`, `transfer.export.*`, `transfer.infer.*`, `jobs.*`. `import.*` is the profile import: leave it. The key-parity test stays green.
- No blocking I/O on the GPUI thread and none on a tokio worker: `read_documents`, `preview` and `infer_schema` run in `spawn_blocking`; async file work uses `tokio::fs`. Nothing is read whole into memory, except a clipboard of at most 16 MiB.
- No key in logs, files, errors, `Debug` or `Display`. Redaction tests are never removed. Import events carry no bodies.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. No commit with a failing check.
- Commits: `git commit -s`, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense: `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, key `xyz`, live tests with `TESSERA_IT_URL=http://localhost:18108 TESSERA_IT_KEY=xyz cargo test -p tessera-client --test live -- --ignored`. **Never send anything to port 8108.** Use throwaway collections, removed at the end of each test.
- UX rules: environment visible everywhere (connection name and `env_badge::render` in dialog titles and on job rows); errors inline and in full; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown on the control via `Kbd::binding_for_action(&Action, Some(CONTEXT), window)`.
- Shortcuts are bound in key contexts. Platform constants follow the `#[cfg(target_os = "macos")]` pairs in `crates/tessera-app/src/actions.rs`. New actions also go into `ACTION_LABELS` with a `palette.action.<Name>` key.
- Writes go through `ConnectionState::ensure_writable(window, cx, then)`, and the control shows `env_badge::lock(id, false, cx)` while `writes_locked()`.
- Anything a user types or a file contains that ends up in a URL path or query is validated by allowlist. Enums are written from fixed strings. Field names pass `field_safe`.
- A write's preview count uses the same selection as the write: `count_documents` already sends `enable_curations=false`, and export ignores curations (design §4.4). Compare counts after every run: a 200 can have done nothing.
- Destructive flows confirm first, and cancelling sends nothing. A job must never stop because a view was dropped.
- NF5: the Linux release binary stays below 41,943,040 bytes. Each PR records the size in its description.
- Each task ends with its CHANGELOG entry under `## [Unreleased]` → `### Added` in `CHANGELOG.md`, in the same commit. Plain prose, in the style of the existing entries.

## Review Focus

1. No `POST` starts after `stop`, and the batch in flight is counted. Test in Task 5.
2. A blank line in the source must not shift later error line numbers. Test in Task 5.
3. A line with invalid UTF-8 must never reach the server: it would be stored with a changed id. Test in Task 3.
4. Import events must carry no request or response body, or the log alone breaks NF3. Test in Task 1.
5. A stray CSV quote must not read the whole file into one record. Test in Task 3.
6. A cancelled or failed export must leave an existing target file untouched and no part file. Test in Task 6.
7. Closing the dialog or the tab must not stop a job. Test in Task 8.
8. A new collection must not exist on the server until Start, past the confirmation. Test in Task 11.
9. Progress must not redraw the root view, and the panel at most 30 times a second. Test in Task 8.
10. `update` or `upsert` above 1,000 estimated documents or on prod must need the name typed. Test in Task 11.

---

## PR A · Engine (branch `feat/m5-engine`, from `main` once M4 is merged, else from the tip of `feat/m4-palette`)

### Task 1: Dependencies and `import_batch`

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/tessera-client/Cargo.toml`, `crates/tessera-client/src/lib.rs`, `crates/tessera-client/src/documents.rs`, `crates/tessera-client/src/send.rs`, `crates/tessera-app/src/views/request_detail.rs`, `crates/tessera-app/src/views/request_log.rs`, locales
- Test: `crates/tessera-client/tests/documents.rs`, `crates/tessera-client/tests/live.rs`, `crates/tessera-app/tests/ui_log.rs`

**Interfaces:**
- Workspace `Cargo.toml`: `reqwest` gains feature `stream`; `tokio` gains `fs`, `io-util`; add `tokio-util = { version = "=0.7.20", features = ["codec", "io"] }`, `futures = { version = "=0.3.34", default-features = false, features = ["std"] }`, `csv = "=1.4.0"`. `tokio-util` and `futures` are the versions already in `Cargo.lock`. Client gains `tokio-util.workspace`, `futures.workspace`.
- Produces in `documents.rs`, exported from `lib.rs`:

```rust
pub enum ImportAction { Create, Upsert, Update, Emplace }           // Debug, Clone, Copy, PartialEq, Eq
pub enum DirtyValues { CoerceOrReject, CoerceOrDrop, Drop, Reject } // same, default CoerceOrReject
impl ImportAction { pub fn as_str(self) -> &'static str; }          // create, upsert, update, emplace
impl DirtyValues { pub fn as_str(self) -> &'static str; }           // coerce_or_reject, coerce_or_drop, drop, reject
pub struct ImportParams { pub action: ImportAction, pub batch_size: u32, pub dirty_values: DirtyValues }
pub struct ImportLine { pub success: bool, pub code: Option<u16>, pub error: Option<String>, pub id: Option<String> }
pub async fn import_batch(&self, collection: &str, params: &ImportParams, body: &str) -> Result<Vec<ImportLine>, Error>
pub fn without_bodies(&self) -> Self
```

- `import_batch`: `collection_path(collection, "/documents/import")`, query through `encode_query` with `action`, `batch_size`, `dirty_values`, `return_id=true`. Calls `self.without_bodies().send_with(node, Method::POST, &path, Some(&query), Some((body.to_string(), "text/plain")), Some(LONG_TIMEOUT))` with `node = self.active_node()`. No failover loop. `2xx`: one `ImportLine` per non-empty line of the answer, in order; a line that does not decode as `{success, code?, error?, id?}` becomes `ImportLine { success: false, code: None, error: Some(line), id: None }`. Other status: `Err(Error::http(status, text))`.
- `without_bodies`: new field `keep_request_body: bool` on `Client`, default true in `Client::new`. In `send.rs` the `request_body` copy is made only when `self.keep_request_body`. `without_response_bodies` stays.
- `request_detail.rs`: for an event with `request_body` `None` on a `POST` to a path ending in `/documents/import`, show `log.detail.import_body_not_kept` ("Bodies of import batches are not kept") and hide Copy as curl for that entry, in the log's button and for `CopyAsCurl`, with `log.detail.no_curl_import` as the disabled tooltip. German: "Inhalte von Import-Batches werden nicht aufbewahrt", "Für Import-Batches gibt es keinen curl-Befehl".

- [ ] **Step 1: Write the failing wiremock tests** in `tests/documents.rs`:
  - `import_sends_action_and_options`: for each action and dirty value, matchers on `query_param`, and header `content-type: text/plain`; `return_id=true`; path `/collections/c/documents/import`.
  - `import_decodes_lines_in_order`: answer `{"success":true,"id":"1"}\n{"code":400,"error":"Field `n` must be an int32.","id":"2","success":false}\n` gives two lines with those fields.
  - `import_keeps_an_undecodable_line`: answer `{"success":true}\nnot json\n` gives 2 lines, the second `success:false` with `error == Some("not json")`.
  - `import_empty_answer_is_empty`.
  - `import_error_status_is_http`: 404 `{"message":"Collection not found"}` gives `typesense_message()`; 503 gives `Error::Http { status: 503, .. }`.
  - `import_is_refused_when_read_only`: `Error::ReadOnly`, mock `expect(0)`.
  - `import_event_has_no_bodies`: hook records; `request_body == None`, `response_body == None`, `size == Some(len)`, `status == Some(200)`; a body over 1 MiB too.
  - `without_bodies_keeps_other_clients_logging`: the original client still logs bodies.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test documents`. Expected: compile errors.
- [ ] **Step 3: Implement.** Run `cargo fetch`, confirm `Cargo.lock` adds only `csv`, `csv-core` as new crates.
- [ ] **Step 4: Live test** `live_import_answers_per_line` (`#[ignore]`) on a throwaway collection with `t: string`, `n: int32 optional`: body with a good line, a wrong type, `not json`, a duplicate id, gives 4 lines with codes 200-success, 400, 400, 409; `"12"` for `n` succeeds with `CoerceOrReject` and fails with `Reject`; `Emplace` is accepted; an alias as target works. Run against 18108.
- [ ] **Step 5: UI test** in `ui_log.rs`: `import_entry_says_bodies_were_not_kept`.
- [ ] **Step 6: Run** the checks, CHANGELOG, commit. `feat(client): import batches of documents`.

### Task 2: Streaming export in the client

**Files:**
- Create: `crates/tessera-client/src/export.rs`
- Modify: `crates/tessera-client/src/send.rs`, `crates/tessera-client/src/lib.rs`
- Test: `crates/tessera-client/tests/export.rs` (new), `crates/tessera-client/tests/live.rs`

**Interfaces:**

```rust
pub const STREAM_STALL: Duration = Duration::from_secs(120);
pub const MAX_LINE: usize = 256 << 20;
pub struct ExportQuery { pub filter_by: Option<String>, pub include_fields: Option<String>, pub exclude_fields: Option<String> }
pub struct ExportStream { /* FramedRead over StreamReader, plus the log guard */ }
impl Client { pub async fn export_stream(&self, collection: &str, query: &ExportQuery) -> Result<ExportStream, Error>; }
impl ExportStream { pub async fn next_line(&mut self) -> Result<Option<String>, Error>; }
```

- `send.rs`: extract `prepare(&self, node, &method, path, query) -> Result<Prepared, Error>` from `send_with`, holding the read-only guard, URL join, key header, `RequestBuilder` without a timeout, and the fields a `RequestEvent` needs. `send_with` calls it, sets `.timeout(...)`, and behaves exactly as before; all existing tests stay green. Add `pub(crate) async fn send_stream(&self, node, path, query) -> Result<(reqwest::Response, StreamLog), Error>`, using `prepare`, setting **no** `.timeout()` (reqwest's is a total deadline over the body, `reqwest-0.13.5/src/async_impl/request.rs` line 291), wrapped in `tokio::time::timeout(STREAM_STALL, request.send())`.
- `StreamLog`: private struct with the event fields, `bytes: u64`, `status`, `finished: bool`, `error: Option<String>`. `Drop` calls the hook once with `response_body: None`, `request_body: None`, `size: Some(bytes)`, `duration`, and `error` = the stored text, or `"cancelled"` if not finished.
- `export_stream`: `collection_path(collection, "/documents/export")`, query by `encode_query` of present options, active node, no failover. Non-`2xx`: read `.text()`, `Err(Error::http(..))`.
- Reading: `response.bytes_stream()` (`stream` feature, `reqwest-0.13.5/src/async_impl/response.rs` line 351) mapped with `std::io::Error::other`, `tokio_util::io::StreamReader::new`, `tokio_util::codec::FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_LINE))`. `next_line`: `tokio::time::timeout(STREAM_STALL, framed.next())`; `Elapsed` is `Error::Timeout`; `LinesCodecError::MaxLineLengthExceeded` is `Error::LineTooLong { limit: MAX_LINE }`, a new variant with text "a line of the answer exceeds {limit} bytes"; other I/O errors become `Error::Stream(String)`, a new variant, because `reqwest::Error` does not survive `io::Error::other` as a type. Count bytes as `line.len() + 1`, and set `finished` on `None`.

- [ ] **Step 1: Write the failing tests** in `tests/export.rs`:
  - `export_reads_lines`: body `{"id":"1"}\n{"id":"2"}` with no trailing newline gives 2 lines, then `None`.
  - `empty_export_has_no_lines`.
  - `export_sends_its_query`: `filter_by=a:=1 && b:=2` arrives as `%26%26`; include and exclude present.
  - `export_error_comes_before_the_first_line`: 400 with `{"message":"Could not find a filter field named `bad` in the schema."}`.
  - `slow_body_is_not_cut_by_the_client_timeout`: client `with_timeout(100 ms)`, mock delays 300 ms, succeeds.
  - `export_passes_a_read_only_client`.
  - `one_event_per_stream_with_the_total_size`: 2 lines, one event, `size` right, no bodies, `error == None`.
  - `dropped_stream_logs_cancelled`: read one line, drop; one event with `error == Some("cancelled")`.
  - `long_line_is_an_error`: serve a line over a test limit through a `#[cfg(test)]`-settable length or a unit test of the mapping function.
- [ ] **Step 2: Run.** Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Live test** `live_export_stream`: on a throwaway collection with a curation set attached, whose rule filters, assert `export_stream` line count equals `count_documents(filter)`, and that `include_fields=nope` gives `{}` lines (documenting why the UI never offers free names). Run against 18108.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(client): stream exports line by line`.

### Task 3: Document readers

**Files:**
- Create: `crates/tessera-core/src/transfer/mod.rs`, `source.rs`, `csv_cells.rs`
- Modify: `crates/tessera-core/src/lib.rs` (`pub mod transfer;`), `crates/tessera-core/Cargo.toml` (`csv`, `tokio` with `sync rt fs io-util time macros`, `tokio-util`, `tempfile` as normal dependencies; drop the duplicates from dev)
- Test: inline `#[cfg(test)]`

**Interfaces:** in `transfer/mod.rs` the constants `DEFAULT_BATCH: u32 = 1000`, `MAX_BATCH: u32 = 100_000`, `BATCH_BYTES: usize = 16 << 20`, `MAX_DOC_BYTES: usize = 32 << 20`, `CHANNEL_BATCHES: usize = 2`, `PREVIEW_LINES: usize = 20`, `SAMPLE_ROWS: usize = 1000`, `REPORT_ROWS: usize = 1000`, `REPORT_PREVIEW_BYTES: usize = 2048`, `CLIPBOARD_LIMIT: usize = 16 << 20`, `BUSY_RETRIES: u32 = 5`, `CSV_HEADER_SAMPLE: usize = 1000`. In `source.rs`, exactly the items of design §5.1. In `csv_cells.rs`: `pub fn convert(cell: &str, ty: Option<&str>) -> Option<Value>`, `pub fn guess(cell: &str) -> Value`, `pub fn nest(doc: &mut Map<String, Value>, path: &str, value: Value)`. `Source`, `Format`, `CsvOptions`, `ReadOptions` derive `Debug, Clone, PartialEq`. `CsvOptions::default()` is `,` with header.

Behaviour, per §5.1 and §5.2. Before writing code, read `~/.cargo/registry/src/*/csv-1.4.0/src/reader.rs` (present after Task 1's `cargo fetch`) for `ReaderBuilder::{delimiter, has_headers, flexible}`, `read_byte_record`, `ByteRecord::position`, `Position::{line, byte}`, and how the header BOM is treated; the test `csv_header_loses_a_bom` decides whether code must strip it.

- `read_documents` opens `Source::File` with `std::fs::File`, wraps it in `RecordLimit` and `BufReader::with_capacity(64 * 1024, …)`. `Source::Bytes` uses `Cursor<Arc<[u8]>>`.
- `RecordLimit<R>`: `Read` wrapper with a shared `Rc<Cell<u64>>` counter, `reset()` after each record, and an `io::Error` of kind `InvalidData` with a private marker past `MAX_DOC_BYTES`, which maps to `ReadError::RecordTooLarge { line }`.
- JSONL: own loop over `BufRead::fill_buf` and `consume`, so that a too-long line is skipped without buffering. `offset` is the byte position after the record, exact. Strip a leading BOM from line 1, then trailing `\r`, then edge ASCII whitespace; a whitespace-only line is skipped, and the line counter still advances. Invalid UTF-8 gives `Rejected { raw: lossy, reason: InvalidUtf8 }`; over `MAX_DOC_BYTES` gives `Rejected { raw: first 2,048 bytes lossy, reason: TooLarge(len) }`.
- JSON array: `serde_json::Deserializer::from_reader(reader).deserialize_seq(visitor)` with a `Visitor` whose `visit_seq` calls `next_element::<Value>()` in a loop and `sink(Record { line: i + 1, offset: reader_bytes, doc: Ok(value.to_string()) })`. `ControlFlow::Break` ends through a private serde error mapped back to `Ok(())`. Afterwards `Deserializer::end()`. A serde error gives `ReadError::Json { line, column, message }`. `offset` comes from an `Arc<AtomicU64>` in a small counting wrapper, so it is approximate by the buffer size.
- CSV: conversion through `convert` with the field's type from `options.fields`, nesting by `nest` for a header holding `.` unless `fields` has that exact name. `id` stays text. Header cells that repeat are a `ReadError::Csv`. Rows longer than the header get `column_{n}`.

- [ ] **Step 1: Write the failing tests:**
  - `sniff`: table of `.csv`, `.tsv`, `[`, BOM then `[`, `{`, whitespace then `[`, no path.
  - JSONL: `blank_lines_are_skipped_but_counted` (lines 1, 3); `whitespace_only_line_is_skipped`; `crlf_and_edges_are_stripped`; `bom_goes`; `last_line_needs_no_newline`; `invalid_utf8_is_rejected_and_never_a_document`; `too_long_line_is_rejected_and_the_next_line_survives` (`MAX_DOC_BYTES + 1` bytes, then a good line); `raw_text_is_unchanged` (`{"n":1.10}` keeps `1.10`); `offset_is_exact`; `break_stops_reading`.
  - JSON array: `elements_come_one_by_one`, `pretty_printed_elements_become_one_line`, `broken_json_stops_with_line_and_column_after_the_good_elements`, `top_level_object_is_an_error`, `empty_array_gives_nothing`, `oversized_element_is_record_too_large`.
  - CSV: `header_names_the_keys`; `no_header_uses_schema_order_then_column_n`; `semicolon_delimiter`; `quoted_comma_and_newline`; `short_row_leaves_keys_out`; `long_row_gets_column_n`; `empty_cell_leaves_the_key_out`; `duplicate_header_is_an_error`; `csv_header_loses_a_bom`; `invalid_utf8_row_is_rejected`; `stray_quote_stops_with_record_too_large` over a generated stream of 40 MiB, asserting the error; `dotted_header_nests`; `dotted_header_stays_flat_when_the_schema_has_it`.
  - `convert` table from design §5.2, including `007` for `int32` staying text, `9223372036854775808` staying text, `NaN` for `float` staying text, `TRUE` for `bool`, `["a"]` for `string[]`, `x` for `string[]` staying text. `guess` table.
- [ ] **Step 2: Run** `cargo test -p tessera-core transfer`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(core): read JSONL, JSON arrays and CSV as streams`.

### Task 4: Schema inference and preview

**Files:**
- Create: `crates/tessera-core/src/transfer/infer.rs`, `preview.rs`
- Test: inline

**Interfaces:** exactly design §5.3. `to_draft` fills `SchemaDraft { name, fields, enable_nested_fields: nested, ..Default::default() }` with `FieldDraft::new(name, FieldType::…)`, `optional` set. `DocIssue` is `tessera_core::documents::validate::DocIssue`; `cell_preview` is `tessera_core::documents::page::cell_preview`.

- [ ] **Step 1: Write the failing tests:**
  - `integers_are_int64`; `integer_and_float_mix_to_float`; `huge_number_is_float`; `bool_stays_bool`; `string_and_number_are_string_with_a_note`; `bool_and_number_are_string_with_a_note`.
  - `arrays_take_the_element_type` for `string[]`, `int64[]`, `float[]`, `bool[]`, `object[]`; `empty_arrays_are_string_arrays_with_a_note`.
  - `object_sets_nested`; `missing_or_null_is_optional`; `only_null_is_optional_string_with_a_note`.
  - `coordinate_pairs_are_suggested_not_decided`: `[[48.1,11.5]…]` stays `float[]`, with `MaybeGeopoint("loc")`; `[100,200]` gets none; `[1,2,3]` none; `{"lat":1,"lng":2}` is `object`.
  - `id_makes_no_field`; `numeric_id_notes`; `skipped_names` for `""`, `"x*"`, `".*"`, `" a"`, `"a\nb"`.
  - `first_seen_order`; `sampled_stops_at_1000` (a type change on row 1,001 does not count).
  - `csv_cells_are_guessed` through `ReadOptions` with no `fields`: `12`, `1.5`, `true`, `007`.
  - `to_draft_passes_validate`: `schema::validate` gives no errors on a draft from a rich sample. `to_draft_json_is_the_schema_the_server_takes` is covered in Task 7's live test.
  - `collection_name_from`: `my data.v2.jsonl` to `my_data_v2`, `日本.csv` to `imported` after replacing, `-x.json` to `-x`.
  - `preview_reads_20_rows_and_estimates`: a 100-line source is `complete: false` for 20 rows when the file is larger, `estimated_docs` within 10 % of the truth; a 5-line source is `complete` and exact.
  - `preview_validates_against_the_schema`; `preview_row_with_bad_json_carries_the_message`; `preview_of_rejected_row_says_why`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** One `Seen` state per key: flags for bool, integer, float, string, object, plus array element flags, a count of rows with a non-null value, and whether every array was a 2-number coordinate pair. Rows are parsed with `serde_json::from_str::<Value>`; a line that does not parse is skipped from inference and counted out of `sampled`.
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(core): infer a schema and preview a file`.

### Task 5: Import pipeline and report

**Files:**
- Create: `crates/tessera-core/src/transfer/import.rs`, `report.rs`
- Test: `crates/tessera-core/tests/transfer_import.rs` (new, wiremock), inline in `report.rs`

**Interfaces:** exactly design §5.4 and §5.5, plus:

```rust
#[derive(Clone, Default)] pub struct JobControl { pub stop: CancellationToken, pub abort: CancellationToken }
pub enum TransferError { Client(tessera_client::Error), Read(ReadError), Report(String), AnswerMismatch { sent: usize, answered: usize }, UnsafeField(String), Io(String) }
```
`ImportProgress` and `ExportProgress` derive `Clone, Debug, PartialEq, Default`, and `Phase` defaults to `Running`. `CancellationToken` is `tokio_util::sync::CancellationToken` (`tokio-util-0.7.20/src/sync/mod.rs`, no feature needed). `TransferError` implements `Display` through thiserror.

Behaviour, per §5.4:

- Reader side, inside `spawn_blocking`: one `Batch { body: String, lines: Vec<u64>, rejected: Vec<ReportRow>, end_offset: u64 }`. Add a `Record`: `Ok(text)` appends the text and `\n`, `Err(rejected)` goes to `rejected` with `code: None`. Full at `batch_size` documents or at `BATCH_BYTES` after the append. Send through `tokio::sync::mpsc::channel(CHANNEL_BATCHES)` with `blocking_send`; a send error returns `ControlFlow::Break`. A `ReadError` is sent last, as `Message::End(Result<(), ReadError>)`.
- Sender loop as in §5.4, one batch at a time. A batch with an empty body sends no request.
- Busy retry: `Err(Error::Http { status: 503, .. })` waits 1, 2, 4, 8, 16 s via `select!` on `stop` and `abort`, then resends the same body, at most `BUSY_RETRIES` times. On `stop` during the wait, nothing of that batch was applied, so its documents are neither sent nor unconfirmed and the job ends `Stopped`.
- `sent` counts documents handed to a request that got an answer or was aborted.
- `stopped_at_line`: the first line of the first batch not answered, on `Stopped` and `Failed`.
- `Report::new()` uses `tempfile::tempfile()`, wrapped by `tokio::fs::File::from_std` for writes. `add` writes one JSON line per row and keeps the first `REPORT_ROWS` with `raw` cut at a char boundary below `REPORT_PREVIEW_BYTES`. `finish` flushes. `save_report` and `save_failed` read a `try_clone` from offset 0 line by line through a `tokio::io::BufReader`, write `{path}.{8 hex}.part` with `create_new`, `flush`, `sync_all`, `rename`; `save_failed` writes each row's `raw` and `\n`. On any error the part file is removed. A `Report` is `Send + Sync` and is shared as `Arc<Report>`.
- Progress: `send_replace` after every batch and on phase change. `rate` is `(imported + failed) / elapsed`. `eta` is `elapsed × (total − done) / done` by bytes, `None` before the first batch or without a total. `fraction` is `bytes_done / total`, clamped, `None` without a total.
- `CountCheck` per §5.4, read with `client.collection(&job.collection)`.

- [ ] **Step 1: Write the failing tests** in `tests/transfer_import.rs`, with a wiremock `Respond` that answers one `{"success":true}` per body line and records bodies:
  - `sends_one_post_per_batch`: 2,500 lines, `batch_size` 1,000: 3 requests, sizes 1,000, 1,000, 500; `batch_size=1000` in each query.
  - `batch_stops_at_batch_bytes`: documents of 6 MiB: 3 per request.
  - `report_lines_survive_blank_lines`: answers fail the 2nd document of a source with a blank line before it; the report says line 3.
  - `rejected_line_is_reported_and_never_sent`: invalid UTF-8 absent from every body, row with its line.
  - `only_rejected_batch_sends_nothing`.
  - `counts_add_up`: imported + failed + unconfirmed equals documents.
  - `first_error_is_published_while_running`.
  - `stop_starts_no_new_post`: responder delays 200 ms and records start times; `stop` after the first request began; after return exactly 1 request happened, its documents count, `end` is `Stopped`, `stopped_at_line` is the next line.
  - `stop_before_start_sends_nothing`.
  - `abort_counts_unconfirmed`: `stop` then `abort` during a 10 s responder; returns at once; `unconfirmed` equals the batch.
  - `fewer_answers_are_unconfirmed`; `more_answers_fail_the_job`.
  - `collection_gone_fails_with_the_message`: 404; earlier batches stay counted.
  - `read_only_fails_the_job`: `client.set_read_only(true)` after the first batch; second gives `Failed(Client(ReadOnly))`.
  - `busy_server_is_retried`: 503 twice, then 200 (with `tokio::time::pause`); the document count is not doubled.
  - `stop_during_backoff_ends_stopped`.
  - `broken_json_array_stops_after_the_good_batches`.
  - `backpressure_keeps_the_reader_close`: responder stalls; after 500 ms the progress and a counting reader show at most 3 batches read.
  - `count_check_flags_a_lazy_server`: `create`, server answers all success, `num_documents` unchanged: `ok == false`.
  - Inline in `report.rs`: `spool_holds_more_than_the_preview`, `raw_is_cut_on_a_char_boundary`, `saves_twice`, `save_failed_writes_raw_lines`, `failed_save_leaves_no_part_file`.
- [ ] **Step 2: Run** `cargo test -p tessera-core --test transfer_import`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(core): import pipeline with an error report`.

### Task 6: Export pipeline

**Files:**
- Create: `crates/tessera-core/src/transfer/export.rs`
- Modify: `crates/tessera-core/src/documents/export.rs` (`fn field` becomes `pub(crate) fn csv_field`)
- Test: `crates/tessera-core/tests/transfer_export.rs` (new, wiremock), inline

**Interfaces:** exactly design §5.6. `Phase` and `JobEnd` come from `import.rs`, re-exported in `transfer`.

Behaviour, per §5.6:

- Reject first: every name in `FieldPick` passes `field_safe`, else `Failed(TransferError::UnsafeField(name))`, and nothing is sent or created. `field_safe`: not empty, `name.trim() == name`, no `,`, `` ` ``, `*`, and no control character.
- Part file: `{file_name}.{8 hex of a v4 uuid}.part` beside the target, `tokio::fs::OpenOptions::new().write(true).create_new(true)`, `tokio::io::BufWriter::with_capacity(64 * 1024, …)`. A `PartFile` guard removes it on `Drop` through `std::fs::remove_file` unless persisted. Success: `flush`, `sync_all`, `tokio::fs::rename`.
- `Selection`: `id_batches(&ids, ID_FILTER_BYTES)` from `documents::ids`; `filters` each stream with `filter_by`; `single` each `client.document(c, id)`, with a 404 skipped and counted by the count check, and projected by `FieldPick` at the top level. Its `expected` is the number of ids.
- Progress: `send_replace` every 256 documents and at the end.
- `stop` is tested per line with `is_cancelled()`.
- CSV per §5.6: documents parse with `serde_json::from_str::<Map<String, Value>>`. A line that does not parse fails the job with its text. Header order: `id` if present, then first-seen keys of the sample. An empty result writes nothing. Rows use `csv_field`, with `\n` ends.

- [ ] **Step 1: Write the failing tests:**
  - `jsonl_is_written_as_it_came`: bytes equal lines with `\n`.
  - `json_array_parses`: `serde_json::from_slice::<Vec<Value>>`; also for 0 and 1 documents.
  - `csv_flattens_and_quotes`: `{"id":"1","a":{"b":"x,y"},"t":["p"]}` gives header `id,a.b,t` and the row `1,"x,y","[""p""]"`.
  - `csv_header_comes_from_the_first_1000`: a key first seen at document 1,001 raises `unlisted_values` and adds no column.
  - `include_and_exclude_reach_the_query`; `filter_reaches_the_query`.
  - `unsafe_field_is_refused_and_sends_nothing`.
  - `selection_runs_one_stream_per_filter_and_single_for_backticks`: ids `a`, `` b`c `` give one export request and one `GET`, and a pick projects the single document.
  - `stop_removes_the_part_and_keeps_the_target`: target exists with old bytes; stop mid-way; old bytes stay and no `.part` remains in the directory.
  - `failure_midway_removes_the_part_and_keeps_the_target`: the mock closes the connection after a partial chunked body.
  - `success_replaces_the_target`.
  - `missing_directory_fails_before_any_request`.
  - `expected_mismatch_is_reported` by comparing `docs` and `expected` in the outcome.
  - Inline: `field_safe` table; `flatten` table (nested, arrays, `null`, empty object).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Live test** in `crates/tessera-core/tests/live_transfer.rs` (`#[ignore]`, env as `live_export.rs`), on throwaway collections against 18108: 2,000 generated documents with a nested object, an array and a geopoint export as JSONL, array and CSV and import into a second collection each, with equal counts and, for JSONL, equal documents after sorting by id; a CSV whose schema needs conversions; failed lines saved and re-imported after fixing the schema; a schema built by `infer_schema(...).to_draft(...)` from a rich sample is accepted by `create_collection`.
- [ ] **Step 5: Run** the checks, CHANGELOG, commit. `feat(core): export to JSONL, JSON and CSV files`.

### Task 7: NF3 measurement

**Files:**
- Create: `xtask/src/nf3.rs`
- Modify: `xtask/src/main.rs`, `xtask/Cargo.toml` (`tessera-client`, `tessera-core`, `secrecy`, `tokio` features), `.github/workflows/nightly.yml`, `docs/building.md`

**Interfaces:** `cargo run --release -p xtask -- nf3 --url <url> [--key <key>] [--docs 1000000] [--format jsonl|json|csv] [--app-baseline-mb N] [--limit-mb 300] [--keep-file <path>]`. `main.rs` dispatches on the first argument and extends `USAGE`; an unknown task still exits 2. Key default `xyz`, no default URL.

Behaviour, per §12:

- Runtime: `tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all()`, as the app.
- Client: `Client::new(&[url], key, ClientOptions::default())` with `with_hook` pushing events into a `tessera_core::log::RequestLog` behind a `Mutex`, with the same capacities as `AppState` uses (5,000 and 500); read the constructor in `crates/tessera-core/src/log.rs`.
- File: new `tempfile::tempdir()`, or `--keep-file <path>`, which is left in place. Documents written through a 1 MiB `BufWriter`: `{"id":"<i>","title":"doc <i>","n":<i>,"payload":"<950 x>"}`. `json` wraps in `[`, `,`, `]`. `csv` writes a header row, four columns.
- Collection `tessera_nf3`: `title: string`, `n: int64`, `payload: string` with `index: false, optional: true`. An existing collection of that name stops the task with exit 1, before anything is touched. `tessera_nf3_cancel` likewise.
- Sampler: `std::thread`, `Command::new("ps").args(["-o", "rss=", "-p", &pid])` every 200 ms, keeping the maximum, in KiB, stopped by an `AtomicBool`.
- Baseline: one sample after the client is built and the file written, before the import.
- Import run: `batch_size` 1,000, `Create`, `CoerceOrReject`. Assert `Finished`, `imported == docs`, `failed == 0`, `unconfirmed == 0`, count check ok.
- Cancel run: `min(docs, 200_000)` documents, `control.stop.cancel()` once `imported >= n / 2`, from a task watching the progress. Record `Instant` of the stop. Pass when no log event with `path` ending `documents/import` has a `started_at` later than the stop's `SystemTime`, and `end` is `Stopped`. Print the stop latency.
- Cleanup in a guard path that also runs on failure: delete both collections, `tempdir` by drop.
- Output: one line of JSON with keys `docs, format, seconds, docs_per_second, peak_rss_mb, baseline_rss_mb, engine_rss_mb, app_baseline_mb, total_rss_mb, limit_mb, slowest_batch_ms, stop_latency_ms, pass`. `slowest_batch_ms` is the longest logged import `duration`. Exit 1 when not `pass`.

- [ ] **Step 1: Write the failing tests** in `nf3.rs`: `options_parse`, `missing_url_is_an_error`, `document_is_about_one_kilobyte`, `json_and_csv_writers_round_trip` through `read_documents`, `verdict_adds_the_baseline`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Measure on 18108:**

```bash
cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000
cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000 --format json
cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000 --format csv
cargo build --release -p tessera-app
TESSERA_SMOKE=1 /usr/bin/time -l target/release/tessera
```

  Expected from the design's probe: about 40 s of server time per run, plus reading. Take the smoke run's "maximum resident set size" in bytes, divide by 1,048,576, and rerun the first command with `--app-baseline-mb`. It must exit 0. If not, stop and raise it: design §14 names the first fallback.
- [ ] **Step 4: Nightly.** New job `nf3` in `nightly.yml`, `ubuntu-24.04`, `services.typesense` image `typesense/typesense:30.2` with `--data-dir /tmp --api-key=xyz --enable-cors=false` and port 8108, wait for `/health` as the cluster job does, the same Linux dependencies and `xvfb` as the smoke step in `ci.yml` line 98, `cargo build --release -p tessera-app`, then:

```bash
kb=$(TESSERA_SMOKE=1 xvfb-run -a /usr/bin/time -f %M target/release/tessera 2>&1 >/dev/null | tail -1)
cargo run --release -p xtask -- nf3 --url http://localhost:8108 --app-baseline-mb $((kb / 1024))
```

  Port 8108 there is the job's own container.
- [ ] **Step 5: Docs.** `docs/building.md` gains "Measuring NF3" with the commands above, what the number includes, and the macOS and Linux forms of the baseline. CHANGELOG. Record the figures in the PR description.
- [ ] **Step 6: Run** the checks, commit. `feat(xtask): measure NF3`.

---

## PR B · Import (branch `feat/m5-import`)

### Task 8: Jobs entity and backend seam

**Files:**
- Create: `crates/tessera-app/src/transfer.rs`, `crates/tessera-app/src/jobs.rs`
- Modify: `crates/tessera-app/src/lib.rs` (modules, globals, `jobs::init(cx)`), `crates/tessera-app/tests/common/mod.rs`, `docs/architecture/async.md`
- Test: `crates/tessera-app/tests/ui_jobs.rs` (new)

**Interfaces:**
- `TransferBackend`, `TransferBackendGlobal(pub Arc<dyn TransferBackend>)`, `tokio_backend(handle)` per design §7.1. `TokioBackend` lives in `collections.rs`; reuse `self.run(…)` as `documents.rs` does. `preview` and `infer` call `tokio::task::spawn_blocking` inside it.
- `jobs.rs`:

```rust
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(34);
pub struct JobId(pub u64);
pub enum JobKind { Import, Export }
pub enum Live { Import(ImportProgress), Export(ExportProgress) }
pub enum JobStatus { Running, Stopping, Finished, Stopped, Failed(SharedString) }
pub enum JobOutcome { Import(ImportOutcome), Export(ExportOutcome) }
pub struct Job { pub id: JobId, pub kind: JobKind, pub connection: Uuid, pub connection_name: SharedString,
    pub environment: Environment, pub collection: String, pub label: SharedString, pub status: JobStatus,
    pub live: Live, pub control: JobControl, pub outcome: Option<JobOutcome>, pub report_saved: bool, /* tasks */ }
pub struct Jobs { /* Vec<Job> */ }
pub struct JobsGlobal(pub Entity<Jobs>);
pub struct RunningChanged(pub usize);
pub struct JobFinished(pub JobId);
pub fn init(cx: &mut App);
impl Jobs {
    pub fn start_import(&mut self, connection: &Entity<ConnectionState>, job: ImportJob, label: SharedString, cx: &mut Context<Self>) -> JobId;
    pub fn start_export(&mut self, connection: &Entity<ConnectionState>, job: ExportJob, label: SharedString, cx: &mut Context<Self>) -> JobId;
    pub fn stop(&mut self, id: JobId, cx: &mut Context<Self>);    // first press: stop; Running becomes Stopping
    pub fn abort(&mut self, id: JobId, cx: &mut Context<Self>);   // Stopping only
    pub fn dismiss(&mut self, id: JobId, cx: &mut Context<Self>); // finished only
    pub fn running(&self) -> usize;
    pub fn jobs(&self) -> &[Job];
}
```

Behaviour, per §7.2:

- `start_import` reads `connected.client` from the connection (clone; flags are shared), makes `watch::channel(default)`, stores two tasks on the `Job`: the outcome task and the throttle task of §7.2. On outcome: set `status` by `JobEnd`, map a `TransferError` to text with `long_error_text(err, &profile)` for `Client` and one `t!` key per other variant; on an import call `connection.update(|c, cx| { c.collection_changed(name, cx); c.invalidate_collections(cx) })` through a `WeakEntity`, ignoring a gone connection; emit `JobFinished`, and `RunningChanged` when the count changed.
- Quit: with the first job, register `cx.on_app_quit` that calls `control.stop.cancel()` on all running jobs and returns a future waiting for the outcome tasks' completion signals (`tokio::sync::oneshot` per job), from the background executor pattern in `state.rs` line 234. GPUI bounds it at `SHUTDOWN_TIMEOUT`.
- Test fake in `common/mod.rs`: `FakeTransfer` with scripted `preview`, `infer`, and for `import`/`export` a future that holds the `watch::Sender` so the test can drive it (`FakeTransfer::progress(id, …)`, `finish(id, outcome)`), and records calls. `fake_transfer(cx)` installs it and calls `jobs::init`.

- [ ] **Step 1: Write the failing tests** in `ui_jobs.rs`:
  - `job_survives_its_starter`: start from a test view, drop the view, `finish` still lands.
  - `disconnect_does_not_stop_a_job`.
  - `progress_is_throttled`: push 1,000 updates over one simulated second with `cx.executor().advance_clock`; a counter in an observer of `Jobs` sees at most 30.
  - `newest_progress_wins`: the shown value is the last one.
  - `progress_does_not_notify_the_root`: a root render counter stays; `RunningChanged` fires only on count changes.
  - `stop_then_abort`: `stop` makes `Stopping` and cancels the token; `abort` only then.
  - `finish_reloads_the_collection`: `CollectionChanged` once.
  - `failure_text_is_kept_whole`.
  - `dismiss_refuses_a_running_job`.
  - `quit_stops_running_jobs`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_jobs`. Expected: compile errors.
- [ ] **Step 3: Implement.** In `docs/architecture/async.md`, replace the last bullet of "Rules" with a section "Jobs" per design §11.
- [ ] **Step 4: Run** the checks. No CHANGELOG entry yet: nothing shows. Commit. `feat(app): jobs that outlive their views`.

### Task 9: Jobs panel, dock and status bar

**Files:**
- Create: `crates/tessera-app/src/views/jobs/mod.rs`, `panel.rs`
- Modify: `crates/tessera-app/src/views/mod.rs`, `views/root.rs`, `views/status_bar.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_jobs.rs`, `ui_log.rs`, `ui_shell.rs`

**Interfaces:**
- Actions `ToggleJobs`, `CancelJob`, `DismissJob`, `SaveReport`, `SaveFailedLines`; `pub const JOBS: &str = "Jobs";`. Bindings: `TOGGLE_JOBS` = `cmd-shift-j` / `ctrl-shift-j` global; `CANCEL_JOB` = `cmd-.` / `ctrl-.`, `StopWaiting` on `STOP_WAITING`, `DismissJob` on `DELETE`, `SaveReport` on `SAVE_QUERY`, `SaveFailedLines` on `SORT`, all in `JOBS`.
- `JobsPanel`: `BasePanel` and `Panel` as `RequestLogPanel` in `views/request_log.rs`, `panel_name` `"Jobs"`, `closable` false, title `t!("jobs.title")`.
- Test ids: `jobs.row.{id}`, `jobs.progress.{id}`, `jobs.text.{id}`, `jobs.error.{id}`, `jobs.cancel`, `jobs.stop_waiting`, `jobs.dismiss`, `jobs.save_report`, `jobs.save_failed`, `jobs.import_saved`, `jobs.report.row.{n}`, `status.jobs`.
- i18n (en): `jobs.title` "Jobs"; `jobs.import_row` "Import"; `jobs.export_row` "Export"; `jobs.import_live` "%{docs} documents · %{rate}/s · ETA %{eta} · %{errors} errors"; `jobs.export_live` "%{docs} of %{total} · %{bytes}"; `jobs.cancel` "Cancel"; `jobs.stop_waiting` "Stop waiting"; `jobs.stay_note` "Batches already imported stay."; `jobs.done_import` "Imported %{imported} · Failed %{failed} · Not confirmed %{unconfirmed}"; `jobs.stopped_at` "Stopped before line %{line}."; `jobs.count_warning` "The collection grew by %{grown}, the server reported %{imported}. Others may have written at the same time."; `jobs.count_unchecked` "The document count could not be checked."; `jobs.report_more` "Showing %{shown} of %{total}. Save the report for all."; `jobs.save_report`, `jobs.save_failed`, `jobs.saved` "Saved %{count} lines to %{path}"; `jobs.import_saved` "Import this file"; `jobs.dismiss`, `jobs.dismiss_title`, `jobs.dismiss_body` "The report of this job is not saved and will be gone."; `jobs.empty` "Imports and exports run here and keep running when you close their tab."; `jobs.toast_import` "Import of %{name} finished: %{imported} imported, %{failed} failed."; `jobs.toast_export` "Export of %{name} finished: %{docs} documents."; `jobs.toast_failed` "%{name} failed. See Jobs."; `status.jobs` "%{count} jobs"; `status.jobs_idle` "Jobs". German with the same keys.

Behaviour, per §7.3. `DataTable` over jobs and a second one over report rows, in a `v_resizable` split, as `request_log.rs` does with `h_resizable`. Live cells recompute when the panel renders; the table delegate reads `Jobs`. `Progress::new(id).value(pct).accessibility_label(…)` from `gpui-component-0.9.0/src/progress/progress.rs`. Format numbers and sizes with the existing helpers of the log panel; ETA as `m:ss`, or `h:mm:ss`.

`root.rs`: the bottom dock becomes `DockLayout::tabs().panel_view(panel_handle(log), cx).panel_view(panel_handle(jobs), cx).active_index(0)` (`gpui-base-0.9.0/src/dock/layout/builder.rs`). One helper `toggle_bottom(&mut self, panel: PanelId, window, cx)`: closed opens and selects, open and active closes (focus handling as today's ⌘J handler at line 548), open and other tab selects. Read the active panel through the `TabGroup` API, `gpui-base-0.9.0/src/dock/tab_group.rs` lines 191 to 202. `show_jobs` opens and selects without moving focus. The root subscribes to `Jobs` for `RunningChanged` (`cx.notify()`) and `JobFinished` (toast via `window.push_notification` unless the Jobs tab is visible). `status_bar::render` gets `jobs: Option<usize>`: `None` while the list is empty.

- [ ] **Step 1: Write the failing tests:**
  - Panel: `running_import_shows_bar_rate_eta_and_errors`; `unknown_total_shows_no_bar`; `row_shows_connection_and_environment`; `first_error_shows_in_full`; `cancel_button_and_shortcut_stop_the_job`; `stopping_offers_stop_waiting`; `finished_shows_the_counts_and_the_note`; `count_warning_shows`; `report_table_shows_rows_and_the_more_note`; `failed_job_shows_error_inline`; `dismiss_of_unsaved_report_asks_with_focus_on_cancel`; `dismiss_of_saved_job_does_not_ask`; `no_toast_while_the_tab_is_visible`; `toast_when_it_is_hidden`.
  - Save: `save_report_uses_the_picked_path`, with `cx.simulate_new_path_selection` (`gpui-pre-0.3.11/src/app/test_context.rs` line 397); `save_failure_shows_inline`; `saved_failed_lines_offer_import`, whose button stays out of the tree until Task 10 gives it a target, so add it there.
  - Dock: `command_j_still_toggles_the_log`; `shift_command_j_opens_jobs`; `shift_command_j_closes_jobs`; `command_j_on_jobs_tab_selects_the_log`; `start_opens_jobs_without_moving_focus`.
  - Status: `no_entry_without_jobs`; `count_shows_with_the_shortcut`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG ("Jobs panel"), commit. `feat(app): jobs panel`.

### Task 10: Import dialog

**Files:**
- Create: `crates/tessera-app/src/views/jobs/import_dialog.rs`
- Modify: `views/jobs/mod.rs`, `views/jobs/panel.rs` (`Import this file`), `views/documents/mod.rs` (toolbar, `on_drop`), `views/collections_list.rs` (toolbar, `on_drop`), `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_import_documents.rs` (new)

**Interfaces:**
- Actions `ImportDocuments`, `PickImportFile`, `PasteImport`, `StartTransfer`; `pub const IMPORT_DIALOG: &str = "DocumentImport";`, `const IMPORT_INPUT: &str = "DocumentImport > Input";`. Bindings: `IMPORT_DOCUMENTS` = `cmd-shift-i` / `ctrl-shift-i` in `COLLECTION` and `COLLECTIONS`; `PickImportFile` on `OPEN_SAVED`; `PasteImport` = `cmd-shift-v` / `ctrl-shift-v`; `StartTransfer` on `SAVE` in `IMPORT_DIALOG` and `IMPORT_INPUT`.
- `pub struct DocumentImportDialog`; `pub fn open(state, connection, target: Option<String>, source: Option<Source>, window, cx)`.
- Test ids: `transfer.import.choose`, `.paste`, `.format.{jsonl,json,csv}`, `.delimiter.{comma,semicolon,tab,pipe}`, `.header`, `.target`, `.new_name`, `.action.{create,upsert,update,emplace}`, `.dirty.{…}`, `.batch`, `.batch_error`, `.preview.row.{n}`, `.preview.issue.{n}`, `.preview.error`, `.source_error`, `.note`, `.start`.
- i18n (en) `transfer.import.title` "Import documents"; `.choose` "Choose file"; `.paste` "Paste"; `.file` "%{name} · %{size}"; `.clipboard` "Clipboard · %{size}"; `.clipboard_empty`; `.clipboard_too_large` "The clipboard holds %{size}. Use a file above 16 MiB."; `.not_a_file` "Choose a file, not a folder."; `.format`, `.delimiter`, `.header`, `.target`, `.new_collection` "New collection…", `.action`, `.action_help.{create,upsert,update,emplace}`, `.dirty`, `.batch`, `.batch_range` "Enter a whole number from 1 to %{max}."; `.preview` "First %{count} lines, about %{docs} documents"; `.preview_exact` "%{docs} documents"; `.rejected_utf8` "Not valid UTF-8. This line would be stored with changed text, so it is not sent."; `.rejected_large` "This line is %{size}. The limit is %{limit}."; `.cancel_note` "Cancelling later stops after the current batch. Batches already imported stay."; `.start` "Start"; `transfer.read_error.*` one per `ReadError` variant.

Behaviour, per §7.4. Reuse: `FieldTypeSelect` in `crates/tessera-widgets/src/field_type_select.rs` as the model for `SelectState`; `ButtonGroup` as `schema.mode` in `views/schema_dialog/mod.rs`; `cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt })` as `views/import_dialog.rs` line 67; `on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| …))` from `gpui-pre-0.3.11/src/elements/div.rs` line 1276 with `ExternalPaths::paths()`. Choosing a file reads its first 4 KiB and metadata inside `spawn_blocking`, then `sniff`.

The preview reruns through `preview` 150 ms after any relevant change, in a stored `Task` that replaces the last one. With no connection loaded list, `load_collection_list` runs on open. Start here validates only and calls a stub `start(window, cx)` that Task 11 fills: until then Start is not rendered for a New target, and for an existing target it runs `ensure_writable` then `Jobs::start_import`, then `window.close_dialog(cx)` and `RootView::show_jobs`.

A New target appears in Task 11, so this task has no "New collection…" entry.

- [ ] **Step 1: Write the failing tests:**
  - `opens_with_the_collection_as_target`; `shortcut_opens_it_from_documents_and_collections`; `shows_connection_and_environment`.
  - `choosing_a_file_sniffs_and_previews`, with `cx.simulate_path_prompt_response` on a temp file and `FakeTransfer`; `csv_options_show_only_for_csv`; `format_override_reruns_the_preview`; `stale_preview_is_dropped`.
  - `preview_lists_issues_and_rejections`; `read_error_is_inline_and_disables_start`.
  - `paste_uses_the_clipboard`, with a limit test; `empty_clipboard_says_so`.
  - `dropped_file_opens_the_dialog`, by `cx.simulate_event(FileDropEvent::…)` as in `gpui-pre-0.3.11/src/window.rs` tests around line 8152, or by calling the handler if the test platform has no file drop; `folder_is_refused`.
  - `batch_size_bounds`.
  - `start_runs_the_job_and_closes`; `job_gets_the_chosen_options`: action, dirty, batch and CSV options reach `ImportJob`.
  - `locked_profile_starts_nothing_until_unlocked`, and Start shows the lock.
  - `cancel_changes_nothing`.
  - `every_control_shows_its_shortcut`.
  - In `ui_jobs.rs`: `saved_failed_lines_offer_import`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG ("Import documents"), commit. `feat(app): import documents from a file`.

### Task 11: Confirmation and new collections

**Files:**
- Modify: `crates/tessera-app/src/views/jobs/import_dialog.rs`, `views/schema_dialog/mod.rs`, `views/schema_dialog/review.rs` where it matches `Mode`, locales
- Test: `crates/tessera-app/tests/ui_import_documents.rs`, `ui_schema_dialog.rs`

**Interfaces:**
- `Mode::Pick(Rc<dyn Fn(Value, &mut Window, &mut App)>)` and `pub fn open_pick(state, connection, draft: SchemaDraft, on_pick: impl Fn(Value, &mut Window, &mut App) + 'static, window, cx) -> Entity<Self>`. Submit in `Pick` validates exactly as create, then calls `on_pick(draft.to_json())`, closes, and sends nothing, with no `ensure_writable`. The button reads `schema.use` ("Use schema").
- Test ids: `transfer.infer.note.{n}`, `transfer.infer.use_geopoint.{field}`, `transfer.import.edit_schema`.
- i18n: `transfer.infer.note.{maybe_geopoint,mixed,empty_array,only_null,skipped,non_string_id}`, `transfer.infer.sampled` "Inferred from %{count} rows."; `transfer.infer.use_geopoint` "Use geopoint"; `transfer.import.confirm.title`, `.consequence.target` "Imports into %{collection} on %{connection}."; `.consequence.source` "From %{name}, about %{docs} documents."; `.consequence.overwrite` "Existing documents with the same id are replaced or changed."; `.consequence.stay` "Batches already imported stay when you cancel."; `.consequence.new` "Creates the collection %{collection} with %{fields} fields first."; `.action` "Import"; `transfer.import.no_update_new` "Update and emplace need an existing collection."; `transfer.import.create_failed`.

Behaviour, per §7.5 and §7.6:

- Target select gains "New collection…". Choosing it runs `infer`, keeps the `InferredSchema`, proposes `collection_name_from(path)` or `imported`, and validates the name by the rules of `schema::validate` inline.
- `update` and `emplace` are disabled for New, and a selected one falls back to `create`.
- The preview validates against a `Collection` built from the draft's JSON. If that does not decode, it runs without a schema.
- `Use geopoint` sets that `FieldDraft.r#type` to `FieldType::Geopoint`, removes the note, and reruns the preview.
- Start: validate, `ensure_writable`, D49 table with `danger_level(preview.estimated_docs, prod)` from `documents::export`, the consequences above, then for New `CollectionsBackend::create(client, schema)`, on failure inline in the dialog and no job, then `Jobs::start_import`. `ConfirmDanger`'s `on_confirm` returns `Task<Result<(), SharedString>>`, whose `Err` shows the create failure inside it.
- A clipboard source has no file name: consequences say "From the clipboard".

- [ ] **Step 1: Write the failing tests:**
  - D49 table: `create_on_local_starts_without_a_dialog`; `create_on_prod_confirms_with_focus_on_cancel`; `upsert_under_1000_confirms`; `upsert_over_1000_types_the_name`; `update_on_prod_types_the_name`; `new_collection_on_local_starts_without_a_dialog`; `new_collection_on_prod_confirms`.
  - `cancelling_the_confirmation_sends_nothing`; `consequences_name_count_and_collection`.
  - `new_shows_the_inferred_fields_and_notes`; `new_proposes_a_safe_name`; `update_is_disabled_for_new`.
  - `nothing_is_created_before_start`: open, infer, edit, cancel; the fake's `create` calls stay 0.
  - `start_creates_then_imports`, in that order; `create_failure_stays_in_the_dialog`.
  - `use_geopoint_changes_the_draft`.
  - `edit_schema_opens_pick_mode`; `pick_mode_sends_nothing`; `escape_closes_only_the_schema_dialog`; `invalid_draft_cannot_be_picked`; `read_only_profile_can_edit_schema_without_unlocking`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): confirm imports and create collections from a file`.

---

## PR C · Export (branch `feat/m5-export`)

### Task 12: Export dialog

**Files:**
- Create: `crates/tessera-app/src/views/jobs/export_dialog.rs`
- Modify: `views/jobs/mod.rs`, `views/documents/mod.rs` (toolbar), `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_export_documents.rs` (new)

**Interfaces:**
- Action `ExportDocuments`, binding `cmd-shift-x` / `ctrl-shift-x` in `COLLECTION`. `pub const EXPORT_DIALOG: &str = "DocumentExport";`, `EXPORT_INPUT`. `StartTransfer` on `SAVE` in both. `OpenExport` stays the search's code export: do not reuse it.
- `pub fn open(state, connection, collection: Collection, filter: Option<String>, selection: Vec<String>, window, cx)`.
- `pub fn field_names(c: &Collection) -> Vec<String>` in `export_dialog.rs`: `id`, then each field name up to its first `.`, deduplicated, skipping names with `*`.
- Test ids: `transfer.export.scope.{all,filter,selection}`, `.filter`, `.count`, `.fields.{all,include,exclude}`, `.field.{name}`, `.format.{jsonl,json,csv}`, `.no_id_note`, `.start`, `.error`.
- i18n (en) `transfer.export.title` "Export documents"; `.scope.*`; `.count` "Exports %{count} documents"; `.count_selection` "%{count} selected"; `.fields.*`; `.field_unsafe` "A name with a comma, backtick or asterisk cannot be sent."; `.empty_pick` "Pick at least one field."; `.no_id_note` "Without id, importing this file makes new ids."; `.format.*`; `.csv_note` "The header comes from the first 1,000 documents. Later new keys are left out and counted."; `.start` "Export…"; `jobs.export_unlisted` "%{count} values were left out: their keys came after the header was written."; `jobs.export_mismatch` "Exported %{docs} of %{expected}. Others may have written at the same time."

Behaviour, per §7.7. The count follows `views/documents/bulk.rs` line 111 and its 300 ms debounce, through `DocumentsBackendGlobal`'s `count`, and the `FilterInput` setup there. `cx.prompt_for_new_path(&dir, Some(&name))` as `views/sidebar.rs` line 95, with `dir` from the same place. Extension `jsonl`, `json` or `csv`. Build `ExportJob` with `expected` from the count. The dialog needs the collection: from `DocumentsView`'s loaded schema, so the button and the shortcut are disabled through `gated` until it has loaded. It is a read: no lock, no `ensure_writable`, no confirmation. In the `selection` scope, the selection comes from `DocRows::selection.ids()`.

- [ ] **Step 1: Write the failing tests:**
  - `opens_on_selection_when_there_is_one`; `opens_on_filter_when_the_table_has_one`; `opens_on_all_otherwise`; `selection_scope_is_disabled_without_one`.
  - `count_runs_after_a_pause`; `stale_count_is_dropped`; `bad_filter_shows_inline_and_disables_start`; `zero_count_allows_start`.
  - `field_names_are_top_level`; `unsafe_field_is_disabled_with_the_reason`; `include_sends_include_fields`; `exclude_sends_exclude_fields`; `empty_pick_disables_start`; `include_without_id_notes_it`.
  - `start_asks_for_a_path_then_runs`; `suggested_name_has_the_extension`; `cancelled_path_changes_nothing`; `job_has_the_expected_count`.
  - `works_on_a_locked_profile`.
  - `finished_export_shows_unlisted_and_mismatch_warnings`, in `ui_jobs.rs`.
  - `every_control_shows_its_shortcut`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG ("Export documents"), commit. `feat(app): export documents to a file`.

### Task 13: Infer from file in the schema dialog

**Files:**
- Modify: `crates/tessera-app/src/views/schema_dialog/mod.rs`, `views/schema_dialog/sync.rs` if refilling needs an entry point, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_schema_dialog.rs`

**Interfaces:** action `InferFromFile`, on `OPEN_SAVED` in `SCHEMA_DIALOG`; test ids `schema.infer`, `schema.infer_error`, `transfer.infer.note.{n}`; i18n `schema.infer` "Infer from file", `schema.infer_replace_title`, `schema.infer_replace_body` "This replaces the fields of this draft."

Behaviour, per §7.8. Shown when `Mode::Create` or `Mode::Pick`, not in edit (UX rule 4). `cx.prompt_for_paths`, read the first 4 KiB for `sniff`, `TransferBackendGlobal`'s `infer` with default CSV options, then `to_draft(&current_name)`, and the existing draft-refill path of `sync.rs`, so the JSON tab follows. A draft with any named field asks through `Confirm`, focus on Cancel. The notes render above the tabs, with `Use geopoint` as in Task 11; extract that note list as one function in `views/jobs/mod.rs` and call it from both dialogs. A `ReadError` shows inline in `schema.infer_error`.

- [ ] **Step 1: Write the failing tests:** `infer_button_shows_in_create_and_clone_not_in_edit`; `infer_fills_the_form_and_the_json_tab`; `name_is_kept`; `nonempty_draft_asks_before_replacing_with_focus_on_cancel`; `declining_keeps_the_draft`; `read_error_is_inline`; `notes_show_and_use_geopoint_changes_the_type`; `cancelled_picker_changes_nothing`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): infer a schema from a file`.

### Task 14: Docs and the final NF3 record

**Files:**
- Modify: `docs/spec.md`, `docs/building.md`, `README.md` if it lists features, `CHANGELOG.md`, `crates/tessera-app/tests/ui_shell.rs` if the palette test lists actions

**Behaviour:**

- Run the §12 measurement again on the finished branch, with `--format jsonl`, `json` and `csv`, and the smoke baseline. Then one run through the UI by hand: `cargo run --release -p tessera-app -- --host localhost --port 18108 --api-key-stdin <<< xyz`, create the file with `xtask nf3 --keep-file <path>`; import into a new collection; sample `while :; do ps -o rss= -p $(pgrep -x tessera); sleep 1; done`; cancel a second run and note that the panel stopped within one batch. Delete the collection afterwards. Record all numbers, with the machine, in `docs/building.md` under "Measuring NF3" and in the PR description.
- `docs/spec.md`: replace "NF3 met" in the M5 row with the measured figure, as M3's and M4's rows do, or with what is still open.
- Check `docs/spec.md` §7.6, §7.7 and §14 against what shipped. Any difference gets a line there.
- `cargo xtask` lint for hardcoded strings stays green (NF8). `cargo deny check` passes.
- Check NF5 on the Linux build.

- [ ] **Step 1: Measure** as above.
- [ ] **Step 2: Update the docs.**
- [ ] **Step 3: Run** the checks, commit. `docs: M5 import and export`.
