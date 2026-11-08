// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads documents from a file or from memory as a stream of records, whatever the format.
//!
//! Blocking: callers run [`read_documents`] in `spawn_blocking`. A document is never parsed
//! for JSONL, so the server sees the line as it was written.

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use serde::de::{self, Deserializer as _, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::MAX_DOC_BYTES;
use super::csv_cells::{convert, guess, nest};

/// Where documents come from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A file, read as it goes.
    File(PathBuf),
    /// Text already in memory, such as the clipboard.
    Bytes(Arc<[u8]>),
}

/// How the documents are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON document per line.
    Jsonl,
    /// One JSON array of documents.
    JsonArray,
    /// Delimited text, one document per row.
    Csv,
}

/// How a CSV file is split.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvOptions {
    /// The byte between cells.
    pub delimiter: u8,
    /// Whether the first row names the keys.
    pub header: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            header: true,
        }
    }
}

/// What [`read_documents`] needs to know besides the source.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOptions {
    /// The format.
    pub format: Format,
    /// CSV settings.
    pub csv: CsvOptions,
    /// Field names and types of the target schema, in order: they type CSV cells, and name
    /// the columns of a CSV without header.
    pub fields: Vec<(String, String)>,
}

impl ReadOptions {
    /// Options for `format` with default CSV settings and no schema.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            csv: CsvOptions::default(),
            fields: Vec::new(),
        }
    }
}

/// One document read from the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// 1-based line for JSONL and CSV (where the record starts), element number for a JSON
    /// array. Blank JSONL lines count.
    pub line: u64,
    /// Bytes of the source up to the end of this record (of the element, for a JSON array).
    pub offset: u64,
    /// The document as one line of JSON text, or why it cannot be sent.
    pub doc: Result<String, Rejected>,
}

/// A record that must not reach the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejected {
    /// The text, lossy and cut to `REPORT_PREVIEW_BYTES`.
    pub raw: String,
    /// Why.
    pub reason: RejectReason,
}

/// Why a record was refused locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Not UTF-8: the server would store it with changed characters (an id changes).
    InvalidUtf8,
    /// Longer than `MAX_DOC_BYTES`; holds its length in bytes.
    TooLarge(u64),
}

/// Why reading stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The file could not be read.
    #[error("cannot read the source: {0}")]
    Io(String),
    /// The JSON array is broken, or the top level is no array.
    #[error("invalid JSON at line {line}, column {column}: {message}")]
    Json {
        /// 1-based line.
        line: u64,
        /// 1-based column.
        column: u64,
        /// What is wrong.
        message: String,
    },
    /// The CSV cannot be read, for example a repeated header name.
    #[error("invalid CSV at line {line}: {message}")]
    Csv {
        /// 1-based line.
        line: u64,
        /// What is wrong.
        message: String,
    },
    /// A CSV record is longer than `MAX_DOC_BYTES`, which a CSV cannot skip: an unclosed
    /// quote makes the rest of the file one record.
    #[error("the record at line {line} is larger than {} MiB", MAX_DOC_BYTES >> 20)]
    RecordTooLarge {
        /// Where the record starts.
        line: u64,
    },
    /// An element of a JSON array is longer than `MAX_DOC_BYTES`.
    #[error("element {element} is larger than {} MiB", MAX_DOC_BYTES >> 20)]
    ElementTooLarge {
        /// 1-based element number.
        element: u64,
    },
}

/// The format and CSV delimiter of a file, from its name and its first bytes. The dialog can
/// override both.
pub fn sniff(path: Option<&Path>, head: &[u8]) -> (Format, CsvOptions) {
    let extension = path
        .and_then(|p| p.extension())
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("csv") => return (Format::Csv, CsvOptions::default()),
        Some("tsv") => {
            let csv = CsvOptions {
                delimiter: b'\t',
                ..CsvOptions::default()
            };
            return (Format::Csv, csv);
        }
        _ => {}
    }
    let head = head.strip_prefix(BOM).unwrap_or(head);
    let format = match head.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'[') => Format::JsonArray,
        _ => Format::Jsonl,
    };
    (format, CsvOptions::default())
}

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// The bytes the CSV reader has been given and not yet finished with, to number lines:
/// `csv` reports the line before it skips blank lines, and half a CRLF behind.
#[derive(Default)]
struct LineTail {
    bytes: Vec<u8>,
    /// Index in `bytes` of the first byte not yet accounted for.
    head: usize,
    /// Source offset of `bytes[head]`.
    offset: u64,
    /// Line (1-based) of `bytes[head]`.
    line: u64,
}

impl LineTail {
    fn new() -> Self {
        Self {
            line: 1,
            ..Self::default()
        }
    }

    /// The line of the first byte after the line breaks at the head: where the next record
    /// starts.
    fn start_line(&self) -> u64 {
        let breaks = self.bytes[self.head..]
            .iter()
            .take_while(|b| matches!(**b, b'\r' | b'\n'))
            .filter(|b| **b == b'\n')
            .count();
        self.line + breaks as u64
    }

    /// Accounts for the bytes up to source offset `end`.
    fn advance_to(&mut self, end: u64) {
        let n = (end.saturating_sub(self.offset) as usize).min(self.bytes.len() - self.head);
        let passed = &self.bytes[self.head..self.head + n];
        self.line += passed.iter().filter(|b| **b == b'\n').count() as u64;
        self.head += n;
        self.offset += n as u64;
        if self.head > 64 * 1024 && self.head * 2 > self.bytes.len() {
            self.bytes.drain(..self.head);
            self.head = 0;
        }
    }
}

/// Counts the bytes that pass, and refuses to go on once a record is longer than
/// `MAX_DOC_BYTES`: an unclosed CSV quote would otherwise swallow the whole file.
struct RecordLimit<R> {
    inner: R,
    since_reset: Rc<Cell<u64>>,
    total: Rc<Cell<u64>>,
    tripped: Rc<Cell<bool>>,
    tail: Option<Rc<RefCell<LineTail>>>,
}

/// The handles on a [`RecordLimit`] that the reader keeps.
#[derive(Clone)]
struct Counters {
    since_reset: Rc<Cell<u64>>,
    total: Rc<Cell<u64>>,
    tripped: Rc<Cell<bool>>,
    /// Only kept for CSV.
    tail: Option<Rc<RefCell<LineTail>>>,
}

impl Counters {
    fn wrap<R>(inner: R, number_lines: bool) -> (RecordLimit<R>, Self) {
        let counters = Self {
            since_reset: Rc::default(),
            total: Rc::default(),
            tripped: Rc::default(),
            tail: number_lines.then(|| Rc::new(RefCell::new(LineTail::new()))),
        };
        let limit = RecordLimit {
            inner,
            since_reset: counters.since_reset.clone(),
            total: counters.total.clone(),
            tripped: counters.tripped.clone(),
            tail: counters.tail.clone(),
        };
        (limit, counters)
    }

    /// Called after every record.
    fn reset(&self) {
        self.since_reset.set(0);
    }
}

impl<R: Read> Read for RecordLimit<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(tail) = &self.tail {
            tail.borrow_mut().bytes.extend_from_slice(&buf[..n]);
        }
        self.total.set(self.total.get() + n as u64);
        let size = self.since_reset.get() + n as u64;
        self.since_reset.set(size);
        if size > MAX_DOC_BYTES as u64 {
            self.tripped.set(true);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "record too large",
            ));
        }
        Ok(n)
    }
}

/// Reads every document of `source`, handing each to `sink` until it breaks or the source
/// ends. Records come in source order; an error ends the read after the records before it.
pub fn read_documents(
    source: &Source,
    options: &ReadOptions,
    sink: impl FnMut(Record) -> ControlFlow<()>,
) -> Result<(), ReadError> {
    match source {
        Source::File(path) => {
            let file = File::open(path).map_err(|e| ReadError::Io(e.to_string()))?;
            read_from(file, options, sink)
        }
        Source::Bytes(bytes) => read_from(Cursor::new(bytes.clone()), options, sink),
    }
}

fn read_from(
    reader: impl Read,
    options: &ReadOptions,
    sink: impl FnMut(Record) -> ControlFlow<()>,
) -> Result<(), ReadError> {
    match options.format {
        Format::Jsonl => read_jsonl(BufReader::with_capacity(64 * 1024, reader), sink),
        Format::JsonArray => read_json_array(BufReader::with_capacity(64 * 1024, reader), sink),
        Format::Csv => {
            let (limited, counters) = Counters::wrap(reader, true);
            read_csv(limited, &counters, options, sink)
        }
    }
}

/// `bytes` as lossy text of at most `REPORT_PREVIEW_BYTES` bytes.
fn lossy_head(bytes: &[u8]) -> String {
    let head = &bytes[..bytes.len().min(super::REPORT_PREVIEW_BYTES)];
    String::from_utf8_lossy(head).into_owned()
}

fn read_jsonl(
    mut reader: impl BufRead,
    mut sink: impl FnMut(Record) -> ControlFlow<()>,
) -> Result<(), ReadError> {
    let io_err = |e: io::Error| ReadError::Io(e.to_string());
    let (mut line, mut offset) = (0u64, 0u64);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // The line's length without its newline, and whether `buf` stopped growing.
        let (mut length, mut overflow, mut seen) = (0u64, false, false);
        loop {
            let chunk = reader.fill_buf().map_err(io_err)?;
            if chunk.is_empty() {
                break;
            }
            seen = true;
            let newline = chunk.iter().position(|&b| b == b'\n');
            let content = &chunk[..newline.unwrap_or(chunk.len())];
            length += content.len() as u64;
            if !overflow && buf.len() + content.len() > MAX_DOC_BYTES {
                overflow = true;
                buf.truncate(super::REPORT_PREVIEW_BYTES);
            }
            if !overflow {
                buf.extend_from_slice(content);
            }
            let used = newline.map_or(chunk.len(), |i| i + 1);
            reader.consume(used);
            offset += used as u64;
            if newline.is_some() {
                break;
            }
        }
        if !seen {
            return Ok(());
        }
        line += 1;
        let mut text: &[u8] = &buf;
        if line == 1 {
            text = text.strip_prefix(BOM).unwrap_or(text);
        }
        let text = text.trim_ascii();
        let doc = if overflow {
            Err(Rejected {
                raw: lossy_head(&buf),
                reason: RejectReason::TooLarge(length),
            })
        } else if text.is_empty() {
            continue;
        } else {
            match std::str::from_utf8(text) {
                Ok(text) => Ok(text.to_string()),
                Err(_) => Err(Rejected {
                    raw: lossy_head(text),
                    reason: RejectReason::InvalidUtf8,
                }),
            }
        };
        if sink(Record { line, offset, doc }).is_break() {
            return Ok(());
        }
    }
}

/// Hands the elements of a JSON array to the sink one by one.
struct Elements<'a, F> {
    sink: &'a mut F,
    counters: &'a Counters,
    stopped: &'a Cell<bool>,
    delivered: &'a Cell<u64>,
}

impl<'de, F: FnMut(Record) -> ControlFlow<()>> Visitor<'de> for Elements<'_, F> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an array of documents")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut index = 0;
        while let Some(value) = seq.next_element::<Value>()? {
            index += 1;
            self.delivered.set(index);
            let record = Record {
                line: index,
                offset: self.counters.total.get(),
                doc: Ok(value.to_string()),
            };
            self.counters.reset();
            if (self.sink)(record).is_break() {
                self.stopped.set(true);
                return Err(de::Error::custom("stopped"));
            }
        }
        Ok(())
    }
}

fn read_json_array(
    mut reader: impl BufRead,
    mut sink: impl FnMut(Record) -> ControlFlow<()>,
) -> Result<(), ReadError> {
    // serde_json refuses a byte order mark.
    let chunk = reader
        .fill_buf()
        .map_err(|e| ReadError::Io(e.to_string()))?;
    let bom = chunk.starts_with(BOM);
    if bom {
        reader.consume(BOM.len());
    }
    // Above the buffer, so `total` is what serde has consumed, not what was read ahead.
    let (reader, counters) = Counters::wrap(reader, false);
    let counters = &counters;
    counters.total.set(if bom { BOM.len() as u64 } else { 0 });
    let (stopped, delivered) = (Cell::new(false), Cell::new(0));
    let mut de = serde_json::Deserializer::from_reader(reader);
    let visitor = Elements {
        sink: &mut sink,
        counters,
        stopped: &stopped,
        delivered: &delivered,
    };
    let result = de.deserialize_seq(visitor).and_then(|()| de.end());
    match result {
        Ok(()) => Ok(()),
        Err(_) if stopped.get() => Ok(()),
        Err(_) if counters.tripped.get() => Err(ReadError::ElementTooLarge {
            element: delivered.get() + 1,
        }),
        Err(e) if e.is_io() => Err(ReadError::Io(e.to_string())),
        Err(e) => Err(ReadError::Json {
            line: e.line() as u64,
            column: e.column() as u64,
            message: e.to_string(),
        }),
    }
}

fn read_csv(
    reader: impl Read,
    counters: &Counters,
    options: &ReadOptions,
    mut sink: impl FnMut(Record) -> ControlFlow<()>,
) -> Result<(), ReadError> {
    let mut csv = csv::ReaderBuilder::new()
        .delimiter(options.csv.delimiter)
        .has_headers(options.csv.header)
        .flexible(true)
        .from_reader(reader);
    let tail = counters.tail.clone().expect("CSV numbers its lines");
    let csv_err = |e: csv::Error, counters: &Counters| {
        let line = tail.borrow().start_line();
        match e.kind() {
            _ if counters.tripped.get() => ReadError::RecordTooLarge { line },
            csv::ErrorKind::Io(io) => ReadError::Io(io.to_string()),
            _ => ReadError::Csv {
                line,
                message: e.to_string(),
            },
        }
    };
    let mut names: Vec<String> = Vec::new();
    if options.csv.header {
        let header = csv.byte_headers().map_err(|e| csv_err(e, counters))?;
        for cell in header.iter() {
            let name = std::str::from_utf8(cell).map_err(|_| ReadError::Csv {
                line: 1,
                message: "the header is not UTF-8".into(),
            })?;
            if !name.is_empty() && names.iter().any(|n| n == name) {
                return Err(ReadError::Csv {
                    line: 1,
                    message: format!("the header names {name} twice"),
                });
            }
            names.push(name.to_string());
        }
    } else {
        names = options
            .fields
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
    }
    tail.borrow_mut().advance_to(csv.position().byte());
    counters.reset();
    let type_of = |name: &str| {
        options
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, ty)| ty.as_str())
    };
    let mut row = csv::ByteRecord::new();
    loop {
        match csv.read_byte_record(&mut row) {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(e) => return Err(csv_err(e, counters)),
        }
        counters.reset();
        let offset = csv.position().byte();
        let line = tail.borrow().start_line();
        tail.borrow_mut().advance_to(offset);
        let mut doc = Map::new();
        let mut invalid = false;
        for (i, cell) in row.iter().enumerate() {
            let Ok(cell) = std::str::from_utf8(cell) else {
                invalid = true;
                break;
            };
            let name = match names.get(i) {
                Some(name) if !name.is_empty() => name.clone(),
                _ => format!("column_{}", i + 1),
            };
            let value = if name == "id" {
                (!cell.is_empty()).then(|| Value::String(cell.to_string()))
            } else {
                match type_of(&name) {
                    Some(ty) => convert(cell, Some(ty)),
                    None => (!cell.is_empty()).then(|| guess(cell)),
                }
            };
            let Some(value) = value else {
                continue;
            };
            if name.contains('.') && type_of(&name).is_none() {
                nest(&mut doc, &name, value);
            } else {
                doc.insert(name, value);
            }
        }
        let doc = if invalid {
            let delimiter = (options.csv.delimiter as char).to_string();
            let joined: Vec<u8> = row.iter().collect::<Vec<_>>().join(delimiter.as_bytes());
            Err(Rejected {
                raw: lossy_head(&joined),
                reason: RejectReason::InvalidUtf8,
            })
        } else {
            Ok(Value::Object(doc).to_string())
        };
        if sink(Record { line, offset, doc }).is_break() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(data: &[u8], options: &ReadOptions) -> (Vec<Record>, Result<(), ReadError>) {
        let mut records = Vec::new();
        let source = Source::Bytes(Arc::from(data));
        let result = read_documents(&source, options, |r| {
            records.push(r);
            ControlFlow::Continue(())
        });
        (records, result)
    }

    fn docs(records: &[Record]) -> Vec<String> {
        records
            .iter()
            .map(|r| r.doc.clone().expect("a document"))
            .collect()
    }

    fn jsonl(data: &[u8]) -> (Vec<Record>, Result<(), ReadError>) {
        run(data, &ReadOptions::new(Format::Jsonl))
    }

    fn csv_with(data: &str, options: ReadOptions) -> Vec<Value> {
        let (records, result) = run(data.as_bytes(), &options);
        result.expect("reads");
        docs(&records)
            .iter()
            .map(|d| serde_json::from_str(d).expect("json"))
            .collect()
    }

    fn csv(data: &str) -> Vec<Value> {
        csv_with(data, ReadOptions::new(Format::Csv))
    }

    #[test]
    fn sniff_table() {
        let none = CsvOptions::default();
        let tab = CsvOptions {
            delimiter: b'\t',
            header: true,
        };
        let cases: Vec<(Option<&str>, &[u8], Format, CsvOptions)> = vec![
            (Some("a.csv"), b"[", Format::Csv, none.clone()),
            (Some("a.CSV"), b"", Format::Csv, none.clone()),
            (Some("a.tsv"), b"{", Format::Csv, tab),
            (Some("a.json"), b"[1]", Format::JsonArray, none.clone()),
            (Some("a.jsonl"), b"{}", Format::Jsonl, none.clone()),
            (None, b"\xEF\xBB\xBF[1]", Format::JsonArray, none.clone()),
            (None, b"{\"a\":1}", Format::Jsonl, none.clone()),
            (None, b"  \n\t [1]", Format::JsonArray, none.clone()),
            (None, b"", Format::Jsonl, none.clone()),
        ];
        for (path, head, format, csv) in cases {
            let got = sniff(path.map(Path::new), head);
            assert_eq!(got, (format, csv), "{path:?} {head:?}");
        }
    }

    #[test]
    fn blank_lines_are_skipped_but_counted() {
        let (records, result) = jsonl(b"{\"a\":1}\n\n{\"a\":2}\n");
        result.unwrap();
        let lines: Vec<u64> = records.iter().map(|r| r.line).collect();
        assert_eq!(lines, [1, 3]);
    }

    #[test]
    fn whitespace_only_line_is_skipped() {
        let (records, _) = jsonl(b"{\"a\":1}\n  \t \n{\"a\":2}");
        assert_eq!(docs(&records), [r#"{"a":1}"#, r#"{"a":2}"#]);
        assert_eq!(records[1].line, 3);
    }

    #[test]
    fn crlf_and_edges_are_stripped() {
        let (records, _) = jsonl(b"  {\"a\":1} \r\n{\"a\":2}\r\n");
        assert_eq!(docs(&records), [r#"{"a":1}"#, r#"{"a":2}"#]);
    }

    #[test]
    fn bom_goes() {
        let (records, _) = jsonl(b"\xEF\xBB\xBF{\"a\":1}\n{\"a\":2}");
        assert_eq!(docs(&records), [r#"{"a":1}"#, r#"{"a":2}"#]);
    }

    #[test]
    fn last_line_needs_no_newline() {
        let (records, result) = jsonl(b"{\"a\":1}");
        result.unwrap();
        assert_eq!(docs(&records), [r#"{"a":1}"#]);
    }

    #[test]
    fn invalid_utf8_is_rejected_and_never_a_document() {
        let (records, _) = jsonl(b"{\"id\":\"bad\xff\",\"t\":\"a\"}\n{\"id\":\"ok\"}\n");
        assert_eq!(records.len(), 2);
        let rejected = records[0].doc.clone().unwrap_err();
        assert_eq!(rejected.reason, RejectReason::InvalidUtf8);
        assert!(rejected.raw.contains('\u{FFFD}'), "{}", rejected.raw);
        assert_eq!(records[1].doc.as_deref(), Ok(r#"{"id":"ok"}"#));
    }

    #[test]
    fn too_long_line_is_rejected_and_the_next_line_survives() {
        let mut data = vec![b'a'; MAX_DOC_BYTES + 1];
        data.extend_from_slice(b"\n{\"id\":\"1\"}");
        let (records, result) = jsonl(&data);
        result.unwrap();
        assert_eq!(records.len(), 2);
        let rejected = records[0].doc.clone().unwrap_err();
        assert_eq!(
            rejected.reason,
            RejectReason::TooLarge(MAX_DOC_BYTES as u64 + 1)
        );
        assert_eq!(rejected.raw.len(), super::super::REPORT_PREVIEW_BYTES);
        assert_eq!(records[1].line, 2);
        assert_eq!(records[1].doc.as_deref(), Ok(r#"{"id":"1"}"#));
    }

    #[test]
    fn raw_text_is_unchanged() {
        let (records, _) = jsonl(b"{\"n\":1.10}\nnot json");
        assert_eq!(docs(&records), [r#"{"n":1.10}"#, "not json"]);
    }

    #[test]
    fn offset_is_exact() {
        let (records, _) = jsonl(b"a\n\nbb\r\nccc");
        let offsets: Vec<u64> = records.iter().map(|r| r.offset).collect();
        assert_eq!(offsets, [2, 7, 10]);
    }

    #[test]
    fn break_stops_reading() {
        let source = Source::Bytes(Arc::from(&b"1\n2\n3\n"[..]));
        let mut seen = 0;
        let result = read_documents(&source, &ReadOptions::new(Format::Jsonl), |_| {
            seen += 1;
            if seen == 2 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!((result, seen), (Ok(()), 2));
        let mut seen = 0;
        let source = Source::Bytes(Arc::from(&b"[1,2,3]"[..]));
        let result = read_documents(&source, &ReadOptions::new(Format::JsonArray), |_| {
            seen += 1;
            ControlFlow::Break(())
        });
        assert_eq!((result, seen), (Ok(()), 1));
    }

    fn array(data: &[u8]) -> (Vec<Record>, Result<(), ReadError>) {
        run(data, &ReadOptions::new(Format::JsonArray))
    }

    #[test]
    fn elements_come_one_by_one() {
        let (records, result) = array(b"[{\"a\":1},{\"a\":2},3]");
        result.unwrap();
        assert_eq!(docs(&records), [r#"{"a":1}"#, r#"{"a":2}"#, "3"]);
        let lines: Vec<u64> = records.iter().map(|r| r.line).collect();
        assert_eq!(lines, [1, 2, 3]);
    }

    #[test]
    fn pretty_printed_elements_become_one_line() {
        let (records, _) = array(b"\xEF\xBB\xBF[\n  {\n    \"a\": 1,\n    \"b\": [1, 2]\n  }\n]\n");
        assert_eq!(docs(&records), [r#"{"a":1,"b":[1,2]}"#]);
    }

    #[test]
    fn broken_json_stops_with_line_and_column_after_the_good_elements() {
        let (records, result) = array(b"[{\"a\":1},\n {\"a\":2},\n {\"a\" 3}]");
        assert_eq!(docs(&records), [r#"{"a":1}"#, r#"{"a":2}"#]);
        match result {
            Err(ReadError::Json { line, column, .. }) => assert_eq!((line, column), (3, 7)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn top_level_object_is_an_error() {
        let (records, result) = array(b"{\"a\":1}");
        assert!(records.is_empty());
        assert!(matches!(result, Err(ReadError::Json { .. })), "{result:?}");
    }

    #[test]
    fn empty_array_gives_nothing() {
        let (records, result) = array(b" [ ] ");
        assert!(records.is_empty());
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn oversized_element_is_record_too_large() {
        let mut data = b"[{\"a\":1},\"".to_vec();
        data.extend(std::iter::repeat_n(b'x', MAX_DOC_BYTES + 10));
        data.extend_from_slice(b"\"]");
        let (records, result) = array(&data);
        assert_eq!(records.len(), 1);
        assert_eq!(result, Err(ReadError::ElementTooLarge { element: 2 }));
    }

    #[test]
    fn header_names_the_keys() {
        let got = csv("id,name,n\n1,Ann,5\n2,Bob,6.5\n");
        assert_eq!(
            got,
            [
                json!({"id": "1", "name": "Ann", "n": 5}),
                json!({"id": "2", "name": "Bob", "n": 6.5})
            ]
        );
    }

    #[test]
    fn csv_lines_and_offsets() {
        let (records, _) = run(b"a,b\n1,2\n\"x\ny\",3\n", &ReadOptions::new(Format::Csv));
        let at: Vec<(u64, u64)> = records.iter().map(|r| (r.line, r.offset)).collect();
        assert_eq!(at, [(2, 8), (3, 16)]);
        // CRLF: the line is where the record starts, whatever the terminator.
        let (records, _) = run(
            b"a,b\r\n1,2\r\n3,4\r\n5,6\r\n",
            &ReadOptions::new(Format::Csv),
        );
        let lines: Vec<u64> = records.iter().map(|r| r.line).collect();
        assert_eq!(lines, [2, 3, 4]);
        // Blank lines count and do not shift the records after them.
        let (records, _) = run(b"a,b\n1,2\n\n\n3,4\n", &ReadOptions::new(Format::Csv));
        let lines: Vec<u64> = records.iter().map(|r| r.line).collect();
        assert_eq!(lines, [2, 5]);
        let (records, _) = run(
            b"a,b\r\n\r\n\"x\r\ny\",1\r\n\r\n5,6",
            &ReadOptions::new(Format::Csv),
        );
        let lines: Vec<u64> = records.iter().map(|r| r.line).collect();
        assert_eq!(lines, [3, 6]);
        let mut options = ReadOptions::new(Format::Csv);
        options.csv.header = false;
        let (records, _) = run(b"\n1,2\n3,4", &options);
        let lines: Vec<u64> = records.iter().map(|r| r.line).collect();
        assert_eq!(lines, [2, 3]);
    }

    #[test]
    fn no_header_uses_schema_order_then_column_n() {
        let mut options = ReadOptions::new(Format::Csv);
        options.csv.header = false;
        options.fields = vec![("a".into(), "int32".into()), ("b".into(), "string".into())];
        let got = csv_with("1,x,extra\n2,y\n", options);
        assert_eq!(
            got,
            [
                json!({"a": 1, "b": "x", "column_3": "extra"}),
                json!({"a": 2, "b": "y"})
            ]
        );
    }

    #[test]
    fn semicolon_delimiter() {
        let mut options = ReadOptions::new(Format::Csv);
        options.csv.delimiter = b';';
        assert_eq!(
            csv_with("a;b\n1,5;x\n", options),
            [json!({"a": "1,5", "b": "x"})]
        );
    }

    #[test]
    fn quoted_comma_and_newline() {
        let got = csv("a,b\n\"x, y\",\"line1\nline2 \"\"q\"\"\"\n");
        assert_eq!(got, [json!({"a": "x, y", "b": "line1\nline2 \"q\""})]);
    }

    #[test]
    fn short_row_leaves_keys_out() {
        assert_eq!(csv("a,b,c\n1,2\n"), [json!({"a": 1, "b": 2})]);
    }

    #[test]
    fn long_row_gets_column_n() {
        assert_eq!(
            csv("a\n1,2,3\n"),
            [json!({"a": 1, "column_2": 2, "column_3": 3})]
        );
    }

    #[test]
    fn empty_cell_leaves_the_key_out() {
        assert_eq!(csv("a,b,id\n,x,\n"), [json!({"b": "x"})]);
    }

    #[test]
    fn id_stays_text() {
        assert_eq!(csv("id,n\n007,7\n"), [json!({"id": "007", "n": 7})]);
    }

    #[test]
    fn duplicate_header_is_an_error() {
        let (records, result) = run(b"a,b,a\n1,2,3\n", &ReadOptions::new(Format::Csv));
        assert!(records.is_empty());
        assert!(
            matches!(result, Err(ReadError::Csv { line: 1, .. })),
            "{result:?}"
        );
    }

    #[test]
    fn csv_header_loses_a_bom() {
        assert_eq!(csv("\u{FEFF}a,b\n1,2\n"), [json!({"a": 1, "b": 2})]);
    }

    #[test]
    fn invalid_utf8_row_is_rejected() {
        let (records, result) = run(b"a,b\n1,\xff\n2,3\n", &ReadOptions::new(Format::Csv));
        result.unwrap();
        assert_eq!(records.len(), 2);
        let rejected = records[0].doc.clone().unwrap_err();
        assert_eq!(rejected.reason, RejectReason::InvalidUtf8);
        assert!(rejected.raw.starts_with("1,"), "{}", rejected.raw);
        assert!(records[1].doc.is_ok());
    }

    #[test]
    fn stray_quote_stops_with_record_too_large() {
        let mut data = b"a,b\n1,\"never closed\n".to_vec();
        data.extend(std::iter::repeat_n(b"x,y\n".iter().copied(), 10 << 20).flatten());
        assert!(data.len() > 40 << 20);
        let (records, result) = run(&data, &ReadOptions::new(Format::Csv));
        assert!(records.is_empty());
        assert_eq!(result, Err(ReadError::RecordTooLarge { line: 2 }));
    }

    #[test]
    fn dotted_header_nests() {
        let got = csv("id,o.a,o.b.c\n1,x,5\n");
        assert_eq!(got, [json!({"id": "1", "o": {"a": "x", "b": {"c": 5}}})]);
    }

    #[test]
    fn dotted_header_stays_flat_when_the_schema_has_it() {
        let mut options = ReadOptions::new(Format::Csv);
        options.fields = vec![("o.a".into(), "int32".into())];
        assert_eq!(
            csv_with("o.a,o.b\n1,2\n", options),
            [json!({"o.a": 1, "o": {"b": 2}})]
        );
    }

    #[test]
    fn typed_cells_follow_the_schema() {
        let mut options = ReadOptions::new(Format::Csv);
        options.fields = vec![
            ("t".into(), "string".into()),
            ("n".into(), "int32".into()),
            ("tags".into(), "string[]".into()),
        ];
        let got = csv_with("t,n,tags\n12,007,\"[\"\"a\"\"]\"\n", options);
        assert_eq!(got, [json!({"t": "12", "n": "007", "tags": ["a"]})]);
    }

    #[test]
    fn a_file_reads_like_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("docs.jsonl");
        std::fs::write(&path, "{\"a\":1}\n{\"a\":2}\n").unwrap();
        let mut seen = Vec::new();
        read_documents(&Source::File(path), &ReadOptions::new(Format::Jsonl), |r| {
            seen.push(r.line);
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(seen, [1, 2]);
        let missing = read_documents(
            &Source::File(dir.path().join("nope")),
            &ReadOptions::new(Format::Jsonl),
            |_| ControlFlow::Continue(()),
        );
        assert!(matches!(missing, Err(ReadError::Io(_))));
    }
}
