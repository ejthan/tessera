// SPDX-License-Identifier: GPL-3.0-or-later

//! What an import could not store: a bounded table in memory, every row in a private spool.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _, BufReader, BufWriter,
};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{REPORT_PREVIEW_BYTES, REPORT_ROWS};

/// One document that did not make it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRow {
    /// Where it is in the source.
    pub line: u64,
    /// The server's code; `None` when the server gave none.
    pub code: Option<u16>,
    /// Why.
    pub error: String,
    /// The `id` of the line that was sent, if it has one. The server's echo is no source for
    /// this: on a line it cannot read it repeats the id of the document before.
    pub id: Option<String>,
    /// The text as it was sent.
    pub raw: String,
}

/// A file beside `target` that replaces it only once it is complete. Dropped unpersisted,
/// it is removed.
pub(super) struct PartFile {
    path: PathBuf,
    persisted: bool,
}

impl PartFile {
    /// Creates the file; it must not exist.
    pub(super) async fn create(target: &Path) -> io::Result<(Self, tokio::fs::File)> {
        let Some(name) = target.file_name() else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        let mut part = name.to_os_string();
        part.push(format!(
            ".{}.part",
            &Uuid::new_v4().simple().to_string()[..8]
        ));
        let path = target.with_file_name(part);
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        let guard = Self {
            path,
            persisted: false,
        };
        Ok((guard, file))
    }

    /// Flushes `writer` to the disk and moves the file onto `target`.
    pub(super) async fn persist(
        mut self,
        mut writer: BufWriter<tokio::fs::File>,
        target: &Path,
    ) -> io::Result<()> {
        writer.flush().await?;
        writer.get_ref().sync_all().await?;
        drop(writer);
        tokio::fs::rename(&self.path, target).await?;
        self.persisted = true;
        Ok(())
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if !self.persisted {
            // Nothing left to tell anyone: the job already ended with its own error.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The failed rows of one job. `Send + Sync`: shared as `Arc<Report>` once the job ended.
#[derive(Debug)]
pub struct Report {
    rows: Vec<ReportRow>,
    total: u64,
    /// `None` when no spool could be made; then no row was added either.
    spool: Option<Spool>,
    /// Clones of a file share one position, so saves take turns.
    saving: Mutex<()>,
}

#[derive(Debug)]
struct Spool {
    file: std::fs::File,
    writer: BufWriter<tokio::fs::File>,
}

impl Report {
    /// A report with an empty spool. The file has no name, so it goes with the process.
    pub async fn new() -> io::Result<Self> {
        let file = tokio::task::spawn_blocking(tempfile::tempfile)
            .await
            .map_err(io::Error::other)??;
        let writer = BufWriter::new(tokio::fs::File::from_std(file.try_clone()?));
        Ok(Self {
            spool: Some(Spool { file, writer }),
            ..Self::without_spool()
        })
    }

    /// The report of a job that could not start.
    pub(super) fn without_spool() -> Self {
        Self {
            rows: Vec::new(),
            total: 0,
            spool: None,
            saving: Mutex::new(()),
        }
    }

    /// Adds a row to the spool and, while the table has room, to the table.
    pub(super) async fn add(&mut self, row: ReportRow) -> io::Result<()> {
        let spool = self.spool.as_mut().ok_or_else(no_spool)?;
        let mut line = serde_json::to_vec(&row)?;
        line.push(b'\n');
        spool.writer.write_all(&line).await?;
        self.total += 1;
        if self.rows.len() < REPORT_ROWS {
            let mut row = row;
            row.raw
                .truncate(row.raw.floor_char_boundary(REPORT_PREVIEW_BYTES));
            self.rows.push(row);
        }
        Ok(())
    }

    /// Writes out what `add` buffered; saving sees only what is flushed.
    pub(super) async fn finish(&mut self) -> io::Result<()> {
        match &mut self.spool {
            Some(spool) => spool.writer.flush().await,
            None => Ok(()),
        }
    }

    /// The first [`REPORT_ROWS`] rows, with `raw` cut to [`REPORT_PREVIEW_BYTES`].
    pub fn rows(&self) -> &[ReportRow] {
        &self.rows
    }

    /// All rows, also those only in the spool.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Saves every row as a line of JSON; returns how many.
    pub async fn save_report(&self, path: &Path) -> Result<u64, io::Error> {
        self.save(path, |line| {
            let mut out = line.into_bytes();
            out.push(b'\n');
            Ok(out)
        })
        .await
    }

    /// Saves the failed documents as they were sent, one per line; returns how many.
    pub async fn save_failed(&self, path: &Path) -> Result<u64, io::Error> {
        self.save(path, |line| {
            let row: ReportRow = serde_json::from_str(&line)?;
            let mut out = row.raw.into_bytes();
            out.push(b'\n');
            Ok(out)
        })
        .await
    }

    async fn save(
        &self,
        path: &Path,
        render: impl Fn(String) -> io::Result<Vec<u8>>,
    ) -> Result<u64, io::Error> {
        let _turn = self.saving.lock().await;
        let spool = self.spool.as_ref().ok_or_else(no_spool)?;
        let mut source = tokio::fs::File::from_std(spool.file.try_clone()?);
        source.seek(io::SeekFrom::Start(0)).await?;
        let (part, file) = PartFile::create(path).await?;
        let mut writer = BufWriter::new(file);
        let mut lines = BufReader::new(source).lines();
        let mut count = 0;
        while let Some(line) = lines.next_line().await? {
            writer.write_all(&render(line)?).await?;
            count += 1;
        }
        part.persist(writer, path).await?;
        Ok(count)
    }
}

fn no_spool() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "the report has no spool")
}

#[cfg(test)]
mod tests {
    // Tests unwrap; a panic is the failure we want.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn row(line: u64, raw: &str) -> ReportRow {
        ReportRow {
            line,
            code: Some(400),
            error: "bad".to_string(),
            id: None,
            raw: raw.to_string(),
        }
    }

    fn is_send_sync<T: Send + Sync>() {}

    #[tokio::test]
    async fn spool_holds_more_than_the_preview() {
        is_send_sync::<Report>();
        let mut report = Report::new().await.unwrap();
        for i in 0..REPORT_ROWS as u64 + 5 {
            report.add(row(i, "x")).await.unwrap();
        }
        report.finish().await.unwrap();
        assert_eq!(report.rows().len(), REPORT_ROWS);
        assert_eq!(report.total(), REPORT_ROWS as u64 + 5);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.jsonl");
        assert_eq!(
            report.save_report(&path).await.unwrap(),
            REPORT_ROWS as u64 + 5
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let last: ReportRow = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last.line, REPORT_ROWS as u64 + 4);
    }

    #[tokio::test]
    async fn raw_is_cut_on_a_char_boundary() {
        let mut report = Report::new().await.unwrap();
        let long = "é".repeat(REPORT_PREVIEW_BYTES);
        report.add(row(1, &long)).await.unwrap();
        report.finish().await.unwrap();
        let shown = &report.rows()[0].raw;
        assert!(shown.len() <= REPORT_PREVIEW_BYTES);
        assert!(long.starts_with(shown.as_str()) && !shown.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failed.jsonl");
        report.save_failed(&path).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{long}\n"),
            "the spool keeps the whole line"
        );
    }

    #[tokio::test]
    async fn saves_twice() {
        let mut report = Report::new().await.unwrap();
        report.add(row(1, "a")).await.unwrap();
        report.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        assert_eq!(report.save_report(&path).await.unwrap(), 1);
        let first = std::fs::read(&path).unwrap();
        assert_eq!(report.save_report(&path).await.unwrap(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), first);
    }

    #[tokio::test]
    async fn save_failed_writes_raw_lines() {
        let mut report = Report::new().await.unwrap();
        report.add(row(1, "{\"a\":1}")).await.unwrap();
        report.add(row(3, "not json")).await.unwrap();
        report.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failed.jsonl");
        assert_eq!(report.save_failed(&path).await.unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"a\":1}\nnot json\n"
        );
    }

    #[tokio::test]
    async fn failed_save_leaves_no_part_file() {
        let mut report = Report::new().await.unwrap();
        report.add(row(1, "a")).await.unwrap();
        report.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        // A file cannot be renamed onto a directory that exists.
        let target = dir.path().join("taken");
        std::fs::create_dir(&target).unwrap();
        assert!(report.save_report(&target).await.is_err());
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["taken"]);
    }
}
