# Tessera

A fast, native desktop client for [Typesense](https://typesense.org), written in Rust with [GPUI Kit](https://gpui-kit.com). Tessera aims to cover the whole Typesense admin API: collections, documents, search, synonyms, curation, keys and cluster operations, with safeguards for production clusters.

**Status:** pre-alpha. The current milestone (M0) connects to a server and shows its version. See [`docs/spec.md`](docs/spec.md) for the full plan.

Tessera is not affiliated with or endorsed by Typesense, Inc.

## Building

```bash
cargo run -p tessera-app
```

Platform prerequisites, the local Typesense server and the test commands are in [`docs/building.md`](docs/building.md).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Commits need a DCO sign-off (`git commit -s`).

## License

[GPL-3.0-or-later](LICENSE).
