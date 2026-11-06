# M6 Relevance Resources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Manage aliases, synonym sets, curation sets, stopwords and presets, with attachment, side-by-side test panels and a reindex assistant, on Typesense 30.0 and 30.2.

**Architecture:** `tessera-client` gains alias, stopword and preset-delete calls, and one `sets` module for both set kinds. `tessera-core` gains `relevance/`: names, drafts, validation, attachment lists, set files, test sides, reindex preflight. `tessera-app` gains a `RelevanceBackend` seam, one resources tab per kind, a shared set view, a test panel on `HitsView`, a curation dialog, preset mode in the workbench, and a reindex assistant that runs M5's export and import jobs unchanged. Three PRs, in order: A (Tasks 1–5), B (Tasks 6–11), C (Tasks 12–14).

**Tech Stack:** Rust 2024, GPUI Kit 0.9.0 (gpui-pre 0.3.11), reqwest 0.13.5, tokio 1.54.1, csv 1.4.0, tempfile 3.27.0, jiff 0.2.39, chrono 0.4.45 (app only, new direct dependency), serde_json 1.0.154, wiremock 0.6.5.

**Spec:** [`docs/superpowers/specs/2026-11-06-m6-relevance-design.md`](../specs/2026-11-06-m6-relevance-design.md). Section numbers below (§4.4 …) refer to it. Read the section a task names before starting it. §4.4 lists every server trap found by probing.

## Global Constraints

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `tessera-client` and `tessera-core` never depend on GPUI. Direction: `app → widgets → core → client`.
- Direct dependencies are pinned exactly (`=x.y.z`) in the workspace `Cargo.toml`. Task 11 adds `chrono = "=0.4.45"` for `tessera-app`. Task 12 gives `tessera-core` the workspace's `jiff`. Task 13 makes `tempfile` a normal dependency of `tessera-app` if it is dev-only. No new crate enters `Cargo.lock`.
- Never guess a GPUI or GPUI Kit API. Read the pinned source under `~/.cargo/registry/src/*/` first (`gpui-component-0.9.0`, `gpui-base-0.9.0`, `gpui-kit-0.9.0`, `gpui-pre-0.3.11`). A file named in a task was verified.
- Typesense truth is `spec/typesense-openapi.yml`, the v30.2 docs and §4.4. Where the OpenAPI file and §4.4 disagree (for example `curation_sets` is not a search parameter), §4.4 was measured and wins.
- Every UI string goes through `t!()`, same keys in `assets/locales/en.yml` and `de.yml`, flat keys, under `resources.*`, `aliases.*`, `synonyms.*`, `curations.*`, `sets.*`, `stopwords.*`, `presets.*`, `test.*`, `reindex.*`. The key-parity test stays green. Core words nothing: it returns enums, and the app translates them.
- No blocking I/O on the GPUI thread and none on a tokio worker. Reading or writing a set file and creating a temp directory go through the backend on the runtime.
- No key in logs, files, errors, `Debug` or `Display`. Redaction tests are never removed. A preset saved in preset mode goes through `without_secrets`.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. No commit with a failing check.
- Commits: `git commit -s`, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Dev Typesense on port 18108, key `xyz`. Live tests: `TESSERA_IT_URL=http://localhost:18108 TESSERA_IT_KEY=xyz cargo test -p tessera-client --test live_relevance -- --ignored`. Each client and core live test also runs against 30.0: `docker run --rm -d --name t300 -p 18208:8108 typesense/typesense:30.0 --data-dir /tmp --api-key=xyz --enable-cors=false`, then the same command with `TESSERA_IT_URL=http://localhost:18208`, then `docker stop t300`. **Never send anything to port 8108.** Throwaway names `tessera_it_*`, removed at the end of each test.
- UX rules: environment visible everywhere (connection name and `env_badge::render` in dialog titles and tab titles); errors inline and in full, through `long_error_text`; default focus never on a destructive action; no dead controls; every action keyboard-reachable with its shortcut shown on the control via `Kbd::binding_for_action(&Action, Some(CONTEXT), window)`.
- Shortcuts are bound in key contexts. Platform constants follow the `#[cfg(target_os = "macos")]` pairs in `crates/tessera-app/src/actions.rs`. New actions go into `ACTION_LABELS` with a `palette.action.<Name>` key. Disabled actions are gated through `gated(context, gates)`.
- Writes go through `ConnectionState::ensure_writable(window, cx, then)`, and the control shows `env_badge::lock(id, false, cx)` while `writes_locked()`.
- Anything a user types or a file holds that ends in a URL path or query passes an allowlist: `name_ok` for a new name, `param_safe` for a name in a query parameter, `addressable` plus `SEGMENT` in every path. Enums are written from fixed strings.
- Every `PUT` here is an upsert and its answer proves little. New checks a fresh list first. A stored result is read back and compared. Destructive flows confirm first (D62), and cancelling sends nothing.
- A write runs detached (`cx.spawn(...).detach()`), so closing a tab never aborts it.
- NF5: the Linux release binary stays below 41,943,040 bytes. Each PR records the size in its description.
- Each task ends with its CHANGELOG entry under `## [Unreleased]` → `### Added` in `CHANGELOG.md`, in the same commit. Plain prose, in the style of the existing entries.

## Review Focus

1. Deleting an attached set must detach it first, stop at a failed detach, and check afterwards; a missing attached synonym set makes every search on that collection fail. Test in Task 9.
2. An attachment `PATCH` must read the collection and the set names right before, and never list a missing name. Test in Task 7 and Task 9.
3. Every New must refuse an existing name after a fresh list: `PUT` overwrites silently. Tests in Tasks 4, 5 and 8.
4. Duplicate item ids must be refused before the `PUT`: the server keeps the last and answers 200. Test in Task 7.
5. A test panel must say what each side means, and no test may write. Test in Task 10.
6. The left side of an attached set is `enable_synonyms=false`, and a curation set cannot be tested unattached. Test in Task 7.
7. The form must not lose fields: `extra` round-trips, and a key the server dropped is reported. Test in Task 11.
8. The reindex must send nothing before Start, must never move the alias before the compare, and must stop if the alias moved. Tests in Task 13.
9. A required `store: false` field must block a copy from the old collection, and an optional one must warn. Test in Task 12.
10. The copy must run as export to a private temp directory, then import, never as one stream: the server ends an export whose reader stalls (design §4.5). The temp directory must go in every ending. Test in Task 13.
11. Preset mode must not save to a preset after `load` from elsewhere. Test in Task 5.

---

## PR A · Simple resources (branch `feat/m6-resources`, from the tip of `feat/m5-export`)

### Task 1: Client for aliases, stopwords and preset delete

**Files:**
- Create: `crates/tessera-client/src/stopwords.rs`, `crates/tessera-client/tests/live_relevance.rs`, `crates/tessera-client/tests/relevance.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-client/src/search.rs`

**Interfaces (exported from `lib.rs`):**

```rust
impl Client {
    pub async fn alias(&self, name: &str) -> Result<Alias, Error>;                                // GET /aliases/{name}
    pub async fn upsert_alias(&self, name: &str, collection: &str) -> Result<Alias, Error>;       // PUT, body {"collection_name":…}
    pub async fn delete_alias(&self, name: &str) -> Result<(), Error>;                            // DELETE
    pub async fn delete_preset(&self, name: &str) -> Result<(), Error>;                           // DELETE /presets/{name}
    pub async fn stopwords(&self) -> Result<Vec<StopwordSet>, Error>;                             // GET /stopwords, {"stopwords":[…]}
    pub async fn stopword_set(&self, id: &str) -> Result<StopwordSet, Error>;                     // GET, {"stopwords":{…}}
    pub async fn upsert_stopwords(&self, id: &str, words: &[String], locale: Option<&str>) -> Result<(), Error>; // PUT
    pub async fn delete_stopwords(&self, id: &str) -> Result<(), Error>;                          // DELETE
}
pub struct StopwordSet { pub id: String, pub stopwords: Vec<String>, #[serde(default)] pub locale: Option<String> }
```

- Paths use a private `resource_path(root, name)`: `addressable(name)?`, then `format!("{root}/{}", utf8_percent_encode(name, SEGMENT))`. Refactor `preset_path` onto it.
- Writes use the private `write`, reads `get_ok`. `upsert_stopwords` leaves `locale` out when `None`.

- [ ] **Step 1: Write the failing wiremock tests** in `tests/relevance.rs`: paths for each call with a name of `a/b c` encoded as `a%2Fb%20c`; `.` and `..` refused without a request; the alias body; `upsert_stopwords` with and without `locale`; `stopword_set` unwraps `{"stopwords":{"id":"x","stopwords":["a"]}}`; `stopwords` reads the list; a 404 keeps `typesense_message()`; each write on a read-only client is `Error::ReadOnly` with `expect(0)`.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test relevance`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Live tests** in `tests/live_relevance.rs` (`#[ignore]`, helpers copied from `tests/live.rs`):
  - `live_alias_round_trip`: create, read, repoint, list, delete.
  - `live_alias_accepts_a_missing_target`: `upsert_alias` to a missing name succeeds, and a search through it is 404.
  - `live_alias_of_an_alias_does_not_resolve`: `client.collection(chain)` is 404.
  - `live_alias_cannot_take_a_collection_name`: 500 with `conflicts` in the message.
  - `live_delete_missing_alias_is_404`.
  - `live_stopwords_round_trip`: `["The","the","  x y "]` stores the set `{"the","x","y"}`, compared as a set; the empty list is 400; a search with `stopwords=<missing>` is 404.
  - `live_preset_delete`: `value: "str"` lists and deletes; `preset=<missing>` in a search still answers 200.
- [ ] **Step 5: Run** against 18108 and 30.0, as in the constraints.
- [ ] **Step 6: Run** the checks, CHANGELOG, commit. `feat(client): aliases, stopwords and preset delete`.

### Task 2: Core names, alias states and preset values

**Files:**
- Create: `crates/tessera-core/src/relevance/mod.rs`, `crates/tessera-core/src/search/presets.rs`
- Modify: `crates/tessera-core/src/lib.rs`, `crates/tessera-core/src/search/mod.rs`, `crates/tessera-app/src/views/search/queries.rs`

**Interfaces:**

```rust
// relevance/mod.rs
pub const MAX_NAME: usize = 128;
pub fn name_ok(name: &str) -> bool;    // first char [A-Za-z0-9_], rest [A-Za-z0-9_.-], at most MAX_NAME
pub fn param_safe(name: &str) -> bool; // not empty, no comma, no control character, no edge whitespace
pub enum AliasState { Ok, TargetMissing, TargetEmpty, TargetIsAlias, Shadowed }
pub fn alias_state(alias: &Alias, collections: &[Collection], aliases: &[Alias]) -> AliasState;
// Order: Shadowed if a collection has the alias's name; TargetEmpty; TargetIsAlias if another
// alias has the target's name and no collection does; TargetMissing; Ok.

// search/presets.rs: moved from views/search/queries.rs, behaviour unchanged, errors typed
pub enum PresetError { NotAnObject, TooMany, Param(ParamError) }
pub enum PresetKind { Search, Multi(usize), Union(usize), Invalid(PresetError) }
pub fn preset_kind(value: &Value) -> PresetKind;
pub fn preset_drafts(value: &Value, current: &str) -> Result<(Vec<SearchDraft>, bool), PresetError>;
pub fn preset_value(drafts: &[SearchDraft], union: bool) -> Result<Value, ParamError>;
pub fn preset_summary(value: &Value) -> String; // q and query_by, or the collections, joined by ", "
```

- `queries.rs` keeps one small `preset_error_text(&PresetError)` with the existing `search.preset.*` keys. `to_search` moves along if it is only used here; if `multi.rs` also uses it, make it `pub` in `params.rs` instead.

- [ ] **Step 1: Write the failing unit tests:** `name_ok` table (`a`, `_a`, `a.b-c_1`, 128 and 129 chars, ``, `.a`, `a b`, `a,b`, `é`); `param_safe` table; `alias_state` one test per variant, including the precedence of `Shadowed`; `preset_kind` for `{}`, `{"searches":[{}]}`, with `union`, `"str"`, 100 searches; `preset_drafts` and `preset_value` moved tests still pass.
- [ ] **Step 2: Run** `cargo test -p tessera-core relevance` and `cargo test -p tessera-core presets`. Expected: compile errors.
- [ ] **Step 3: Implement, move the preset code,** and keep `ui_search.rs` preset tests green.
- [ ] **Step 4: Run** the checks, CHANGELOG (a short line, or none if nothing user-visible: then add it with Task 5), commit. `feat(core): names, alias states and preset values`.

### Task 3: Backend seam, navigation and the resources tab

**Files:**
- Create: `crates/tessera-app/src/relevance.rs`, `crates/tessera-app/src/views/resources/mod.rs`
- Modify: `crates/tessera-app/src/lib.rs`, `crates/tessera-app/src/state.rs`, `crates/tessera-app/src/actions.rs`, `crates/tessera-app/src/views/mod.rs`, `views/dock.rs`, `views/root.rs`, `views/sidebar.rs`, `views/palette.rs`, locales
- Test: `crates/tessera-app/tests/common/mod.rs`, `crates/tessera-app/tests/ui_resources.rs` (new)

**Interfaces:**

```rust
// relevance.rs. Same shape as search.rs: trait, Global, impl for TokioBackend, tokio_backend().
// This task: the alias, stopword and preset methods of design §7.1. Task 8 and 9 add the rest.
pub trait RelevanceBackend: Send + Sync { /* … */ }
pub struct RelevanceBackendGlobal(pub Arc<dyn RelevanceBackend>);

// state.rs
pub enum ResourceKind { Aliases, Synonyms, Curations, Stopwords, Presets } // Debug, Clone, Copy, PartialEq, Eq
pub struct OpenResource(pub Uuid, pub ResourceKind);
impl AppState { pub fn open_resource(&mut self, id: Uuid, kind: ResourceKind, cx: &mut Context<Self>); }

// views/dock.rs
pub struct ResourcePanel { /* connection, kind, view: ResourceView, focus_handle */ }
impl ResourcePanel { pub fn new(connection, state, kind, window, cx) -> Self; pub fn connection(&self); pub fn kind(&self) -> ResourceKind; }
```

- Install the global next to the others in `lib.rs` (`run`) and in the test setup (`tests/common/mod.rs`, a `FakeRelevance` shaped like `FakeSearch`, recording calls in a `Mutex<Vec<String>>`).
- `root.rs`: add `Kind::Resource(ResourceKind)` and `ConnectionTab::Resource(Entity<ResourcePanel>)` to every match, and the `connection_tab` downcast chain. Subscribe to `OpenResource` as to `OpenConsole`. `panel_name` is `"Resource"`. Title as `ConsolePanel::title` with `sidebar.{kind}` (`aliases`, `synonyms`, `curations`, `stopwords`, `presets`).
- `views/resources/mod.rs` starts with a placeholder `ResourceView` for all five, replaced by the real views in Tasks 4, 5, 8, 11. It must not ship as a dead control: this PR's branch adds Synonyms and Curations rows only in Task 8 and Task 11 (UX rule 4). So in this task only the Aliases, Stopwords and Presets rows appear, and Task 8 and 11 add theirs.
- Sidebar: focus maps `resource_focus: HashMap<(Uuid, ResourceKind), FocusHandle>`, one `link_row` per kind after Console, with `Kbd::global_binding_for_action`. `Synonyms` and `Curations` only with `info.capabilities.supported`.
- Actions: `OpenAliases`, `OpenSynonyms`, `OpenCurations`, `OpenStopwords`, `OpenPresets` bound globally (`alt-cmd-1` to `5`, `ctrl-alt-1` to `5`), handled in `root.rs` under `can_search` like `OpenSearchTab`, gated off when the row is not shown. `NewResource` (`NEW_COLLECTION`), `EditResource` (`EDIT`), `DeleteResource` (`DELETE`), `ReloadResource` (`REFRESH`), `FocusResourceFilter` (`FIND`) in the new context `RESOURCES = "Resources"`.
- Palette: the tabs appear as actions for the selected connection through `ACTION_LABELS`.

- [ ] **Step 1: Write the failing UI tests** in `ui_resources.rs`: `sidebar_shows_resource_rows_when_connected`; `synonyms_rows_hide_below_v30` (use `ServerCapabilities::from_version("29.0")`); `opening_twice_selects_the_same_tab`; `tab_title_shows_environment`; `shortcut_opens_the_tab`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_resources`. Expected: fails.
- [ ] **Step 3: Implement.** The five locale keys: `sidebar.aliases` "Aliases"/"Aliase", `sidebar.synonyms` "Synonyms"/"Synonyme", `sidebar.curations` "Curations"/"Kuratierungen", `sidebar.stopwords` "Stopwords"/"Stoppwörter", `sidebar.presets` "Presets"/"Presets".
- [ ] **Step 4: Run** the checks, commit. `feat(app): resources tab and navigation`.

### Task 4: Aliases view

**Files:**
- Create: `crates/tessera-app/src/views/resources/aliases.rs`
- Modify: `views/resources/mod.rs`, locales
- Test: `crates/tessera-app/tests/ui_aliases.rs`

Design §7.4 and §7.3, shortcuts §7.12.

- Table on `DataTable` with a `TableDelegate` as `collections_list.rs` (`gpui-component-0.9.0/src/table`), filter input, columns Name, Target, Documents, State. Rows from `ConnectionState::collection_cache` (`load_collection_list`), reloaded with ⌘R, `aliases_error` inline.
- State text per `AliasState`: `aliases.state.target_missing` "Target does not exist", `.target_empty` "No target", `.target_is_alias` "Target is an alias: it does not resolve", `.shadowed` "Hidden by a collection of the same name".
- `AliasDialog` for New and Repoint: name `Input` (read-only on Repoint), `SelectState` over collection names (`gpui-component-0.9.0/src/select.rs`), inline errors, focus on the first control, Save ⌘↩ in a new context `ALIAS_DIALOG = "AliasDialog"` with an `… > Input` twin.
- New: `ensure_writable`, fresh `CollectionsBackend::list`, refuse a name that fails `name_ok`, equals a collection, or equals an alias. The last says `aliases.exists` with the current target and a Repoint button. Then per D62, `save_alias`, compare `collection_name`, `invalidate_collections`, close.
- Repoint: D62 via `Danger`: `Confirm` off prod, `ConfirmDanger` with `expected: alias` on prod. Consequences: `aliases.repoint.line` "{alias}: {old} ({old_docs} documents) → {new} ({new_docs} documents)", and `aliases.repoint.differs` "Searches through this alias change at once." Default focus per widget.
- Delete: same split, consequence "Searches through this alias answer 404." A 404 shows `aliases.gone` and reloads.
- The Reindex button arrives in Task 13.
- Empty state: `aliases.empty` with the docs link `https://typesense.org/docs/30.2/api/collection-alias.html`.

- [ ] **Step 1: Write the failing UI tests:** states per row; `new_refuses_an_existing_alias` (no `save_alias` call); `new_refuses_a_collection_name`; `new_refuses_a_bad_name`; `locked_profile_sends_nothing_until_unlocked`; `repoint_confirms_and_saves`; `repoint_on_prod_needs_the_name`; `cancelling_sends_nothing`; `mismatch_in_the_answer_shows_inline`; `delete_of_a_gone_alias_reloads`; `focus_starts_on_cancel_in_confirm`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_aliases`. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): aliases view`.

### Task 5: Stopwords, presets and preset mode

**Files:**
- Create: `crates/tessera-app/src/views/resources/stopwords.rs`, `views/resources/presets.rs`
- Modify: `views/resources/mod.rs`, `views/search/mod.rs`, `views/search/queries.rs`, `views/root.rs`, `state.rs`, `views/palette.rs`, locales
- Test: `crates/tessera-app/tests/ui_stopwords.rs`, `crates/tessera-app/tests/ui_presets.rs`

Design §7.10, §7.11.

- Stopwords: split with `h_resizable` as `views/search/mod.rs`; right side `JsonEditor::plain` plus locale `Input`. New: `name_ok`, fresh `stopwords`, refuse existing. Save: lines trimmed, empties dropped, inline `stopwords.empty` if none, D62, `save_stopwords` (reads back), replace the text with the stored words, sorted for display, and when the sorted lowercased sent words differ from them show `stopwords.normalised` "Stored {n} words: the server lowercases, splits at spaces and removes duplicates."
- Stopwords delete consequence: `stopwords.delete.line` "Searches that name this set fail with 404 until they stop naming it."
- Presets table: name, kind (`preset_kind`), summary (`preset_summary`). `Invalid` shows the reason and disables Open.
- `OpenSearch` becomes `OpenSearch(pub Uuid, pub Option<(Vec<SearchDraft>, bool)>, pub Option<String>)`, `open_search` gains `preset: Option<String>`, and every caller passes `None` (`views/palette.rs:312`, `views/sidebar.rs:520`, `views/root.rs:695`). The root's handler calls `workbench.load` then `workbench.set_preset(preset, cx)`.
- `SearchWorkbench`: new field `preset: Option<String>`, `pub fn preset(&self) -> Option<&str>`, `pub fn set_preset(&mut self, name: Option<String>, cx)`. `load` clears it, and the handler sets it after, so anything else that loads leaves the mode.
- The band: above the toolbar, `presets.mode` "Preset {name}", `Save` with `Kbd` for `SaveQuery`, and `Leave`. In this mode the `SaveQuery` listener calls `save_preset_mode`: `ensure_writable`, `Confirm` on prod, `preset_value(&without_secrets(&self.drafts), self.union)`, `send_preset`. Errors land in `queries.write_error`, which already shows inline.
- New preset: name dialog (`name_ok`, fresh `backend.presets`), then `open_search(id, Some((vec![SearchDraft{collection: current, params: default}], false)), Some(name))`, where `current` is the workbench's selected collection or the first collection. No write until Save.
- Delete: D62, with `presets.delete.line` "Searches that name this preset keep working, but silently lose its parameters."

- [ ] **Step 1: Write the failing UI tests.** Stopwords: `save_shows_the_stored_words`; `normalised_note_shows`; `empty_is_refused`; `new_refuses_existing`; `locked_sends_nothing`; `delete_on_prod_needs_the_name`. Presets: `kinds_show`; `invalid_cannot_open_but_deletes`; `open_loads_drafts_in_preset_mode`; `save_writes_to_that_name`; `save_strips_secrets`; `new_sends_nothing_before_save`; `load_from_history_leaves_the_mode`; `leave_keeps_the_drafts`.
- [ ] **Step 2: Run** the two test binaries. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): stopwords, presets and preset mode`.

---

## PR B · Sets (branch `feat/m6-sets`, from `feat/m6-resources`)

### Task 6: Client for synonym and curation sets

**Files:**
- Create: `crates/tessera-client/src/sets.rs`
- Modify: `crates/tessera-client/src/lib.rs`, `crates/tessera-client/tests/relevance.rs`, `crates/tessera-client/tests/live_relevance.rs`

**Interfaces:** design §4.2 exactly, exported from `lib.rs`. `SetKind::path(self) -> &'static str` gives `synonym_sets` or `curation_sets`. Paths: `{kind}/{name}` and `{kind}/{name}/items/{id}`, each segment through `addressable` and `SEGMENT`. `upsert_set` sends `{"items":…}`. `upsert_set_item` removes `id` from a clone of an object body.

- [ ] **Step 1: Write the failing wiremock tests:** for both kinds, every path with a name and an id holding `/`, `,`, ` `; `upsert_set_item` body has no `id`; list decodes a bare array; a 404 keeps its message; every write on a read-only client is refused with `expect(0)`.
- [ ] **Step 2: Run** `cargo test -p tessera-client --test relevance`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Live tests** (`#[ignore]`):
  - `live_sets_round_trip` for both kinds: create, list, read, item add, item read, item delete, set delete.
  - `live_set_duplicate_ids_are_reduced`: two sent, one stored.
  - `live_item_put_echoes_but_stores_less`: unknown key in the `PUT` answer, not in `set_item`.
  - `live_item_of_a_missing_set_is_404`.
  - `live_attachment_by_patch`: `update_collection(c, {"synonym_sets":[s]})`, read back, `{"synonym_sets":[]}`; a missing name is 404 and changes nothing; the same for `curation_sets`.
  - `live_deleted_attached_synonym_set_breaks_search`: `q=*`, `enable_synonyms=false` and `multi_search` all fail with `Synonym index not found`; `export_stream` works; `PATCH` to `[]` heals; recreating the set heals.
  - `live_deleted_attached_curation_set_leaves_search`: 200, while a `PATCH` adding another name to the stale list is 404.
  - `live_synonym_sets_parameter_adds`: attached `sofa=couch`, named `sofa=chair`, finds all three; `enable_synonyms=false` finds one.
  - `live_curation_sets_is_no_parameter`: on an unattached set, `curation_sets=name` gives the same hits as the plain search, and a missing name is 200.
  - `live_curation_tags_gate_both_ways`: design §4.4 cases.
  - `live_rule_flags_by_version`: `rule.stem` true reads back true on ≥ 30.2 and absent on 30.0, by `debug()`; `diversity` with `1.0` is stored on both.
  - `live_bad_sort_breaks_matching_search`: 404 on the matching query only.
- [ ] **Step 5: Run** against 18108 and 30.0.
- [ ] **Step 6: Run** the checks, CHANGELOG, commit. `feat(client): synonym and curation sets`.

### Task 7: Core models, validation, attachment, set files and test sides

**Files:**
- Create: `crates/tessera-core/src/relevance/synonyms.rs`, `curations.rs`, `changes.rs`, `attachment.rs`, `synonym_file.rs`, `test.rs`
- Modify: `crates/tessera-core/src/relevance/mod.rs`, `crates/tessera-core/src/lib.rs` (`ServerCapabilities::rule_flags(&self) -> bool`: known version ≥ 30.2, unknown true)

**Interfaces:** design §5.2 to §5.5 exactly. Additional:

```rust
pub enum Level { Error, Warning }
pub enum ItemIssue { NoId, DuplicateId { id: String, rows: (usize, usize) }, TooFewSynonyms, NoSynonyms,
    BadMatch(String), NoTrigger, NoAction, BadInclude(String), BadPosition(i64), IncludedAndExcluded(String),
    DatesReversed, UnknownSortField { field: String, collection: String } }
impl ItemIssue { pub fn level(&self) -> Level; }
pub fn sort_warnings(sort_by: &str, collections: &[Collection]) -> Vec<ItemIssue>;
pub fn dropped_keys(sent: &Value, stored: &Value) -> Vec<String>; // dotted paths, one level into `rule`
pub enum SetFormat { Json, Csv }
pub fn format_of(path: &Path) -> SetFormat; // .csv and .tsv are Csv, else Json
pub enum SetFileError { Json(String), Csv { row: u64, message: String }, Items(Vec<ItemIssue>), TooLarge(u64) }
```

- `to_value` writes: synonyms `id`, `synonyms`, `root` only if not empty, `locale` and `symbols_to_index` only if set, `extra`. Curations per design §5.2. `rule.synonyms` and `rule.stem` only when true. `includes` as `{"id","position"}`, `excludes` as `{"id"}`. Empty includes and excludes are left out.
- `from_value` on a stored item maps `root: ""` to empty. It keeps any key without a field in `extra`, at both levels, including `diversity`.
- `CurationDraft::new_default()`: `stop_processing: true`. `CurationDraft::default_remove_tokens(&self) -> bool`: true when includes and excludes are empty (design §4.4).
- `test_sides` per the table in design §7.6, `broad = attached.len() > 1`. `tags` is sent as `curation_tags` on the right side only, when not empty. `TestBlocked::NotAttached` for an unattached curation set, `NameNotSendable` for an unattached synonym set failing `param_safe`.
- CSV reader: `csv::ReaderBuilder` with `.flexible(true)`, required headers `id` and `synonyms`, the rest optional, in any order. Writer: `csv::Writer`. List cell per D67. `row_{n}` is 1-based on data rows.

- [ ] **Step 1: Write the failing unit tests:** one test per row of the issue table; round trip of a stored 30.2 rule with `diversity` and an unknown key; `changes` with reorder only (no changes); `with_set`, `without_set` keeping order and dropping unknown names; `missing`; `attachment_patch`; CSV and JSON round trips including `;` in a value, `[` first, quotes, newline in a cell; a duplicate id in a file reports both rows; `format_of`; `test_sides` for each row of §7.6; `sort_fields` skipping `_text_match(buckets:10):desc`, `_eval(x:1):desc`, `$a(b:asc)`; `dropped_keys`; `rule_flags` at 29.0, 30.0, 30.2, 31.0, unknown.
- [ ] **Step 2: Run** `cargo test -p tessera-core relevance`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, commit. `feat(core): relevance models and validation`. CHANGELOG with Task 8.

### Task 8: Synonym view: sets, items and JSON

**Files:**
- Create: `crates/tessera-app/src/views/resources/sets.rs`, `views/resources/synonyms.rs`
- Modify: `crates/tessera-app/src/relevance.rs`, `views/resources/mod.rs`, `views/sidebar.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_sets.rs`

Design §7.1, §7.5.

- `RelevanceBackend` gains `sets`, `set`, `save_set`, `delete_set`, `save_item` (`PUT` then `GET`), `delete_item`.
- `SetsView<K: ItemKind>`, or a plain struct holding `kind: SetKind` and an enum for the item columns and dialog: take the plain struct. It owns the split, the set table, the tab bar, and the JSON tab, so Task 11 adds only a table column list and a dialog.
- Tabs with `TabBar` as `views/search/mod.rs:840`, actions `ShowItems`, `ShowSetJson`, `ShowAttachment`, `ShowTest` on `SHOW_FORM`, `SHOW_JSON`, `SHOW_HITS`, `SHOW_RAW`. This task shows Items and JSON; Tasks 9 and 10 add theirs.
- New set: `name_ok`, fresh `sets`, refuse existing, `save_set(name, vec![])` with `Confirm` on prod.
- Items: `SynonymItemDialog`, context `SET_ITEM_DIALOG`, `SaveResource` on `SAVE`. Id, a `ButtonGroup` multi-way or one-way, `root` shown for one-way only, synonyms by `JsonEditor::plain` one per line, locale, `symbols_to_index` comma-separated, raw block as `JsonEditor` when `extra` is not empty. Errors from `check_synonym` inline, Save disabled on `Level::Error`.
- Edit with a changed id: `save_item` new, then `delete_item` old; a failed delete shows inline and says both exist.
- After a save, `dropped_keys(sent, stored)` not empty shows `sets.dropped` "The server did not keep: {keys}".
- JSON tab: Save parses `{"items":[…]}`, `check_items`, `changes`. No removals and off prod: `save_set` at once. Otherwise D62 with the three counts in the consequences. Compare `items.len()` with the answer's; a difference shows inline.
- Row action Delete item: `Confirm`.
- Empty state with `https://typesense.org/docs/30.2/api/synonyms.html`.
- The Synonyms sidebar row appears in this task.

- [ ] **Step 1: Write the failing UI tests:** `sets_list_with_counts`; `new_refuses_existing`; `item_save_uses_the_item_endpoint`; `validation_blocks_save`; `changing_an_id_saves_then_deletes`; `dropped_keys_show`; `json_review_counts`; `json_duplicate_ids_are_refused`; `json_removal_on_prod_needs_the_name`; `locked_sends_nothing`; `cancelling_sends_nothing`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_sets`. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): synonym sets`.

### Task 9: Attachment, missing sets, delete, import and export

**Files:**
- Create: `crates/tessera-app/src/views/resources/attachment.rs`, `views/resources/set_file.rs`
- Modify: `crates/tessera-app/src/relevance.rs`, `views/resources/sets.rs`, `views/resources/synonyms.rs`, `actions.rs`, locales, `crates/tessera-core/src/transfer/report.rs`
- Test: `crates/tessera-app/tests/ui_sets.rs`

Design §7.5, §7.7, §5.3, §5.4.

```rust
pub struct AttachOutcome { pub list: Vec<String>, pub dropped: Vec<String>, pub mismatch: bool }
fn attach(&self, client: Client, kind: SetKind, collection: String, set: String, attach: bool) -> BoxFuture<Result<AttachOutcome, Error>>;
fn read_set_file(&self, path: PathBuf) -> BoxFuture<Result<String, SetFileError>>;       // size check first, then read
fn write_set_file(&self, path: PathBuf, text: String) -> BoxFuture<Result<(), String>>;
```

- `attach` in the tokio impl: `client.collection(c)`, `client.sets(kind)`, `with_set` or `without_set`, `update_collection(c, attachment_patch)`, `client.collection(c)` again; `mismatch` when the read list differs.
- `tessera_core::transfer::write_text(path: &Path, text: &str) -> io::Result<()>`: public wrapper on `PartFile` in `report.rs`, with `private = true`, write, flush, `sync_all`, persist. `write_set_file` calls it.
- Collections tab: filter, `Checkbox` per collection, attached first. `pending: Option<String>` disables boxes while a toggle runs.
- Missing sets above the set table, from `missing(collections, kind, known)`, with `Detach` running `attach(…, false)` per collection, in order, stopping at an error, and `New` prefilled. Text `sets.missing.synonym` "`{set}` does not exist but is attached to {collections}. Every search on them fails." and `sets.missing.curation` "`{set}` does not exist but is attached to {collections}. Schema changes that keep it will fail."
- Delete set per design §7.7. Inside `ConfirmDanger`'s task, or for `Confirm` a detached task with the view showing busy: fresh `list`, fresh `users`, `attach(false)` each, `delete_set`, fresh `collections`, and if any lists the set, inline `sets.delete.still_listed`. A failed detach returns `Err` with `sets.delete.stopped` "Stopped at {collection}: {error}. The set still exists. Detached from: {done}."
- Import ⇧⌘I (`ImportResource`, `IMPORT_DOCUMENTS`) and Export ⇧⌘X (`ExportResource`, `EXPORT_DOCUMENTS`) in the Synonyms view only; Curations does not get them, per design §3. `SetImportDialog`: file, format override, target new set or selected with Merge or Replace, counts by `changes`, Start ⌘↩, D62. `cx.prompt_for_paths` as `views/import_dialog.rs`, `cx.prompt_for_new_path` as `views/sidebar.rs`.

- [ ] **Step 1: Write the failing UI tests:** `toggle_attaches_with_fresh_lists` (call order); `box_shows_the_answer_not_the_click`; `dropped_names_show`; `mismatch_shows`; `missing_sets_show_with_detach`; `delete_detaches_then_deletes` (call order); `delete_stops_at_a_failed_detach` (set not deleted, text names both); `delete_checks_afterwards`; `cancelling_delete_sends_nothing`; `import_merge_counts`; `import_replace_on_prod_needs_the_name`; `import_refuses_duplicate_ids`; `export_writes_csv`; `import_has_no_control_on_curations`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_sets`. Expected: fails.
- [ ] **Step 3: Implement,** with a unit test for `write_text`: target unchanged when writing fails, permissions `0600` on Unix.
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): attach, detach and move synonym sets`.

### Task 10: Test panel

**Files:**
- Create: `crates/tessera-app/src/views/resources/test_panel.rs`
- Modify: `views/resources/sets.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_test_panel.rs`

Design §7.6, §5.5.

```rust
pub struct TestPanel { /* connection, kind, set, collection SelectState, schema: Option<Collection>,
    q, query_by, tags: InputState, left, right: Entity<HitsView>, sides: Result<TestSides, TestBlocked>,
    answers: [Option<Result<SearchAnswer, SharedString>>; 2], tasks: [Option<Task<()>>; 2] }
```

- Picking a collection: `CollectionsBackend::get`, then `query_by` = `query_fields(&schema).join(",")`, then `sides = test_sides(kind, set, attached, tags)`. A failed read shows inline.
- Run ⌘↩ (`RunTest`, context `TEST_PANEL`, `… > Input` twin): pairs are `q` (`*` when blank), `query_by` when not blank, `per_page=10`, `enable_analytics=false`, plus the side's. Two `SearchBackend::search` calls with default tags, each kept in `tasks`, so a new Run or dropping the view cancels the old. Each answer goes into its `HitsView` with `set_result`, or an inline error on that side.
- Header of each side: meaning text (`test.side.as_configured`, `.with_set`, `.no_synonyms`, `.no_curations`), `found`, `search_time_ms`.
- Note lines: `test.note.added`, `test.note.broad_synonyms`, `test.note.broad_curations`, `test.note.tags` "A rule with tags fires only when its tag is named here, and rules without tags do not fire then.", `test.blocked.not_attached`, `test.blocked.name`. A `curation_tags` value fails `param_safe` per tag: inline, Run disabled.
- Blocked: Run disabled and the note shown, with a button to the Collections tab (`ShowAttachment`). No attach control here.

- [ ] **Step 1: Write the failing UI tests**, with `FakeSearch` recording params: one per row of §7.6 with exact params; `broad_note_shows`; `one_side_failing_leaves_the_other`; `new_run_drops_old_answers`; `runs_on_a_locked_profile`; `blocked_sends_nothing`; `no_side_writes` (no `RelevanceBackend` call); `query_by_from_schema`; `meaning_follows_a_fresh_read` (the cache says unattached, the fresh collection says attached).
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_test_panel`. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): test panels for sets`.

### Task 11: Curation sets

**Files:**
- Create: `crates/tessera-app/src/views/resources/curations.rs`, `views/resources/curation_dialog.rs`
- Modify: `Cargo.toml`, `crates/tessera-app/Cargo.toml`, `Cargo.lock`, `views/resources/mod.rs`, `views/resources/sets.rs`, `views/sidebar.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_curations.rs`

Design §7.8.

- Add `chrono = { version = "=0.4.45", default-features = false, features = ["std"] }` to the workspace and `chrono.workspace = true` to `tessera-app`. `cargo fetch` must add no new crate.
- Items table columns: id, trigger (query with match, or tags), includes, excludes, active, stops. `active`: `Scheduled`, `Expired` or `Active`, by `effective_*_ts` against `jiff::Timestamp::now()`, computed in core: `RuleTime` from `CurationDraft::timing(now: i64)`.
- `CurationItemDialog`: width 760, title with connection name and `env_badge::render`, tabs Form ⌘1 and JSON ⌘2 converting through `CurationDraft`. Bad JSON keeps the text, disables Save, and shows `JsonEditor::error` inline.
- Rows: new `INCLUDE_ROW` context, `MoveRowUp` and `MoveRowDown` on `alt-up`, `alt-down`, focusable rows. Drag: copy the `DragChip` pattern in `views/search/form.rs:70` and `:609`. A move renumbers positions 1 to n. `position` `Input` per row, integers only.
- Picker: `SelectState` over collections, attached first; `q` input; 150 ms debounce through a stored task as `views/jobs/import_dialog.rs`; `SearchBackend::search` with `per_page=20`, `enable_curations=false`, `enable_analytics=false`, `exclude_fields` of vector fields (`is_vector`), `query_by` from `query_fields`. A row: id, the first string field, `+ Pin`, `+ Hide`. A manual id input with `Add` too.
- Dates: `DatePickerState::new(window, cx).time_precision(TimePrecision::Minute)` from `gpui_kit::component::date_picker` and `time_field`, `DatePicker::new(&state).cleanable(true)`, labels `curations.from_utc`, `curations.to_utc`. Conversion `NaiveDateTime::and_utc().timestamp()` and `DateTime::from_timestamp(ts, 0)`. Read `DatePickerEvent::Change`. Verify `time_precision`'s exact receiver before using it: `date_picker.rs:269`.
- `rule.synonyms` and `rule.stem` checkboxes only when `capabilities.rule_flags()`. With a draft holding one on an older server: `curations.flags_dropped`.
- `sort_warnings` against `users(collections, Curation, set)` schemas from the cache, shown as a warning; Save stays enabled.
- Raw blocks: `metadata` and `extra` (rule-level and item-level merged for display under `rule.` and top-level keys) each as a `JsonEditor`.
- Save: `check_curation`, D62, `save_item`, `dropped_keys`.
- Sets share everything else from `sets.rs`, including delete and attachment. Docs link `https://typesense.org/docs/30.2/api/curation.html`. The Curations sidebar row appears here.

- [ ] **Step 1: Write the failing UI tests:** `form_to_json_keeps_extra`; `json_to_form_keeps_unknown`; `bad_json_blocks_save_and_keeps_text`; `new_rule_defaults`; `remove_tokens_follows_the_rule_until_touched`; `include_moves_by_key`; `include_moves_by_drag`; `positions_renumber`; `picker_adds_pin_and_hide`; `picker_sends_enable_curations_false`; `dates_round_trip_in_utc`; `from_after_to_blocks_save`; `flags_hide_on_30_0`; `sort_warning_names_the_collection`; `dropped_keys_show`; `locked_sends_nothing`; `tab_order_reaches_every_control`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_curations`. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): curation sets`.

---

## PR C · Reindex (branch `feat/m6-reindex`, from `feat/m6-sets`)

### Task 12: Core preflight and compare

**Files:**
- Create: `crates/tessera-core/src/relevance/reindex.rs`
- Modify: `crates/tessera-core/Cargo.toml`, `Cargo.lock`, `crates/tessera-core/src/relevance/mod.rs`, `crates/tessera-core/tests/live_transfer.rs`

**Interfaces:** design §5.6 exactly. `Compared.expected` is the source's fresh count, or for a file source the job's `imported`; the caller decides.

- `reindex_name` with `jiff::Timestamp` formatted through `.strftime("%Y%m%d_%H%M%S")` in UTC. Add `jiff.workspace = true` to `crates/tessera-core/Cargo.toml`. The stripped base is cut so the result fits `MAX_NAME`.
- `referenced_by`: `field.attr("reference").and_then(Value::as_str)`, the part before the first `.`, equal to `source.name` or to any alias on it.
- `required_unstored`: names in `unstored` whose draft field has `optional` false and is not dropped from the draft. The draft's field model is `SchemaDraft::fields` in `schema/mod.rs`.

- [ ] **Step 1: Write the failing unit tests:** `reindex_name` plain, with an old suffix, at the length limit; `preflight` one test per field, including `unstored` versus `required_unstored`, a reference by alias, another alias, a missing set, `embed`, blanked secrets; `without_missing` touches both `synonym_sets` and `extra["curation_sets"]`; `Compared::equal` table.
- [ ] **Step 2: Run** `cargo test -p tessera-core reindex`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Live test** `live_copy_through_a_file` in `tests/live_transfer.rs` (`#[ignore]`): a collection of 2,000 documents with nested, geopoint, a vector and a key outside the schema; `run_export` into a `tempfile::tempdir()`, create from `SchemaDraft::from_collection(..).to_json()` under a new name, `run_import` with `Source::File`, `action Create`; counts equal; exports of both, sorted by `id`, are byte equal. A second test: an optional `store:false` field is gone, and a required one fails every line in the report.
- [ ] **Step 5: Run** against 18108 and 30.0.
- [ ] **Step 6: Run** the checks, commit. `feat(core): reindex preflight and comparison`.

### Task 13: Reindex assistant

**Files:**
- Create: `crates/tessera-app/src/views/resources/reindex.rs`
- Modify: `crates/tessera-app/Cargo.toml`, `crates/tessera-app/src/connection.rs`, `relevance.rs`, `state.rs`, `views/dock.rs`, `views/root.rs`, `views/resources/aliases.rs`, `actions.rs`, locales
- Test: `crates/tessera-app/tests/ui_reindex.rs`

Design §7.9.

```rust
pub struct ReindexState { /* alias, source, draft, preflight, target name, source choice, step, dir, jobs subscription */ }
pub enum CopySource { Collection, File(PathBuf) }
pub enum Step { Schema, Exporting(JobId), Importing(JobId), Compare(Compared), Switched(Compared), Done }
// connection.rs
pub reindex: HashMap<String, Entity<ReindexState>>,
// relevance.rs
fn copy_dir(&self) -> BoxFuture<Result<Arc<tempfile::TempDir>, String>>; // Builder::new().prefix("tessera-reindex-").tempdir(), on spawn_blocking
```

- `Kind::Reindex(String)` and `ReindexPanel` in `dock.rs`, as `ResourcePanel`. Opened by `OpenReindex(Uuid, String)`. `ReindexAlias` on `cmd-shift-r` / `ctrl-shift-r` in `RESOURCES`, disabled for a broken alias with the reason. `REVERT`'s constant is the same chord in another context.
- Step Schema: on open a fresh `list`, `sets` for both kinds, then `from_collection`, `blank_secrets`, `preflight`, `without_missing`. `Edit schema…` (`EditResource`) is `SchemaDialog::open_pick(state, connection, draft, on_pick)`, whose JSON goes through `SchemaDraft::from_json` and reruns `preflight`. `reindex.notes.*` per design sketch. Start disabled per design §7.9, reason inline.
- Start (`NextStep`, `SAVE`, context `REINDEX`): `ensure_writable`, re-check that the name is free with a fresh list, `Confirm` on prod, `CollectionsBackend::create`. Failure stays, inline.
- From the old collection: `copy_dir`, then `Jobs::start_export` with `ExportJob { collection: source, scope: All, fields: All, format: Jsonl, target: dir.path().join("documents.jsonl"), expected: Some(count) }`, label `reindex.job_export`. `ReindexState` subscribes to `Jobs`'s `JobFinished`. On `Finished` with `docs == expected`, or with a `Confirm`-free inline warning when they differ, `Jobs::start_import` with `ImportJob { source: Source::File(path), options: ReadOptions::new(Format::Jsonl), collection: target, params: ImportParams { action: Create, batch_size: DEFAULT_BATCH, dirty_values: CoerceOrReject } }`. From a file: `sniff` for the options and straight to import.
- The dock opens on Jobs (existing `toggle_bottom` helper path), without moving focus. Both job rows show in the panel, with their own Cancel.
- The state holds `dir: Option<Arc<TempDir>>` and drops it as soon as the import job ends in any way, on a stopped or failed export, and on `Start over`. A disconnect drops the entity and with it the directory. `cx.on_app_quit` drops it as well. Read `Jobs::start`'s quit registration in `jobs.rs` first: the jobs must be told to stop before or independently of this, and on Windows a file still open only leaves the directory behind.
- A stopped or failed job: `reindex.stopped` with the alias unchanged, `Delete {new}` through `collection_danger::delete`, `Start over` (back to Schema with a new name).
- Compare: two fresh `CollectionsBackend::get`, with failed and unconfirmed from the import's `outcome`. `Save report` reuses the job's report through the panel: the button opens the Jobs tab on that job. `Compare again` ⌘R.
- Switch: `RelevanceBackend::alias`; if it no longer equals `source`, stop with `reindex.alias_moved`. D62: typed when prod, unequal or `referenced_by` not empty. `save_alias`, `alias` again, compare, `invalidate_collections`, `collection_changed`. `Switch back` re-checks and uses `Confirm`.
- Clean up: `collection_danger::delete(state, connection, old, count, window, cx)`, then Done, which removes the entry from `connection.reindex`. `Keep it` does too.

- [ ] **Step 1: Write the failing UI tests** with `FakeBackend`, `FakeTransfer`, `FakeRelevance`: `nothing_is_sent_before_start`; `cancelling_nothing_created`; `required_unstored_blocks_collection_source`; `file_source_is_allowed_then`; `blank_secret_blocks`; `missing_set_is_dropped_from_the_draft_and_named`; `start_creates_then_exports_then_imports` (call order, exact `ExportJob` and `ImportJob`); `closing_the_tab_keeps_state_and_jobs`; `stop_during_export_leaves_alias_and_removes_dir`; `failed_import_stays_on_copy_with_delete_offer`; `compare_equal_goes_on`; `compare_unequal_raises_danger`; `file_source_compares_with_imported`; `alias_moved_stops_the_switch` (no `save_alias`); `mismatch_after_put_shows`; `switch_back_rechecks`; `referenced_by_is_in_the_consequences`; `locked_sends_nothing`; `broken_alias_disables_reindex`; `done_removes_state`.
- [ ] **Step 2: Run** `cargo test -p tessera-app --test ui_reindex`. Expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `feat(app): reindex assistant`.

### Task 14: Seed, docs and the full check on both versions

**Files:**
- Modify: `xtask/src/seed.rs`, `docs/architecture/async.md`, `docs/building.md`, `docs/spec.md` (§14 acceptance note only if something changed), `README.md` (feature list, if it has one), `CHANGELOG.md`

- `seed`: add a curation set `books_curation` attached to `books` with one rule pinning a document for `q=novel`, a tag rule `sale`, a stopword set `en_common`, a preset `books_fantasy`, with `--reset` removing them, as `SYNONYMS` and `ALIAS` do. `§12.2` lists curation sets.
- `async.md`: a paragraph on the reindex copy: export then import, and why not one stream (design §4.5).
- `building.md`: a section with the on-screen checks for M6: each view, the curation dialog in light and dark, drag of an include, the date picker in a stacked dialog, one reindex of the seeded `books` through `books_current`, and the shortcuts on macOS.
- Full run: all `live_relevance`, `live`, `live_transfer` tests against 18108 and against a fresh 30.0 container, `cargo xtask seed --url http://localhost:18108 --reset`, then stop the container.

- [ ] **Step 1: Write the seed changes** and run `cargo run -p xtask -- seed --url http://localhost:18108 --key xyz --reset` twice: the second run must pass too.
- [ ] **Step 2: Write the docs.**
- [ ] **Step 3: Run** the live tests on both versions. Record the result in the PR description.
- [ ] **Step 4: Run** the checks, CHANGELOG, commit. `docs: M6 relevance resources`.
