# Building Tessera

Tessera builds with the Rust toolchain pinned in `rust-toolchain.toml`; `rustup` installs it on the first `cargo` call. The platform prerequisites come from the GPUI Kit 0.9 installation guide.

## Prerequisites

### macOS

- macOS 15 or later to build (the app targets macOS 13+)
- Xcode Command Line Tools: `xcode-select --install`, then check `xcode-select -p`

### Windows

- Windows 10 or later
- Visual Studio 2022 Build Tools or Community with the **Desktop development with C++** workload (MSVC and a Windows SDK). Use the MSVC Rust toolchain, not GNU.
- CMake on `PATH` (`cmake --version`)

### Linux (Ubuntu 24.04)

```bash
sudo apt update
sudo apt install -y gcc g++ clang libfontconfig-dev libwayland-dev \
  libwebkit2gtk-4.1-dev libxkbcommon-x11-dev libx11-xcb-dev \
  libssl-dev libzstd-dev vulkan-validationlayers libvulkan1
```

Other distributions need the equivalent development packages. Running the app needs a Wayland or X11 session with a working Vulkan driver.

## Run

```bash
cargo run -p tessera-app
```

`TYPESENSE_API_KEY` prefills the key in the connect dialog. `RUST_LOG=debug` shows one log line per request (never the key).

`cargo run -p tessera-app --features frame-overlay` adds a frame-time overlay, cycled with ⌘⌥F / Ctrl+Alt+F in the documents view.

## Local Typesense

```bash
docker compose -f dev/docker-compose.yml up -d
```

This starts Typesense 30.2 on port 8108 with the local-only key `xyz`. If port 8108 is taken, set `TYPESENSE_PORT`, for example `TYPESENSE_PORT=18108`. `TYPESENSE_VERSION` picks another image tag.

To run Tessera against a dev server on a custom port:

```bash
TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d
cargo run -p tessera-app -- --host localhost --port 18108 --api-key-stdin <<< xyz
```

### Seed data

```bash
cargo xtask seed --url http://localhost:18108 [--key xyz] [--reset]
```

Fills the server with `books` (10,000 documents, with the alias `books_current` and the synonym set `books_synonyms`), `authors` (500), `books_ref` (2,000, references `authors`), `places` (1,000, geopoints), `articles_vec` (1,000, 8-dimensional vectors) and `products` (200,000, with a nested `dimensions` object, about 10 s to load). The documents are generated from a fixed seed, so every run gives the same data. There is no default URL; without `--url` the command prints its usage and exits with 2. The key defaults to `xyz`. Documents are streamed in batches of 1,000. If one of these collections exists, the command stops and names it; `--reset` drops those collections, the alias and the synonym set first, and nothing else.

## Tests

```bash
cargo test --workspace
```

Integration tests against the local server are ignored by default:

```bash
cargo test -p tessera-client --test live -- --ignored
```

They use `http://localhost:18108` and key `xyz`; override with `TESSERA_IT_URL` and `TESSERA_IT_KEY`.

### Cluster tests

A 3-node Raft cluster (ts1 to ts3, API on ports 18111 to 18113, key `xyz`) lives behind the compose profile `cluster`, next to the single node. Plain `up -d` does not start it.

```bash
docker compose -f dev/docker-compose.yml --profile cluster up -d ts1 ts2 ts3
cargo test -p tessera-client --test cluster -- --ignored --test-threads=1
docker compose -f dev/docker-compose.yml --profile cluster stop ts1 ts2 ts3
```

The cluster uses the fixed subnet 172.30.111.0/24 (static peering IPs); if Docker reports "Pool overlaps", change it in `dev/docker-compose.yml` and `dev/cluster/nodes`.

Run them single-threaded: two tests stop and restart ts3. `TESSERA_CLUSTER_URLS` (comma separated, in the order ts1, ts2, ts3) overrides the URLs. With the override set, the two stop/start tests skip (they print why), because they stop the local compose ts3, not a node of the other cluster. CI runs them nightly (`.github/workflows/nightly.yml`).

## Where the request log keeps bodies

Everything stays in memory; nothing about a request reaches the disk. An entry holds its bodies cut to 1 MiB. A body up to 32 MiB also keeps its whole text beside the entries, 64 MiB over all of them, oldest out first, so the entry says "Full body no longer kept" once its text is gone. Document pages keep no response body at all. Show full (⇧⌘B / Ctrl+Shift+B in the log) masks the text on the background executor before it shows, and pages through it in chunks of 512 KiB; Copy all takes the whole masked text.

## Keyboard shortcuts

`keymap.toml` in the config directory adds to the default shortcuts or replaces them: `~/Library/Application Support/org.tessera.Tessera/` on macOS, `~/.config/tessera/` on Linux, `%APPDATA%\tessera\Tessera\config\` on Windows. The file is read once at start.

```toml
[[binding]]
key = "cmd-shift-p"
action = "tessera::OpenPalette"

[[binding]]
key = "cmd-k"        # no action: the chord does nothing
context = "Workbench"
```

- `key` is one chord, or several separated by spaces, with the modifiers `cmd`, `ctrl`, `alt`, `shift`.
- `action` is an action name such as `tessera::Refresh`, the names in `crates/tessera-app/src/actions.rs`. Leave it out to unbind the chord.
- `context` limits the binding to a key context such as `Workbench`, `Console` or `Documents`. Leave it out for everywhere.
- A binding without a modifier (`cmd`, `ctrl` or `alt`) also fires while typing in a field, so bind a chord.

A control shows the chord that is bound now. A bad entry is skipped and named above the tabs, and the others apply. A file that does not parse, or has a field the format does not know, changes nothing: the default shortcuts stay, and the line above the tabs names the place. `escape` cannot be bound, because it closes every dialog. An unbound action stays reachable from its control and the palette.

## Manual checks of the relevance resources (M6)

Nothing here has been done yet: the views are covered by UI tests with fakes and the client and core by live tests, and none of it has been looked at. Run against the dev server after `cargo run -p xtask -- seed --url http://localhost:18108 --key xyz --reset`. Record the result in the PR description.

1. Each view, ⌥⌘1 to ⌥⌘5 (Ctrl+Alt+1 to 5): aliases, synonym sets, curation sets, stopwords and presets list the seeded `books_current`, `books_synonyms`, `books_curation`, `en_common` and `books_fantasy`. Create, edit and delete one of each.
2. The curation dialog of `books_curation` in light and in dark: Form ⌘1, JSON ⌘2, include rows, hide rows, the document picker. Nothing is cut off at the smallest window, and the body scrolls.
3. Drag an include to another position, then the same with ⌥↑ and ⌥↓. The positions follow the order.
4. Open the date picker in the curation dialog, which is a stacked dialog: it opens over the dialog, its time field shows, and Esc closes the picker and leaves the dialog.
5. The Test tab of `books_synonyms` and of `books_curation`: both sides run, and each says what it means.
6. One reindex of `books` through `books_current`: on the Aliases tab select it and press ⇧⌘R. Check that nothing is sent before Start and that the Jobs tab opens without taking focus. After the copy, the compare shows 10,000 on both sides. Switch, check that `books_current` points to the new collection, Switch back, Switch again, then Delete `books`, typing the name. `--reset` does not know the new collection, so delete it by hand before the next seed.
7. A stopped reindex: Start again, press ⌘. during the export, and check that the alias did not move, the temp directory under `$TMPDIR` named `tessera-reindex-*` is gone, and `Delete {new}` and `Start over` show.
8. On macOS, every shortcut shown on a control in these views does what its control does.

## Manual checks of import and export (M5)

Nothing here has been done yet. The headless run above is NF3's number; this is the run through the window, and it stays open until someone records it in the last PR's description.

1. `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, then `cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000 --keep-file /tmp/nf3.jsonl`, which also leaves the file for the import below.
2. `cargo run --release -p tessera-app -- --host localhost --port 18108 --api-key-stdin <<< xyz`. Import `/tmp/nf3.jsonl` into a new collection with Import → New collection….
3. While it runs, in a terminal: `while :; do ps -o rss= -p $(pgrep -x tessera); sleep 1; done`. Pass: the peak stays below 300 MB.
4. Start a second import of the same file, press Cancel in the Jobs panel, and note that the panel stopped within one batch (the job row says `stopped`, and no further batch arrives).
5. Delete the new collection afterwards. Record the machine, both RSS series and the stop time.
6. Export the collection as JSONL, JSON and CSV with a filter and with a selection, open each file, and check that a stopped export leaves an existing target untouched.
7. Open the import dialog, the export dialog and the schema dialog at 960×600. Nothing may be cut off.
8. Record the Linux release binary size from the CI job (NF5, below 41,943,040 bytes). A macOS release binary is 23,397,360 bytes, which says nothing about Linux.

## Manual checks of the search milestone (M4)

Nothing here has been done yet; record the results in PR D's description.

- Open the Search tab, the Console, the palette, ⌘P and the keymap error line at 960×600 (the smallest window). Nothing may be cut off.
- NF4: scroll a large response in Raw and in the request log's full view with the frame overlay on.
- Press ⌘K / Ctrl+K in the documents table and run Delete documents: the confirmation names the rows.
- Record the Linux release binary size from the CI job.

## Manual checks of the document browser (M3)

Nothing here runs in CI or from a script: it needs a person at the machine, because the numbers come from the frame overlay on screen. NF2 is **not measured** until someone records the results below in PR C's description.

### NF2: 200k rows at 60 fps

Hardware: an Apple M1 or comparable machine, a release build, no other heavy load.

1. `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, then `cargo xtask seed --url http://localhost:18108 --reset` (about 10 s for `products`).
2. `cargo run --release -p tessera-app --features frame-overlay`, connect to `localhost:18108` with key `xyz`, open `products`.
3. Press ⌥⌘F (Ctrl+Alt+F) twice to show the detailed overlay.
4. With the default columns: wheel-scroll for 30 s, then drag the scrollbar from end to end for 30 s. Read the draw time p95 from the overlay. Repeat with all columns shown (Columns popover).
5. In a terminal run `ps -o rss= -p <pid>` every 10 s during a 5-minute scroll. Pass: flat after the first pass over the data.
6. Pass: p95 draw time at most 16.6 ms. Record p95 for both column sets, the RSS at the start and the end, and the machine in the PR description. If p95 is higher, profile before merging; the first suspects are the allocations in `render_td` and the number of visible columns.

### Other checks to do on screen

- `products`: scroll to the middle and the end, sort by `price`, filter `price:>100`, and check that the status line and the rows agree.
- Tick rows with the mouse, Shift+click and Space; delete two, then delete by query with a filter that matches more than 1,000 documents and check that it asks for the collection name.
- Update by query with a value of the wrong type shows the warning, not a success.
- Copy a selection as JSONL, CSV and curl and paste each into a terminal or editor.

## Measuring NF3

NF3 says an import of 1 million documents stays below 300 MB of RSS. `cargo xtask nf3` measures it without a display: it writes 1,000,000 documents of about 1 KB to a temporary file, imports them through the real `run_import` on a 2-worker runtime with a request log of 5,000 and 500 entries, samples `ps -o rss=` every 200 ms, then imports 200,000 documents into a second collection and stops half way. It refuses to run when `tessera_nf3` or `tessera_nf3_cancel` exists, and drops both at the end, also on a failure. It has no default URL; never point it at a server you do not own. `--keep-file <path>` writes the documents there and leaves them; a file that exists is refused, not overwritten.

```bash
TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d
cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000
cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000 --format json
cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000 --format csv
```

The number that counts adds the app. `engine_rss_mb` is the peak minus the baseline of the process, so what the import adds; `--app-baseline-mb` adds the RSS of the app with its window, and `total_rss_mb` is the sum. It prints one line of JSON and exits 1 when the total reaches `--limit-mb` (300) or a check fails: all documents imported, none failed or unconfirmed, `num_documents` right, no import started after the stop, and the run returned only after the batch in flight answered.

The baseline comes from the smoke start of a release build, `cargo build --release -p tessera-app` first:

```bash
# macOS: "maximum resident set size" in bytes, divide by 1,048,576
TESSERA_SMOKE=1 /usr/bin/time -l target/release/tessera
# Linux: "Maximum resident set size" in KiB, divide by 1,024
TESSERA_SMOKE=1 xvfb-run -a /usr/bin/time -v target/release/tessera
```

That baseline holds the window, fonts and theme, but no connection, sidebar content or jobs panel, so the total is a lower bound of the running app. The stop in the cancel run lands half a round trip after a progress update, inside a batch in flight. A stop in the moment between two batches can see one more `POST` start about 100 µs later: it passed the check of `stop` before the stop came, and is awaited and counted like any batch in flight.

The nightly workflow runs this as the job `nf3`, on Linux.

### Record of the headless run (M5)

Measured on the finished M5 branch (`b036c25`), release builds, Apple M3 Max with 128 GB, macOS 27.0.1, Typesense 30.2 from `dev/docker-compose.yml` on port 18108, 1,000,000 documents of about 1 KB each, `--app-baseline-mb 93`:

| Format | Time | Documents per second | Peak RSS (MB) | Engine RSS (MB) | Total with the app (MB) | Slowest batch | Stop latency |
|---|---|---|---|---|---|---|---|
| JSONL | 33.7 s | 29,659 | 33.1 | 24.6 | 117.6 | 214 ms | 13 ms |
| JSON array | 35.2 s | 28,429 | 29.2 | 19.7 | 112.7 | 201 ms | 17 ms |
| CSV | 39.5 s | 25,290 | 30.0 | 21.6 | 114.6 | 286 ms | 17 ms |

All three passed every check against the 300 MB limit. The app's baseline from three smoke starts of `target/release/tessera` was 93.0 MB (the first, cold start), then 64.1 and 64.0 MB; the table uses the 93.0. This is macOS: the nightly job measures Linux, and the numbers above do not stand for it. The collections `tessera_nf3` and `tessera_nf3_cancel` were gone afterwards.


## Smoke start

```bash
TESSERA_SMOKE=1 cargo run -p tessera-app
```

The app opens its window, renders one frame and exits with code 0. CI uses this to prove the app starts on every platform.
