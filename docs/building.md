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

Logs go to stderr and to `tessera.log.YYYY-MM-DD` in the `logs` folder of the local data directory: `~/Library/Application Support/org.tessera.Tessera/logs` on macOS, `~/.local/share/tessera/logs` on Linux, `%LOCALAPPDATA%\tessera\Tessera\data\logs` on Windows. A new file starts each day, and the last 7 stay. Keys and tokens are masked in both, and neither holds a request body. If the folder cannot be made, the app logs to stderr alone and says so.

The contrast guarantee of `tests/ui_a11y.rs` covers the built-in light and dark themes. A theme of the registry keeps the colors its author set, so it may fall below WCAG AA.

Synchronous UI work that takes longer than a 16 ms frame (NF4) logs a warning: one line with the span's `name` and its `ms`, from `log.drain`, `palette.rank`, `log.filter`, `jobs.progress` and `documents.page`. The default level `info` keeps these lines on in every build, with no rate limit, on stderr and in the log file. stderr is written on the UI thread, so a full pipe blocks the thread being measured. `RUST_LOG=tessera::nf4=warn` leaves only these lines and silences every other log. A debug frame can take that long by itself, so judge the numbers in a release build. A debug build also panics with `blocking call on the UI thread: …` when a config write, a keychain call or a backup archive runs on the UI thread; a release build does not check.

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

Fills the server with `books` (10,000 documents, with the alias `books_current` and the synonym set `books_synonyms`), `authors` (500), `books_ref` (2,000, references `authors`), `places` (1,000, geopoints), `articles_vec` (1,000, 8-dimensional vectors) and `products` (200,000, with a nested `dimensions` object, about 10 s to load). The documents are generated from a fixed seed, so every run gives the same data. There is no default URL; without `--url` the command prints its usage and exits with 2. The key defaults to `xyz`. Documents are streamed in batches of 1,000. If one of these collections exists, the command stops and names it; `--reset` drops those collections, the alias and the sets first, and nothing else.

It also adds the curation set `books_curation`, the stopword set `en_common`, the preset `books_fantasy`, the search-only key described `tessera-seed-search`, of which it prints only the id and the first characters, the `popular_queries` rule `books_popular` from `books` to `books_queries`, and the stemming dictionary `seed_plurals` with three words. `--reset` removes these too.

## Checks

`cargo xtask i18n` fails with `file:line: what` when `en.yml` and `de.yml` differ in keys or `%{…}` placeholders, when a literal `t!("key")` has no entry, or when a letter-bearing literal sits in a UI position in `tessera-app` or `tessera-widgets`. `// i18n-ok: <reason>` on that line or the line above lets one through, and the reason is required. A German value equal to the English one needs its key in `xtask/i18n-same.txt`.

## Tests

```bash
cargo test --workspace
```

Integration tests against the local server are ignored by default:

```bash
cargo test -p tessera-client --test live -- --ignored
```

They use `http://localhost:18108` and key `xyz`; override with `TESSERA_IT_URL` and `TESSERA_IT_KEY`.

`live_ai` runs models against a local stub, so it never reaches a provider. `TESSERA_IT_STUB_HOST` names this machine as the server sees it, such as `host.docker.internal`; without it the stub tests return early. `live_builtin_model` downloads a model, so CI leaves it to the nightly job.

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

## Manual checks

The checks that need a person, another OS or a workflow run are in [`docs/qa-checklist.md`](qa-checklist.md).

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


## Packaging

`cargo xtask dist` builds the packages of the OS it runs on into `target/dist/`, with `SHA256SUMS` beside them. Nothing it does uploads or publishes.

| OS | Packages | Tools |
|---|---|---|
| macOS | `.app`, `.dmg` | `cargo-packager` |
| Linux | `.deb`, AppImage, `.rpm` | `cargo-packager`, `cargo-generate-rpm` |
| Windows | `.msi`, `.zip` | `cargo-packager`, the system `tar` |

Install the tools first. `dist` finds them on `PATH` or in `target/tools/bin`, never installs them itself, and prints the line it misses:

```bash
cargo install cargo-packager --locked --version 0.11.8 --root target/tools
cargo install cargo-generate-rpm --locked --version 0.21.1 --root target/tools
```

```bash
cargo xtask dist
```

The first run of `cargo-packager` downloads helpers into its own cache: the `create-dmg` script on macOS, WiX on Windows, and `linuxdeploy` with its plugins on Linux. The AppImage plugin comes from a moving tag and has no checksum.

On macOS, `dist` sets `MACOSX_DEPLOYMENT_TARGET=13.0` and `CI=true`, which keeps the `.dmg` step from automating Finder. The app is signed ad-hoc, with the identity `-` in `crates/tessera-app/Cargo.toml`. `dist` stops when the binary reaches 41,943,040 bytes (NF5).

`cargo xtask icons` draws the placeholder icon into `assets/icons/app/`. The files are committed, so packaging does not need it. `cargo xtask release-notes <version>` prints that version's section of `CHANGELOG.md`, and fails if it has none.

Only macOS arm64 has been built this way so far. The Linux and Windows paths compile here and have not run.

## Smoke start

```bash
TESSERA_SMOKE=1 cargo run -p tessera-app
```

The app opens its window, renders one frame and exits with code 0. CI uses this to prove the app starts on every platform.
