# Tessera

A fast, native desktop client for [Typesense](https://typesense.org), written in Rust with [GPUI Kit](https://gpui-kit.com). Tessera aims to cover the whole Typesense admin API: collections, documents, search, synonyms, curation, keys and cluster operations, with safeguards for production clusters.

**Status:** pre-alpha. Milestones M1 and M2 are complete: Tessera manages saved connections with keys in the OS keychain, logs every request it sends, and shows an overview of each server or cluster with live node status, metrics and API stats, and browses and edits collections. The document browser (M3) is implemented on its feature branches; its 200k-row scroll speed (NF2) has not been measured yet. Search, the raw console and the command palette (M4) are implemented on feature branches too, with their checks on screen still open, see [`docs/building.md`](docs/building.md). Import and export of documents with a jobs panel and schema inference from a file (M5) are implemented on a feature branch; the import of 1,000,000 documents stays far below the 300 MB limit in the headless measurement (NF3), and the run through the window is still open. Aliases with a reindex assistant, synonym and curation sets with test panels, stopwords and presets (M6) are implemented on feature branches; their live client and core tests pass on 30.0 and 30.2, and their checks on screen are still open. API keys with the scoped key generator, analytics rules, stemming dictionaries, conversation and NL search models with test panels, and auto-embedding fields in the schema form (M7) are implemented on feature branches; their live client and core tests pass on 30.0 and 30.2, AI against a local stub, so no real provider has been tried yet, and their checks on screen are still open. Server operations, backup and restore, settings with a saved layout, log files with masked keys, migrations and a security checklist (M8) are implemented on feature branches; their views are covered by UI tests with fakes, and their live tests on the last commit and checks on screen are still open. See [`docs/spec.md`](docs/spec.md) for the full plan.

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

## Request log

Open the request log with ⌘J / Ctrl+J. It lists every request Tessera sends, with filters for errors, writes, methods, connections and polling traffic. The detail view shows headers and bodies with secrets masked. "Copy as curl" (⌘⇧C / Ctrl+Shift+C) puts a curl command on the clipboard that reads the key from `$TYPESENSE_API_KEY` instead of containing it, so export the variable before pasting it into a shell.

## Building

```bash
cargo run -p tessera-app
```

Platform prerequisites, the local Typesense server and the test commands are in [`docs/building.md`](docs/building.md).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Commits need a DCO sign-off (`git commit -s`).

## License

[GPL-3.0-or-later](LICENSE). Notices for bundled third-party data are in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
