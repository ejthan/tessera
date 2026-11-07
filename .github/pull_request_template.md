## What and why

<!-- What changes for the user, and why. Link the issue if there is one. -->

## Checklist

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo xtask i18n`
- [ ] `cargo xtask governance`
- [ ] Integration tests against a dev server, if an endpoint changed
- [ ] A test, then the docs and `CHANGELOG.md`, for what changed
- [ ] Every commit carries a `Signed-off-by` line (`git commit -s`, see `CONTRIBUTING.md`)

## Linux binary size (NF5)

<!-- If this adds a dependency or a large asset: the size of `target/release/tessera` on Linux. It stays below 40 MB. -->
