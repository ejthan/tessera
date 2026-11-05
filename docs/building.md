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

## Smoke start

```bash
TESSERA_SMOKE=1 cargo run -p tessera-app
```

The app opens its window, renders one frame and exits with code 0. CI uses this to prove the app starts on every platform.
