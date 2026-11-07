// SPDX-License-Identifier: GPL-3.0-or-later

//! The log: stderr and a daily file, both through [`redact::line`].

use std::io::{self, Write};
use std::path::Path;

use tessera_core::redact;
use tracing::Dispatch;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{InitError, RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// A `MakeWriter` whose writers mask every event before it reaches the inner writer.
pub struct Redacted<M>(pub M);

/// The writer of [`Redacted`].
pub struct RedactedWriter<W>(W);

impl<W: Write> Write for RedactedWriter<W> {
    // `fmt::Layer` writes one formatted event with one `write_all`, so no secret is split.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let masked = redact::line(&String::from_utf8_lossy(buf));
        self.0.write_all(masked.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Redacted<M> {
    type Writer = RedactedWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactedWriter(self.0.make_writer())
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        RedactedWriter(self.0.make_writer_for(meta))
    }
}

/// The writer for `tessera.log.YYYY-MM-DD` in `dir`, off the calling thread, since a log call
/// may run on the GPUI thread. The directory is made if needed.
pub fn file_writer(dir: &Path) -> Result<(NonBlocking, WorkerGuard), InitError> {
    // The crate counts files, at each start and each rollover. Spec §11 says 7 days, which is
    // the same at one file a day; a day without a start writes no file, so 7 files can reach
    // further back than a week.
    let roller = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("tessera.log")
        .max_log_files(7)
        .build(dir)?;
    Ok(tracing_appender::non_blocking(roller))
}

/// The subscriber: stderr, and `file` if there is one.
pub fn dispatch(file: Option<NonBlocking>, filter: EnvFilter) -> Dispatch {
    let file = file.map(|w| fmt::layer().with_ansi(false).with_writer(Redacted(w)));
    let stderr = fmt::layer().with_writer(Redacted(io::stderr));
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .into()
}

/// Starts logging, to `dir` too if it works. Keep the guard: dropping it flushes the file.
pub fn init(dir: Option<&Path>) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let (file, guard, failed) = match dir.map(file_writer) {
        Some(Ok((writer, guard))) => (Some(writer), Some(guard), None),
        Some(Err(e)) => (None, None, Some(e.to_string())),
        None => (None, None, Some("no home directory".to_string())),
    };
    dispatch(file, filter).init();
    if let Some(why) = failed {
        tracing::warn!("logging to stderr only: {why}");
    }
    guard
}
