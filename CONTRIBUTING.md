# Contributing to Tessera

Thanks for your interest. A few rules keep the project consistent.

## Sign your commits (DCO)

Tessera uses the [Developer Certificate of Origin](https://developercertificate.org/) instead of a CLA. Sign off every commit:

```bash
git commit -s
```

## Language

Everything in the repository is English: code, comments, docs, commit messages and i18n keys. The only exception is `assets/locales/de.yml`, which holds the German UI translation.

## How a feature is built

1. Client function with a test (`crates/tessera-client`)
2. Core logic with a test (`crates/tessera-core`)
3. UI view (`crates/tessera-app`)
4. UI test
5. Docs and `CHANGELOG.md`

Every new Typesense endpoint gets an integration test against the dev server before it gets a UI.

## Before you open a pull request

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

Integration tests need a running Typesense server:

```bash
docker compose -f dev/docker-compose.yml up -d
cargo test -p tessera-client -- --ignored
```

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).
