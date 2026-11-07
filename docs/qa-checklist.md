# QA checklist for 1.0

What only a person, another OS or a workflow run can prove. Nothing here has been done unless a line says so. Tick a step, and fill in the machine and the result, as you do it. In the steps moved from `docs/building.md`, the pass condition is what the step states.

## 1. Before anything

- [x] Decide D32 in `docs/spec.md` §16: should delete and update by query also refuse a predicate that happens to match everything? **Pass:** the decision is recorded there. Result: recorded on 2026-11-07, predicates that match all stay allowed; the maintainer can reverse it.
- [x] Run every `live_*` target on 30.0 and on 30.2 on the last commit, except `live_builtin_model`. M8 could not rerun them on later commits, and M9 ran only what its Task 13 report lists. **Pass:** no failure on either version. Machine: Apple M3 Max, macOS 27.0.1. Result: on commit `6ed55b1`, 87 passed on 30.0 and 87 on 30.2, in the full run; see the record below.
- [x] Run the cluster test on the last commit (`docs/building.md`, Cluster tests). **Pass:** all four tests pass. Machine: Apple M3 Max, macOS 27.0.1. Result: on commit `6ed55b1`, 4 passed on a 3-node 30.2 cluster.

Record: on 2026-11-07, on commit `6ed55b1` (the code of this release; later commits change only documents), every `live_*` target passed on 30.0 and on 30.2, 87 tests on each, except `live_builtin_model`, which did not run because it downloads a model. `TESSERA_IT_STUB_HOST=host.docker.internal` was set, so the stub tests of `live_ai` ran, and `TESSERA_IT_ANALYTICS=1`. No real provider was called. The four tests of the cluster target passed on a 3-node 30.2 cluster, with the two stop/start tests really stopping ts3, in a separate compose project. The cluster did not run on 30.0. `live_operations` and the restore target of `live_backup` ran on throwaway containers, as did every target on 30.0.

One intermittent failure: the first `live_admin` run on 30.2, on a dev server just started, failed `live_rule_crud` and `live_put_clears_the_tag_and_the_meta_fields` with a client timeout on `POST /analytics/rules`. The server log has a 10-second gap between its two analytics persist lines at that moment. It did not come back in two reruns, in a run right after restarting the server, in the full 30.2 run above or on 30.0. The cause is not proved.

## 2. Performance

- [ ] NF1: start a release build on an M1 and read `ready in N ms` in the log. **Pass:** under 1 s. Machine: ____ Result: ____
- [ ] NF3 through the window: the steps under Import and export below. **Pass:** the peak stays below 300 MB. Machine: ____ Result: ____
- [ ] NF4: walk a debug build through each flow with `RUST_LOG=tessera::nf4=warn`. **Pass:** no panic, and no such warning in a release build. Machine: ____ Result: ____
- [ ] NF5: read the binary sizes of Linux, Windows and macOS x64 from the first `release.yml` run. **Pass:** each is below 41,943,040 bytes. Machine: ____ Result: ____
- [ ] NF6: an idle window with polling paused, connected and not connected, read with `ps -o %cpu` or Task Manager. **Pass:** under 1% in both states. Machine: ____ Result: ____

### NF2: 200k rows at 60 fps

Hardware: an Apple M1 or comparable machine, a release build, no other heavy load.

- [ ] `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, then `cargo xtask seed --url http://localhost:18108 --reset` (about 10 s for `products`). Machine: ____ Result: ____
- [ ] `cargo run --release -p tessera-app --features frame-overlay`, connect to `localhost:18108` with key `xyz`, open `products`. Machine: ____ Result: ____
- [ ] Press ⌥⌘F (Ctrl+Alt+F) twice to show the detailed overlay. Machine: ____ Result: ____
- [ ] With the default columns: wheel-scroll for 30 s, then drag the scrollbar from end to end for 30 s. Read the draw time p95 from the overlay. Repeat with all columns shown (Columns popover). Machine: ____ Result: ____
- [ ] In a terminal run `ps -o rss= -p <pid>` every 10 s during a 5-minute scroll. Pass: flat after the first pass over the data. Machine: ____ Result: ____
- [ ] Pass: p95 draw time at most 16.6 ms. Record p95 for both column sets, the RSS at the start and the end, and the machine in the PR description. If p95 is higher, profile before merging; the first suspects are the allocations in `render_td` and the number of visible columns. Machine: ____ Result: ____

## 3. On screen

### New in M9

- [ ] The breadcrumb and the Search everything field at 960 px and at font size 20. **Pass:** the path shrinks first, the environment badge stays, nothing overlaps. Machine: ____ Result: ____
- [ ] The danger color in the light and the dark theme. **Pass:** text on it reads clearly. Machine: ____ Result: ____
- [ ] Filled danger, success and warning buttons are solid now instead of tinted. **Pass:** they look right in both themes. Machine: ____ Result: ____
- [ ] Outlined danger, success and warning buttons are below AA at 1.0, a known gap. **Pass:** the text stays readable at rest, on hover and pressed; otherwise file a follow-up. Machine: ____ Result: ____
- [ ] The focus ring on each kind of control: button, input, checkbox, radio, tab, table, sidebar row. **Pass:** visible in both themes. Machine: ____ Result: ____
- [ ] Two windows, prod beside staging. **Pass:** each shows its own band, frame, badge, sidebar highlight, chords and Jobs Import. Machine: ____ Result: ____
- [ ] Close the last window on each OS. **Pass:** the app quits on every platform. Machine: ____ Result: ____
- [ ] A real macOS close of the last window, seconds after a change that writes. Tests are headless. **Pass:** the write is on disk at the next start. Machine: ____ Result: ____
- [ ] On macOS, a layout change younger than 500 ms when the last window closes. **Pass:** the change comes back at the next start. Machine: ____ Result: ____
- [ ] The welcome flow: the Docker command, Connect to it, and the line with the shortcuts. **Pass:** the form is filled in, and nothing connects until you do. Machine: ____ Result: ____
- [ ] A jittery click on a crumb or on the settings gear in the title bar. **Pass:** it does not start a window move; a double click on them does not zoom. Machine: ____ Result: ____
- [ ] A crumb after a disconnect. **Pass:** it is not dead. Machine: ____ Result: ____
- [ ] Windows title-bar hit testing. **Pass:** crumbs, gear and field take clicks, and the empty bar drags the window. Machine: ____ Result: ____
- [ ] ⌘W on a tab alone in a side region while another tab is active. **Pass:** the tab with the keyboard closes, or nothing does. Machine: ____ Result: ____
- [ ] A window with no connection tab. **Pass:** note that it has no connection chords until the sidebar reopens one, and that this is acceptable. Machine: ____ Result: ____
- [ ] Close a window while a tab runs a delete. Closing ignores the tab's refusal, so the delete's result is not shown. **Pass:** the delete itself is not aborted. Machine: ____ Result: ____

### Manual checks of operations and backup (M8)

Nothing here has been done yet: the views are covered by UI tests with fakes and the engines by live tests. Start the dev server, and for a restore a second, throwaway one: `docker run --rm -d --name t302 -p 18208:8108 --tmpfs /data typesense/typesense:30.2 --data-dir /data --api-key=xyz --enable-cors=false`. Stop it afterwards.

- [ ] The Ops tab, ⌥⌘0 (Ctrl+Alt+0): the picker names each node with its role, a bad snapshot path says why, and each row shows its result with the node and the time. Machine: ____ Result: ____
- [ ] On the cluster (`docker compose -f dev/docker-compose.yml --profile cluster up -d ts1 ts2 ts3`, and afterwards `--profile cluster rm -sf ts1 ts2 ts3` and `docker volume rm dev_ts1-data dev_ts2-data dev_ts3-data`, never `down`): with the Overview tab closed, the leader's vote button is disabled, a vote on a follower says that the election started, and within 10 seconds the picker shows the new leader and the old one can vote again. Machine: ____ Result: ____
- [ ] The transport banner on an `http://` remote profile: ⌥⌘X (Ctrl+Alt+X) and the button open the profile. Machine: ____ Result: ____
- [ ] The Backup tab, ⌥⌘B (Ctrl+Alt+B): the checkboxes, All and the format radio in light and in dark, the warnings for a selection, and Start (⌘↩), which opens the save dialog. A name that exists is refused under the form. Machine: ____ Result: ____
- [ ] A backup of the seeded collections to a folder and to a `.tar.zst`: the row in Jobs shows step, collection and numbers, it goes on with the tab closed, and Stop leaves nothing at the target. Machine: ____ Result: ____
- [ ] ⌘2, Choose… (⌘O) with that archive into the second server: each stage shows, every row says New, the plan names what it warns of, and Start asks with counts. Afterwards the rows show their results and the collections hold the same counts. Machine: ____ Result: ____
- [ ] The same backup again into the same server: collections are blocked with the reason, sets read Same, and a changed stopword set reads Differs until Replace, which asks for the profile's name. Machine: ____ Result: ____
- [ ] A backup of a collection with an embedding key: the row stays blocked and names the two files to use. Follow that text by hand once: create the collection with its keys, then import the JSON Lines file. Machine: ____ Result: ____
- [ ] A read-only profile: a backup needs no unlock, and a restore unlocks first. Machine: ____ Result: ____

- [ ] Settings, ⌘, (Ctrl+,): Tab reaches each control at the smallest window, in light and in dark. The update check is off; with it on and a published release, a restart shows the result and, for a newer version, one toast. A firewall log shows `api.github.com` and nothing else. Machine: ____ Result: ____
- [ ] Open logs, ⌥⌘L (Ctrl+Alt+L), shows the log folder, and the folder is `0700` on macOS and Linux. Clear cache asks first and says how many went. Machine: ____ Result: ____
- [ ] Change the language with a table open: the column titles follow, and resized columns go back. Close and reopen a masked input to see it follow. Machine: ____ Result: ____
- [ ] Drag the sidebar and the bottom dock, open Jobs, quit and start: sizes and the tab come back, whether you quit at once or after the 500 ms save. Machine: ____ Result: ____
- [ ] Each empty state in light and in dark: Jobs, API stats, saved queries, the request log, and their links. Machine: ____ Result: ____

- [ ] Record the Linux release binary size from the CI job (NF5, below 41,943,040 bytes). At the close of M8 the macOS arm64 release binary is 28,868,288 bytes, stripped, built with `cargo build --release -p tessera-app`. Machine: ____ Result: ____
Record: Done on 2026-11-07 on the code of the last commit of M8 (`e7dc850`): every `live_*` target passes on 30.0 and on 30.2, 87 tests on each, with no failure. `live_builtin_model` did not run, because it downloads a model; the nightly job covers it. The four tests of the cluster target pass on a 3-node 30.2 cluster, with the two stop/start tests running against a separate compose project (`COMPOSE_PROJECT_NAME`), so the dev project was not touched. The cluster did not run on 30.0. The job `integration` in `.github/workflows/ci.yml` now runs every `live_*` target too, on both versions, except `live_builtin_model`; that job has not run yet with the four added in M9, `live_relevance`, `live_export`, `live_raw` and `live_transfer`.

### Manual checks of keys, analytics and AI (M7)

Nothing here has been done yet: the views are covered by UI tests with fakes and the client and core by live tests, and none of it has been looked at. Run against the dev server after `cargo run -p xtask -- seed --url http://localhost:18108 --key xyz --reset`. For the analytics steps the server needs its flags: recreate it once with `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, the volume stays. Record the result in the PR description.

- [ ] The key dialog in light and in dark, ⌥⌘6 (Ctrl+Alt+6), then New: the action matrix scrolls, nothing is cut off at the smallest window, and the presets fill it. Machine: ____ Result: ____
- [ ] The reveal after a create: the value shows masked, Copy works, Esc does not close it, and only Done does. Machine: ____ Result: ____
- [ ] The date picker in the key dialog, which is a stacked dialog: it opens over the dialog, and Esc closes the picker and leaves the dialog. Machine: ____ Result: ____
- [ ] The scoped key panel, ⌘2: a key generates as you type from a parent with `documents:search` only, a pasted key decodes, and Test counts documents on a read-only connection. Machine: ____ Result: ____
- [ ] The analytics view, ⌥⌘7 (Ctrl+Alt+7), on a server with the flags: `books_popular` lists, a search on `books` reaches `books_queries` after Flush now, and a test event goes out. Machine: ____ Result: ____
- [ ] A stemming import, ⌥⌘8 (Ctrl+Alt+8): a good file, a file with a bad line that stops before anything is sent, and an import into `seed_plurals` that asks first. Machine: ____ Result: ____
- [ ] The AI tab, ⌥⌘9 (Ctrl+Alt+9): ⌘1 and ⌘2 switch, the eye shows the key field's text, and an edit starts with an empty key field. Machine: ____ Result: ____
- [ ] If a key is at hand, one real provider for each of a conversation model, an NL search model and an embedding field. Check that a provider's message shows in full and without the key. Machine: ____ Result: ____
- [ ] The embedding block in the schema form: Auto-embed on a `float[]` row, the completion of the model, a `ts/` model hiding the key and showing the note, the JSON tab showing `••••` for a typed key, and the dialog still reaching Create with two embedded fields at the smallest window. Machine: ____ Result: ____
- [ ] On macOS, every shortcut shown on a control in these views does what its control does. Machine: ____ Result: ____

What the stub cannot prove, so only step 8 can (design §8.3): that real answers of OpenAI, Azure, Google, Cloudflare, vLLM and GCP parse, the `extraction_method` variants in `llm_response`, the request shapes of Google and GCP, which differ from OpenAI's, and the refresh of GCP tokens.

### Manual checks of the relevance resources (M6)

Nothing here has been done yet: the views are covered by UI tests with fakes and the client and core by live tests, and none of it has been looked at. Run against the dev server after `cargo run -p xtask -- seed --url http://localhost:18108 --key xyz --reset`. Record the result in the PR description.

- [ ] Each view, ⌥⌘1 to ⌥⌘5 (Ctrl+Alt+1 to 5): aliases, synonym sets, curation sets, stopwords and presets list the seeded `books_current`, `books_synonyms`, `books_curation`, `en_common` and `books_fantasy`. Create, edit and delete one of each. Machine: ____ Result: ____
- [ ] The curation dialog of `books_curation` in light and in dark: Form ⌘1, JSON ⌘2, include rows, hide rows, the document picker. Nothing is cut off at the smallest window, and the body scrolls. Machine: ____ Result: ____
- [ ] Drag an include to another position, then the same with ⌥↑ and ⌥↓. The positions follow the order. Machine: ____ Result: ____
- [ ] Open the date picker in the curation dialog, which is a stacked dialog: it opens over the dialog, its time field shows, and Esc closes the picker and leaves the dialog. Machine: ____ Result: ____
- [ ] The Test tab of `books_synonyms` and of `books_curation`: both sides run, and each says what it means. Machine: ____ Result: ____
- [ ] One reindex of `books` through `books_current`: on the Aliases tab select it and press ⇧⌘R. Check that nothing is sent before Start and that the Jobs tab opens without taking focus. After the copy, the compare shows 10,000 on both sides. Switch, check that `books_current` points to the new collection, Switch back, Switch again, then Delete `books`, typing the name. `--reset` does not know the new collection, so delete it by hand before the next seed. Machine: ____ Result: ____
- [ ] A stopped reindex: Start again, press ⌘. during the export, and check that the alias did not move, the temp directory under `$TMPDIR` named `tessera-reindex-*` is gone, and `Delete {new}` and `Start over` show. Machine: ____ Result: ____
- [ ] On macOS, every shortcut shown on a control in these views does what its control does. Machine: ____ Result: ____

### Manual checks of import and export (M5)

Nothing here has been done yet. The headless run above is NF3's number; this is the run through the window, and it stays open until someone records it in the last PR's description.

- [ ] `TYPESENSE_PORT=18108 docker compose -f dev/docker-compose.yml up -d`, then `cargo run --release -p xtask -- nf3 --url http://localhost:18108 --docs 1000000 --keep-file /tmp/nf3.jsonl`, which also leaves the file for the import below. Machine: ____ Result: ____
- [ ] `cargo run --release -p tessera-app -- --host localhost --port 18108 --api-key-stdin <<< xyz`. Import `/tmp/nf3.jsonl` into a new collection with Import → New collection…. Machine: ____ Result: ____
- [ ] While it runs, in a terminal: `while :; do ps -o rss= -p $(pgrep -x tessera); sleep 1; done`. Pass: the peak stays below 300 MB. Machine: ____ Result: ____
- [ ] Start a second import of the same file, press Cancel in the Jobs panel, and note that the panel stopped within one batch (the job row says `stopped`, and no further batch arrives). Machine: ____ Result: ____
- [ ] Delete the new collection afterwards. Record the machine, both RSS series and the stop time. Machine: ____ Result: ____
- [ ] Export the collection as JSONL, JSON and CSV with a filter and with a selection, open each file, and check that a stopped export leaves an existing target untouched. Machine: ____ Result: ____
- [ ] Open the import dialog, the export dialog and the schema dialog at 960×600. Nothing may be cut off. Machine: ____ Result: ____
- [ ] Record the Linux release binary size from the CI job (NF5, below 41,943,040 bytes). A macOS release binary is 23,397,360 bytes, which says nothing about Linux. Machine: ____ Result: ____

### Manual checks of the search milestone (M4)

Nothing here has been done yet; record the results in PR D's description.

- [ ] Open the Search tab, the Console, the palette, ⌘P and the keymap error line at 960×600 (the smallest window). Nothing may be cut off. Machine: ____ Result: ____
- [ ] NF4: scroll a large response in Raw and in the request log's full view with the frame overlay on. Machine: ____ Result: ____
- [ ] Press ⌘K / Ctrl+K in the documents table and run Delete documents: the confirmation names the rows. Machine: ____ Result: ____
- [ ] Record the Linux release binary size from the CI job. Machine: ____ Result: ____

### Manual checks of the document browser (M3)

Nothing here runs in CI or from a script: it needs a person at the machine, because the numbers come from the frame overlay on screen. NF2 is **not measured** until someone records the results below in PR C's description.

#### Other checks to do on screen

- [ ] `products`: scroll to the middle and the end, sort by `price`, filter `price:>100`, and check that the status line and the rows agree. Machine: ____ Result: ____
- [ ] Tick rows with the mouse, Shift+click and Space; delete two, then delete by query with a filter that matches more than 1,000 documents and check that it asks for the collection name. Machine: ____ Result: ____
- [ ] Update by query with a value of the wrong type shows the warning, not a success. Machine: ____ Result: ____
- [ ] Copy a selection as JSONL, CSV and curl and paste each into a terminal or editor. Machine: ____ Result: ____

## 4. Assistive technology

Known gaps at 1.0: dock tabs and other kit-owned controls (dock close buttons, the menu, toggles, the Combobox wrapper) stay unnamed for screen readers, and placeholder-only inputs are invisible to the audit in `tests/ui_a11y.rs`.

- [ ] VoiceOver on macOS: reach and name the sidebar, a table and a dialog. **Pass:** each is reached and named, and the prod state is announced. Machine: ____ Result: ____
- [ ] Narrator on Windows: the same. **Pass:** as above. Machine: ____ Result: ____
- [ ] Orca on Linux: the same. **Pass:** as above. Machine: ____ Result: ____

## 5. Installers, per OS

- [ ] `.dmg` on a clean Mac. **Pass:** Gatekeeper warns, `xattr -cr /Applications/Tessera.app` clears it, and the app opens. Machine: ____ Result: ____
- [ ] Keychain prompts on that Mac, also after installing a newer build. **Pass:** they match `book/src/install.md`. Machine: ____ Result: ____
- [ ] macOS 13. **Pass:** the app starts. Machine: ____ Result: ____
- [ ] The x64 `.dmg`. **Pass:** the app starts on an Intel Mac. Machine: ____ Result: ____
- [ ] `.msi`: install, upgrade, uninstall. WiX 3 on `windows-2025` is unproven. **Pass:** each works, and no console window opens. Machine: ____ Result: ____
- [ ] `.zip`. **Pass:** `tessera.exe` starts from an unpacked folder. Machine: ____ Result: ____
- [ ] `.deb` on a clean Ubuntu 24.04, with the `ldd` list from `release.yml`. **Pass:** it installs, starts, and `deb.depends` covers the list. Machine: ____ Result: ____
- [ ] `.rpm` on Fedora. **Pass:** it installs and starts. Machine: ____ Result: ____
- [ ] AppImage with and without FUSE. **Pass:** it starts both ways, without FUSE by `--appimage-extract-and-run`. Machine: ____ Result: ____
- [ ] Wayland and X11. **Pass:** the app starts on both. Machine: ____ Result: ____
- [ ] `app_id` against the desktop entry. **Pass:** the window groups under the Tessera launcher. Machine: ____ Result: ____
- [ ] The `.deb` `Maintainer` line is `Tessera contributors` and the `.rpm` has no packager, because the repository holds no contact address. **Pass:** the maintainer sets one, or accepts this. Machine: ____ Result: ____

## 6. Release steps, maintainer only

- [ ] Push the branches, open the PRs and merge them in order. **Pass:** `main` holds everything and CI is green. Machine: ____ Result: ____
- [ ] Run `release.yml` by `workflow_dispatch` and fix what breaks. **Pass:** all four builds finish. Machine: ____ Result: ____
- [ ] Know that after the merge the schedule publishes a public `nightly` every day, even with no new commit. **Pass:** accepted, or the schedule is changed first. Machine: ____ Result: ____
- [ ] Create the labels from `.github/labels.yml`. **Pass:** they show on GitHub. Machine: ____ Result: ____
- [ ] Enable GitHub Pages and run `docs.yml`. **Pass:** the book is online. Machine: ____ Result: ____
- [ ] Set the date of `## [1.0.0]` in `CHANGELOG.md`. **Pass:** `cargo xtask release-notes 1.0.0` prints it. Machine: ____ Result: ____
- [ ] Tag `v1.0.0`. **Pass:** the tag names the workspace version, and the release holds every package and `SHA256SUMS`. Machine: ____ Result: ____
- [ ] Fill the cask's checksums and publish a tap. **Pass:** the cask installs. Machine: ____ Result: ____
- [ ] Announce. **Pass:** done. Machine: ____ Result: ____

## 7. Signing, when an account exists

Credentials are the maintainer's and never enter the repository.

- [ ] Set the Developer ID identity (`Developer ID Application: …`) in `[package.metadata.packager.macos]`. **Pass:** `codesign --verify --deep` passes. Machine: ____ Result: ____
- [ ] Give the packager `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and either `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` or `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH`. **Pass:** `xcrun notarytool` runs through the packager and accepts the app. Machine: ____ Result: ____
- [ ] `xcrun stapler validate` on the `.app` and the `.dmg`. **Pass:** valid. Machine: ____ Result: ____
- [ ] Windows signing (P2): `windows.certificate-thumbprint` or `sign-command`. **Pass:** the `.msi` shows a publisher. Machine: ____ Result: ____
