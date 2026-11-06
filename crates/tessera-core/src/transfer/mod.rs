// SPDX-License-Identifier: GPL-3.0-or-later

//! Import and export of documents: readers for JSONL, JSON arrays and CSV, schema inference
//! and preview, the import pipeline and its report. No UI; the readers are blocking and run
//! off the async workers.

pub mod csv_cells;
pub mod import;
pub mod infer;
pub mod preview;
pub mod report;
pub mod source;

pub use import::{JobControl, JobEnd, Phase, TransferError};

/// Documents per import batch unless the user says otherwise.
pub const DEFAULT_BATCH: u32 = 1000;
/// The largest batch size accepted; refuses a typo (`BATCH_BYTES` decides first anyway).
pub const MAX_BATCH: u32 = 100_000;
/// A batch is cut here whatever its document count, to bound memory (16 MiB).
pub const BATCH_BYTES: usize = 16 << 20;
/// The largest document the client reads (32 MiB). A memory limit, not the server's.
pub const MAX_DOC_BYTES: usize = 32 << 20;
/// Batches queued between the reader and the sender: one for the next round trip, one
/// being filled.
pub const CHANNEL_BATCHES: usize = 2;
/// Rows the preview shows.
pub const PREVIEW_LINES: usize = 20;
/// Rows schema inference reads.
pub const SAMPLE_ROWS: usize = 1000;
/// Failed rows a report keeps in its table.
pub const REPORT_ROWS: usize = 1000;
/// Bytes of a rejected row a report shows.
pub const REPORT_PREVIEW_BYTES: usize = 2048;
/// The most a pasted clipboard text may hold (16 MiB).
pub const CLIPBOARD_LIMIT: usize = 16 << 20;
/// Retries of a batch the server answers 503 to.
pub const BUSY_RETRIES: u32 = 5;
/// Rows of a CSV file that decide its header names.
pub const CSV_HEADER_SAMPLE: usize = 1000;
