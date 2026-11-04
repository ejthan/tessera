# M1 Connections and overview: design

- **Date:** 2026-11-04
- **Status:** approved in brainstorming, written spec pending review
- **Product spec:** [`docs/spec.md`](../../spec.md) (M1 row in §14; §7.1, §7.2, §7.19)
- **Previous milestone:** [M0 foundation design](2026-11-04-m0-foundation-design.md)
- **UX mockups:** [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d), screens 2–4

## 1. Goal

M1 turns M0's single URL-and-key form into real connection management, adds the server and cluster overview, and makes every request visible in a request log in the bottom dock.

**Acceptance (product spec §14):** the acceptance criteria of §7.1 and §7.2 hold:

- A key never appears in plain text in logs, config files or crash reports.
- In a read-only profile, no UI path sends a write request. A client hook that reports every write request proves it.
- A failed node is marked red while the other nodes keep working.
- Polling stops while the window is hidden.

## 2. Delivery

One spec, one plan, three pull requests in this order. Each PR has green CI and its own CHANGELOG entries.

| PR | Content | Why this order |
|---|---|---|
| **A · Connections** | Profiles, `connections.json`, keychain, environment tags and their visibility, read-only guard, connection dialog, multi-node client with failover, import from typesense-dashboard, env vars and CLI flags, sidebar with all profiles | Everything else keys on profiles: log entries name a connection, the cluster view needs the node list |
| **B · Dock and request log** | Shell rebuilt on GPUI Kit `DockArea`, tabs per connection, request log panel with filters, detail view, redaction and "Copy as curl" | The log is the tool for debugging the overview's polling in PR C |
| **C · Overview** | Node cards, key figures, live metrics charts, API stats table, polling lifecycle, 3-node dev cluster and nightly cluster tests | Builds on A's client and B's tabs |

## 3. Scope

### In M1

- Product spec §7.1 complete, except unlocking read-only (D10)
- §7.2 except the per-collection memory estimate (P2)
- §7.19 except the full view of bodies cut at 1 MB
- The M0 deferred minors listed in §11

### Deferred, and where it goes

| Item | Milestone | Why not now |
|---|---|---|
| Unlock read-only by typing the profile name, "Writes unlocked" banner, "Lock again" | M2 | M1 has no write action. An unlock with nothing to unlock is a dead control (UX rule 4). See D10 |
| Sidebar entries Collections, Aliases, Keys … and the bottom tabs "Jobs" and "Raw console" | M2, M4, M5 … | No dead controls |
| Layout persistence, several windows | later (as decided in M0) | |
| Settings page (§7.21) | later | M1 keeps its two settings in `connections.json` (§4.2) and the session (polling interval) |
| Memory estimate per collection | v1.x (P2) | |
| Full view of request log bodies cut at 1 MB | M4 | No M1 response comes near 1 MB; search and the raw console are the first that can |
| Round robin across nodes | not planned | D14 |

## 4. tessera-core: profiles, secrets, import

No GPUI dependency. Everything here is unit tested.

### 4.1 Profile

```rust
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub color: ProfileColor,        // Gray, Blue, Teal, Violet, Amber, Red (the canvas swatches)
    pub environment: Environment,   // Local, Dev, Staging, Prod, Custom(String)
    pub nodes: Vec<Node>,           // at least one
    pub nearest_node: Option<Node>,
    pub timeout_secs: u32,          // default 5
    pub retries: u32,               // default 2
    pub read_only: bool,
    pub tls: TlsOptions,            // ca_file: Option<PathBuf>, allow_insecure: bool
}

pub struct Node { pub protocol: Protocol, pub host: String, pub port: u16, pub path: String }
```

- `Environment::is_prod()` is true only for `Prod`. A custom tag is never treated as prod.
- `Node::url()` builds the base URL. `Node::label()` gives `host:port` for display.
- Choosing `Prod` in the dialog switches `read_only` on; the user may switch it off again.

### 4.2 ProfileStore and connections.json

Location: `ProjectDirs::from("org", "tessera", "Tessera")` → `config/connections.json` (product spec §11), via the `directories` crate.

```json
{ "schema_version": 1, "profiles": [ … ], "last_used": "<uuid>", "reopen_last": false }
```

- `ProfileStore::load(path)` and `save(path)`. A missing file means "no profiles". A file that does not parse is not overwritten: the app shows the parse error on the welcome screen and keeps the profiles in memory only.
- Writes are atomic: write `connections.json.tmp`, then rename. Later schema migrations write `connections.json.bak` before changing anything.
- `reopen_last` is the "Reopen last connection at startup" option (§7.1). It lives here because M1 has no settings page and no `settings.toml` yet.
- The file never holds a key. A test serializes a store whose profiles have keys in the `SecretStore` and asserts that no key value occurs in the bytes.
- Loading happens once in `main` before the window opens (not in the render path). Saves run on tokio's `spawn_blocking`.

### 4.3 SecretStore

```rust
pub trait SecretStore: Send + Sync {
    fn get(&self, profile: Uuid) -> Result<Option<SecretString>, SecretError>;
    fn set(&self, profile: Uuid, key: &SecretString) -> Result<(), SecretError>;
    fn delete(&self, profile: Uuid) -> Result<(), SecretError>;
}
```

- `KeyringStore` uses `keyring` 4.2.0 (default feature `v1`: macOS Keychain, Windows Credential Manager, Secret Service via `zbus` on Linux) with service `tessera` and account = profile UUID.
- `MemoryStore` serves the tests and temporary connections.
- All calls are blocking and run on `spawn_blocking`. The macOS Keychain may show its own permission prompt.
- `SecretError` says what failed ("No Secret Service is running", "Keychain access was denied") and never contains the key.

### 4.4 Import and temporary connections

**typesense-dashboard `config.json`** (format from the reference README and `config.json.sample`):

- The top-level `node` with `apiKey`, and every `history[]` entry, each become one profile.
- Entries that share a `clusterTag` become **one** profile with several nodes, named after the tag. If their keys differ, the profile takes the first key and the preview says so.
- `connectionTimeoutSeconds` becomes `timeout_secs`. `node.port` is a string in that format and is parsed.
- `host: "SAME"` only makes sense in the web version; such entries are skipped with a note.
- An entry whose nodes equal an existing profile's nodes is marked as a duplicate and unchecked by default.
- `ui` and other unknown fields are ignored.
- Imported profiles get environment `Dev` and color `Gray`; the preview lets the user change both per row before importing.

`import::dashboard_config(json) -> Vec<ImportCandidate>` is pure and table-tested.

**Environment variables and CLI flags** produce one **temporary connection**:

| Source | Names |
|---|---|
| Environment | `TYPESENSE_HOST`, `TYPESENSE_PORT`, `TYPESENSE_PROTOCOL`, `TYPESENSE_PATH`, `TYPESENSE_API_KEY` |
| CLI | `--host`, `--port`, `--protocol`, `--path`, `--api-key-stdin`, `--env <local\|dev\|staging\|prod>`, `--read-only`, `--profile <name>`, `--help` |

- A temporary connection exists when `TYPESENSE_HOST` or `--host` is set. Flags win over environment variables.
- Defaults: protocol `http` for `localhost`, `127.0.0.1` and `::1`, otherwise `https`. Port 8108. Environment `local` for those hosts, otherwise `dev`. Read-only off unless `--read-only` or `--env prod`.
- `--api-key-stdin` reads one line from stdin in `main`, before the window opens. A key is never accepted as a flag value, because it would show in the process list and the shell history.
- `--profile <name>` opens a saved profile instead. A name that matches no profile, or a flag combination that makes no sense, prints an error and exits with code 2.
- The key of a temporary connection lives in memory only. Nothing is written to disk or the keychain.
- `TYPESENSE_API_KEY` alone (no host) keeps M0's behavior: it prefills the key in the new-connection dialog.
- Parsing is a small hand-written function in core (`cli::parse(args, env) -> Result<Launch, CliError>`), table-tested. A dozen flags do not justify `clap` and its binary size.

**Export** writes the profiles as JSON without keys (`schema_version` included) to a path from the save dialog.

## 5. tessera-client: nodes, failover, read-only, request events

### 5.1 Client

```rust
pub struct ClientOptions {
    pub timeout: Duration,
    pub retries: u32,
    pub nearest_node: Option<String>,
    pub ca_file: Option<PathBuf>,
    pub allow_insecure: bool,
    pub read_only: bool,
}

impl Client {
    pub fn new(nodes: &[&str], key: SecretString, options: ClientOptions) -> Result<Self, Error>;  // validates each URL as in M0
    pub fn with_hook(self, hook: RequestHook) -> Self;
    pub fn active_node(&self) -> usize;
    pub fn set_active_node(&self, index: usize);
    pub fn node(&self, index: usize) -> NodeClient;   // one node, no failover
    pub fn polling(&self) -> Self;                    // same client, events tagged Origin::Polling
}
```

- All nodes share one `reqwest::Client`. The CA file and `danger_accept_invalid_certs` are applied when it is built. Redirects stay off (M0).
- The active node starts as the nearest node if set, else the first node. A nearest node that is not in the node list is inserted at index 0 of `nodes()`, so failover order and the cluster view index from there. It is shared state (`Arc<AtomicUsize>`), so clones of the client agree on it.
- `NodeClient` offers the same endpoint methods for exactly one node. The cluster view uses it, because failover would make a dead node look alive.
- The endpoint methods (`health`, `debug`, `metrics`, `stats`, `collections_summary`, `keys_probe`) are implemented once and work on both `Client` and `NodeClient` through one internal `send` path.

### 5.2 Failover (D14)

- Requests go to the active node.
- Only on `Network` or `Timeout`, and only for GET, HEAD, PUT and DELETE, does the client try the other nodes in order, up to `retries` extra attempts, waiting 100 ms × 2ⁿ between them. HTTP errors are answers and never trigger failover.
- When another node answers, it becomes the active node.
- No round robin: in an admin tool it must be predictable which node answers.
- Each attempt emits its own `RequestEvent`, so the log shows "node-3 failed, node-1 answered".

### 5.3 Read-only guard

- With `read_only` set, POST, PATCH, PUT and DELETE return `Error::ReadOnly` **before** anything is sent. PUT counts as a write because it is an upsert in Typesense. GET and HEAD pass.
- The guard sits in the client's single `send` path, so no caller can bypass it.
- The §7.1 AC test: a read-only client with a hook that records every event that reached the network; every write method returns `Error::ReadOnly` and the hook records nothing. A second test sends GET through the same client and sees it pass.
- M1 issues no writes at all, so the guard is proven by tests, not by UI. M2 adds the unlock flow (D10).

### 5.4 RequestEvent

```rust
pub struct RequestEvent {
    pub id: u64,                        // per process, increasing
    pub started_at: SystemTime,
    pub node: Url,
    pub method: Method,
    pub path: String,
    pub query: Option<String>,
    pub request_headers: Vec<(String, String)>,  // never X-TYPESENSE-API-KEY
    pub key_prefix: String,             // first 4 characters if the key is longer than 8, else empty
    pub request_body: Option<Body>,
    pub response_body: Option<Body>,
    pub status: Option<u16>,
    pub duration: Duration,
    pub size: Option<u64>,              // response bytes
    pub origin: Origin,                 // User or Polling
    pub error: Option<String>,          // Display text of the error, never the key
}

pub struct Body { pub text: Arc<str>, pub original_len: usize }   // text cut at 1 MB
```

- The hook receives the event by reference after the response or error, as in M0. The `tracing` line stays and still logs only method, path, status and duration.
- Bodies are cut at 1 MB on a UTF-8 boundary; `original_len` keeps the real size so the log can say how much is missing. M1's responses (metrics, stats, collection summaries) stay far below 1 MB, so the on-demand full view of cut bodies (§7.19) arrives with the first endpoints that exceed it (M4 search and raw console).

### 5.5 New endpoints

Each gets an integration test against the dev server before any UI uses it (CLAUDE.md rule 2). Response shapes were checked against a live 30.2 node.

| Method | Endpoint | Model |
|---|---|---|
| `metrics()` | `GET /metrics.json` | All values are **strings**. Typed fields for `system_cpu_active_percentage`, `system_memory_used/total_bytes`, `system_disk_used/total_bytes`, `system_network_received/sent_bytes` (cumulative), plus every `typesense_memory_*` and per-CPU field kept in a `BTreeMap<String, String>` for forward compatibility |
| `stats()` | `GET /stats.json` | Numbers. `latency_ms` and `requests_per_second` are maps keyed by `"GET /collections"` style endpoint names; the scalar totals (`search_latency_ms`, `total_requests_per_second` …) are typed; unknown keys kept as `serde_json::Value` |
| `collections_summary()` | `GET /collections?exclude_fields=fields` | name and `num_documents` per collection |
| `keys_probe()` | `GET /keys` | Only the status matters in M1: 200 → `KeyAccess::Admin`, 401/403 → `KeyAccess::Restricted` |

`Error` gains `ReadOnly` and `Tls`. `Tls` is produced when rustls reports a certificate problem, so the dialog can say "The certificate of {host} is not trusted" instead of "Could not reach".

## 6. tessera-core: request log and metrics history

### 6.1 RequestLog

- Two ring buffers: 5,000 entries for `Origin::User`, 500 for `Origin::Polling` (D12). `entries(filter)` merges them by `id`.
- `Filter { errors_only, writes_only, methods, connections, show_polling }`. `show_polling` is off by default; `errors_only` and `writes_only` apply to both buffers, so a failed polling request is still found under "Errors".
- Each entry carries the connection id and display name next to the event.

### 6.2 Redaction

- The API key is never in the event (§5.4). The display shows `abcd••••` from `key_prefix`, or only `••••` when the prefix is empty.
- `redact_json(body)` masks the values of `api_key` and `value` keys at any depth. `value` covers the key value in `POST /keys` responses. Bodies that are not JSON are shown as they are.
- Redaction is applied when an entry is displayed or copied, never in place, and is tested with fixtures. These are the redaction tests CLAUDE.md rule 8 protects.

### 6.3 Copy as curl

```
curl -X POST 'https://ts-eu-1.example.net:443/collections' \
  -H "X-TYPESENSE-API-KEY: $TYPESENSE_API_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @- <<'EOF'
{"name":"books", …}
EOF
```

- The key is always the placeholder `$TYPESENSE_API_KEY`. Bodies are copied unredacted except for `api_key`/`value` secrets, which stay masked.
- Single quotes in URLs are escaped for POSIX shells. `insta` snapshot tests cover GET, POST with a body, a query string and quoting.

### 6.4 MetricsHistory

- One ring buffer per node and metric, sized for 15 minutes at the current polling interval (180 points at 5 s), with timestamps so a changed interval keeps the time axis right.
- Network rx/tx are stored as rates: the difference of two cumulative samples divided by the time between them. A counter that goes down (server restart) starts over instead of producing a negative rate.

## 7. tessera-app

### 7.1 State

- `AppState` entity: profiles and `ProfileStore` path, the `SecretStore`, open `ConnectionState`s (one per connected profile or temporary connection), the `RequestLog`, and the temporary connection from the CLI.
- `ConnectionState` entity: profile (or temporary profile), `Client`, per-node `NodeStatus` (last `Debug`, health, latency, error, next retry), capabilities, and `MetricsHistory`.
- The `Connector` global stays the seam for UI tests. Its signature changes from URL plus key to profile plus key; tests still install fakes.
- The client's hook pushes events into the `RequestLog` through a channel that a GPUI task drains, so the hook (which runs on tokio) never touches GPUI state.

### 7.2 Shell

**PR A** keeps M0's hand-built shell:

- The sidebar lists all profiles, each with color dot, environment badge and a lock badge for read-only.
- Enter or a click connects. A spinner shows inline, and a failure shows inline under the entry with the full text (UX rule 2).
- Each entry has a context menu with Connect or Disconnect, Edit (⌘E / Ctrl+E) and Delete (⌘⌫ / Ctrl+Backspace). The shortcuts are shown in the menu.
- The temporary connection from env vars or flags is marked "not saved". Its menu offers "Save as profile…", which opens the dialog prefilled (D13), instead of Edit and Delete.
- The sidebar header has "Add connection" (⌘N / Ctrl+N), "Import from typesense-dashboard…", "Export profiles…" and the "Reopen last connection at startup" check item.
- The main area shows the most recently selected connection's overview.

**PR B** rebuilds the shell on GPUI Kit `DockArea`:

- Left dock: the sidebar panel, now a tree (connection → Overview).
- Center: tabs. Each tab shows a color dot in the profile color, the connection name and the environment badge. Deviation (Task 15 spike): GPUI Kit 0.9 builds the tab chrome itself and offers no per-panel tab styling (`Panel::title_style` only colors the single-panel title bar), so the top edge in the profile color sits on the tab's content, directly under the tab bar. While no tab is open, the center shows the welcome screen.
- Bottom dock: the request log panel, toggled with ⌘J / Ctrl+J. The shortcut is shown on the toggle.
- Opening a connection's Overview again focuses the existing tab. Disconnecting closes that connection's tabs.
- Panels can be moved and closed. The layout is not saved yet. Closing a tab (⌘W / Ctrl+W) does not disconnect; zooming a panel is ⌘⇧↩ / Ctrl+Shift+Enter. The tab menu shows both shortcuts. The active tab is the overview tab selected last, in the center or in an open dock; the sidebar's Overview entry opens the dock its tab was moved into. DockArea keeps the last panel of a region, so the last open tab cannot be closed; disconnecting closes it. The sidebar and the request log cannot be closed, since nothing could bring them back.

### 7.3 Environment visibility (UX rule 1)

- **Title bar:** connection label (no switcher, see D16) with color dot, name and environment badge (`PROD` red and filled, other tags subdued), plus the read-only lock badge.
- **Sidebar and tabs:** as in §7.2.
- **Status bar:** "Connected · staging-eu · v30.2.0 · leader · node-1 · 11 ms", "1 of 3 nodes unreachable", and on prod "prod · read-only".
- **Prod (D11):** a 4 px red band across the top and a 2 px red frame inside the window edge while the **active tab** belongs to a prod connection. In PR A, before tabs exist, it is the selected connection. The band is drawn in the window content, which works the same with macOS's native title bar and the client-side decorations on Linux and Windows.

### 7.4 Connection dialog (canvas screen 2)

- **Fields:** name, color swatches, environment as a segmented control (local, dev, staging, prod, other with a text field), node rows (protocol, host, port, path prefix, remove) with "Add node for a cluster", API key, read-only switch.
- **API key:** masked. The show toggle becomes a focusable button with its own shortcut (⌥⌘K / Ctrl+Alt+K) and German strings (M0 minor). The hint below says where the key is stored, by platform: Keychain, Credential Manager or Secret Service.
- **Advanced**, collapsed: timeout, retries, nearest node, CA file (file dialog), "Allow insecure certificates" with a warning banner. The collapsed summary repeats the values ("timeout 5 s · 2 retries · system certificates").
- **Test** (⌘T / Ctrl+T) runs four requests:
  1. `/health`
  2. `/debug`
  3. `GET /collections?limit=1&exclude_fields=fields`
  4. `GET /keys`

  The result shows inline. `/keys` answering 401 or 403 means "Restricted key: can read collections, cannot list API keys"; 200 means "Admin key". The result also says when it was tested and how long it took.
- **Buttons:** "Cancel" (Esc) and "Save and connect" (⌘↩ / Ctrl+↩). Save runs the test first if none has run for the current values.
- **Cancel during a running test** drops the task, which aborts the requests, and nothing connects (M0 minor). A UI test covers it.
- **Edit** uses the same dialog. An empty key field means "keep the stored key".
- **Delete** is danger level medium: a confirmation dialog that names the profile and says the stored key is removed too. Focus is on Cancel.
- **Keychain failure** shows inline with the platform's message and the option "Use for this session only". The profile is then saved without a key and the key kept in memory.
- **Remote `http://`:** a non-local host with `http://` shows the warning from product spec §9 under the node rows.

### 7.5 Import preview

A file dialog, then a dialog listing the candidates:

- Each row shows a checkbox, name, nodes, environment and color selects, and notes (duplicate, differing cluster keys, skipped `SAME`).
- "Import N profiles" writes the profiles and the keys. Keychain failures are listed per row and leave those profiles without keys.

### 7.6 Request log panel (PR B, canvas screen 3)

- **Columns:** time (local, milliseconds), method, path, node, status ("failed" in red when no response arrived), duration and size. A "Connection" column appears while more than one connection is open.
- **Table:** GPUI Kit `Table`, virtualized. It follows new entries while scrolled to the bottom.
- **Filters:** All, Errors and Writes as a segmented control, a method menu, a connection menu and the "Show polling" switch.
- **Detail:** a split on the right of the panel with request line, headers (key redacted), request body and response body (JSON pretty-printed, secrets masked, cut bodies say "showing 1 MB of 3.4 MB").
- **Copy as curl** (⌘⇧C / Ctrl+Shift+C, shown on the button) copies the selected entry.

### 7.7 Overview (PR C, canvas screen 3)

- **Header:** "Live · every 5 s" with Pause/Resume (Space, shown on the button) and an interval menu (2, 5, 10, 30 s). The interval is kept for the session.
- **Node cards** side by side:
  - Each card shows name, `host:port`, role (leader highlighted), latency, and "active node" on the active one.
  - An unreachable node's card is red, with the full error text and "retry in N s".
  - Click or Enter on a card makes that node active.
  - A single-node profile shows one card; there is no special case.
- **Key figures:**
  - Version, collections, documents and Typesense memory (`typesense_memory_active_bytes`)
  - CPU, system memory, disk and network rx · tx
- **Charts:** four line charts (GPUI Kit `LineChart`) for CPU, memory, disk and network with 15 minutes of the active node's history. Switching the active node switches the curves; history is kept per node.
- **API stats:** a table of endpoint, requests per second and average latency from the active node's `/stats.json`, sorted by clicking a header (Enter on a focused header). An empty `latency_ms` (a fresh server) shows a sentence explaining that the table fills once the server receives requests.
- **Old servers:** the warning banner below 30.0 from M0 stays.

### 7.8 Polling

- One GPUI task per node of each open Overview tab, owned by the tab's poller, so a hanging node never delays the others.
- Each tick asks every node in parallel, through `node(i)` without failover and with `Origin::Polling`, for `/health`, `/debug`, `/metrics.json` and `/stats.json`.
- Collection and document counts load when the tab opens and on ⌘R / Ctrl+R, not every tick, because `/collections` is expensive on large clusters.
- Polling stops while the window is hidden (`observe_window_visibility`), while paused, and when the tab closes. Closing drops the task, and `AbortOnDrop` cancels requests in flight.
- A failing node (an error from `/health` or `/debug`, or `/health` answering `ok: false`) gets its own backoff (5 s doubling to at most 30 s) shown as a countdown on its card. Other nodes keep their interval. This is the §7.2 AC.

### 7.9 i18n

- Every new string goes through `t!()`, with keys in `en.yml` and `de.yml`.
- Shortcut labels on controls come from GPUI Kit's `Kbd`, which formats per platform, instead of hand-written "Ctrl+N" strings (M0 minor).
- Units and the version prefix ("v", "ms", "MB/s", "%") get keys too (M0 minor).
- German strings for the GPUI Kit components that M1 starts using (dialog buttons, table, menus) are added to `de.yml` where the components read them.

## 8. Dev environment

- `dev/docker-compose.yml` gains a profile `cluster` with three nodes on host ports 18108, 18109, 18110 and a shared `--nodes` file. This confirms `state = 4` for followers, which M0 left open.
- The default single node keeps `TYPESENSE_PORT` (default 8108).
- `docs/building.md` documents both.

## 9. Testing

| Level | What it checks |
|---|---|
| Core unit | Profile serialization round trip; no key in `connections.json` bytes; atomic save leaves the old file intact when the write fails; unparseable file is not overwritten; `dashboard_config` table (single node, history, cluster tag grouping, differing keys, `SAME`, duplicates, string ports); `cli::parse` table (flags over env, defaults per host, `--profile`, errors, exit code 2); `RequestLog` buffers, eviction per origin, filters, merge order; `redact_json` fixtures; curl snapshots (`insta`); `MetricsHistory` rates and counter reset |
| Client unit (`wiremock`) | Failover on refused connection and timeout, none on HTTP 500; backoff count equals `retries`; the active node moves to the answering node; `NodeClient` never fails over; read-only guard (the §7.1 AC test, §5.3); one event per attempt with the right node; body cut at 1 MB on a UTF-8 boundary; no key in any event, `Debug` or `Display` |
| Client integration (dev server, `#[ignore]`, run by CI with `--ignored`) | `metrics`, `stats`, `collections_summary`, `keys` with the admin key and with a search-only key created in test setup (expects 401/403) |
| Cluster integration (nightly, 3-node profile) | Leader and follower states; stopping one node makes its `NodeClient` fail while the others answer; `Client` fails over |
| App UI (`#[gpui_kit::test]`, fake `Connector`, `MemoryStore`, temp config dir) | Dialog: save creates a profile and stores the key; prod switches read-only on; Cancel during a running test does not connect; show-key toggle reachable by Tab and shortcut; keychain failure offers "session only". Sidebar: Enter connects, failure shows inline. Import preview creates the checked profiles. PR B: ⌘J toggles the log, a request appears in it, "Copy as curl" puts the placeholder on the clipboard, "Show polling" off hides polling entries. PR C: a fake node error turns one card red while the others update; hiding the window stops ticks; Pause stops ticks |
| Secrets | Redaction tests (CLAUDE.md rule 8) plus a test that runs a full fake session and searches the log entries, `tracing` output and `connections.json` for the key |

## 10. CI

- The `integration` job gains the search-only key setup.
- The new `nightly` workflow (scheduled, plus manual dispatch) starts the 3-node cluster and runs the cluster tests.
- The `build` job keeps the NF5 size check. Each PR records the Linux binary size in its description.
- Linux CI has no Secret Service. Tests use `MemoryStore`; `KeyringStore` is covered by one `#[ignore]` test run manually on each platform before the PR is merged, as listed in the PR checklist.

## 11. M0 deferred minors

| Minor | Where in M1 |
|---|---|
| Cancel during a connect attempt must not connect | Dialog (§7.4), UI test, PR A |
| Key mask toggle not keyboard-reachable, no German strings | Dialog (§7.4), PR A |
| "Ctrl+N", "v", "ms" bypass `t!()` | §7.9, PR A |
| `docs/architecture/async.md` should describe the `Connector` global | PR A, together with the signature change (§7.1) |
| Timeout text hard-codes 5 seconds | `connect.error.timeout` takes `%{seconds}` from the profile, PR A |
| Third-party notices for CDLA-Permissive-2.0 | `THIRD_PARTY_NOTICES.md` with the `webpki-root-certs` notice, linked from the README, PR A |
| NF5: Linux binary at 37.3 of 40 MB | Measured in every PR (§10, §12) |

## 12. New dependencies

All pinned exactly in the workspace `Cargo.toml`; versions checked at plan time.

| Crate | Crate using it | Why |
|---|---|---|
| `keyring` 4.2.0 | core | OS keychain (§4.3) |
| `directories` | core | config path (§4.2) |
| `uuid` (v4, serde) | core | profile ids |
| `jiff` | app | local wall-clock time in the log |
| `insta` (dev) | core | curl snapshots |

`reqwest` gains the features its TLS options need, if any (checked at plan time). No CLI parser crate (§4.4).

## 13. Changes to the product spec

Recorded in `docs/spec.md` together with this design:

- §16 gains D10–D14.
- §5.3 describes the two request log buffers.
- §7.1 notes that unlocking arrives in M2.
- The §14 M1 row links this design.

| # | Question | Decision |
|---|---|---|
| D10 | Read-only unlock in M1? | No. M1 has the profile flag, the client guard and the badges; the unlock flow arrives in M2 with the first write action |
| D11 | Prod frame with several connections in one window | Red band and frame while the active tab belongs to a prod connection |
| D12 | Polling traffic in the request log | Logged, in its own 500-entry buffer next to the 5,000-entry buffer for user requests; hidden by default |
| D13 | Connection from env vars or CLI flags | Temporary, never saved, key in memory only; "Save as profile" opens the prefilled dialog |
| D14 | Request routing in cluster profiles | Active node with failover for idempotent methods; no round robin |

## 14. Risks

| Risk | Impact | Fallback |
|---|---|---|
| `keyring` plus `jiff` push the Linux binary over 40 MB (NF5) | Build job fails | Use GPUI's own credential API (`cx.read_credentials` …) behind the same `SecretStore` trait (its Linux items carry the label `zed-github-account`); format log times with `std` in UTC |
| Secret Service missing or locked on Linux desktops | Saving a profile fails | "Use for this session only" (§7.4); the error names the missing service |
| `DockArea` in 0.9.0 does not support a panel the way the canvas shows it (bottom dock with its own split) | PR B slower | The detail view becomes a resizable split inside the panel's own view; the plan starts PR B with a dock spike |
| `LineChart` redraw cost with four charts × 180 points every tick | Idle CPU above NF6 | Redraw only on new data; drop to one combined chart if needed |
| Failover hides a dead node from the user | Confusing state | The log shows every attempt; the status bar and node cards show unreachable nodes |
