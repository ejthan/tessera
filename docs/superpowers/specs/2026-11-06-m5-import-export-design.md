# M5 Import and export: design

- **Date:** 2026-11-06
- **Status:** written autonomously (maintainer takes the recommendations), pending final review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M5 row in §14; §7.6, §7.7, §7.3 schema inference, §4, §5.4, §8, §10 NF3, §12, §16)
- **Previous milestone:** [M4 search design](2026-11-06-m4-search-design.md)
- **UX mockups:** [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d), bottom panel

All behaviour marked "confirmed" was checked on 2026-11-06 with curl on `localhost:18108` (Typesense 30.2), on throwaway collections that are gone again. The points in §4.4 marked "both" also ran on a throwaway 30.0 container with the same result. Every GPUI Kit, reqwest and tokio-util API named here was read in the pinned source (`gpui-pre-0.3.11`, `gpui-base-0.9.0`, `gpui-component-0.9.0`, `reqwest-0.13.5`, `tokio-util-0.7.20`). `csv 1.4.0` is not in the registry cache yet; its API is named from its published docs and the first task reads it after the download.

## 1. Goal

M5 moves whole collections in and out: a file of a million documents goes in through a progress panel at constant memory, every failed line lands in a report that can be saved and imported again, a new collection can be created from a schema inferred from the file, and any collection, filter or selection can be written to a file. Jobs outlive the views that started them.

**Acceptance (product spec §14, §7.3, §7.6, §7.7, NF3):**

- NF3: 1 million documents (about 1 GB of JSONL) import with less than 300 MB RSS for the app, measured as in §12.
- Cancelling stops within one batch round trip: no `POST` starts after the stop, and the job ends when the one in flight has answered. Batches already imported stay, and the UI says so before the import starts.
- Memory does not depend on file size, for JSONL, JSON arrays and CSV.
- Schema inference tells `int64` from `float` and detects `bool`, `string[]`, nested objects and coordinate pairs (the last as a suggestion).
- Integration tests for every new endpoint are green against 30.0 and 30.2.
- In a read-only profile no import leaves the client until the user unlocks it.

## 2. Delivery

One spec, one plan, three pull requests in this order, each with green CI and its own CHANGELOG entries. Branch from `main` once M4 is merged, or from the tip of `feat/m4-palette` if it is not.

| PR | Branch | Content | Why this order |
|---|---|---|---|
| **A · Engine** | `feat/m5-engine` | Dependencies, client (`import_batch`, `ExportStream`), core `transfer` (readers, inference, preview, import, report, export), `cargo xtask nf3`, nightly job | No UI, so no dead control. NF3 is measured here, before anything is built on the engine |
| **B · Import** | `feat/m5-import` | Jobs entity and backend seam, jobs panel, status bar, import dialog, preview, confirmation, new collection, drag and drop, clipboard, report | The first job kind proves the panel |
| **C · Export** | `feat/m5-export` | Export dialog, Infer from file in the schema dialog, docs, final NF3 record | Reuses all of B |

## 3. Scope

### In M5

§7.6 and §7.7 without the backup assistant, including CSV export (P1 in §7.7, but §14 moves only the backup assistant away), schema inference (§7.3), the jobs panel in the bottom dock, and the `N jobs` entry of the status bar (§8.1).

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Backup and restore assistant | M8 | §14 |
| Reindex assistant | M6 | §14, with aliases |
| Resuming an interrupted import | not planned | Re-run with `upsert`, or import the saved failed lines |
| Compressed input or output (`.gz`) | v1.x | Not in §7.6 |
| Persisting finished jobs across restarts | not planned | Reports are session data (D47) |
| Client-side validation of every document | not planned | §7.6 asks for it on the preview; the server's per-line answer decides the rest (D27) |
| `remote_embedding_batch_size` | v1.x | No request for it; `batch_size` covers the common case |
| Failover during an export | v1.x | A stream cannot change node mid-way, and restarting needs a resumable cursor the API lacks. The next ordinary request fails over, and the job can be started again |

## 4. tessera-client

### 4.1 Dependencies

| Crate or feature | Used by | Why |
|---|---|---|
| `reqwest` feature `stream` | client | `Response::bytes_stream` for exports. Adds `tokio/fs` and edges to crates already in `Cargo.lock` |
| `tokio-util = "=0.7.20"`, features `codec`, `io` | client, core | `StreamReader`, `FramedRead`, `LinesCodec` with a maximum length, `CancellationToken`. Same version as `Cargo.lock` |
| `futures = { version = "=0.3.34", default-features = false, features = ["std"] }` | client | `StreamExt` over the byte stream. Same version as `Cargo.lock` |
| `csv = "=1.4.0"` | core | CSV import. New crates in the tree: `csv`, `csv-core`. License `Unlicense OR MIT`, which `deny.toml` allows through MIT |
| `tokio` features `fs`, `io-util` | workspace | Part files and report spool |
| `tokio`, `tokio-util`, `tempfile` as normal dependencies of `tessera-core` | core | The pipelines run there, and the report spool is an unnamed temp file |
| `tessera-core`, `tessera-client`, `secrecy` in `xtask` | xtask | §12 |

`csv` wins over `csv-async` because the JSON array reader must be synchronous anyway (§5.1), so one blocking reader task serves all three formats.

### 4.2 Import

```rust
pub enum ImportAction { Create, Upsert, Update, Emplace }
pub enum DirtyValues { CoerceOrReject, CoerceOrDrop, Drop, Reject }
pub struct ImportParams { pub action: ImportAction, pub batch_size: u32, pub dirty_values: DirtyValues }
pub struct ImportLine { pub success: bool, pub code: Option<u16>, pub error: Option<String>, pub id: Option<String> }

impl Client {
    pub async fn import_batch(&self, collection: &str, params: &ImportParams, body: &str)
        -> Result<Vec<ImportLine>, Error>;
}
```

- `POST collections/{c}/documents/import?action=&batch_size=&dirty_values=&return_id=true`, content type `text/plain`, active node, no failover (`POST` is not idempotent), `LONG_TIMEOUT`, through `send_with`, so the read-only guard applies with no new path. `dirty_values` is always sent (§4.4).
- Each batch is one `POST` (D45). The whole answer is at most one line per document without an echo, so `response.text()` stays fine here, and no streamed request body is needed.
- Enums are written from fixed strings, so nothing typed reaches the query.
- A `2xx` decodes line by line. A line that is no `{success,…}` object stays in the result as `ImportLine { success: false, code: None, error: Some(<the raw line>), id: None }`, so a result never gets shorter by decoding. A non-`2xx` is `Error::Http`.
- A new `Client::without_bodies(&self) -> Self` clears both body flags (`keep_request_body`, `keep_response_body`). `import_batch` uses it, because `Body::new` keeps up to 1 MiB per event and up to 32 MiB as `full`, and the log holds 5,000 events: gigabytes, the same trap as D24. Status, size and timing stay. The log detail says the bodies were not kept, for both directions, and "Copy as curl" of such an entry is not offered, because it would repeat a different request.

### 4.3 Export

```rust
pub struct ExportQuery { pub filter_by: Option<String>, pub include_fields: Option<String>, pub exclude_fields: Option<String> }
pub struct ExportStream { /* private */ }

impl Client {
    pub async fn export_stream(&self, collection: &str, query: &ExportQuery) -> Result<ExportStream, Error>;
}
impl ExportStream {
    pub async fn next_line(&mut self) -> Result<Option<String>, Error>;
}
```

- `send.rs` splits `send_with` into a private `prepare` (read-only guard, URL, key header, event fields) and the rest, and `send_stream` uses the same `prepare`, so "the one path every request takes" stays true.
- `send_stream` sets no reqwest `.timeout()`, which is a total deadline covering the body, and the client has no builder timeout. Instead `tokio::time::timeout(STREAM_STALL, …)` wraps `send()` and every `next()`. `STREAM_STALL` is 120 s. The largest gap between reads in a 1 GB export was 0.08 s.
- A non-`2xx` reads the body whole and is `Error::Http` from `export_stream`. A bad filter answers 400 before the first byte (both).
- Lines come from `FramedRead::new(StreamReader::new(bytes_stream mapped to io::Error), LinesCodec::new_with_max_length(MAX_LINE))`. `MAX_LINE` is 256 MiB, above any document Typesense stored in the probe. `LinesCodec` strips `\r`, and `decode_eof` yields the last line, which has no newline (both). A blank last line never appears because an empty export has an empty body (both).
- One `RequestEvent` fires when the stream ends or is dropped, from a private guard's `Drop`: no bodies, `size` is the bytes read, `error` is `"cancelled"` if it was dropped early. So a cancelled export is visible in the log.
- A body cut off mid-way is a reqwest error, which `next_line` returns; the job fails and the part file goes (§5.6).

### 4.4 Traps found while probing

- **A blank line gets no answer line.** Ten input lines with one empty line gave nine answers, so counting would shift every later error by one line (both). A line of spaces does get an answer, a `Bad JSON` error. The reader drops all whitespace-only lines (§5.1) and pairs answers with what it sent.
- **Invalid UTF-8 is silently truncated.** `{"id":"bad\xff","t":"a"}` answered `{"id":"bad","success":true}` (both): a changed id on a success line. The reader refuses such a line locally (§5.1).
- **`dirty_values` defaults to rejecting.** Without the parameter `"12"` for an `int32` is a 400, on import and on single documents (both). With `coerce_or_reject` it is stored as 12. So the dialog's default is `coerce_or_reject`, as §7.6 implies, and it is sent explicitly (D46). This also means M3's note "the default `dirty_values` coerces it" (M3 design §5.5) is wrong for the inspector's saves: `validate_document` calls such a value a `Warning` while the server refuses it. Out of scope here; §14 of this design names it.
- **`dirty_values=bogus` is accepted silently** (200, documents stored), and a bad `action` is a 400 whose text omits `emplace`, which works (both). The enums close both.
- **The failed document echo differs from the success echo.** With `return_doc=true`, a success has `document` as an object and a failure as a JSON string (both). `return_doc` stays off: the client holds the line it sent.
- **An empty body answers 200 with no lines,** and a body of only rejected rows is not sent at all.
- **A 200 can hold only failures.** A batch's status says nothing, so every line counts. Fewer answers than lines sent counts the rest as `unconfirmed`, and more answers fail the job (§5.4).
- **Aborting a `POST` mid-body stored nothing** in one probe: 20 MB of 64 MB uploaded with `batch_size=1000`, connection dropped, `num_documents` unchanged after 3 s. The server does not appear to index before the body is complete. One probe proves little, so an aborted batch is reported as `unconfirmed`, never as not imported.
- **The server took a 30 MiB single document** and returned it from export, so `MAX_DOC_BYTES` is a client memory limit, not a server one.
- **`include_fields=nope` answers 200 with `{}` per document** (both): right count, no data. The field picker offers only names from the schema (§5.6).
- **Field names in a schema are almost unchecked.** `a b`, `a,b`, ``a`b`` and `x*` all create fields, and `x*` is a wildcard. A field named `id` gives a collection with `"fields":null`. Inference must skip such keys (§5.3), and a comma cannot be expressed in `include_fields` (§5.6).
- **Export ignores curation sets; search does not.** With a set attached, default search found 3, `enable_curations=false` found 4, and export wrote 4 (`wc -l` shows 3 because the last line has no newline). `count_documents` already sends `enable_curations=false`, so the preview count is the export's count.
- **`include_fields=*`, nested paths such as `o.a.b` and `exclude_fields=o.a` work,** and a space after a comma is tolerated.
- **Aliases work as the target** of import, export and `GET /collections/{name}`, which reports the real name.
- **Bad values the server stores anyway:** `geopoint` `[100,200]` is accepted, `{"lat":…,"lng":…}` and `"1,2"` are refused, and `1.5` for `int64` is refused even with coercion off. Inference must not suggest `geopoint` for object pairs.

### 4.5 Numbers measured on the dev server

On this machine (Docker with 16 CPUs and 15.66 GiB, 1.5 TB free), 1,000,000 documents of about 1 KB in batches of 1,000, one connection at a time, with the big field `index: false`:

| Measure | Value |
|---|---|
| Import | 37.6 s, 26,566 documents per second |
| Slowest batch round trip | 116 ms |
| `num_documents` right after the last answer | 1,000,000 |
| Export of the same collection | 1.01 GB in 45.4 s, largest gap 80 ms |
| Export of the seeded `products` | 81 MB in 10.9 s |
| Server memory after dropping the collection | 452 MiB, from 361 MiB |

So the one-batch cancel promise means about a tenth of a second here.

## 5. tessera-core

No GPUI dependency. New module tree `crates/tessera-core/src/transfer/`. `tessera_core::import` stays what it is, the dashboard `config.json` import, and `documents/export.rs` keeps the clipboard texts.

### 5.1 Readers (`source.rs`)

```rust
pub enum Source { File(PathBuf), Bytes(Arc<[u8]>) }
pub enum Format { Jsonl, JsonArray, Csv }
pub struct CsvOptions { pub delimiter: u8, pub header: bool }
pub struct ReadOptions { pub format: Format, pub csv: CsvOptions, pub fields: Vec<(String, String)> } // name, type
pub struct Record { pub line: u64, pub offset: u64, pub doc: Result<String, Rejected> }
pub struct Rejected { pub raw: String, pub reason: RejectReason }
pub enum RejectReason { InvalidUtf8, TooLarge(u64) }
pub enum ReadError { Io(String), Json { line: u64, column: u64, message: String }, Csv { line: u64, message: String }, RecordTooLarge { line: u64 } }

pub fn sniff(path: Option<&Path>, head: &[u8]) -> (Format, CsvOptions);
pub fn read_documents(source: &Source, options: &ReadOptions,
    sink: impl FnMut(Record) -> ControlFlow<()>) -> Result<(), ReadError>;
```

- **Push style, synchronous, run off the async workers.** A JSON array of a million elements can only be read element by element inside a serde `Visitor`, which has no suspension point, and `csv` is synchronous too. So `read_documents` is plain blocking code, and every caller runs it in `spawn_blocking`.
- **`sniff`:** `.csv` is CSV with `,`, `.tsv` CSV with tab, otherwise the first non-whitespace byte after a BOM: `[` is a JSON array, anything else JSONL. The dialog can override it.
- **JSONL:** `BufRead::read_until` into a buffer that stops growing at `MAX_DOC_BYTES`; the rest of a longer line is skipped without buffering and the record is `Rejected(TooLarge)`. A UTF-8 BOM goes. `\r` and edge whitespace go, whitespace-only lines are skipped but still counted. Invalid UTF-8 gives `Rejected(InvalidUtf8)` with the lossy text. Anything else passes **as the raw line**, unparsed: the server judges it line by line, which keeps number spelling and costs nothing here. A non-blank line gets exactly one answer (confirmed for `null`, `{}`, `"str"`, `7`, `[1,2]`, garbage).
- **JSON array:** `Deserializer::from_reader` with a `Visitor` whose `visit_seq` reads `Value` elements and hands each compact to `sink`. `line` is the 1-based element number. A top-level value that is no array, or broken JSON, is `ReadError::Json`, which stops the job there. `u64` and float values survive `Value`; integers above `u64` become floats, as serde_json does without `arbitrary_precision`.
- **CSV:** `csv::ReaderBuilder` with `.delimiter()`, `.has_headers()`, `.flexible(true)`, reading `ByteRecord`s so that bad UTF-8 is per record. Without a header, names come from `fields` in order, then `column_{n}`. Conversion follows §5.2.
- **Bounded records, also on a stray quote.** An unclosed quote makes all of the rest of a CSV file one record, and `csv` has no size limit. A small `RecordLimit<R: Read>` wrapper counts bytes since `reset()`, called after every record, and turns more than `MAX_DOC_BYTES` into an `io::Error` that becomes `ReadError::RecordTooLarge`. The same wrapper guards a JSON array element.

### 5.2 CSV cells (`csv_cells.rs`)

D51. `pub fn convert(cell: &str, ty: Option<&str>) -> Option<Value>`.

| Field type | Result |
|---|---|
| any, empty cell | `None`: the key is left out, so optional fields stay missing |
| `string`, `string*`, `image` | the text |
| `int32`, `int64` | integer if it matches `-?(0|[1-9][0-9]*)` and fits `i64`, else the text, for the server to judge under `dirty_values` |
| `float` | `f64` if finite and it parses, else the text |
| `bool` | `true`/`false` in any case, else the text |
| arrays, `object`, `object[]`, `geopoint`, `geopoint[]` | the cell parsed as JSON, else the text |
| unknown column, no schema, `auto` | `guess`: integer as above, float with a dot or exponent, `true`/`false`, else the text. `007` stays text |

`id` is always the text. A header `a.b.c` nests into objects unless `fields` has that exact name, which is how a CSV export comes back.

### 5.3 Schema inference and preview (`infer.rs`, `preview.rs`)

```rust
pub struct InferredSchema { pub fields: Vec<FieldDraft>, pub nested: bool, pub notes: Vec<InferNote>, pub sampled: u64 }
pub enum InferNote { MaybeGeopoint(String), MixedTypes(String), EmptyArray(String), OnlyNull(String), SkippedName(String), NonStringId }
pub fn infer_schema(source: &Source, options: &ReadOptions) -> Result<InferredSchema, ReadError>;
pub fn collection_name_from(path: &Path) -> String;
impl InferredSchema { pub fn to_draft(&self, name: &str) -> SchemaDraft; }

pub struct PreviewRow { pub line: u64, pub text: String, pub issues: Vec<DocIssue>, pub error: Option<PreviewError> }
pub enum PreviewError { Json(String), InvalidUtf8, TooLarge(u64) } // core words nothing: the app translates, rule 7
pub struct Preview { pub rows: Vec<PreviewRow>, pub complete: bool, pub estimated_docs: u64, pub total_bytes: Option<u64> }
pub fn preview(source: &Source, options: &ReadOptions, schema: Option<&Collection>) -> Result<Preview, ReadError>;
```

**Inference** reads the first `SAMPLE_ROWS` (1,000) records and folds one small state per top-level key, in first-seen order (D50):

- `bool` stays `bool`. Integers give `int64`, never `int32`: a sample cannot prove the range. Integers and floats mix to `float`. A number above `i64::MAX` is `float`.
- Strings give `string`. Anything else mixed with strings or bools gives `string` with `MixedTypes`, which works under `coerce_or_reject`.
- Arrays take the type of their elements, with the same rules: `string[]`, `int64[]`, `float[]`, `bool[]`, `object[]`. An array that is always empty is `string[]` with `EmptyArray`.
- An object gives `object` and sets `nested`, which sets `enable_nested_fields`. Sub-keys are not listed.
- A key missing from a row, or `null`, makes the field `optional`. All `null` gives optional `string` with `OnlyNull`.
- A `float[]` or `int64[]` whose every value has exactly 2 numbers within ±90 and ±180 stays as it is and gets `MaybeGeopoint`. §7.3 says suggestion, and two numbers may be anything. `{lat,lng}` objects are not suggested: the server refuses them.
- `id` makes no field (§4.4), and a non-string id gives `NonStringId`. A key that is empty, has a `*`, has a control character or has edge whitespace is skipped with `SkippedName`.

`collection_name_from` keeps `[A-Za-z0-9_-]` of the file stem, replaces the rest by `_`, and falls back to `imported`. The result is only a proposal in an input; `collection_path` encodes whatever is sent.

**Preview** reads `PREVIEW_LINES` (20) records. Each row parses its text, shows `cell_preview`, and with a `schema` runs `validate_document` (D27). `estimated_docs` is exact when the source ended (`complete`), else `total_bytes / (offset / rows)`.

### 5.4 Import pipeline (`import.rs`)

```rust
pub struct JobControl { pub stop: CancellationToken, pub abort: CancellationToken } // Clone, Default
pub struct ImportJob { pub source: Source, pub options: ReadOptions, pub collection: String, pub params: ImportParams }
pub enum Phase { Running, Stopping }
pub struct ImportProgress { pub phase: Phase, pub bytes_done: u64, pub total_bytes: Option<u64>, pub sent: u64,
    pub imported: u64, pub failed: u64, pub unconfirmed: u64, pub elapsed: Duration, pub first_error: Option<Arc<str>> }
impl ImportProgress { pub fn rate(&self) -> f64; pub fn eta(&self) -> Option<Duration>; pub fn fraction(&self) -> Option<f32>; }
pub enum JobEnd<E> { Finished, Stopped, Failed(E) }
pub struct CountCheck { pub before: u64, pub after: u64, pub ok: bool }
pub struct ImportOutcome { pub end: JobEnd<TransferError>, pub progress: ImportProgress, pub report: Report,
    pub stopped_at_line: Option<u64>, pub count: Option<CountCheck> }

pub async fn run_import(client: Client, job: ImportJob, control: JobControl,
    progress: watch::Sender<ImportProgress>) -> ImportOutcome;
```

```
spawn_blocking: read_documents ──Batch──▶ mpsc(CHANNEL_BATCHES = 2) ──▶ run_import loop ──POST──▶ server
   stops when the receiver is gone                                         │ one batch at a time
                                                                            ├─ watch: ImportProgress
                                                                            └─ Report: spool and preview rows
```

- **`Batch`** is the body `String`, the `line` of each document, the `Rejected` rows that fell into it, and the offset after its last record. A batch is full at `params.batch_size` documents or `BATCH_BYTES` (16 MiB), whichever comes first: a thousand 1 MB documents must not make a 1 GB body. The bound channel gives backpressure, so memory is at most 3 batches, 48 MiB, plus the `Report` preview. A typical 1 GB run holds about 3 MB of documents.
- **The loop.** Check `stop`; receive with `select!` on `stop`; send with `select!` on `abort`; evaluate; update `progress` with `send_replace`. Nothing starts after `stop`, and the batch in flight is awaited, so the counts stay true. That is the cancel AC. `abort` is the second press ("Stop waiting", D45): it drops the request and counts its documents as `unconfirmed`.
- **Evaluating.** Answers pair with the sent lines by position (§4.4). A success adds to `imported`, anything else to `failed` with a report row. Fewer answers: the rest are `unconfirmed` with a row saying so. More answers than lines: `Failed(TransferError::AnswerMismatch { sent, answered })`, because the server did something else than asked, and the job stops. The first failure text goes to `first_error`, so a user sees a wrong schema in the first second, not after an hour.
- **A whole-batch error** stops the job with `Failed`, batches done stay. Exception: `503` retries the same batch up to `BUSY_RETRIES` (5) times with 1, 2, 4, 8, 16 s, cancel-aware, because the Typesense docs name it for a lagging node and nothing was applied. Not reproducible on the dev server; wiremock tests it.
- **Read-only.** A `Lock again` while running makes the next `import_batch` `Error::ReadOnly`, a normal `Failed`. The guard stays authoritative.
- **Reader errors** arrive through the channel as a final message, so all earlier batches are sent first, then the job is `Failed` with the line.
- **Count check** (lesson from M3: a 200 can have done nothing or too much). `run_import` reads `num_documents` before and after a `Finished` run. `Create`: growth must equal `imported`. `Upsert`, `Emplace`: growth at most `imported`. `Update`: no growth. A difference is a warning, not a failure, because others may write at the same time. If the read fails, `count` is `None` and the job says it was not checked.

### 5.5 Report (`report.rs`)

D47. `Report` keeps the first `REPORT_ROWS` (1,000) rows in memory, with `raw` cut to `REPORT_PREVIEW_BYTES` (2,048), and every row in a spool: `tempfile::tempfile()`, an unnamed private file, gone with the job or the process even after a crash. A wrong schema on 1 million lines cannot grow the app.

```rust
pub struct ReportRow { pub line: u64, pub code: Option<u16>, pub error: String, pub id: Option<String>, pub raw: String }
impl Report {
    pub fn rows(&self) -> &[ReportRow];
    pub fn total(&self) -> u64;
    pub async fn save_report(&self, path: &Path) -> Result<u64, io::Error>; // JSONL of rows
    pub async fn save_failed(&self, path: &Path) -> Result<u64, io::Error>; // the raw lines
}
```

Spool lines are JSON rows written through a `tokio::io::BufWriter` over `tokio::fs::File::from_std`. Saving reads from `try_clone` of the file, writes `{path}.{uuid8}.part` and renames, like exports, so a failed save leaves no half file and the same `Report` can be saved twice. A row the server refused for its content goes back as it was read; a `Rejected(InvalidUtf8)` row is saved lossily, and the report row says so.

### 5.6 Export pipeline (`export.rs`)

```rust
pub enum ExportScope { All, Filter(String), Selection(Vec<String>) }
pub enum ExportFormat { Jsonl, JsonArray, Csv }
pub enum FieldPick { All, Include(Vec<String>), Exclude(Vec<String>) }
pub struct ExportJob { pub collection: String, pub scope: ExportScope, pub fields: FieldPick,
    pub format: ExportFormat, pub target: PathBuf, pub expected: Option<u64> }
pub struct ExportProgress { pub phase: Phase, pub docs: u64, pub expected: Option<u64>, pub bytes: u64, pub elapsed: Duration }
pub struct ExportOutcome { pub end: JobEnd<TransferError>, pub progress: ExportProgress, pub unlisted_values: u64 }

pub fn field_safe(name: &str) -> bool;
pub fn flatten(doc: &Map<String, Value>) -> Vec<(String, Value)>;
pub async fn run_export(client: Client, job: ExportJob, control: JobControl,
    progress: watch::Sender<ExportProgress>) -> ExportOutcome;
```

- **Target.** Writes `{target}.{uuid8}.part` with `create_new`, through a `BufWriter` of 64 KiB, and renames onto the target after `flush` and `sync_all`, so cancelling or failing never touches an existing file (§11 "all writes are atomic"). A guard's `Drop` removes the part file. The path comes from a save dialog, which asks about overwriting itself.
- **Scopes.** `All` sends no filter. `Filter` sends `filter_by` as typed, with no length limit on this endpoint (M3: 9 KB tested). `Selection` runs `id_batches(ids, ID_FILTER_BYTES)`: one stream per filter, then `client.document` per id in `single`, projected locally through `FieldPick`, which is why picks are top-level names. At most `MAX_SELECTION` ids.
- **`field_safe` is an allowlist** at the boundary: not empty, no comma, backtick, `*`, control character or edge whitespace. `run_export` refuses an unsafe name with `Failed`; the dialog disables such a field.
- **Formats.** JSONL writes each line and `\n`. JSON array writes `[`, lines joined by `,\n`, and `]`, with no parsing. CSV buffers the first `CSV_HEADER_SAMPLE` (1,000) documents, builds the header from `flatten` in first-seen order with `id` first, then writes rows through `documents::export`'s existing RFC 4180 `field` helper, made `pub(crate)`. A later document's key that is not in the header counts into `unlisted_values`, and the job says "N values were left out" at the end, rather than spooling the export twice. `flatten` recurses objects to `a.b.c`, arrays stay compact JSON, `null` is empty.
- **Count check.** At the end `docs` is compared with `expected`. A difference is a warning ("Exported N of M"), since writers may run meanwhile.
- **Cancel** is `stop`, checked per line; the stream drops at once and the part file goes.

### 5.7 Constants

| Constant | Value | Why |
|---|---|---|
| `DEFAULT_BATCH` | 1,000 | §7.6 |
| `MAX_BATCH` | 100,000 | Above it `BATCH_BYTES` decides anyway; refuses a typo |
| `BATCH_BYTES` | 16 MiB | Memory bound for large documents |
| `MAX_DOC_BYTES` | 32 MiB | Same as `FULL_BODY_MAX`; the server took 30 MiB |
| `CHANNEL_BATCHES` | 2 | One queued for the next round trip, one being filled |
| `PREVIEW_LINES` | 20 | §7.6 |
| `SAMPLE_ROWS` | 1,000 | §7.3 |
| `REPORT_ROWS` | 1,000 | Table size, §5.5 |
| `CLIPBOARD_LIMIT` | 16 MiB | §7.6 "small amounts" |
| `BUSY_RETRIES` | 5 | §5.4 |
| `CSV_HEADER_SAMPLE` | 1,000 | §5.6 |

## 6. tessera-widgets

No new widget. `Confirm`, `ConfirmDanger`, `FilterInput` and `JsonEditor` serve as they are.

## 7. tessera-app

### 7.1 Backend seam

`crates/tessera-app/src/transfer.rs`, same structure as `DocumentsBackend`, because GPUI's test executor does not wait for I/O.

```rust
pub trait TransferBackend: Send + Sync {
    fn preview(&self, source: Source, options: ReadOptions, schema: Option<Collection>) -> BoxFuture<Result<Preview, ReadError>>;
    fn infer(&self, source: Source, options: ReadOptions) -> BoxFuture<Result<InferredSchema, ReadError>>;
    fn import(&self, client: Client, job: ImportJob, control: JobControl, progress: watch::Sender<ImportProgress>) -> BoxFuture<ImportOutcome>;
    fn export(&self, client: Client, job: ExportJob, control: JobControl, progress: watch::Sender<ExportProgress>) -> BoxFuture<ExportOutcome>;
    fn save_report(&self, report: Arc<Report>, path: PathBuf, failed_only: bool) -> BoxFuture<Result<u64, String>>;
}
```

`preview` and `infer` wrap `spawn_blocking`. The rest run on `TokioBackend::run`.

### 7.2 Jobs

`crates/tessera-app/src/jobs.rs`. `Jobs` is an entity in the global `JobsGlobal`, installed by `jobs::init(cx)` in `run` and in the test setup. It is not a field of a view or of `ConnectionState`, so closing a tab, closing a dialog, or disconnecting never drops a job (D48). It emits `RunningChanged(usize)` when the running count changes and `JobFinished(JobId)`.

```rust
pub struct Job { pub id: JobId, pub kind: JobKind, pub connection: Uuid, pub connection_name: SharedString,
    pub environment: Environment, pub collection: String, pub label: SharedString, pub status: JobStatus,
    pub control: JobControl, pub outcome: Option<JobOutcome>, /* tasks */ }
pub enum JobKind { Import, Export }
pub enum JobStatus { Running(Live), Stopping(Live), Finished, Stopped, Failed(SharedString) }
```

- **Two GPUI tasks per job,** both owned by `Job`. One awaits the backend future and stores the outcome. One throttles:

```rust
while rx.changed().await.is_ok() {
    let snapshot = rx.borrow_and_update().clone();
    this.update(cx, |jobs, cx| { jobs.set_live(id, snapshot); cx.notify() })?;
    cx.background_executor().timer(PROGRESS_INTERVAL).await; // 34 ms
}
```

  `watch` keeps only the newest value, so a slow UI skips states and never queues them. That is the product spec's "at most 30 updates per second" (§5.4 there). The loop ends when the pipeline drops the sender.
- Only the jobs panel observes `Jobs`. The root subscribes to `RunningChanged`, so progress never redraws the whole window.
- On a finished import: `connection.collection_changed(name, cx)` once, if the connection still exists, so an open table reloads, and `invalidate_collections`. A finished job with a report keeps it until dismissed.
- **Quit** has no veto (D31). `on_app_quit` calls `stop` on every job and waits for up to GPUI's `SHUTDOWN_TIMEOUT` (200 ms), registered with the first job and on the background executor, like the save chain, so an export can remove its part file. An import's in-flight batch may finish on the server. A part file left by a hard kill has a `.part` name beside the target.

### 7.3 Jobs panel

`views/jobs/panel.rs`. `JobsPanel` is a second tab in the bottom dock, next to the request log (`DockLayout::tabs().panel_view(log).panel_view(jobs)`, `gpui-base-0.9.0/src/dock/layout/builder.rs`), `closable` false.

```
┌ Request log │ Jobs ───────────────────────────────────────────────────────────────┐
│ ▶ import  ●prod  shop · products  catalog.jsonl  ▓▓▓▓░░░ 41%   412 MB of 1.0 GB     │
│           412,000 docs · 26,100/s · ETA 0:22 · 3 errors          [Cancel ⌘.]        │
│ ✓ export  local  dev · books      books.jsonl    10,000 of 10,000 · 4.1 MB          │
├ selected job ───────────────────────────────────────────────────────────────────────┤
│ Imported 411,997 · Failed 3 · batches already imported stay                          │
│ [Save report ⌘S] [Save failed lines ⇧⌘S] [Import a file…]                            │
│ Line   Code  Error                                  Document                         │
│ 1204   400   Field `n` must be an int32.            {"id":"1204","n":"x"}            │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

- Rows use `DataTable` with the same delegate shape as the log. `Progress::new(id).value(pct)` for the bar (`gpui-component-0.9.0/src/progress/progress.rs`); an unknown total shows none. Each row carries `env_badge::render` and the connection name (UX rule 1).
- Live text: bytes, documents, rate, ETA, error count, `first_error` in full under the row. Cancel while `Running`, which reads "Stop waiting" (⌥⌘.) while `Stopping`, as the console does.
- The cancel note states that batches already imported stay, here and in the dialog (§7.6).
- The error table shows `rows()` (at most 1,000), and above it "Showing 1,000 of 412,000. Save the report for all." `Save report` and `Save failed lines` use `prompt_for_new_path`, run on the backend, and show the result inline. After a saved failed-lines file, `Import this file` opens the import dialog on it.
- `Dismiss` (⌘⌫) is enabled for finished jobs only. A job whose report has rows and was not saved asks through `Confirm`, focus on Cancel. It releases the spool.
- A failed job's error is inline and full, through `long_error_text` (UX rule 2), with the line it stopped at.
- Empty state: one sentence and `Import` if a connection is selected.
- `JobFinished` shows a toast with `window.push_notification` from the root, unless the Jobs tab is visible: a job finishing is a background event, so this is the one toast (product spec §5.5). Text: `Import of catalog.jsonl finished: 411,997 imported, 3 failed.`

**Dock.** `ToggleJobs` (⇧⌘J) and ⌘J share one helper: closed opens the dock on that tab; open on another tab selects it; open on that tab closes it. Focus inside a closing panel moves to the root as ⌘J does today. Starting a job opens the dock on Jobs and does not move focus. `area.select_panel` and `is_dock_open` are in `gpui-base-0.9.0/src/dock/dock_area.rs`.

**Status bar.** `status_bar::render` gains `Jobs` with a count while any run (`3 jobs`), with `Kbd::global_binding_for_action(&ToggleJobs, window)`. It shows only when a job exists (UX rule 4).

### 7.4 Import dialog

`views/jobs/import_dialog.rs`, type `DocumentImportDialog` (`ImportDialog` is the profile import). Opened by `ImportDocuments` ⇧⌘I in `COLLECTION` (target preset) and `COLLECTIONS`, and by the toolbar button of both views, with the shortcut shown. It stays available on a locked profile and shows the lock on Start. Width 900, connection name and environment badge in the title, as the schema dialog.

```
┌ Import documents   shop  PROD ─────────────────────────────────────────────┐
│ File   catalog.jsonl · 1.0 GB    [Choose ⌘O] [Paste ⇧⌘V]                   │
│ Format [JSONL][JSON array][CSV]   Delimiter [,][;][tab][|]  [x] Header row  │
│ Target [products ▾]  or New collection: name [catalog]  [Edit schema…]      │
│ Action [create][upsert][update][emplace]                                    │
│ Dirty values [coerce_or_reject][coerce_or_drop][drop][reject]               │
│ Batch size [1000]                                                           │
├ preview: first 20 lines, about 1,000,000 documents ─────────────────────────┤
│ 1  {"id":"1","title":"…"}                                                   │
│ 2  {"id":"2","n":"x"}   ⚠ n: expected int32                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│ Cancelling later stops after the current batch. Batches already imported    │
│ stay.                                                     [Cancel] [Start ⌘↩]│
└─────────────────────────────────────────────────────────────────────────────┘
```

- **Source.** File dialog through `cx.prompt_for_paths` (as `views/import_dialog.rs` does); drop onto the dialog, or onto the documents view or collections list, which opens the dialog (`on_drop` with `ExternalPaths`, `gpui-pre-0.3.11/src/interactive.rs`, `elements/div.rs`). Only the first path; a directory shows inline that a file is needed. Clipboard: `cx.read_from_clipboard()`; `Source::Bytes`; above `CLIPBOARD_LIMIT` an inline message. `metadata` runs off the GPUI thread.
- **Format** is sniffed from `sniff` on the first 4 KiB, then overridable. Delimiter and header show for CSV only (UX rule 4).
- **Target** is a `SelectState` over the connection's collections (`ConnectionState::collections`, loaded on open) plus "New collection…". Choosing New shows the name input (`collection_name_from`) and `Edit schema…`.
- **Preview** reruns on any change of source, format, CSV options or target, through a stored task, 150 ms after the last change; stale results drop with the task. It validates against the target's schema, or the inferred draft. A `ReadError` shows inline in full, and Start is disabled. A rejected or unparsable row shows its message on the row.
- **Batch size** must be an integer in `1..=MAX_BATCH`, inline otherwise.
- `update` and `emplace` need an existing target: with New collection they are disabled, and the note says why.

### 7.5 New collection

D54. A collection is created only when the import starts, so cancelling the dialog changes nothing on the server.

- Choosing New runs `infer` on the current source, and the draft lands in the dialog. Notes list below the target.
- `Edit schema…` opens `SchemaDialog::open_pick(state, connection, draft, notes, on_pick)`. `Mode::Pick` shows the same form and JSON tabs and validation. The submit button reads "Use schema", sends nothing, and hands the JSON to `on_pick`. This dialog sits on top of the import dialog: `window.open_dialog` stacks (`unlock` already does).
- `MaybeGeopoint(field)` shows `Use geopoint` on its note, which sets the field's type in the draft.
- Start with New: confirmation if needed, `CollectionsBackend::create` with the JSON, then the job. A failed create shows inline and the dialog stays. An import that later fails or is stopped leaves the collection with what arrived, which the panel says.

### 7.6 Start

1. Validate. 2. `ConnectionState::ensure_writable`. 3. Confirm by D49. 4. For New, create. 5. `Jobs::start_import`, close the dialog, open the Jobs tab. The job clones `connected.client`, whose flags are shared, so unlock state stays live.

| Case | Dialog |
|---|---|
| `create` or new collection, not prod | none |
| `create` or new collection, prod | `Confirm` |
| `upsert`, `update`, `emplace`, not prod, at most 1,000 estimated documents | `Confirm` |
| `upsert`, `update`, `emplace`, prod or above 1,000 estimated | `ConfirmDanger`, collection name typed |

This is `danger_level(estimated_docs, prod)` for overwriting actions. Consequences name the connection, collection, action, file and estimated count ("about"), that existing documents with the same id are replaced or changed, and that batches already imported stay when cancelled. `Confirm` focuses Cancel, `ConfirmDanger` the input. Cancelling sends nothing.

### 7.7 Export dialog

`views/jobs/export_dialog.rs`, opened by `ExportDocuments` ⇧⌘X in `COLLECTION` and the toolbar button. Available on a locked profile: it is a read.

- **Scope:** All, Filter (a `FilterInput` starting from the table's filter), Selection (with `N selected`, disabled with none). Opening with a selection preselects it, with a filter preselects Filter.
- **Count:** 300 ms after a change `DocumentsBackend::count` shows "Exports N documents" or the server's message inline, as the bulk dialog does. An error disables Start. Zero is allowed. `Selection` shows the selection size. The count is `expected`.
- **Fields:** All, Include, Exclude (`ButtonGroup`), with one `Checkbox` per top-level schema name: `id`, then the part of each field name before the first dot, without duplicates and without names holding `*`. So vector fields are listed too. A name that fails `field_safe` stays visible, disabled, with the reason. An empty pick disables Start. Include without `id` says that a re-import would make new ids.
- **Format:** JSONL, JSON array, CSV. CSV notes that the header comes from the first 1,000 documents.
- **Target:** Start ⌘↩ opens `cx.prompt_for_new_path(dir, Some("{collection}.{ext}"))`, as `views/sidebar.rs` does. Cancelling that changes nothing and the dialog stays. Then `Jobs::start_export`, close, open Jobs. The default directory is the user's home, as the sidebar export.

### 7.8 Infer from file

`Infer from file…` ⌘O in the schema dialog in create mode only, including clone (§7.3). File dialog, `infer`, then `to_draft` with the name kept and the form refilled through the existing sync path. A dirty draft asks through `Confirm`, focus on Cancel, because it is replaced. Notes list above the tabs. A `ReadError` shows inline.

### 7.9 Shortcuts

All shown with `Kbd` on the control. New contexts: `JOBS` ("Jobs"), `IMPORT_DIALOG` ("DocumentImport"), `EXPORT_DIALOG` ("DocumentExport"), each with a `… > Input` twin for the Start chord, as `BULK_INPUT`.

| Action | macOS | Linux, Windows | Context |
|---|---|---|---|
| Jobs panel | ⇧⌘J | Ctrl+Shift+J | global |
| Import documents | ⇧⌘I | Ctrl+Shift+I | `COLLECTION`, `COLLECTIONS` |
| Export documents | ⇧⌘X | Ctrl+Shift+X | `COLLECTION` |
| Choose file | ⌘O | Ctrl+O | `IMPORT_DIALOG` |
| Paste from clipboard | ⇧⌘V | Ctrl+Shift+V | `IMPORT_DIALOG` |
| Start | ⌘↩ | Ctrl+Enter | both dialogs |
| Infer from file | ⌘O | Ctrl+O | `SCHEMA_DIALOG` |
| Cancel job | ⌘. | Ctrl+. | `JOBS` |
| Stop waiting | ⌥⌘. | Ctrl+Alt+. | `JOBS` |
| Save report | ⌘S | Ctrl+S | `JOBS` |
| Save failed lines | ⇧⌘S | Ctrl+Shift+S | `JOBS` |
| Dismiss job | ⌘⌫ | Ctrl+Backspace | `JOBS` |

All chords were checked against `actions.rs`. ⇧⌘S is `SortByColumn`, but only in `DOCUMENTS`. New actions go into `ACTION_LABELS` with a `palette.action.*` key; `gated` hides what is disabled.

### 7.10 i18n

Flat keys under `transfer.import.*`, `transfer.export.*`, `transfer.infer.*`, `jobs.*`, same keys in `en.yml` and `de.yml`. `import.*` stays the profile import.

## 8. Testing

| Level | What it checks |
|---|---|
| Client unit (`wiremock`) | `import_batch`: query for each enum, content type, order, undecodable line kept, 404 and 503 are `Error::Http`; read-only refusal; no body in the event, size kept. `export_stream`: lines, no trailing newline, empty body, 400 before the first byte, stall gives `Timeout`, a cut body is an error, one event per stream with total size, an event on drop, read-only passes |
| Client integration (`#[ignore]`, 30.0 and 30.2) | The §4.4 traps as assertions: blank line, emplace, `dirty_values`, failed batch, `include_fields`, `export` count against `count_documents` under a curation set, alias target |
| Core unit | Readers on every edge of §5.1 (BOM, CRLF, blank lines, invalid UTF-8, a line of `MAX_DOC_BYTES + 1`, stray CSV quote, flexible rows, array with a bad element); `sniff` table; `convert` table; dotted headers; inference table from §5.3 including skipped names; `preview` estimate; report spool beyond `REPORT_ROWS`, save twice; `field_safe`; `flatten` |
| Core pipeline (`wiremock`, tokio) | Batch boundaries by count and by bytes; line numbers after a blank line and after rejected rows; stop: no POST after it, in-flight answer counted; abort: `unconfirmed`; fewer answers, more answers; 503 retries then succeeds, cancel during backoff; backpressure: a stalled server leaves the reader within 3 batches; count check per action; export formats, part file gone on stop, target untouched on failure, selection with a backtick id, `unlisted_values` |
| Core live (`tests/live_transfer.rs`, `#[ignore]`) | JSONL, array and CSV of 2,000 documents round trip through export and import; failed lines re-import |
| App UI (fake `TransferBackend`) | Job survives closing the dialog and the tab; progress at most 30 updates per simulated second; only the panel redraws; cancel, Stopping, Stop waiting; dismiss asks when unsaved; toast only when the tab is hidden; status bar count; import dialog: preview, sniff, CSV options only for CSV, batch size bounds, read errors inline; D49 table including focus; locked profile sends nothing before the unlock; New: nothing created on cancel, create failure stays; `Mode::Pick` sends nothing; export dialog: scopes, count, disabled unsafe field, Cancel in the save dialog changes nothing; ⌘J and ⇧⌘J; drop opens the dialog |
| Secrets | An import event has no request body. A report never contains the key: the client adds none |

`simulate_new_path_selection` and `simulate_path_prompt_response` exist in `gpui-pre-0.3.11/src/app/test_context.rs`; UI tests drive the dialogs with them. §12.1 lists import cancel as a required UI test.

## 9. CI

- The integration job runs the new live tests on 30.0 and 30.2.
- `nightly.yml` gains an `nf3` job (§12).
- NF5: PR descriptions record the Linux binary size. `csv` is the only new code of any size.
- `cargo deny` must pass with `csv` and `csv-core`.

## 10. New dependencies

See §4.1. `gzip` from §4 of the product spec is still not needed.

## 11. Architecture notes

- `docs/architecture/async.md` gains a section on jobs: `spawn_blocking` reader, bounded channel, `watch`, the throttle, ownership by `Jobs`, quit.
- Nothing new persists, so §11 of the product spec gains no file (D47).

## 12. Measuring NF3 headlessly

D53. NF3 is about the app, so it has two parts that add up. Both run without a person. The engine measurement needs no display at all.

**`cargo run --release -p xtask -- nf3 --url http://localhost:18108 [--key xyz] [--docs 1000000] [--format jsonl|json|csv] [--app-baseline-mb N] [--limit-mb 300] [--keep-file <path>]`**

1. Has no default URL, as `seed`, so it cannot hit port 8108 by accident. Stops if a collection named `tessera_nf3` exists.
2. Writes the file into a new temporary directory, or to `--keep-file`, which also keeps it for the run by hand: `--docs` documents of about 1,020 bytes, with `id`, `title`, `n`, and a 950-byte `payload`. The schema has `payload` with `index: false`, so the server's memory stays small (§4.5).
3. Creates the collection and runs the real `run_import` on a 2-worker tokio runtime, like the app, with a hook that pushes each event into a core `RequestLog` of 5,000 and 500 entries, so the log's memory counts.
4. A sampler thread runs `ps -o rss= -p <pid>` every 200 ms. This works on macOS and Linux, and on both the number is KiB.
5. Then a cancel run over 200,000 documents into a second collection: `stop` at half way. Pass: no `POST` in the log has `started_at` after the stop, and `run_import` returned after the batch in flight answered.
6. Checks `imported == docs`, `num_documents == docs`, `failed == 0`. Drops both collections and the directory, also on failure.
7. Prints one JSON line: peak RSS, baseline RSS before the run, seconds, documents per second, slowest batch, stop latency. Exits non-zero when `peak - baseline + app_baseline_mb >= limit_mb`.

**App baseline.** `TESSERA_SMOKE=1` already makes the real binary open its window, render one frame and quit.

- macOS: `TESSERA_SMOKE=1 /usr/bin/time -l target/release/tessera`, and read "maximum resident set size" (bytes).
- Linux: `TESSERA_SMOKE=1 xvfb-run -a /usr/bin/time -v target/release/tessera`, and read "Maximum resident set size" (KiB).

Passing that number as `--app-baseline-mb` gives the whole figure: window, fonts and theme, plus what the import adds. It does not count a connection, the sidebar's content or the jobs panel, so it is a lower bound of the idle app; the panel draws at most 30 times a second and holds a thousand rows, which is why PR C also records one run by hand, `ps -o rss= -p <pid>` while importing through the UI.

**On this machine** the dev server on 18108 is ready and the server side takes about 40 s (§4.5), so one full measurement is under two minutes plus the build. `--format json` and `--format csv` run the same check for the other readers. Never point it at 8108, which belongs to another project.

**Nightly.** Job `nf3`: Typesense 30.2 service, build release, xvfb smoke through `/usr/bin/time -v` to get the baseline, then `nf3 --url http://localhost:8108 --app-baseline-mb $base`. The container in that job is the runner's own, so port 8108 is fine there.

## 13. Changes to the product spec

Recorded in `docs/spec.md` together with this design: §4 (the crates actually used), §7.3, §7.6, §7.7, §10 NF3's measurement, §12.1, §14's M5 row, and §16 gains D45 to D54.

| # | Question | Decision |
|---|---|---|
| D45 | One streamed `POST` or one `POST` per batch? | One per batch, sent in sequence. The cancel AC speaks of a batch round trip, an abort of a streamed body showed nothing stored after 20 MB of 64 MB, a failed batch stays one batch, and the answer stays bound. A second press drops the request in flight, and its documents count as unconfirmed |
| D46 | Import defaults | `dirty_values` is `coerce_or_reject`, always sent, because the server's own default rejects on 30.0 and 30.2. `return_id=true` always, so the report has ids, and no option for it. `return_doc` never: the client holds what it sent |
| D47 | Where the error report lives | The first 1,000 rows in memory, all rows in an unnamed private temp file that disappears with the job or process. A million failures must not grow the app. §9 concerns keys and the log file; this holds the user's own documents for as long as the job row exists |
| D48 | Who owns a job? | A global `Jobs` entity. Closing a dialog or tab or disconnecting does not stop it. `Lock again` stops an import at its next batch. Quit stops all jobs; there is no veto (D31) |
| D49 | Confirmation for imports | `create` or a new collection: none, or `Confirm` on prod. `upsert`, `update`, `emplace`: `danger_level` on the estimated count, so typing the name above 1,000 or on prod. Export needs none: it is a read |
| D50 | Inference rules | Integers are `int64`, since a sample cannot prove `int32`. Mixed types are `string` with a note. A 2-number array in lat/lng range stays numeric with a "Use geopoint" suggestion. Keys Typesense would misread (`*`, `id`) make no field |
| D51 | CSV conversion | Cells follow the target's types. Arrays and objects are JSON in the cell, the same form CSV export writes. An empty cell leaves the key out. `a.b` headers nest. The sync `csv` crate, not `csv-async`, because the JSON array reader is sync anyway |
| D52 | Which fields can an export pick? | Top-level names from the schema, by checkbox, each through the `field_safe` allowlist, because `include_fields=nope` answers 200 with `{}` per document and a comma cannot be written. No free text. Keys outside the schema export under All and Exclude |
| D53 | How is NF3 measured? | `cargo xtask nf3` runs the real pipeline with a real request log against a real server and adds the app's baseline from the `TESSERA_SMOKE` run. Nightly on Linux, and one run through the UI by hand in PR C |
| D54 | When is a new collection created? | At Start, after the confirmation. The schema dialog gets a pick mode that sends nothing, so cancelling the import leaves the server untouched |

## 14. Risks

| Risk | Impact | Fallback |
|---|---|---|
| RSS grows through allocator fragmentation rather than live data | NF3 fails with small live memory | PR A measures first. Reuse the `Batch` `String`s through a return channel before anything larger |
| `Value` overhead on a 32 MiB JSON array element of small tokens | A brief spike of hundreds of MB, only in that format | Accepted: the limit bounds it, and JSONL is unparsed. Lower `MAX_DOC_BYTES` for arrays if `--format json` shows it |
| One-element-at-a-time JSON reading is slow | Import limited by the client, not the server | Measured by `--format json`. It only costs time |
| 503 retry behaviour was not observable here | A lagging node may answer differently | Worst case the job stops with the message, and batches done stay |
| A job outlives a disconnect, with its node possibly gone | The batch fails, job `Failed` | Inline message, re-run with `upsert` |
| Bottom dock tabs and `ToggleRequestLog` tests assume a single panel | Regressions in ⌘J | One helper, existing `ui_log.rs` tests stay |
| Dialog stacked on a dialog (`Mode::Pick`) mishandles Esc | Wrong dialog closes | `unlock` already stacks; a UI test covers Esc |
| The estimate of documents is from 20 lines | Danger level off near 1,000 | It says "about". A mixed file can under-estimate, which is why prod always types the name |
| `validate_document` warns of coercion, while the server refuses without `coerce_or_reject` | Inspector saves of `"12"` are refused despite a mere warning | Not M5's: the import dialog's default makes the warning true there. Open for the maintainer: M3 design §5.5 and the inspector need a fix |
| `csv 1.4.0` API is unread until the download | Small rename | Task 3 starts with `cargo fetch` and reading it |
| Full bodies aren't kept for imports | "Copy as curl" is missing on those log entries | Intentional, and said in the detail |
