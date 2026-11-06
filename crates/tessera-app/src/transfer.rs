// SPDX-License-Identifier: GPL-3.0-or-later

//! Import and export: the backend seam the dialogs and jobs go through.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::Global;
use rust_i18n::t;
use tessera_client::{Client, Collection};
use tessera_core::transfer::export::{ExportJob, ExportOutcome, ExportProgress, run_export};
use tessera_core::transfer::import::{ImportJob, ImportOutcome, ImportProgress, run_import};
use tessera_core::transfer::infer::{InferredSchema, infer_schema};
use tessera_core::transfer::preview::{Preview, preview};
use tessera_core::transfer::report::Report;
use tessera_core::transfer::source::{ReadError, ReadOptions, Source};
use tessera_core::transfer::{JobControl, MAX_DOC_BYTES};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::collections::{BoxFuture, TokioBackend};

/// What the dialogs and jobs do with a file and the server. The app installs
/// [`tokio_backend`]; UI tests install fakes, because GPUI's test executor does not wait for
/// real I/O. Dropping a returned future cancels it, which is why [`crate::jobs::Jobs`] owns
/// the running ones.
pub trait TransferBackend: Send + Sync {
    /// The first rows of `source`, checked against `schema`.
    fn preview(
        &self,
        source: Source,
        options: ReadOptions,
        schema: Option<Collection>,
    ) -> BoxFuture<Result<Preview, ReadError>>;

    /// A schema proposed from the first rows of `source`.
    fn infer(
        &self,
        source: Source,
        options: ReadOptions,
    ) -> BoxFuture<Result<InferredSchema, ReadError>>;

    /// Runs an import to its end.
    fn import(
        &self,
        client: Client,
        job: ImportJob,
        control: JobControl,
        progress: watch::Sender<ImportProgress>,
    ) -> BoxFuture<ImportOutcome>;

    /// Runs an export to its end.
    fn export(
        &self,
        client: Client,
        job: ExportJob,
        control: JobControl,
        progress: watch::Sender<ExportProgress>,
    ) -> BoxFuture<ExportOutcome>;

    /// Saves `report` to `path`, or with `failed_only` the failed lines; how many lines.
    fn save_report(
        &self,
        report: Arc<Report>,
        path: PathBuf,
        failed_only: bool,
    ) -> BoxFuture<Result<u64, String>>;
}

/// The backend the dialogs and jobs use.
pub struct TransferBackendGlobal(pub Arc<dyn TransferBackend>);

impl Global for TransferBackendGlobal {}

impl TransferBackend for TokioBackend {
    fn preview(
        &self,
        source: Source,
        options: ReadOptions,
        schema: Option<Collection>,
    ) -> BoxFuture<Result<Preview, ReadError>> {
        self.run(async move {
            let read =
                tokio::task::spawn_blocking(move || preview(&source, &options, schema.as_ref()));
            read.await.map_err(|e| ReadError::Io(e.to_string()))?
        })
    }

    fn infer(
        &self,
        source: Source,
        options: ReadOptions,
    ) -> BoxFuture<Result<InferredSchema, ReadError>> {
        self.run(async move {
            let read = tokio::task::spawn_blocking(move || infer_schema(&source, &options));
            read.await.map_err(|e| ReadError::Io(e.to_string()))?
        })
    }

    fn import(
        &self,
        client: Client,
        job: ImportJob,
        control: JobControl,
        progress: watch::Sender<ImportProgress>,
    ) -> BoxFuture<ImportOutcome> {
        self.run(run_import(client, job, control, progress))
    }

    fn export(
        &self,
        client: Client,
        job: ExportJob,
        control: JobControl,
        progress: watch::Sender<ExportProgress>,
    ) -> BoxFuture<ExportOutcome> {
        self.run(run_export(client, job, control, progress))
    }

    fn save_report(
        &self,
        report: Arc<Report>,
        path: PathBuf,
        failed_only: bool,
    ) -> BoxFuture<Result<u64, String>> {
        self.run(async move {
            let saved = if failed_only {
                report.save_failed(&path).await
            } else {
                report.save_report(&path).await
            };
            saved.map_err(|e| e.to_string())
        })
    }
}

/// The sentence for a source that cannot be read.
pub fn read_error_text(err: &ReadError) -> String {
    match err {
        ReadError::Io(detail) => t!("transfer.read_error.io", detail = detail),
        ReadError::Json {
            line,
            column,
            message,
        } => t!(
            "transfer.read_error.json",
            line = line,
            column = column,
            detail = message
        ),
        ReadError::Csv { line, message } => {
            t!("transfer.read_error.csv", line = line, detail = message)
        }
        ReadError::RecordTooLarge { line } => t!(
            "transfer.read_error.record_too_large",
            line = line,
            limit = MAX_DOC_BYTES >> 20
        ),
        ReadError::ElementTooLarge { element } => t!(
            "transfer.read_error.element_too_large",
            element = element,
            limit = MAX_DOC_BYTES >> 20
        ),
    }
    .to_string()
}

/// The real backend, on the tokio runtime.
pub fn tokio_backend(handle: Handle) -> Arc<dyn TransferBackend> {
    Arc::new(TokioBackend(handle))
}

#[cfg(test)]
mod tests {
    // Tests unwrap; a panic is the failure we want.
    #![allow(clippy::unwrap_used)]

    use tessera_core::transfer::source::Format;

    use super::*;

    #[tokio::test]
    async fn preview_and_infer_read_off_the_workers() {
        let backend = tokio_backend(Handle::current());
        let source = Source::Bytes(Arc::from(&b"{\"id\":\"1\",\"n\":2}\n"[..]));
        let options = ReadOptions::new(Format::Jsonl);
        let preview = backend
            .preview(source.clone(), options.clone(), None)
            .await
            .unwrap();
        assert_eq!(preview.rows.len(), 1);
        let inferred = backend.infer(source, options).await.unwrap();
        assert_eq!(inferred.sampled, 1);
    }
}
