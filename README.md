# Tessera

A fast, native desktop client for [Typesense](https://typesense.org), written in Rust with [GPUI Kit](https://gpui-kit.com). Tessera aims to cover the whole Typesense admin API: collections, documents, search, synonyms, curation, keys and cluster operations, with safeguards for production clusters.

**Status:** pre-alpha. The current milestone (M0) connects to a server and shows its version. See [`docs/spec.md`](docs/spec.md) for the full plan.

Tessera is not affiliated with or endorsed by Typesense, Inc.

## Connecting

Start Tessera and add a connection in the dialog, or launch it with a temporary connection:

```bash
tessera --host localhost --port 18108 --api-key-stdin <<< xyz
```

| Option | Variable | Meaning |
| --- | --- | --- |
| `--host <host>` | `TYPESENSE_HOST` | Host of the server. Required for a temporary connection. |
| `--port <port>` | `TYPESENSE_PORT` | Port. Default 8108. |
| `--protocol <http\|https>` | `TYPESENSE_PROTOCOL` | Default: `http` for localhost, otherwise `https`. |
| `--path <path>` | `TYPESENSE_PATH` | URL path prefix. |
| `--env <local\|dev\|staging\|prod>` | | Environment. Default: `local` for localhost, otherwise `dev`. |
| `--read-only` | | Block write requests. Always on for `--env prod`. |
| `--api-key-stdin` | `TYPESENSE_API_KEY` | Read the API key as one line from stdin. |
| `--profile <name>` | | Open a saved profile. Cannot be combined with the connection options. |
| `--help` | | Show the usage text. |

Flags win over variables. A key is never accepted as a flag value, because it would show in the process list. A temporary connection is shown as "not saved" and can be saved as a profile.

Profiles live in `connections.json` in the app's config directory, without keys. Keys are stored in the OS keychain (service `tessera`).

## Building

```bash
cargo run -p tessera-app
```

Platform prerequisites, the local Typesense server and the test commands are in [`docs/building.md`](docs/building.md).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Commits need a DCO sign-off (`git commit -s`).

## License

[GPL-3.0-or-later](LICENSE). Notices for bundled third-party data are in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
