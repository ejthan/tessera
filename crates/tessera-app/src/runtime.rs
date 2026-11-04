// SPDX-License-Identifier: GPL-3.0-or-later

//! Bridge between GPUI's executor and the tokio runtime that network I/O needs.
//! See `docs/architecture/async.md`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use gpui_kit::{App, Global};
use tokio::runtime::Handle;
use tokio::task::{JoinError, JoinHandle};

/// The app's tokio runtime, registered as a GPUI global at startup.
pub struct Tokio(pub Handle);

impl Global for Tokio {}

/// Runs `future` on the tokio runtime. Await the result from a GPUI task;
/// dropping the returned handle aborts the tokio task.
pub fn spawn<F>(cx: &App, future: F) -> AbortOnDrop<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    AbortOnDrop::from_join_handle(cx.global::<Tokio>().0.spawn(future))
}

/// A tokio task handle that aborts the task when dropped.
pub struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> AbortOnDrop<T> {
    /// Wraps an existing tokio task.
    pub fn from_join_handle(handle: JoinHandle<T>) -> Self {
        Self(handle)
    }
}

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn awaits_result() {
        let handle = AbortOnDrop::from_join_handle(tokio::spawn(async { 7 }));
        assert_eq!(handle.await.unwrap(), 7);
    }

    #[tokio::test]
    async fn dropping_handle_aborts_task() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = AbortOnDrop::from_join_handle(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let _ = tx.send(());
        }));
        drop(handle);
        let result = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("aborted task drops its sender within 1 s");
        assert!(
            result.is_err(),
            "task ran to completion instead of aborting"
        );
    }
}
