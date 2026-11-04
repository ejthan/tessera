# Async: GPUI and tokio

Tessera runs two executors.

- **GPUI's executor** drives the UI. It is part of the platform event loop, and every view update happens on it.
- **A tokio runtime** (multi-thread, 2 workers) runs network I/O, because `reqwest` needs tokio. `tessera_app::run` creates it once at startup and registers its handle as the GPUI global `runtime::Tokio`.

A tokio `JoinHandle` is an ordinary future, so GPUI tasks can await work that runs on tokio.

## The pattern

```rust
let io = runtime::spawn(cx, async move { client.debug().await }); // AbortOnDrop<Result<…>>
self.task = Some(cx.spawn(async move |this, cx| {
    let result = io.await;
    this.update(cx, |view, cx| {
        view.apply(result);
        cx.notify();
    })
    .ok();
}));
```

- `runtime::spawn` starts the future on tokio and returns an `AbortOnDrop`. Dropping it aborts the tokio task.
- Keep the GPUI `Task` in the view's state. Replacing it (a second click on Connect) or dropping the view drops the awaited `AbortOnDrop`, which cancels the request. A stale request can therefore never overwrite a newer result.
- `this.update` fails once the view is gone. The error is ignored on purpose: there is nothing left to update.

## The Connector global

`connection::ConnectorGlobal` holds the function that turns a profile and an API key into a live connection:

```rust
pub type Connector = Arc<dyn Fn(Profile, SecretString) -> ConnectFuture + Send + Sync>;
pub type ConnectFuture = Pin<Box<dyn Future<Output = Result<Connected, Error>> + Send>>;
// Connected { client: Client, info: ServerInfo, access: KeyAccess }
```

- The app installs `tokio_connector`. It builds the `Client` from the profile (node URLs, timeout, retries capped at 10, nearest node, TLS options, read-only), then runs `GET /health`, `GET /debug`, `GET /collections?limit=1&exclude_fields=fields` and the `GET /keys` probe on tokio. The probe comes after `/debug`, because Typesense answers `401` to a wrong key too. The latency shown is the duration of `/debug`.
- It is the seam for UI tests. GPUI's test executor runs on a simulated clock and does not wait for real network I/O, so tests install a fake that answers immediately and can record which profile and key it was called with.
- `AppState::connect` and the connect dialog are its only callers. Views never build a `Client` themselves.
- `AppState::connect` does not call it for a profile whose nodes do not form URLs, or when no key is in memory or stored. The connection then shows the reason inline.

## Keychain and file I/O

`SecretStore` calls (Keychain, Credential Manager, Secret Service) and writes of `connections.json` block. `AppState` runs them on tokio's `spawn_blocking` through the `Tokio` global and awaits the `JoinHandle` from a GPUI task. A blocking call that has started cannot be aborted, so dropping the GPUI task only stops waiting for its result. A save that has not reached `spawn_blocking` yet is lost when its task is dropped.

- Saves are chained: each save's task awaits the previous one before it writes, so the last edit is the one on disk.
- The save chain runs on GPUI's background executor, not the foreground one. On quit, GPUI blocks the main thread on the `on_app_quit` handlers and runs no foreground tasks meanwhile (`platform_scheduler.rs` `block`), so a foreground save could not finish.
- On quit, `AppState` awaits the pending save, for at most GPUI's `SHUTDOWN_TIMEOUT` (200 ms). A save still running after that may be cut off when the process exits. Because `store::save` writes a temporary file and renames it, the previous `connections.json` then stays intact; only the last edits are lost.
- The connect dialog does not reconnect after saving: it hands the `Connected` from its test (the client and server info for the values it saved) to `AppState::add_connected`, so there is no second probe round and no keychain `get` racing the `set` it just did. After "Save anyway" (the test failed) it closes any open connection of that profile instead, so no connection keeps the old profile and client.
- `connect` still takes an optional key, used instead of the stored one; no caller in the app passes one today.
- After a failed load, `AppState::save` does nothing for the rest of the session, so a file that does not parse is never overwritten.
- `connections.json` is read once, in `run`, before the window opens.
- Tests that touch the secret store or the file install a real tokio runtime as the `Tokio` global and call `cx.executor().allow_parking()`, so the test scheduler waits for the wake-up from tokio's threads.

## Rules

- No blocking I/O on the main thread. Network calls go through `runtime::spawn`, blocking keychain and file calls through tokio's `spawn_blocking`.
- CPU-heavy work (large JSON, schema inference) runs on GPUI's background executor or tokio's `spawn_blocking`, never in `render`.
- UI tests do not perform real network I/O: GPUI's test executor runs on a simulated clock. Views reach the network through the `Connector` global, and tests install a fake that answers immediately.
- Progress reporting for long jobs (import, export) arrives in M5 with `tokio::sync::watch` and UI updates throttled to 30 per second.
