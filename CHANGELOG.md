# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Connect to a Typesense server by URL and API key and see its version, Raft role and health.
- Welcome screen with the Docker command for a local Typesense server.
- Errors from the server are shown in full inside the connect dialog.
- English and German UI, chosen from the system language.
- Connection dialog for new and edited profiles: name, color, environment, cluster nodes, read-only, timeout and retries. Test tells an admin key from a restricted one, keys go to the system keychain, and a keychain failure offers "Use for this session only".
- Delete a saved profile (⌘⌫ / Ctrl+Backspace) after a confirmation that removes its stored key too; import profiles from a typesense-dashboard `config.json` with a preview; export profiles as JSON without keys; reopen the last connection at startup; save a connection from flags or environment variables as a profile.
- Connection profiles with environment tags (local, dev, staging, prod), a visible prod band and window frame, and a read-only guard that blocks writes.
- Multi-node connections with failover between nodes.
- Live overview: a card per node with role, latency, the active node (click or Enter to switch) and, for an unreachable node, the full error and a retry countdown; version, collections, documents, Typesense memory, CPU, memory, disk and network figures; 15-minute CPU, memory, disk and network (rx · tx) charts; API stats sorted by requests per second. Polling pauses with Space and runs every 2, 5, 10 or 30 s; ⌘R / Ctrl+R reloads the collections. Every node is polled on its own loop and retried with a backoff of 5 s up to 30 s. The status bar follows the active node (role, host, latency of its last poll), says when it does not answer, and shows "N of M nodes unreachable" ("Node unreachable" for a single node). Document and collection counts use the locale's digit grouping. Polling stops while the window is hidden. When the active node is down, its figures and charts are dimmed and show the age of the last sample.
- Docked window layout: the sidebar lists each connection with its Overview, every connection opens in its own tab (color dot, name, environment badge), and the request log panel opens at the bottom with ⌘J / Ctrl+J. The prod band and frame follow the active tab.
- Request log panel: every request with time, method, path, node, status, duration and size (plus the connection while several are open); filters for errors, writes, methods, connections and polling traffic; a detail view with headers and bodies, secrets masked; Copy as curl (⌘⇧C / Ctrl+Shift+C) with a `$TYPESENSE_API_KEY` placeholder. The log keeps 5000 user and 500 polling requests and masks `api_key` and `value` secrets and the `x-typesense-api-key` query parameter. ⌘W / Ctrl+W closes a tab without disconnecting; ⌘⇧↩ / Ctrl+Shift+Enter zooms a tab.
- Command-line flags `--host`, `--port`, `--protocol`, `--path`, `--env`, `--read-only`, `--api-key-stdin`, `--profile` and `--help`, plus the `TYPESENSE_HOST`, `TYPESENSE_PORT`, `TYPESENSE_PROTOCOL`, `TYPESENSE_PATH` and `TYPESENSE_API_KEY` variables. A key is never accepted as a flag value.
- `THIRD_PARTY_NOTICES.md` with the `webpki-root-certs` notice (CDLA-Permissive-2.0).
- A 3-node dev cluster (compose profile `cluster`) and nightly cluster tests for leader detection and failover.
- CI on macOS, Linux and Windows: lint, tests, integration tests against Typesense 30.0 and 30.2, release builds with a size check and a smoke start.
- Collections tab per connection: a sortable table with document and field counts, creation time, aliases and synonym and curation sets, a search over names and aliases (⌘F / Ctrl+F), favorites first (⌘D / Ctrl+D) and Enter to open a collection. Favorites also appear under "Collections" in the sidebar. Reload with ⌘R / Ctrl+R.
- Collection schema view: fields, facets, sorting, references and embedding settings of a collection, opened with Enter from the list.
- Favorite collections, kept per connection.
- `cargo xtask seed --url <url> [--key <key>] [--reset]` fills a dev server with five reproducible sample collections, an alias and a synonym set (see `docs/building.md`).
- `tessera-widgets` crate for reusable GPUI widgets.
- Change a collection's schema (⌘E / Ctrl+E) with a review before anything is sent: added, dropped and changed fields, the consequences (documents rewritten, default sorting field cleared, references that would break) and the exact PATCH body. Applying shows live schema-change progress, and "Stop waiting" leaves the change running on the server.
- Truncate a collection (⌘⇧⌫ / Ctrl+Shift+Backspace) behind a type-the-name confirmation that shows the current document count.
- Delete a collection (⌘⌫ / Ctrl+Backspace) behind a type-the-name confirmation that shows the current document count and the aliases that would dangle.
- On read-only connections, every write (create, clone, schema change, truncate, delete) first asks to unlock writes for the session.
- Create collections from a form or from JSON, kept in sync, with live validation and server errors shown inline (New collection ⌘⇧N / Ctrl+Shift+N). Clone a collection (⌘⇧D / Ctrl+Shift+D) from the list or the schema view; the clone blanks embedding credentials.
- Unlock writes on a read-only connection by typing the profile name. A "Writes unlocked" banner offers "Lock again" (⌘⇧L / Ctrl+Shift+L); writes lock again on disconnect and the unlock is never saved.
- Client calls to read documents: a page with filter, sort and excluded fields, counts, facet values, one document by id and a JSONL export. The request log notes when a response body is not kept for document pages.

### Fixed

- Embedding credentials (`api_key` and similar) are masked in the schema view, the request log and "Copy as curl".
- A key that may read collections but not aliases still lists its collections; the list, the schema view and the delete dialog say inline that the aliases could not be loaded and why.
- A long truncate or delete no longer blocks the window: "Stop waiting" closes its dialog while the server keeps going, and the list reloads when it is done. A schema change, truncate or delete that hits the 30-minute timeout is not retried on another node and says the server may still be applying it.
- On read-only connections, the write controls (New collection, Edit schema, Clone, Truncate, Delete) show the lock until writes are unlocked.

### Changed

- The release profile aborts on panic (`panic = "abort"`), which keeps the Linux binary under the 40 MB limit (NF5).
- Profiles are stored in `connections.json` without keys; keys are stored in the OS keychain.
- The live tests use `http://localhost:18108` by default instead of port 8108. The dev server still starts on 8108 unless `TYPESENSE_PORT` is set.
