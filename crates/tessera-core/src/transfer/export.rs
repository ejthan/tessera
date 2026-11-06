// SPDX-License-Identifier: GPL-3.0-or-later

//! Writes documents to a file as JSONL, a JSON array or CSV, replacing the target only when
//! the export is complete.

use std::collections::HashMap;
use std::future::Future;
use std::mem;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tessera_client::{Client, Error, ExportQuery};
use tokio::io::{AsyncWriteExt as _, BufWriter};
use tokio::sync::watch;

use super::import::{JobControl, JobEnd, Phase, TransferError};
use super::report::PartFile;
use super::source::ReadError;
use super::{CSV_HEADER_SAMPLE, REPORT_PREVIEW_BYTES};
use crate::documents::ID_FILTER_BYTES;
use crate::documents::export::csv_field;
use crate::documents::ids::id_batches;

/// Progress is published after this many documents.
const PROGRESS_EVERY: u64 = 256;

/// Which documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportScope {
    /// All of them.
    All,
    /// Those matching a `filter_by`.
    Filter(String),
    /// These ids.
    Selection(Vec<String>),
}

/// How the file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// One document per line, as the server wrote it.
    Jsonl,
    /// One JSON array.
    JsonArray,
    /// RFC 4180 with nested keys as `a.b`.
    Csv,
}

/// Which top-level fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldPick {
    /// All of them.
    All,
    /// Only these.
    Include(Vec<String>),
    /// All but these.
    Exclude(Vec<String>),
}

/// What to export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportJob {
    /// The collection.
    pub collection: String,
    /// Which documents.
    pub scope: ExportScope,
    /// Which fields.
    pub fields: FieldPick,
    /// The format.
    pub format: ExportFormat,
    /// The file to write.
    pub target: PathBuf,
    /// How many documents the count said, for comparing at the end.
    pub expected: Option<u64>,
}

/// How far an export is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportProgress {
    /// The phase.
    pub phase: Phase,
    /// Documents written.
    pub docs: u64,
    /// Documents expected.
    pub expected: Option<u64>,
    /// Bytes written.
    pub bytes: u64,
    /// Time since the start.
    pub elapsed: Duration,
}

/// What an export did.
#[derive(Debug)]
pub struct ExportOutcome {
    /// How it ended.
    pub end: JobEnd<TransferError>,
    /// The final numbers.
    pub progress: ExportProgress,
    /// CSV values left out: their key was not in the header, or it repeated a key of the
    /// same document once flattened (`{"a.b":1,"a":{"b":2}}` keeps 1).
    pub unlisted_values: u64,
    /// Nothing was found, so the file that was there stays untouched.
    pub kept_existing: bool,
}

/// Whether `name` can be sent in `include_fields` or `exclude_fields`. A comma would split
/// it and `*` is a wildcard, and the schema lets both through.
pub fn field_safe(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name
            .chars()
            .any(|c| matches!(c, ',' | '`' | '*') || c.is_control())
}

/// `doc` as `a.b.c` keys in document order. Arrays stay as they are, and so does an object
/// with nothing in it.
pub fn flatten(doc: &Map<String, Value>) -> Vec<(String, Value)> {
    fn walk(prefix: &str, doc: &Map<String, Value>, out: &mut Vec<(String, Value)>) {
        for (key, value) in doc {
            let path = format!("{prefix}{key}");
            match value {
                Value::Object(inner) if !inner.is_empty() => {
                    walk(&format!("{path}."), inner, out);
                }
                _ => out.push((path, value.clone())),
            }
        }
    }
    let mut out = Vec::with_capacity(doc.len());
    walk("", doc, &mut out);
    out
}

/// The text of a cell, verbatim. A cell starting with `=` stays one, which a spreadsheet may
/// run as a formula: decision D55.
fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The CSV state: a sample until the header is known, then only the header.
#[derive(Default)]
struct Csv {
    sample: Vec<Vec<(String, Value)>>,
    /// Column of each header name; `None` while sampling.
    columns: Option<HashMap<String, usize>>,
    unlisted: u64,
}

impl Csv {
    fn line(cells: impl Iterator<Item = String>) -> String {
        let mut out = String::new();
        for (i, text) in cells.enumerate() {
            if i > 0 {
                out.push(',');
            }
            csv_field(&mut out, &text);
        }
        out.push('\n');
        out
    }

    fn row(&mut self, pairs: Vec<(String, Value)>) -> String {
        let columns = self.columns.as_ref().map_or(0, HashMap::len);
        let mut cells: Vec<Option<String>> = vec![None; columns];
        for (key, value) in pairs {
            match self.columns.as_ref().and_then(|c| c.get(&key)) {
                Some(&column) if cells[column].is_none() => cells[column] = Some(cell(&value)),
                _ => self.unlisted += 1,
            }
        }
        Self::line(cells.into_iter().map(Option::unwrap_or_default))
    }

    /// The header and the rows held back for it, or nothing when no document came.
    fn release(&mut self) -> String {
        let sample = mem::take(&mut self.sample);
        let mut names: Vec<String> = Vec::new();
        if sample.iter().flatten().any(|(key, _)| key == "id") {
            names.push("id".to_string());
        }
        let mut columns: HashMap<String, usize> = names.iter().cloned().zip(0..).collect();
        for (key, _) in sample.iter().flatten() {
            if !columns.contains_key(key) {
                columns.insert(key.clone(), names.len());
                names.push(key.clone());
            }
        }
        self.columns = Some(columns);
        let mut out = String::new();
        if !sample.is_empty() {
            out = Self::line(names.into_iter());
        }
        for pairs in sample {
            out.push_str(&self.row(pairs));
        }
        out
    }

    /// Takes the document at `number`, 1-based.
    fn add(&mut self, line: &str, number: u64) -> Result<String, TransferError> {
        let doc: Map<String, Value> = serde_json::from_str(line).map_err(|e| ReadError::Json {
            line: number,
            column: e.column() as u64,
            message: format!(
                "{e}: {}",
                &line[..line.floor_char_boundary(REPORT_PREVIEW_BYTES)]
            ),
        })?;
        let pairs = flatten(&doc);
        if self.columns.is_some() {
            return Ok(self.row(pairs));
        }
        self.sample.push(pairs);
        if self.sample.len() < CSV_HEADER_SAMPLE {
            return Ok(String::new());
        }
        Ok(self.release())
    }
}

/// Writes documents in the job's format and keeps the count.
struct Sink {
    format: ExportFormat,
    writer: BufWriter<tokio::fs::File>,
    csv: Csv,
    state: ExportProgress,
    started: Instant,
    progress: watch::Sender<ExportProgress>,
}

impl Sink {
    async fn write(&mut self, text: &str) -> Result<(), TransferError> {
        self.state.bytes += text.len() as u64;
        self.writer
            .write_all(text.as_bytes())
            .await
            .map_err(|e| TransferError::Io(e.to_string()))
    }

    async fn document(&mut self, line: &str) -> Result<(), TransferError> {
        match self.format {
            ExportFormat::Jsonl => {
                self.write(line).await?;
                self.write("\n").await?;
            }
            ExportFormat::JsonArray => {
                self.write(if self.state.docs == 0 { "[" } else { ",\n" })
                    .await?;
                self.write(line).await?;
            }
            ExportFormat::Csv => {
                let text = self.csv.add(line, self.state.docs + 1)?;
                self.write(&text).await?;
            }
        }
        self.state.docs += 1;
        if self.state.docs.is_multiple_of(PROGRESS_EVERY) {
            self.publish();
        }
        Ok(())
    }

    /// What is held back or still open in the format.
    async fn close(&mut self) -> Result<(), TransferError> {
        match self.format {
            ExportFormat::Jsonl => Ok(()),
            ExportFormat::JsonArray => {
                let end = if self.state.docs == 0 { "[]" } else { "]" };
                self.write(end).await
            }
            ExportFormat::Csv if self.csv.columns.is_none() => {
                let text = self.csv.release();
                self.write(&text).await
            }
            ExportFormat::Csv => Ok(()),
        }
    }

    fn publish(&mut self) {
        self.state.elapsed = self.started.elapsed();
        self.progress.send_replace(self.state.clone());
    }
}

/// How the writing loop ended, short of a failure.
enum Written {
    All,
    Stopped,
}

fn joined(names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| names.join(","))
}

/// `future`, unless `stop` comes first.
async fn or_stop<T>(control: &JobControl, future: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        () = control.stop.cancelled() => None,
        value = future => Some(value),
    }
}

fn project(doc: &mut Value, fields: &FieldPick) {
    let Some(map) = doc.as_object_mut() else {
        return;
    };
    match fields {
        FieldPick::All => {}
        FieldPick::Include(names) => map.retain(|key, _| names.contains(key)),
        FieldPick::Exclude(names) => map.retain(|key, _| !names.contains(key)),
    }
}

/// One stream into `sink`.
async fn stream(
    client: &Client,
    job: &ExportJob,
    filter_by: Option<String>,
    control: &JobControl,
    sink: &mut Sink,
) -> Result<Written, TransferError> {
    let (include_fields, exclude_fields) = match &job.fields {
        FieldPick::All => (None, None),
        FieldPick::Include(names) => (joined(names), None),
        FieldPick::Exclude(names) => (None, joined(names)),
    };
    let query = ExportQuery {
        filter_by,
        include_fields,
        exclude_fields,
    };
    let mut lines = tokio::select! {
        biased;
        () = control.stop.cancelled() => return Ok(Written::Stopped),
        lines = client.export_stream(&job.collection, &query) => lines?,
    };
    loop {
        let line = tokio::select! {
            biased;
            () = control.stop.cancelled() => return Ok(Written::Stopped),
            line = lines.next_line() => line?,
        };
        match line {
            Some(line) => sink.document(&line).await?,
            None => return Ok(Written::All),
        }
    }
}

async fn write_all(
    client: &Client,
    job: &ExportJob,
    control: &JobControl,
    sink: &mut Sink,
) -> Result<Written, TransferError> {
    match &job.scope {
        ExportScope::All => stream(client, job, None, control, sink).await,
        ExportScope::Filter(filter) => {
            stream(client, job, Some(filter.clone()), control, sink).await
        }
        ExportScope::Selection(ids) => {
            let batches = id_batches(ids, ID_FILTER_BYTES);
            for filter in batches.filters {
                let written = stream(client, job, Some(filter), control, sink).await?;
                if matches!(written, Written::Stopped) {
                    return Ok(written);
                }
            }
            for id in &batches.single {
                let Some(found) = or_stop(control, client.document(&job.collection, id)).await
                else {
                    return Ok(Written::Stopped);
                };
                let mut doc = match found {
                    // Only a document that went is skipped, not all of them with the collection.
                    Err(Error::Http { status: 404, .. }) => {
                        match or_stop(control, client.collection(&job.collection)).await {
                            Some(collection) => collection?,
                            None => return Ok(Written::Stopped),
                        };
                        continue;
                    }
                    other => other?,
                };
                project(&mut doc, &job.fields);
                sink.document(&doc.to_string()).await?;
            }
            Ok(Written::All)
        }
    }
}

/// One export from start to end. An existing target stays as it was unless the whole export
/// succeeded.
pub async fn run_export(
    client: Client,
    job: ExportJob,
    control: JobControl,
    progress: watch::Sender<ExportProgress>,
) -> ExportOutcome {
    let started = Instant::now();
    let expected = match &job.scope {
        ExportScope::Selection(ids) => Some(ids.len() as u64),
        _ => job.expected,
    };
    let state = ExportProgress {
        expected,
        ..ExportProgress::default()
    };
    let failed = |e: TransferError, state: ExportProgress, unlisted_values| ExportOutcome {
        end: JobEnd::Failed(e),
        progress: state,
        unlisted_values,
        kept_existing: false,
    };

    let names = match &job.fields {
        FieldPick::All => &[][..],
        FieldPick::Include(names) | FieldPick::Exclude(names) => names,
    };
    if let Some(name) = names.iter().find(|name| !field_safe(name)) {
        return failed(TransferError::UnsafeField(name.clone()), state, 0);
    }
    // `include_fields=` would answer with every field.
    if job.fields == FieldPick::Include(Vec::new()) {
        return failed(TransferError::UnsafeField(String::new()), state, 0);
    }
    let (part, file) = match PartFile::create(&job.target, false).await {
        Ok(created) => created,
        Err(e) => return failed(TransferError::Io(e.to_string()), state, 0),
    };

    let mut sink = Sink {
        format: job.format,
        writer: BufWriter::with_capacity(64 * 1024, file),
        csv: Csv::default(),
        state,
        started,
        progress: progress.clone(),
    };
    sink.publish();
    let mut written = write_all(&client, &job, &control, &mut sink).await;
    if matches!(written, Ok(Written::All)) {
        written = sink.close().await.map(|()| Written::All);
    }
    let mut kept_existing = false;
    let end = match written {
        Ok(Written::Stopped) => {
            sink.state.phase = Phase::Stopping;
            JobEnd::Stopped
        }
        Err(e) => JobEnd::Failed(e),
        // Nothing found is no reason to lose the file that is there.
        Ok(Written::All) if sink.state.docs == 0 && part.replaces() => {
            drop(sink.writer);
            drop(part);
            kept_existing = true;
            JobEnd::Finished
        }
        Ok(Written::All) => match part.persist(sink.writer, &job.target).await {
            Ok(()) => JobEnd::Finished,
            Err(e) => JobEnd::Failed(TransferError::Io(e.to_string())),
        },
    };
    let mut state = sink.state;
    state.elapsed = started.elapsed();
    progress.send_replace(state.clone());
    ExportOutcome {
        end,
        progress: state,
        unlisted_values: sink.csv.unlisted,
        kept_existing,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn field_safe_table() {
        for good in ["a", "a.b", "ü b", "a-b_c"] {
            assert!(field_safe(good), "{good}");
        }
        for bad in ["", " a", "a ", "a,b", "a`b", "x*", "a\nb", "a\u{7f}"] {
            assert!(!field_safe(bad), "{bad:?}");
        }
    }

    fn flat(doc: Value) -> Vec<(String, Value)> {
        flatten(doc.as_object().unwrap_or(&Map::new()))
    }

    #[test]
    fn flatten_table() {
        assert_eq!(
            flat(json!({"a":{"b":{"c":1}},"d":2})),
            [("a.b.c".to_string(), json!(1)), ("d".to_string(), json!(2))]
        );
        assert_eq!(
            flat(json!({"t":[1,{"x":2}]})),
            [("t".to_string(), json!([1, {"x":2}]))]
        );
        assert_eq!(flat(json!({"n":null})), [("n".to_string(), Value::Null)]);
        assert_eq!(flat(json!({"o":{}})), [("o".to_string(), json!({}))]);
        assert_eq!(flat(json!({})), []);
    }

    #[test]
    fn cells() {
        assert_eq!(cell(&Value::Null), "");
        assert_eq!(cell(&json!("x")), "x");
        assert_eq!(cell(&json!(1.5)), "1.5");
        assert_eq!(cell(&json!(["p"])), "[\"p\"]");
    }
}
