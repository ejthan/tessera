# M2 Collections: design

- **Date:** 2026-11-05
- **Status:** written autonomously (maintainer takes the recommendations), pending final review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M2 row in §14; §7.3, §8.2)
- **Previous milestone:** [M1 connections and overview design](2026-11-04-m1-connections-design.md)
- **UX mockups:** [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d), screens 1 (main) and 4 (safety)

## 1. Goal

M2 makes collections visible and editable: a list per connection, a schema view, create in a form or as JSON, clone, change the schema with a diff preview, truncate and delete. It is the first milestone that writes, so it also brings the read-only unlock (D10) and the type-to-confirm dialog.

**Acceptance (product spec §14 and §7.3):**

- Integration tests for every collection endpoint are green against Typesense 30.0 and 30.2.
- A schema built in the form is equal, apart from field order, to the same schema typed in JSON mode. A test compares the two as JSON values.
- In a read-only profile, no write leaves the client until the user unlocks it by typing the profile name (§7.1).

## 2. Delivery

One spec, one plan, three pull requests in this order, each with green CI and its own CHANGELOG entries.

| PR | Content | Why this order |
|---|---|---|
| **A · Browse** | Collection endpoints (read side), `tessera-widgets` crate, collection list tab, schema view tab, sidebar entries, favorites, `xtask seed` | Read-only views first; seed data makes them worth looking at |
| **B · Create** | Write endpoints, read-only unlock, schema draft model, form and JSON editor with two-way sync, create and clone | The first write; it needs the unlock flow |
| **C · Change and remove** | Schema diff and PATCH with progress, truncate, delete, type-to-confirm dialog | Builds on B's form, which edits an existing schema too |

## 3. Scope

### In M2

- Product spec §7.3 except schema inference and the JOIN graph
- The read-only unlock (D10)
- `tessera-widgets` with the first three widgets (§6)
- `cargo xtask seed` with the small data sets (§8)

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Infer a schema from a JSONL, JSON or CSV file | M5 | It shares the file readers with import, and the M5 row lists it (D19) |
| JOIN graph of `reference` fields | v1.x (P2) | |
| Attaching synonym and curation sets in the editor | M6 | The sets themselves arrive in M6. M2 shows them in the list and the schema view, and the JSON editor accepts `synonym_sets` |
| Documents tab of a collection | M3 | |
| The 200k `products` seed set | M3 | NF2 belongs to the document browser |
| Schema change status outside a running PATCH (operations panel) | M8 | M2 polls it only while its own PATCH runs (§5.3) |

## 4. tessera-client: collection endpoints

### 4.1 Models

Own models with passthrough of unknown attributes (D5):

```rust
pub struct Collection {
    pub name: String,
    pub num_documents: u64,
    pub created_at: i64,                   // Unix seconds
    pub fields: Vec<Field>,
    pub default_sorting_field: String,     // "" when unset
    pub token_separators: Vec<String>,
    pub symbols_to_index: Vec<String>,
    pub enable_nested_fields: bool,
    pub synonym_sets: Vec<String>,
    pub curation_sets: Vec<String>,        // serde default: optional, though 30.0 and 30.2 both send it
    pub metadata: Option<Map<String, Value>>,
    pub extra: Map<String, Value>,         // every other top-level attribute, kept as sent
}

pub struct Field {
    pub name: String,
    pub r#type: String,                    // kept as a string: new types must not break decoding
    pub attributes: Map<String, Value>,    // facet, optional, index, sort, infix, locale, … as sent
}

pub struct Alias { pub name: String, pub collection_name: String }
pub struct SchemaChange { pub collection: String, pub validated_docs: u64, pub altered_docs: u64 }
```

- `Field` keeps every attribute in one map, so the schema view shows attributes this version of Tessera does not know (§7.3 "forward compatible"). Typed accessors (`facet()`, `optional()`, `index()` …) read the map with the server defaults (`index` true, the rest false).
- Checked against a live 30.2 node: `GET /collections/{name}` sends `facet`, `index`, `infix`, `locale`, `optional`, `sort`, `stem`, `stem_dictionary`, `store` and `truncate_len` on every field, plus `curation_sets` on the collection. The PATCH response sends far more per field (`hnsw_params`, `model_config`, `nested`, `cascade_delete` …); it is not used for display.

### 4.2 Endpoints

| Method | Endpoint | Notes |
|---|---|---|
| `collections()` | `GET /collections` | Full schemas, for the field count. Sorted by the server, newest first |
| `collection(name)` | `GET /collections/{name}` | |
| `aliases()` | `GET /aliases` | Response `{"aliases": [...]}` |
| `create_collection(&Value)` | `POST /collections` | Body is the JSON the editor shows, sent as is. 201 → `Collection` |
| `update_collection(name, &Value)` | `PATCH /collections/{name}` | Long timeout (§4.3). Response ignored apart from the status |
| `delete_collection(name)` | `DELETE /collections/{name}` | Long timeout |
| `truncate_collection(name)` | `DELETE /collections/{name}/documents?truncate=true` | Returns `num_deleted`. Long timeout. Confirmed on 30.0 and 30.2 without `filter_by` |
| `schema_changes()` | `GET /operations/schema_changes` | `[]` when nothing runs |

- Collection names go into the path as one percent-encoded segment, so a name with `/`, `?` or a space cannot change the request. A unit test covers it.
- `send` gains a request body (JSON, `Content-Type: application/json`) and an optional per-request timeout. The body is recorded in the `RequestEvent` as in M1, so the request log shows it, with secrets masked (§7.6).
- The read-only guard, failover rules and request events of M1 apply unchanged. POST and PATCH are never retried.

### 4.3 Long operations

A PATCH on a large collection rewrites every document before it answers, and deleting or truncating a big collection takes time as well. The profile timeout (default 5 s) would report a failure while the server keeps working.

- These three calls use `LONG_TIMEOUT` = 30 minutes instead of the profile timeout.
- While a PATCH runs, the app polls `schema_changes()` every second on the same node and shows "validated N, altered M of T documents" (§7.4).
- Cancelling only stops the wait. The dialog says that the server keeps going and that the collection is reloaded when the user comes back.

### 4.4 Read-only unlock

- `Client` gets `set_read_only(bool)`. The flag becomes an `Arc<AtomicBool>` shared by all clones, like the active node, so the poller's clones agree. The guard in `send` reads it on each request.
- The M1 test of the guard stays. A new test unlocks, sends a write that passes, locks again, and sees the next write refused.

## 5. tessera-core

No GPUI dependency. Everything here is unit tested.

### 5.1 SchemaDraft

The model behind the form and the JSON editor.

```rust
pub struct SchemaDraft {
    pub name: String,
    pub fields: Vec<FieldDraft>,
    pub default_sorting_field: String,
    pub token_separators: Vec<String>,
    pub symbols_to_index: Vec<String>,
    pub enable_nested_fields: bool,
    pub synonym_sets: Vec<String>,
    pub metadata: Option<Map<String, Value>>,
    pub extra: Map<String, Value>,         // top-level keys the form does not edit
}

pub struct FieldDraft {
    pub name: String,
    pub r#type: FieldType,                 // enum of all §7.3 types plus Other(String)
    pub facet: bool, pub optional: bool, pub index: bool, pub sort: Option<bool>,
    pub infix: bool, pub stem: bool, pub store: bool, pub range_index: bool,
    pub locale: String, pub reference: String, pub async_reference: bool,
    pub num_dim: Option<u32>, pub vec_dist: Option<String>,
    pub embed: Option<Value>,              // edited in JSON mode only
    pub extra: Map<String, Value>,         // field attributes the form does not edit
}
```

- `to_json(&self) -> Value` writes only what differs from the server default, in a fixed key order (`name`, `type`, then attributes alphabetically). `sort` is `Option` because its default depends on the type (numbers sort by default, strings do not).
- `from_json(&Value) -> Result<SchemaDraft, DraftError>` accepts anything Typesense accepts and keeps unknown keys in `extra`, so a JSON → form → JSON round trip loses nothing. `DraftError` carries the line and column for the editor.
- `from_collection(&Collection) -> SchemaDraft` builds the draft for clone and modify. It drops the attributes the server fills in with defaults, so the clone's JSON shows what a person would have typed.
- **Field types:** `string`, `string[]`, `string*`, `int32`, `int32[]`, `int64`, `int64[]`, `float`, `float[]`, `bool`, `bool[]`, `geopoint`, `geopoint[]`, `geopolygon`, `object`, `object[]`, `image`, `auto`. Unknown type strings become `FieldType::Other` and pass through.
- **The AC test:** a table of schemas, each built once through `FieldDraft` setters and once typed as JSON text; `to_json()` of both must be equal as `serde_json::Value` after sorting `fields` by name.

### 5.2 Validation

`validate(&SchemaDraft) -> Vec<Issue>`, where `Issue { path: IssuePath, message_key, severity }` points at the collection or at field *n*. Errors block "Create"; warnings do not.

| Check | Severity |
|---|---|
| Collection name empty | error |
| No fields | error |
| Field name empty, or used twice | error |
| `default_sorting_field` names no field, or a field that is not `int32`, `int64`, `float`, or `string` with `sort` on (30.2 answers "is not a sortable type" for a plain string) | error |
| `num_dim` or `vec_dist` on a type other than `float[]` | error |
| `reference` not of the form `collection.field` | error |
| `embed` on a type other than `float[]`, or without `from` | error |
| A field named `id` with a type other than `string` | warning (Typesense ignores it for the document id) |
| `infix` on a non-string field | warning |

The server stays the authority. Its 400 message is shown inline under the editor in full (UX rule 2).

### 5.3 SchemaDiff

`diff(old: &Collection, new: &SchemaDraft) -> SchemaDiff` drives the modify flow.

- Field in `new` only → **add**. Field in `old` only → **drop** (`{"name": …, "drop": true}`). Same name, different attributes or type → **change**, sent as drop plus add in one request (§7.3, confirmed on 30.2).
- `synonym_sets` and `metadata` are patchable and appear in the diff when they change.
- The other collection options (`token_separators`, `symbols_to_index`, `enable_nested_fields`, `default_sorting_field`) cannot be changed by PATCH. The editor shows them read-only in modify mode.
- **Consequences** the preview names: the number of documents that will be rewritten (`num_documents` when any field is added or changed); dropping the `default_sorting_field` clears it on the server (seen on 30.2: dropping field `n` reset `default_sorting_field` to `""`); dropping a field that a `reference` elsewhere points to, when the list is loaded.
- `SchemaDiff::patch_body() -> Value` is what PATCH sends. An empty diff disables "Apply".

### 5.4 Secrets inside schemas

Embedding fields can carry credentials in `embed.model_config`: `api_key`, `access_token`, `refresh_token` and `client_secret`.

- `redact` masks these four keys as well as `api_key` and `value`, at any depth. This covers the request log and "Copy as curl" (§6.2 and §6.3 of the M1 design).
- The schema view masks them with the same function.
- **Clone** blanks them, and the field's row says "Enter the key for the embedding model again". A cloned schema never silently carries another collection's credentials.
- **Modify** never sends `embed` for an unchanged field, so a masked value never travels back to the server.

## 6. tessera-widgets

The crate the product spec reserves for reusable GPUI Kit compositions without app state (§5.1). Dependency direction `app → widgets → core`.

| Widget | What it does |
|---|---|
| `JsonEditor` | GPUI Kit `EditorState` with language `json`, line numbers and folding. Shows a parse error inline under the editor with line and column, and moves the cursor there on Enter. Read-only mode for the schema view |
| `ConfirmDanger` | The high-danger dialog of §8.2: title, the concrete consequences as a list, a text field that must equal the given name exactly, and a red action button that stays disabled until it does. Focus starts in the text field, never on the button (UX rule 3). Esc cancels |
| `FieldTypeSelect` | GPUI Kit `Select` over the field types of §5.1, with a short description per type |

- The medium-danger confirmation that M1 keeps in `views/confirm.rs` moves into the crate as `Confirm`, so all confirmations live in one place.
- JSON highlighting needs GPUI Kit's `tree-sitter` feature (tree-sitter plus the JSON grammar). The size of the Linux binary is measured in PR A. If it breaks NF5, the editor runs without highlighting (§12).

## 7. tessera-app

### 7.1 State

- `ConnectionState` gets a `CollectionsCache`: the list of `Collection`s, the aliases, when they were loaded and the last error. It loads when a collections tab opens and on ⌘R / Ctrl+R in such a tab, and it is cleared after every mutation of that connection.
- `ConnectionState` gets `writes_unlocked: bool`, which starts false and is never saved.
- `Profile` gets `favorite_collections: Vec<String>` (serde default, so M1 files load unchanged). For a temporary connection the favorites live in memory only.

### 7.2 Sidebar and tabs

- Under each connected profile, below Overview: **Collections** (opens the list tab), and under it the favorite collections (each opens its schema tab). The full list lives in the tab, so a cluster with a thousand collections does not flood the sidebar (D18).
- **Collections tab:** one per connection, titled "Collections" with the connection's color dot and environment badge, like the overview tab.
- **Collection tab:** one per collection, titled with the collection name. In M2 it shows the schema; M3 adds documents next to it.
- Opening a tab again focuses it. Deleting a collection closes its tab. Disconnecting closes all tabs of the connection, as in M1.

### 7.3 Collections list (canvas screen 1)

- **Columns:** favorite star, name, documents, fields, created (local date and time), aliases (names), synonym sets, curation sets. Counts are right-aligned and grouped by thousands per locale.
- **Search** above the table filters by name and alias, as you type. ⌘F / Ctrl+F focuses it.
- **Sort** by clicking a header or Enter on a focused header, as in M1's stats table. Favorites always come first.
- **Row actions,** in the context menu and as shortcuts on the focused row, each shown in the menu: Open (Enter), Toggle favorite (⌘D / Ctrl+D), Clone… (⌥⌘D / Ctrl+Alt+D; ⌘⇧D is M1's Disconnect, D15), Edit schema… (⌘E / Ctrl+E), Truncate… (⌘⇧⌫ / Ctrl+Shift+Backspace), Delete… (⌘⌫ / Ctrl+Backspace).
- **Header:** "New collection" (⌘⇧N / Ctrl+Shift+N) and Refresh (⌘R / Ctrl+R), both with the shortcut shown.
- **Empty list:** one sentence on what a collection is, the docs link, "New collection", and the `cargo xtask seed` hint for local connections.
- **Read-only connection:** the write actions show the lock and open the unlock dialog (§7.5) instead of being hidden, so the user learns why they are unavailable.

### 7.4 Collection tab: schema view

- **Header:** name, document count, created, aliases, synonym and curation sets, and the actions Edit schema, Clone, Truncate and Delete (same shortcuts as the list).
- **Field table:** name, type, then one column per attribute (`facet`, `optional`, `index`, `sort`, `infix`, `locale`, `stem`, `store`, `range_index`, `reference`, `num_dim`, `vec_dist`, `embed`, `truncate_len`). Boolean attributes show a check mark when on. Attributes not in this list get their own columns, filled with the raw JSON value, so a newer server's attributes still show.
- **Options:** `default_sorting_field`, `token_separators`, `symbols_to_index`, `enable_nested_fields`, metadata (pretty JSON).
- **JSON:** a read-only `JsonEditor` with the full response, secrets masked (§5.4), and "Copy JSON".

### 7.5 Read-only unlock (D10)

- Any write action in a read-only connection opens the unlock dialog first: `ConfirmDanger` with the profile name as the text to type and the consequences "Writes to {name} ({environment}) are allowed until you lock again or disconnect".
- After the unlock the action continues. The connection gets a "Writes unlocked" banner at the top of each of its tabs, in the environment color, with "Lock again" (⌘⇧L / Ctrl+Shift+L, shown on the button).
- The lock badges in the title bar, sidebar and tabs change to an open lock while unlocked.
- Disconnecting, or quitting, locks again. Nothing about the unlock is written to disk.

### 7.6 Create and clone (canvas screen 4 for the dialog frame)

A large dialog (GPUI Kit `Dialog`) with a Form | JSON switch at the top (⌘1 / ⌘2, Ctrl+1 / Ctrl+2).

- **Form:** collection name; the field list, one row per field (name, `FieldTypeSelect`, facet, optional, index, sort, and "More" for the remaining attributes); "Add field" (⌘↩ in the last row adds the next); remove and reorder per row; collection options below (`default_sorting_field` as a select over eligible fields, token separators, symbols to index, nested fields, synonym sets as text, metadata as JSON).
- **JSON:** a `JsonEditor` with the draft's `to_json()`, pretty-printed.
- **Sync, both ways:** every form change rewrites the JSON text. Every JSON edit that parses updates the form. While the JSON does not parse, the form shows "The JSON has an error on line N" with a button that jumps there, and the form is read-only until the JSON parses again. A guard flag stops the two updates from triggering each other.
- **Validation** (§5.2) runs on every change. Errors show next to the field they concern and in a summary above the buttons; "Create" stays disabled while there are errors.
- **Buttons:** Cancel (Esc) and Create (⌘↩ / Ctrl+↩). The request body is exactly the JSON tab's content. A server error shows inline above the buttons, in full; the dialog stays open with the draft.
- **Clone:** the same dialog, filled from `from_collection` with the name "{name}_copy" selected, and with embedding secrets blanked (§5.4).
- After a successful create, the collections cache reloads and the new collection's tab opens.

### 7.7 Edit schema

- The create dialog in modify mode: the name is fixed, the non-patchable options are read-only (§5.3), and "Create" becomes "Review changes…" (⌘↩ / Ctrl+↩).
- **Review** shows the `SchemaDiff`: added fields green, dropped red, changed fields with old and new attributes side by side, then the consequences (§5.3) and the exact PATCH body in a read-only `JsonEditor`. This is a medium-danger confirmation (§8.2). Focus starts on "Back", not on "Apply" (UX rule 3).
- **Apply** sends the PATCH and switches the dialog to progress: the counts from `schema_changes()`, a spinner, and "Stop waiting" with the note from §4.3. On success the dialog closes and the schema tab reloads. On failure the full server message shows and the dialog goes back to the review.

### 7.8 Truncate and delete

Both are high danger (§8.2) and use `ConfirmDanger` with the collection name as the text to type.

- **Truncate** consequences: "Deletes all N documents of {name}. The schema, aliases and synonym sets stay."
- **Delete** consequences: "Deletes {name} with its N documents and its schema." When aliases point at the collection: "These aliases will point to a collection that no longer exists: a, b." The alias list is reloaded when the dialog opens, so it is current.
- On prod connections the dialog title carries the PROD badge.
- After success: toast-free. The list updates in place, a deleted collection's tab closes, a truncated collection's count drops to 0. Failures show inline in the dialog.

### 7.9 i18n and keyboard

- Every new string goes through `t!()` with keys in `en.yml` and `de.yml`, including GPUI Kit `Select` and `Dialog` strings the new views start to use.
- Every shortcut above is shown on its control with `Kbd`. The plan checks each against GPUI Kit's and M1's bindings for conflicts (⌘D and ⌘E are only bound inside the collections views).

## 8. Seed data: `cargo xtask seed`

- A new `xtask` crate (workspace member, `publish = false`) and the alias `xtask = "run -p xtask --"` in `.cargo/config.toml`.
- `cargo xtask seed --url <url> [--key <key>] [--reset]`. The key defaults to `xyz`, the dev key. There is **no default URL**: the maintainer's port 8108 belongs to another project, and a seed command must never write to a server nobody named. Without `--url` it prints the usage and exits with 2.
- It refuses to touch a collection that already exists unless `--reset` is given, which drops the seed collections (and only those) first.
- **Sets,** generated in code from a fixed seed so every run gives the same documents:

| Collection | Documents | Shows |
|---|---|---|
| `books` | 10,000 | facets, sorting, `default_sorting_field`, an alias `books_current` and a synonym set `books_synonyms` |
| `authors` | 500 | target of a reference |
| `books_ref` | 2,000 | `reference: "authors.id"` |
| `places` | 1,000 | `geopoint` |
| `articles_vec` | 1,000 | `float[]` with `num_dim: 8`, no embedding model |

- Documents go in through `POST /collections/{c}/documents/import` in batches of 1,000. The import endpoint is used here directly over `reqwest`; the client's import API arrives in M5.
- `docs/building.md` documents the command.

## 9. Testing

| Level | What it checks |
|---|---|
| Client unit (`wiremock`) | Each endpoint's method, path, body and decoding; percent-encoding of names with `/`, `?` and spaces; unknown field attributes and top-level keys survive decoding; long timeout on PATCH, DELETE and truncate; unlock and lock with `set_read_only` |
| Client integration (dev server, `#[ignore]`, CI with `--ignored`, 30.0 and 30.2) | create → get → list → PATCH (add, drop, change) → schema_changes → truncate → delete, aliases list; a 409 on a duplicate name and a 400 on a bad schema carry the server message |
| Core unit | The AC table of §5.1; `from_json` keeps unknown keys; `to_json` omits defaults; `from_collection` drops server defaults; validation table of §5.2; diff table (add, drop, change, metadata, synonym sets, dropped sorting field, unchanged embed not sent); redaction of the four embedding secrets |
| App UI (`#[gpui_kit::test]`, fake `Connector` and a fake collections backend) | List loads and filters; favorites persist and come first; Enter opens the schema tab; form edit updates the JSON and a JSON edit updates the form; invalid JSON locks the form; Create sends the JSON tab's body; read-only write opens the unlock dialog and nothing is sent until the name is typed; Lock again; review focuses Back; truncate and delete need the exact name and default focus is in the text field; delete closes the tab |
| Secrets | A schema with an embedding `api_key` never shows it in the schema view, the request log or a clone |

UI tests do not reach the network, so the app gets a `CollectionsBackend` seam: a trait object global like M1's `Connector`, with the real implementation over `Client` and a fake in tests.

## 10. CI

- The `integration` job runs the new collection tests on 30.0 and 30.2.
- The `build` job keeps the NF5 size check; each PR records the Linux binary size in its description.
- `cargo xtask seed` runs once in the integration job against the service container, so the seed sets stay valid against both versions.

## 11. New dependencies

| Crate or feature | Used by | Why |
|---|---|---|
| `gpui-kit` feature `tree-sitter` (exposed by 0.9.0, forwards to `gpui-component`; pulls in `tree-sitter` and `tree-sitter-json`) | widgets | JSON highlighting in `JsonEditor` |
| `percent-encoding` (already in the tree through `url`) | client | path segment encoding |
| `reqwest` (already in the workspace) | xtask | seed import |

Versions are pinned exactly in the workspace `Cargo.toml` and checked at plan time.

## 12. Changes to the product spec

Recorded in `docs/spec.md` together with this design:

- §16 gains D18 and D19.
- §7.3 notes that schema inference arrives in M5.
- The §14 M2 row links this design.

| # | Question | Decision |
|---|---|---|
| D18 | Collections in the sidebar | Favorites only, under "Collections"; the full list is the collections tab |
| D19 | Schema inference from a file: M2 or M5? | M5, with import, which shares the file readers |

## 13. Risks

| Risk | Impact | Fallback |
|---|---|---|
| tree-sitter and the JSON grammar push the Linux binary over 40 MB (NF5) | Build job fails | `JsonEditor` without highlighting; parse errors still show |
| Two-way sync between form and JSON loops or loses the cursor | Editor unusable | The JSON text is only rewritten from the form while the JSON tab is hidden; switching tabs syncs |
| GPUI Kit `Table` with a column per unknown attribute gets wide | Schema view hard to read | Unknown attributes go into one "Other" column as compact JSON |
| A PATCH outlives the 30-minute timeout | The dialog reports a timeout while the server works on | The timeout message says the server may still be applying the change and offers Reload |
