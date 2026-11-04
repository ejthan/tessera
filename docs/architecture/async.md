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

## Rules

- No blocking I/O on the main thread. Everything that touches the network or disk goes through `runtime::spawn`.
- CPU-heavy work (large JSON, schema inference) runs on GPUI's background executor or tokio's `spawn_blocking`, never in `render`.
- UI tests do not perform real network I/O: GPUI's test executor runs on a simulated clock. Views reach the network through the `Connector` global, and tests install a fake that answers immediately.
- Progress reporting for long jobs (import, export) arrives in M5 with `tokio::sync::watch` and UI updates throttled to 30 per second.
