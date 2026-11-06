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

A control shows the chord that is bound now. A bad entry is skipped and named above the tabs, and the others apply. A file that does not parse, or has a field the format does not know, changes nothing: the default shortcuts stay, and the line above the tabs names the place. `escape` cannot be bound, because it closes every dialog. An unbound action stays reachable from its control and the palette.

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

## Smoke start

```bash
TESSERA_SMOKE=1 cargo run -p tessera-app
```

The app opens its window, renders one frame and exits with code 0. CI uses this to prove the app starts on every platform.
