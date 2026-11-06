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

use super::{ERROR_BYTES, REPORT_PREVIEW_BYTES, REPORT_ROWS};

/// Starts a line of a saved failed file whose text is not the source's.
pub const LOSSY_MARK: &str = "# changed or cut, fix it in the source:";

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
    /// `raw` is not what the source holds, because it was not UTF-8 or was cut: it never
    /// reached the server and must not by way of a saved file, an id could change.
    #[serde(default)]
    pub lossy: bool,
}

/// A file beside `target` that replaces it only once it is complete. Dropped unpersisted,
/// it is removed.
pub(super) struct PartFile {
    path: PathBuf,
    persisted: bool,
    replaces: bool,
}

impl PartFile {
    /// Creates the file. It takes the permissions of an existing target, or with `private`
    /// is for the owner only, or else follows the umask. A symbolic link is refused: the
    /// rename would replace the link, not what it points at.
    pub(super) async fn create(
        target: &Path,
        private: bool,
    ) -> io::Result<(Self, tokio::fs::File)> {
        let Some(name) = target.file_name() else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        let existing = match tokio::fs::symlink_metadata(target).await {
            Ok(meta) if meta.is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is a symbolic link", target.display()),
                ));
            }
            Ok(meta) => Some(meta.permissions()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let mut part = name.to_os_string();
        part.push(format!(
            ".{}.part",
            &Uuid::new_v4().simple().to_string()[..8]
        ));
        let path = target.with_file_name(part);
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // Never wider than the target, not even until the permissions are copied.
        #[cfg(unix)]
        if private || existing.is_some() {
            options.mode(0o600);
        }
        let file = options.open(&path).await?;
        let guard = Self {
            path,
            persisted: false,
            replaces: existing.is_some(),
        };
        if let Some(permissions) = existing {
            file.set_permissions(permissions).await?;
        }
        Ok((guard, file))
    }

    /// Whether a file stands at the target.
    pub(super) fn replaces(&self) -> bool {
        self.replaces
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

/// Writes `text` to `path`, for the owner only: an existing file is replaced only once the new
/// one is complete on disk.
pub async fn write_text(path: &Path, text: &str) -> io::Result<()> {
    let (part, file) = PartFile::create(path, true).await?;
    let mut writer = BufWriter::new(file);
    writer.write_all(text.as_bytes()).await?;
    part.persist(writer, path).await
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

    /// Adds a row to the spool and, while the table has room, to the table, with `error`
    /// cut to [`ERROR_BYTES`].
    pub async fn add(&mut self, mut row: ReportRow) -> io::Result<()> {
        row.error
            .truncate(row.error.floor_char_boundary(ERROR_BYTES));
        let spool = self.spool.as_mut().ok_or_else(no_spool)?;
        let mut line = serde_json::to_vec(&row)?;
        line.push(b'\n');
        spool.writer.write_all(&line).await?;
        self.total += 1;
        if self.rows.len() < REPORT_ROWS {
            row.raw
                .truncate(row.raw.floor_char_boundary(REPORT_PREVIEW_BYTES));
            self.rows.push(row);
        }
        Ok(())
    }

    /// Writes out what `add` buffered; saving sees only what is flushed.
    pub async fn finish(&mut self) -> io::Result<()> {
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

    /// Saves the failed documents as they were sent, one per line; returns how many. A
    /// `lossy` row starts with [`LOSSY_MARK`] and cannot be imported.
    pub async fn save_failed(&self, path: &Path) -> Result<u64, io::Error> {
        self.save(path, |line| {
            let row: ReportRow = serde_json::from_str(&line)?;
            let text = if row.lossy {
                // Not JSON, so a re-import reports it instead of storing a changed id.
                let raw = row.raw.replace(['\r', '\n'], " ");
                format!("{LOSSY_MARK} line {}: {raw}\n", row.line)
            } else {
                format!("{}\n", row.raw)
            };
            Ok(text.into_bytes())
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
        let (part, file) = PartFile::create(path, true).await?;
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

    #[tokio::test]
    async fn write_text_replaces_a_file_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("set.csv");
        write_text(&path, "one").await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        write_text(&path, "two").await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_write_text_leaves_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real");
        std::fs::write(&target, "kept").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_text(&link, "new").await.is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept");
        let missing = dir.path().join("no_dir").join("file");
        assert!(write_text(&missing, "new").await.is_err());
    }

    fn row(line: u64, raw: &str) -> ReportRow {
        ReportRow {
            line,
            code: Some(400),
            error: "bad".to_string(),
            id: None,
            raw: raw.to_string(),
            lossy: false,
        }
    }

    fn is_send_sync<T: Send + Sync>() {}

    #[tokio::test]
    async fn error_is_capped_in_table_and_spool() {
        let mut report = Report::new().await.unwrap();
        let mut big = row(1, "x");
        big.error = "é".repeat(ERROR_BYTES);
        report.add(big).await.unwrap();
        report.finish().await.unwrap();
        assert!(report.rows()[0].error.len() <= ERROR_BYTES);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.jsonl");
        report.save_report(&path).await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let saved: ReportRow = serde_json::from_str(text.trim()).unwrap();
        assert!(saved.error.len() <= ERROR_BYTES);
    }

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
    async fn lossy_rows_cannot_be_imported_from_a_saved_file() {
        let mut report = Report::new().await.unwrap();
        let mut bad = row(4, "{\"id\":\"bad\u{fffd}\"}");
        bad.lossy = true;
        report.add(bad).await.unwrap();
        report.add(row(5, "{\"id\":\"ok\"}")).await.unwrap();
        report.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failed.jsonl");
        assert_eq!(report.save_failed(&path).await.unwrap(), 2);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with(LOSSY_MARK), "{}", lines[0]);
        assert!(lines[0].contains("line 4"), "{}", lines[0]);
        assert!(serde_json::from_str::<serde_json::Value>(lines[0]).is_err());
        assert_eq!(lines[1], "{\"id\":\"ok\"}");

        let report_file = dir.path().join("report.jsonl");
        report.save_report(&report_file).await.unwrap();
        let text = std::fs::read_to_string(&report_file).unwrap();
        let saved: ReportRow = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert!(saved.lossy);
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn saves_are_private_unless_the_target_says_otherwise() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut report = Report::new().await.unwrap();
        report.add(row(1, "a")).await.unwrap();
        report.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new.jsonl");
        report.save_report(&new).await.unwrap();
        assert_eq!(mode(&new), 0o600);

        let shared = dir.path().join("shared.jsonl");
        std::fs::write(&shared, "old").unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o640)).unwrap();
        report.save_failed(&shared).await.unwrap();
        assert_eq!(mode(&shared), 0o640);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symbolic_link_is_refused() {
        let mut report = Report::new().await.unwrap();
        report.add(row(1, "a")).await.unwrap();
        report.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, "old").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let error = report.save_report(&link).await.unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "old");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
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
