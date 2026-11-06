// SPDX-License-Identifier: GPL-3.0-or-later

//! Import and export: the backend seam the dialogs and jobs go through.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::Global;
use rust_i18n::t;
use tessera_client::{Client, Collection};
use tessera_core::backup::archive::ArchiveError;
use tessera_core::backup::format::ManifestProblem;
use tessera_core::backup::restore::{RestoreJob, RestoreOutcome, RestoreProgress, run_restore};
use tessera_core::backup::run::{BackupOutcome, BackupPlan, BackupProgress, run_backup};
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

/// Size and mtime, which tell that a file changed since it was read.
pub type FileStamp = (u64, Option<SystemTime>);

/// The stamp of `path` now. Blocks.
pub fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.len(), meta.modified().ok()))
}

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

    /// The stamp of `path` now.
    fn file_stamp(&self, path: PathBuf) -> BoxFuture<Result<FileStamp, ReadError>>;

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

    /// Runs a backup to its end.
    fn backup(
        &self,
        client: Client,
        plan: BackupPlan,
        control: JobControl,
        progress: watch::Sender<BackupProgress>,
    ) -> BoxFuture<BackupOutcome>;

    /// Runs a restore to its end.
    fn restore(
        &self,
        client: Client,
        job: RestoreJob,
        control: JobControl,
        progress: watch::Sender<RestoreProgress>,
    ) -> BoxFuture<RestoreOutcome>;

    /// Whether anything stands at `path`, a link that leads nowhere too.
    fn path_taken(&self, path: PathBuf) -> BoxFuture<bool>;

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
    fn file_stamp(&self, path: PathBuf) -> BoxFuture<Result<FileStamp, ReadError>> {
        self.run(async move {
            let read = tokio::task::spawn_blocking(move || file_stamp(&path));
            let stamp = read.await.map_err(|e| ReadError::Io(e.to_string()))?;
            stamp.map_err(|e| ReadError::Io(e.to_string()))
        })
    }

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

    fn backup(
        &self,
        client: Client,
        plan: BackupPlan,
        control: JobControl,
        progress: watch::Sender<BackupProgress>,
    ) -> BoxFuture<BackupOutcome> {
        self.run(run_backup(client, plan, control, progress))
    }

    fn restore(
        &self,
        client: Client,
        job: RestoreJob,
        control: JobControl,
        progress: watch::Sender<RestoreProgress>,
    ) -> BoxFuture<RestoreOutcome> {
        self.run(run_restore(client, job, control, progress))
    }

    fn path_taken(&self, path: PathBuf) -> BoxFuture<bool> {
        self.run(async move {
            let read = tokio::task::spawn_blocking(move || std::fs::symlink_metadata(path).is_ok());
            // Unknown counts as taken: a backup never replaces anything.
            read.await.unwrap_or(true)
        })
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

fn manifest_problem_text(problem: &ManifestProblem) -> String {
    match problem {
        ManifestProblem::Unreadable(detail) => {
            t!("backup.manifest.unreadable", detail = detail)
        }
        ManifestProblem::Format(format) => t!("backup.manifest.format", format = format),
        ManifestProblem::TooManyCollections => t!("backup.manifest.too_many_collections"),
        ManifestProblem::TooManyFiles => t!("backup.manifest.too_many_files"),
        ManifestProblem::BadPath(path) => t!("backup.manifest.bad_path", path = path),
        ManifestProblem::DuplicatePath(path) => t!("backup.manifest.duplicate_path", path = path),
        ManifestProblem::BadDigest(path) => t!("backup.manifest.bad_digest", path = path),
        ManifestProblem::DuplicateFile(file) => t!("backup.manifest.duplicate_file", file = file),
        ManifestProblem::MissingFile(path) => t!("backup.manifest.missing_file", path = path),
    }
    .to_string()
}

/// The sentence for a backup that cannot be packed, unpacked or verified.
pub fn archive_error_text(err: &ArchiveError) -> String {
    match err {
        ArchiveError::Stopped => t!("backup.archive.stopped"),
        ArchiveError::SymlinkTarget(path) => {
            t!("backup.archive.link", path = path.display().to_string())
        }
        ArchiveError::PartExists(path) => {
            t!("backup.target_exists", path = path.display().to_string())
        }
        ArchiveError::ManifestNotFirst => t!("backup.archive.manifest_not_first"),
        ArchiveError::ManifestTooLarge => t!("backup.archive.manifest_too_large"),
        ArchiveError::Manifest(problem) => {
            return manifest_problem_text(problem);
        }
        ArchiveError::NotRegular(path) => t!("backup.archive.not_regular", path = path),
        ArchiveError::Unlisted(path) => t!("backup.archive.unlisted", path = path),
        ArchiveError::Duplicate(path) => t!("backup.archive.duplicate", path = path),
        ArchiveError::TooLong(path) => t!("backup.archive.too_long", path = path),
        ArchiveError::TooShort(path) => t!("backup.archive.too_short", path = path),
        ArchiveError::Missing(path) => t!("backup.archive.missing", path = path),
        ArchiveError::Digest(path) => t!("backup.archive.digest", path = path),
        ArchiveError::Io(detail) => t!("backup.archive.io", detail = detail),
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
