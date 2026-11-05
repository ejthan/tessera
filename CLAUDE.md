# Working on Tessera

Tessera is a native Typesense desktop client in Rust with GPUI Kit. The product spec is [`docs/spec.md`](docs/spec.md). Milestone designs live in `docs/superpowers/specs/`, implementation plans in `docs/superpowers/plans/`.

## Rules

1. **Never guess APIs.** GPUI and GPUI Kit are pre-1.0 and change often. Before using an API, read the GPUI Kit docs (https://gpui-kit.com/docs, machine-readable at https://gpui-kit.com/llms-full.txt) and check the source of the pinned version (`~/.cargo/registry/src/*/gpui-kit-0.9.0`, `gpui-component-0.9.0`, `gpui-pre-0.3.11`). When docs and code disagree, the pinned code wins.
2. **Typesense truth** is `spec/typesense-openapi.yml` plus the v30.2 docs (https://typesense.org/docs/llms.txt). Every new endpoint gets an integration test against the dev server first, then the UI.
3. **Order per feature:** client function with test → core logic with test → UI view → UI test → docs and CHANGELOG.
4. **Small steps:** run `cargo check`, `cargo clippy --all-targets -- -D warnings` and the affected tests after every change. No commits with failing checks.
5. **bfritscher/typesense-dashboard** is a feature and UX reference only. Never translate its Vue or TypeScript code; write idiomatic Rust.
6. **No blocking code in the render path.** All I/O goes through the tokio bridge (`docs/architecture/async.md`).
7. **Never hardcode UI strings.** All UI text goes through `t!("…")` with keys in `assets/locales/*.yml`.
8. **Secrets:** never log keys, never commit them in fixtures (the dev key `xyz` is local-only). Redaction tests must not be removed.
9. **Versions:** direct dependencies are pinned exactly in the workspace `Cargo.toml`; update them only in a dedicated PR with a note on breaking changes.
10. **English only** in the repository, except `assets/locales/de.yml`.
11. **Follow the UX rules** below.
12. **When unsure,** add an entry to `docs/spec.md` §16 and ask instead of deciding silently.

## UX rules

1. A connection's environment is visible everywhere: title bar, tabs, sidebar, and for prod the window frame.
2. Errors show where they happen, inline and in full. Toasts are only for background events.
3. Default focus is never on a destructive action. High danger means typing the name, and the dialog states the concrete consequences.
4. No dead controls: navigation entries, panels and buttons appear in the milestone that makes them work.
5. Every action is reachable by keyboard, and its shortcut is shown on the control.

## Plant sandbox

When you run inside the plant's Linux sandbox (`.plant/Dockerfile`), there is no Docker:

- Live tests: `bash dev/plant-it.sh` starts `typesense-server` on port 18108 with key `xyz` and runs them. The cluster tests (`--test cluster`) cannot run there; they run nightly in CI.
- Smoke start: `TESSERA_SMOKE=1 xvfb-run -a cargo run -p tessera-app`.
- Evidence for the app: start it under `xvfb-run -a -s "-screen 0 1440x900x24"`, drive it with `xdotool`, take screenshots with `import -window root evidence/ACn.png` and record with `ffmpeg -f x11grab … evidence/ACn.webm`.

## Conventions

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- Commits are signed off (`git commit -s`).
- `tessera-client` and `tessera-core` never depend on GPUI.
