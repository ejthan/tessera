# M9 Release 1.0: design

- **Date:** 2026-11-07
- **Status:** written autonomously (maintainer takes the recommendations), pending final review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M9 row in §14; §2.2, §5.5, §7, §8.1, §8.3, §8.4, §10, §12, §13, §16)
- **Previous milestone:** [M8 operations design](2026-11-06-m8-operations-design.md)
- **UX mockups:** [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d), main screen, for the title bar

**How this was verified.** Docker did not answer and 18108 timed out on 2026-11-07, so nothing here was checked against a server. M9 needs no new endpoint, so that costs nothing in this design, and the live tests M8 could not rerun stay a maintainer step. GPUI and GPUI Kit APIs were read in the pinned source. `cargo tree` confirmed what is pinned. `cargo-packager` is **not installed** here and its crate is not in the registry cache. Its configuration was read from docs.rs for 0.11.8 and its packaging code from the `main` branch on GitHub, which can differ from the 0.11.8 tag. So every packager fact below is marked **packager**, and Task 9 checks each one against the real tool before relying on it. Contrast figures were computed from the kit's `default-theme.json` and `default-colors.json`. The ad-hoc signing flags of §9.3 were run with `codesign` on a test bundle in a scratch directory.

## 1. Goal

M9 makes Tessera ready to release: both languages complete and enforced, accessibility checked, a first start that leads somewhere, the last spec gaps closed, installers buildable, a docs site, and a QA checklist that says exactly what only a human on a real screen can confirm.

M9 does not publish. No task pushes, tags, creates a release or a pull request, or uploads anything. So "v1.0.0 published", the acceptance in §14, stays with the maintainer, and this milestone ends when everything up to that step is ready.

**Acceptance:**

- `cargo xtask i18n` and `cargo xtask governance` pass, and `ci.yml` calls them in the `lint` job. The workflow change has not run.
- A contrast test passes for both default themes at WCAG AA, and a headless audit finds no interactive control without a name on the screens of §7.3.
- Two windows can show two connections side by side, each with its own band and tabs, and a connection opens in one window at a time.
- `cargo xtask dist` on this Mac produces `Tessera_1.0.0_aarch64.dmg` and `SHA256SUMS` in `target/dist/`, ad-hoc signed, and the `.app` inside passes `codesign --verify --deep` and a smoke start.
- `mdbook build book` produces the site locally.
- `docs/qa-checklist.md` holds every open on-screen and other-OS check, with NF1, NF2, NF5 and NF6, and the release steps of the maintainer.

## 2. Delivery

One spec, one plan, three slices stacked on `feat/m8-settings`, because merging is the maintainer's step. Branches stay local.

| Slice | Branch | Content | Why this order |
|---|---|---|---|
| **A · Gates** | `feat/m9-gates` | Tasks 1 to 3: NF8 lint, governance, templates, accessibility | Later tasks add strings and controls, and these gates then cover them |
| **B · App** | `feat/m9-app` | Tasks 4 to 8: NF4, error link, title bar, several windows, onboarding | The last behaviour changes, before anything is packaged |
| **C · Release** | `feat/m9-release` | Tasks 9 to 13: icon, metadata, `xtask dist`, workflows, book, QA checklist, version | Packages what A and B finished |

### What is real at the end, and what is not

| Thing | State at the end of M9 | Who proves it |
|---|---|---|
| `Tessera.app`, `.dmg` for macOS arm64, `SHA256SUMS` | **Built locally** in `target/dist/`, not committed, not uploaded | Task 10 |
| `xtask i18n`, `governance`, `dco`, `icons`, `dist`, `release-notes` | **Run locally** | Tasks 1, 2, 9, 10 |
| Docs site | **Built locally** in `target/book/`, not published | Task 12 |
| Icon files, templates, `labels.yml`, Homebrew cask file, `.desktop` file | **Committed**, not used by any service yet | Tasks 2, 9, 11 |
| `.github/workflows/release.yml`, `docs.yml`, the changes to `ci.yml` | **Written, never run**, checked only as YAML | Maintainer, after the first push |
| `.dmg` for macOS x64, `.msi`, `.zip`, `.deb`, `.rpm`, AppImage | **Not built by anyone yet**: `xtask dist` chooses by `std::env::consts::OS`, so those paths compile here and never run | Maintainer, first run of `release.yml` |
| Labels on GitHub, GitHub Pages, tag, release, Homebrew tap | **Not done** | Maintainer, `docs/qa-checklist.md` |
| Developer ID signing, notarization, Windows signing | **Documented steps only** | Maintainer |

## 3. Scope

### In M9

The M9 row of §14 and each row below marked with a task. Publishing is out of scope.

### Confirmed against the M8 sweep

The M8 design, §3, sends these to M9, and D99 and the §14 row agree. All are confirmed open in the code.

| M8 row | Evidence today | Task |
|---|---|---|
| §8.1 several windows | One `open_window` in `lib.rs:162`; each `RootView` subscribes to `OpenOverview` and its kin, so a second window would open every tab too | 7 |
| §8.1 breadcrumb and ⌘K field | `TitleBar::new()` holds the switcher and ⚙ only | 6 |
| §5.5 link from an error to the log | `Error` holds no request id, and 153 `error_text` calls in 44 files yield strings | 5 |
| §2.2 issue and PR templates, labels | `.github/` holds `workflows` only | 2 |
| §10 NF4 | No span and no assertion in `crates/` | 4 |
| §10 NF8 | No lint. `xtask` has `seed` and `nf3` | 1 |
| §10 NF6, §12.1 `docs/qa-checklist.md` | The file does not exist | 13 |
| §8.4, §13.2 Homebrew and `.rpm` | No packaging at all; accessibility is partly there (`aria_label` throughout, no audit) | 3, 9 to 11 |
| §10 NF2, NF5 Linux, on-screen checks | Open; need a screen or another OS | 13, maintainer |
| §2.2 governance checks | Files exist, nothing keeps them true | 2 |

### Full sweep: §7 and NF

Each §7 subsection and each NF read against the code, by locale keys, source and tests, not only the CHANGELOG. **Done** means the code and a test exist. A search can miss a partial delivery; an implementer who finds a gap in a Done row reports it and does not fix it silently.

| Item | Status | Owner |
|---|---|---|
| §7.1 Profiles, nodes, nearest node, TLS options, custom CA | Done | M1 |
| §7.1 Key in keychain, connection test, restricted key (D83) | Done | M1, M7 |
| §7.1 Read-only, unlock | Done | M2 |
| §7.1 Import `config.json`, env vars, CLI, key from stdin | Done | M1 |
| §7.1 Profile export, reopen last, several connections with tab color | Done | M1 |
| §7.2 Cards, live metrics, API stats, cluster view, click to activate, polling stops while hidden | Done | M1 |
| §7.2 Uptime "if available" | Closed: no endpoint offers it (none in `spec/typesense-openapi.yml`), so not available. D112 | Task 13 |
| §7.2 Memory estimate (P2) | Deferred | v1.x |
| §7.3 List, schema view, create, JSON sync, clone, PATCH with diff, delete, truncate | Done | M2 |
| §7.3 Infer schema from a file | Done | M5 |
| §7.3 JOIN graph (P2) | Deferred | v1.x |
| §7.4 Table, paging, filter, inspector, edit, new, delete, bulk, copy, go to ID | Done | M3 |
| §7.4 Card view (P1) | Deferred by §14 | v1.x |
| §7.5 Form, JSON, results, instant, multi, presets, saved, history, code export | Done | M4 |
| §7.5 Compare mode, Elasticsearch helper (P2) | Deferred | v1.x |
| §7.6 Import | Done; the run through the window stays open | M5, Task 13 |
| §7.7 Export, backup, restore | Done | M5, M8 |
| §7.8 Aliases and reindex assistant | Done | M6 |
| §7.9 Synonym sets | Done | M6 |
| §7.10 Curation sets | Done | M6 |
| §7.11 Stopwords | Done | M6 |
| §7.12 Presets | Done | M6 |
| §7.13 Stemming dictionaries | Done, earlier than its P2 | M7 |
| §7.14 Keys, scoped generator, decoder, test | Done | M7 |
| §7.14 Rotate keys (P2) | Deferred | v1.x |
| §7.15 Analytics | Done | M7 |
| §7.16 AI features | Done; real providers stay a manual check | M7, Task 13 |
| §7.17 Raw console | Done | M4 |
| §7.18 Server operations | Done | M8 |
| §7.19 Request log | Done | M1, M4 |
| §7.20 Palette, quick switch, keymap | Done | M4, M8 |
| §7.21 Settings | Done. The kit ships two themes, Default Light and Default Dark, and both are offered | M8 |
| §7.22 Legacy servers (P2) | Deferred | v1.x |
| NF1 Cold start < 1 s | The `ready in N ms` line exists; the M1 figure is not measured | Task 13, maintainer |
| NF2 200k rows ≥ 60 fps | Overlay exists; not measured | Task 13, maintainer |
| NF3 1M documents < 300 MB | Met headlessly; the window run is open | Task 13, maintainer |
| NF4 No UI block over 16 ms | Missing | Task 4 |
| NF5 Binary < 40 MB | macOS 28,868,288 bytes; Linux and Windows wait for CI | Tasks 10, 13, maintainer |
| NF6 Idle CPU < 1% | Missing, manual by definition | Task 13, maintainer |
| NF7 Errors shown in full | Done | M1 and later |
| NF8 EN and DE complete, no hardcoded strings | Keys match: 1,714 in each, none missing. 146 German values equal the English and need review. A first scan finds 6 literals in UI positions, all Typesense parameter names. No lint | Task 1 |

### Other gaps the sweep found

No milestone owned these. The first three are real defects for a release.

| Item | Status | Owner |
|---|---|---|
| §8.4 contrast: light `danger` text on white is 3.76:1, white on the light danger button 3.61:1, dark danger button 1.75:1, light `muted` on `secondary` 3.76:1, light `success` and `warning` as text under 3:1, light focus ring about 2.5:1 | Fails WCAG AA | Task 3 |
| macOS: the default `QuitMode` is `Explicit`, no `on_reopen` is set and no menu exists, so closing the only window leaves a running app with no way back | Bug | Task 7 |
| Windows: `main.rs` has no `windows_subsystem`, so a release build opens a console window | Bug | Task 9 |
| Linux: `WindowOptions.app_id` is unset, so a desktop file cannot match the window on Wayland | Missing | Task 9 |
| §13.2 needs `.rpm` and `.zip`, and `cargo-packager` builds neither (**packager**: `PackageFormat` has no such variant) | Needs other means | Tasks 10, 11, D108 |
| App icon: none exists, and the `assets/` folder holds locales only | Missing | Task 9 |
| §13.1 `lint` job names an i18n check | Missing | Task 1 |
| §13.3 tag-driven release, `stable` and `nightly` channels, checksums, changelog excerpt | Missing | Tasks 10, 11 |
| §2.2 DCO has no check. History: every non-merge commit is signed off | Missing | Task 2 |
| §8.2 "low: an undo hint where possible" | No action can be undone on a server, so nothing to show. Closed by D112 | Task 13 |
| §5.1 lists `xtask spec-fetch`, `assets/icons`, `assets/themes`, `dev/seed/`; none exists or is needed as written | Spec stale. D112 | spec edited with this design |
| §11 says `history.jsonl` is rotated; it is capped at 500 per kind (D39) | Spec stale. D112 | spec edited with this design |
| §12.1 scroll benchmark "if available": GPUI publishes none for this | Closed; NF2 stays by hand. D112 | spec edited with this design |
| `D32` | Open for the maintainer | `docs/qa-checklist.md` |

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Link on each inline error | v1.x | D104: 153 call sites hold plain strings |
| macOS menu bar | v1.x | The spec asks for none, and all actions have chords and the palette |
| Icon inside `tessera.exe` | v1.x | Needs a resource compiler crate. The MSI shortcut has the icon |
| Developer ID signing, notarization | maintainer | Needs an Apple account and credentials |
| Windows signing, winget, Flatpak | v1.x | P2 |
| A link from the app to the docs site | v1.x | The site is not published, and a dead link breaks UX rule 4 |
| Same connection in two windows | not planned | D105 |
| Frame watchdog | not planned | D103 |

## 4. tessera-client

No change, and no new endpoint. Nothing in M9 touches a request.

## 5. tessera-core

### 5.1 `ui_thread.rs` (NF4, D103)

```rust
/// Marks this thread as the UI thread. Called once, inside `application().run`.
pub fn mark();
/// Debug builds: panics when called on the marked thread. Release: nothing.
pub fn assert_off(what: &'static str);
```

A `thread_local!` `Cell<bool>`. It needs no GPUI, so the direction rule holds. No test harness marks a thread, so GPUI tests, where background work shares a thread, cannot trip it.

Callers: `store::write_atomic`, `store::load` and the other readers of config files reached after start, `KeyringStore::{get, set, delete}`, `backup::archive::{pack, unpack, verify}`. Loads in `run` come before `mark`, so start-up does not trip it.

### 5.2 `log.rs`

```rust
/// Id of the newest failed user request of `connection`.
pub fn latest_failed(&self, connection: Uuid) -> Option<u64>;
```

It walks the user buffer from newest to oldest with `LogEntry::is_error`, and it leaves polling out. One polling failure does show: a failed collection list fills `collections_error` on the overview, and that error gets no control (D104).

## 6. tessera-widgets

No change.

## 7. tessera-app

### 7.1 Contrast (D101)

`settings::apply_appearance` is the one place both `apply` and `appearance_changed` pass through, after the theme loads. It gains a second `Theme::update` (the kit's own docs say colors go in an update after the mode change) that calls a pure `aa_colors(&mut ThemeColor, dark: bool)`. Only the listed tokens change.

| Theme | Token | Kit | New | Result |
|---|---|---|---|---|
| Light | `muted_foreground` | `#737373` | `#525252` | 7.81 on white, 6.2 on `#e5e5e5` |
| Light | `danger` | `#ef4444` | `#b91c1c` | 6.47 on white, 5.93 on `#f5f5f5` |
| Light | `danger_hover` | kit | `#991b1b` | darker than `danger` |
| Light | `danger_foreground` | `#fafafa` | `#ffffff` | 6.47 on `danger` |
| Light | `success`, `success_foreground` | `#22c55e`, `#fafafa` | `#15803d`, `#ffffff` | 5.02 |
| Light | `warning`, `warning_foreground` | `#eab308`, `#fafafa` | `#a16207`, `#ffffff` | 4.92 |
| Light | `ring` | `#a3a3a3` | `#737373` | 4.74 on white, above the 3:1 for non-text |
| Dark | `danger_foreground` | `#dc2626` | `#450a0a` | 5.84 on `#f87171` |
| Dark | `success_foreground` | `#16a34a` | `#052e16` | 8.55 on `#4ade80` |
| Dark | `warning_foreground` | `#ca8a04` | `#422006` | 9.52 on `#facc15` |

The table holds for the built-in themes alone: `aa_colors` skips a theme of the registry. The hover and active states of `danger`, `success` and `warning` move with them, also in dark, and `ui_a11y` measures each.

Filled buttons (D113) draw from `button_danger`, `button_success`, `button_warning` and their hover, active and foreground tokens. `aa_colors` sets them to the solid pairs above in both themes, and `ui_a11y` measures each.

`PROD_RED` stays `#DC2626`: white on it reads 4.83.

### 7.2 Names and focus

`Button` registers through `.test_support()`, so `gpui_kit::base::test_support::snapshots(window)` lists each button with its role and label; the module is public and the function `#[doc(hidden)]`. GPUI's real tree stays empty in a test window, because no assistive technology activates it, so the audit reads these snapshots and cannot say what a screen reader announces. D102 splits the proof: names, focus and contrast headless, real screen readers in the QA checklist.

### 7.3 The audit (`tests/ui_a11y.rs`)

For the welcome screen, connect dialog, overview, collections, documents, search, console, every `ResourceKind` tab, settings and palette: render a frame, walk `snapshots`, and fail on a visible snapshot whose role is interactive and whose `label` is `None` or empty, naming its path. Roles: Button, CheckBox, Switch, RadioButton, Tab, TextInput, ComboBox, Link. An input with a `placeholder` still needs a label. Fixes go at the site with `aria_label` or `accessibility_label`, through `t!`.

The kit falls back to the placeholder as an input's label (`gpui-component-0.9.0/src/input/input.rs:741`), so a snapshot cannot tell the two apart and the audit does not see an input that has only a placeholder. Task 3 gave each such `Input` in the views an `aria_label` by reading the source. The skip list in the test holds what the kit draws and Tessera cannot name: the dock's tabs, close buttons, menu and toggles (`TabGroupSkin`), and the `Combobox` wrapper of the field picker, whose trigger button is named. The role list also takes `MultilineTextInput`, `PasswordInput` and `SearchInput`.

### 7.4 Budget spans (D103)

`crates/tessera-app/src/ui_budget.rs`:

```rust
pub const BUDGET: Duration = Duration::from_millis(16);
#[must_use] pub fn span(name: &'static str) -> Budget;   // Drop logs at warn, target "tessera::nf4", above BUDGET
```

Sites: the update closure of `spawn_log_drain`, the rank in `palette.rs`, the entry filter of `request_log.rs`, the jobs progress update, and the page apply in `views/documents/`. A warning does not panic: a debug frame takes longer than 16 ms by itself.

### 7.5 Last failed request (D104)

The status bar gains a control while `latest_failed(active connection)` is `Some`: "Last failed request" with its chord. Action `ShowLastFailure`, ⌥⌘R / Ctrl+Alt+R, also in the palette. It opens the log tab through `RootView::reveal`, and `RequestLogPanel::show_entry(id, cx)` resets the filter, because a filter could hide the entry, selects it and scrolls to it. An entry evicted by the ring buffer yields `None`, so the control goes away.

### 7.6 Title bar

`TitleBar::new()` holds the switcher, then a `Breadcrumb` (`gpui-component-0.9.0/src/breadcrumb.rs`), then the search field, then ⚙.

- Crumbs come from `ConnectionTab::kind` of the active tab: connection, section, and the collection or alias for those two kinds. The connection crumb opens its overview, and the section of a collection tab opens the list. The last crumb has no click. No active tab, no breadcrumb.
- The field is a `Button` with `t!("titlebar.search")`, the `Kbd` of `OpenPalette`, and a click that calls `open_palette(PaletteMode::All)`. It is not a text input: typing belongs to the palette.
- At the minimum width of 960 px the crumbs shrink with `min_w_0` and truncate; the environment badge never does (UX rule 1).

### 7.7 Several windows (D105, D106)

- `lib.rs` extracts `pub fn open_main_window(state, primary: bool, keymap_errors: Vec<String>, cx) -> anyhow::Result<AnyWindowHandle>` from `run`, used for both windows. `app_id` goes into its options.
- Action `NewWindow`, ⌥⌘N / Ctrl+Alt+N, bound globally and offered in the palette.
- `AppState` gains `tab_windows: HashMap<Uuid, AnyWindowHandle>`, the window that shows a connection. A connection has one window at a time.
- `targeted(state, id, window, cx)`, private in `root.rs`: the owner in `tab_windows` when one is set, otherwise `cx.active_window()`, otherwise the first of `cx.windows()`. All seven `Open*` subscriptions and `CloseCollection` return early unless it is this window. The owner, when not active, gets `window.activate_window()`, so clicking prod in the sidebar of a staging window brings prod's window forward.
- `open_tab` records the owner. `layout_changed` clears the entry when this window holds no more tabs of the connection. `cx.on_release` clears all entries of a closed window.
- Only the primary window restores and saves the layout, checks for updates, shows keymap errors and registers the quit hook. A later window starts at the default layout. Closing the primary window ends saving for the session; what was saved stays.
- `UpdateFound` and job toasts go to the active window, or the first one, not to all.
- `cx.set_quit_mode(QuitMode::LastWindowClosed)` at the start of `run`, on every platform. Quit hooks flush as before. D31's rule for unsaved inspector changes on quit applies to closing a window too: there is no veto.

### 7.8 Onboarding

§8.3's welcome screen has its three entries (M8). What a first start still lacks is the step after them.

- The "Run Typesense locally" card gains **Connect to it**: `ConnectDialog::open_new` with `Profile::new("local", …)`, node `http://localhost:8108`, environment `local`, no key. A hint names the key of the shown command. The dialog only fills the form; nothing connects by itself, and no test opens a socket.
- `welcome.next`: one line with the chords of `OpenPalette` and `OpenSettings` through `Kbd`, because a first-time user has no other way to learn them.
- Import and Copy keep working by Tab, and the audit covers the screen.

## 8. xtask

Hand-written scanners and no parser crate (D100). All new logic has unit tests on strings, so a check is tested without touching the repository.

### 8.1 `cargo xtask i18n`

Exit 1 with `file:line: what` per finding.

1. `en.yml` and `de.yml` hold the same keys, by the line parser of `i18n.rs`.
2. Each value of a key has the same set of `%{name}` placeholders in both.
3. Each literal `t!("key")` in `crates/` is in `en.yml`. Dynamic keys, as in `t!(key)`, are skipped. There are about 460 keys reached that way, so a check for unused keys is left out.
4. No letter-bearing literal in a UI position in `crates/tessera-app/src` and `crates/tessera-widgets/src`, before `#[cfg(test)]`: after `.child(`, `.label(`, `.tooltip(`, `.tooltip_with_action(`, `.placeholder(`, `.title(`, `.aria_label(`, `.aria_description(`, `.accessibility_label(`, `.message(`, `.description(`, `Label::new(`, `.push_notification(`, `.set_window_title(`, with whitespace and line breaks skipped. The same holds for `format!("…")` there when two letters in a row stay outside `{…}`.
5. A German value equal to the English one needs its key in `xtask/i18n-same.txt`. Task 1 reviews the 146 and translates the real misses.

A line or the line above holding `// i18n-ok: <reason>` passes rule 4. The six known hits get one each: `query_by` and `filter_by` are API names.

### 8.2 `cargo xtask governance`

Checks: `LICENSE` is the GPL-3.0 text; `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant, 2.1), `SECURITY.md`, `CHANGELOG.md` with `## [Unreleased]`, the two templates and `labels.yml` exist; the §2.3 notice is in the README and in `welcome.trademark`; each `.rs` under `crates` and `xtask` starts with the SPDX line; each entry of `[workspace.dependencies]` other than a path is pinned with `=`; no crate manifest names a version directly for a dependency in that table; the app's product name holds no "Typesense".

`cargo xtask dco <base>` checks that every non-merge commit in `<base>..HEAD` has a `Signed-off-by` line, through `git log`. `ci.yml` runs it in a job for pull requests only.

### 8.3 `cargo xtask icons`

Generates, from code, a 4×4 mosaic of rounded squares in five fixed colors, into `assets/icons/app/`: PNGs at 32, 128, 256, 512 and 1024, `tessera.icns` and `tessera.ico`. It is drawn by a few lines of pixel arithmetic and encoded with the `png` crate. The two containers are small enough to write by hand: `.icns` is `icns`, a length, and entries `ic07` to `ic10` holding PNGs; `.ico` is a 6-byte header, a 16-byte entry per size, and PNGs. **packager:** `create_icns_file` takes an `.icns` from the list and ignores the rest, so the PNGs in the same list cause no error on macOS. The files are committed, so a packaging run does not need the task. The art is a placeholder the maintainer can replace (D109).

### 8.4 `cargo xtask dist`

```
cargo xtask dist
```

The tool has no flag to skip its build hook, and the hook does nothing when the binary is fresh, so `dist` takes no option.

1. Find `cargo-packager` (and `cargo-generate-rpm` on Linux) on `PATH` or in `target/tools/bin`. Missing: print the exact install command and exit 1. It never installs anything by itself.
2. Set `MACOSX_DEPLOYMENT_TARGET=13.0` on macOS. **packager:** set `CI=true` for the `dmg` step, which passes `--skip-jenkins` to the `create-dmg` script, so no Finder automation is asked for.
3. Run `cargo packager -p tessera-app --release --formats <formats_for(os)>`: macOS `app,dmg`, Linux `deb,appimage`, Windows `wix`. **packager:** the command downloads helpers on first use: the `create-dmg` script (pinned commit), WiX 3.11.2 (SHA-256 checked), and `linuxdeploy` and its plugins (the AppImage plugin from a moving `continuous` tag).
4. Linux: `cargo generate-rpm -p crates/tessera-app`. Windows: `tar -a -c -f` makes the `.zip` of `tessera.exe`, `LICENSE`, `THIRD_PARTY_NOTICES.md`. Windows 10 and later ship `tar`, so no crate is needed.
5. Check NF5 on `target/release/tessera[.exe]`: below 41,943,040 bytes.
6. Copy the packages into `target/dist/`, and write `SHA256SUMS` in `sha256sum` format, sorted, through `sha2`.

`formats_for`, `package_names`, and `sha256sums` are pure and tested. **packager** names: `Tessera_<v>_aarch64.dmg`, `Tessera_<v>_x64.dmg`, `tessera_<v>_x64_en-US.msi`, `tessera_<v>_amd64.deb`, `tessera_<v>_x86_64.AppImage`. `cargo-generate-rpm` names `tessera-<v>-1.x86_64.rpm`, with `name = "tessera"` in its metadata, in `target/generate-rpm/`.

### 8.5 `cargo xtask release-notes <version>`

Prints the section of `CHANGELOG.md` for `<version>`, from its `## [` heading to the next. No section, exit 1.

## 9. Packaging

### 9.1 Configuration

`[package.metadata.packager]` in `crates/tessera-app/Cargo.toml`, not a `Packager.toml`. **packager:** only metadata configs get defaults for `version`, `out_dir`, `binaries_dir` and `binaries`, and a file config would have to repeat them. `product-name = "Tessera"`, `identifier = "org.tessera.Tessera"`, which matches the data directory, a `before-packaging-command` that builds with `--locked`, `icons`, `resources`, category, `macos.minimum-system-version = "13.0"`, `macos.signing-identity = "-"`, `deb.depends`.

Settled on the installed 0.11.8 in Task 9. Before it reads any path, the tool changes into the directory of the crate's `Cargo.toml`, so `icons` and `resources` start with `../../`, and `before-packaging-command` runs from the crate. Nested tables accept the kebab-case keys `minimum-system-version` and `signing-identity`. The category is `DeveloperTool`. The `.deb` writes `authors` as its `Maintainer` line, and the crate has none, so the config sets `authors = ["Tessera contributors"]`. `--skip-jenkins` needs `CI` to be exactly `true`.

### 9.2 License in packages

GPL distribution needs the license with the binary. `LICENSE` and `THIRD_PARTY_NOTICES.md` go in as `resources`. **packager:** `license_file` feeds WiX's `WixUILicenseRtf`, which wants RTF, so it is left unset and the MSI shows no license dialog. The `.zip` and `.rpm` list the files themselves.

### 9.3 Signing (D108)

`codesign --force -s - --timestamp` and the same with `--options runtime` both succeeded here, and `--verify --deep` passed. **packager** passes the identity through unchanged and adds `--timestamp`, so `signing-identity = "-"` gives the ad-hoc signature §13.2 asks for, needing no credential. It tries notarizing only when `APPLE_*` variables are set, and warns and goes on otherwise. No task, workflow or `xtask` code reads those variables. `docs/qa-checklist.md` lists the steps: replace the identity with `Developer ID Application: …`, and give the packager `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and either `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` or `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH`.

### 9.4 Linux

Dependencies for `.deb`: `libfontconfig1`, `libxkbcommon0`, `libxkbcommon-x11-0`, `libxcb1`, `libvulkan1`. `Cargo.lock` holds no `wry`, and `keyring` uses `zbus` and `reqwest` uses `rustls`, so webkit, libdbus and OpenSSL are not runtime needs, despite the CI build packages. The list is unverified on Linux: `release.yml` prints `ldd`, and a clean install is a QA step. The `.rpm` uses `cargo-generate-rpm`'s automatic requirements. `packaging/linux/tessera.desktop` serves the `.rpm`; **packager** makes its own for `.deb`. `app_id = "tessera"` matches both.

### 9.5 Windows

`#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]` in `main.rs`. The cost: `tessera --help` prints nothing in a release build there, so the book documents the flags. **packager:** the MSI is per-machine, adds Start menu and desktop shortcuts, and its `UpgradeCode` derives from the binary name, so upgrades hold.

### 9.6 Homebrew

`packaging/homebrew/tessera.rb`: a cask with `version`, per-architecture `url` and `sha256` placeholders, and the §13.2 note on the ad-hoc signature. It needs a published release and a tap, so it is a file only.

## 10. Workflows (never run)

- `ci.yml`: `lint` gains `cargo xtask i18n` and `cargo xtask governance`. A new `dco` job on pull requests runs `cargo xtask dco origin/${{ github.base_ref }}` after a checkout with `fetch-depth: 0`.
- `release.yml`: on tag `v*`, a nightly `schedule` and `workflow_dispatch`. The matrix is `build`'s four runners. Each installs the tools with pinned `--version`, runs `cargo xtask dist`, prints `ldd` on Linux, and uploads `target/dist/`. A `publish` job with `contents: write` downloads everything, joins the `SHA256SUMS`, and runs `gh release create`: for a tag with `cargo xtask release-notes`, for a schedule as the `nightly` pre-release, replacing the earlier one. No secret other than `GITHUB_TOKEN`.
- `docs.yml`: on push to `main` under `book/`, installs mdBook, builds, deploys through Pages.
- Versions verified on 2026-11-07 with `gh api`, read-only: `actions/download-artifact` v8.0.2, `actions/configure-pages` v6.0.0, `actions/upload-pages-artifact` v5.0.0, `actions/deploy-pages` v5.0.1. The others follow `ci.yml`.

## 11. Docs site (D110)

`book/` with `book.toml` and `src/`, output to `target/book`. English only. Pages follow the sidebar: introduction, install per OS (with `xattr -cr`), getting started, connections, collections and schema, documents, search, import and export, backup and restore, relevance resources, keys, analytics and AI, operations, raw console, request log, shortcuts, command-line flags, settings, security, troubleshooting. `docs/building.md` stays for developers. The book states the §2.3 notice on its first page. Written from the CHANGELOG, the spec and `docs/building.md`, in the user's terms; it invents no behaviour.

## 12. Testing

| Level | What it checks |
|---|---|
| xtask unit | Each rule of the lint on small strings, including a multi-line sink, `// i18n-ok`, a test module skipped, placeholder mismatch; governance checks on a temp tree; `.ico` and `.icns` headers; `formats_for`; `sha256sums` order; `release-notes` bounds |
| Core unit | `ui_thread::assert_off` panics on a marked thread in debug, passes on another; `latest_failed` picks the newest of its connection, skips polling, `None` after eviction |
| App UI, fakes | Contrast of each pair in both themes, after a theme switch too; `ui_a11y`; budget span logs above the limit; status control appears, chord shows the entry through a filter that hid it; breadcrumb per tab kind and click; search field opens the palette; two windows: an open goes to one, the owner is activated, band and tabs separate, close clears the owner, layout saved by the primary only; welcome **Connect to it** prefills and sends nothing |
| Local release | `cargo xtask dist`; `codesign --verify --deep`; `hdiutil verify`; plist keys; `.app` smoke start; `mdbook build book` |
| Maintainer | `docs/qa-checklist.md` |

No task claims an on-screen result, a screen reader result, another OS, or a workflow run.

## 13. New tools and dependencies (decisions)

| Name | Version | Kind | Why |
|---|---|---|---|
| `png` | `=0.18.1` | New direct dependency of `xtask` only, already in `Cargo.lock` through `image` | Encoding the icons. Never linked into the app |
| `sha2` | `=0.11.1` | Existing workspace pin, new use in `xtask` | Checksums |
| `cargo-packager` | 0.11.8 (2025-11-27), Apache-2.0 OR MIT | External CLI, not in `Cargo.toml` | D3 |
| `cargo-generate-rpm` | 0.21.1 (2026-10-17), MIT | External CLI, not in `Cargo.toml` | No `rpm` in `cargo-packager`; needs no `rpmbuild` |
| `mdbook` | 0.5.4 (2026-07-06), MPL-2.0, Rust 1.88+ | External CLI, not in `Cargo.toml` | §14 |

All three CLIs are older than two weeks, as D92 asks, and install with `cargo install --locked --version <v> --root target/tools`. No crate is added to the app, so NF5 does not move through dependencies.

## 14. Changes to the product spec

Made with this design: §5.1, §5.5, §8.1, §8.2, §11, §12.1, §13.2, the §14 row, §16 D100 to D112. Task 13 updates the §14 row's state at close.

## 15. Risks

1. **`cargo-packager` was read from `main`, not 0.11.8, and never run.** Formats, flags, names and helpers may differ. Task 9 installs it first and stops if it does not build on Rust 1.99.1 or differs in a way this design depends on.
2. **Nothing but macOS arm64 gets built.** The first `release.yml` run will likely fail somewhere. WiX 3 needs VBScript on recent Windows images, and the `.deb` list is unverified. That run is the check, before a tag.
3. **The AppImage helper comes from a moving tag** and has no checksum. A later release could change or break it. `release.yml` publishes `SHA256SUMS` of the result, not of the tool.
4. **Ad-hoc signed apps warn at first open,** and macOS may ask for keychain access after each update, as the signature changes. The book says so.
5. **`MACOSX_DEPLOYMENT_TARGET=13.0` may be refused** by a dependency built on macOS 15. Task 10 checks `vtool -show-build`. If it fails, the task stops and reports, since §3 is at stake.
6. **Last failed request is not always the error on screen.** With two failures at once it shows the newer. D104 accepts that and the log shows both.
7. **One window per connection surprises** someone wanting the same cluster twice. The second window brings the first forward, which at least says where it is.
8. **Changing `danger` to `#b91c1c`** changes how every destructive control looks. Prod red stays apart. Seen only on screen, so it is a QA step.
9. **The audit reads snapshots, not the real tree.** A name can pass here and still read badly in VoiceOver. D102.
10. **`LastWindowClosed` on macOS** differs from the usual Mac habit. It is the honest choice without a menu bar, and easy to revisit.
11. **The 1.0.0 version bump** comes before the QA checklist has run, because artifacts carry the version. If QA fails, the maintainer fixes on the branch; nothing is tagged.
