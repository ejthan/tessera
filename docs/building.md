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

## Tests

```bash
cargo test --workspace
```

Integration tests against the local server are ignored by default:

```bash
cargo test -p tessera-client -- --ignored
```

They use `http://localhost:18108` and key `xyz`; override with `TESSERA_IT_URL` and `TESSERA_IT_KEY`.

## Smoke start

```bash
TESSERA_SMOKE=1 cargo run -p tessera-app
```

The app opens its window, renders one frame and exits with code 0. CI uses this to prove the app starts on every platform.
