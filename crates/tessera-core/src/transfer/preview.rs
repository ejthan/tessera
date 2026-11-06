// SPDX-License-Identifier: GPL-3.0-or-later

//! The first rows of a file as they will be read, checked against a schema.

use std::ops::ControlFlow;

use serde_json::Value;
use tessera_client::Collection;

use super::PREVIEW_LINES;
use super::source::{ReadError, ReadOptions, RejectReason, Source, read_documents};
use crate::documents::page::cell_preview;
use crate::documents::validate::{DocIssue, validate_document};

/// One row of the preview.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewRow {
    /// The record's line, as in [`Record::line`](super::source::Record::line).
    pub line: u64,
    /// The document on one line, cut for display.
    pub text: String,
    /// What the schema says about it. Empty without a schema.
    pub issues: Vec<DocIssue>,
    /// Why the row cannot be sent as it is: invalid JSON, not UTF-8, too large.
    pub error: Option<String>,
}

/// The start of a source.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// Up to [`PREVIEW_LINES`] rows.
    pub rows: Vec<PreviewRow>,
    /// Whether these are all the rows.
    pub complete: bool,
    /// Exact when `complete`, else the size of the source over the average size of a shown
    /// row.
    pub estimated_docs: u64,
    /// Size of the source in bytes, if known.
    pub total_bytes: Option<u64>,
}

/// Reads the first [`PREVIEW_LINES`] records of `source`. With a `schema`, each document
/// that parses is validated against it.
pub fn preview(
    source: &Source,
    options: &ReadOptions,
    schema: Option<&Collection>,
) -> Result<Preview, ReadError> {
    let total_bytes = match source {
        Source::File(path) => std::fs::metadata(path).ok().map(|m| m.len()),
        Source::Bytes(bytes) => Some(bytes.len() as u64),
    };
    let mut rows = Vec::new();
    let mut offset = 0;
    let mut complete = true;
    read_documents(source, options, |record| {
        // The row after the last shown one only tells that there is more.
        if rows.len() == PREVIEW_LINES {
            complete = false;
            return ControlFlow::Break(());
        }
        offset = record.offset;
        let row = match record.doc {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => PreviewRow {
                    line: record.line,
                    text: cell_preview(&value).to_string(),
                    issues: schema
                        .map(|c| validate_document(c, &value))
                        .unwrap_or_default(),
                    error: None,
                },
                Err(e) => PreviewRow {
                    line: record.line,
                    text: cell_preview(&Value::String(text)).to_string(),
                    issues: Vec::new(),
                    error: Some(e.to_string()),
                },
            },
            Err(rejected) => PreviewRow {
                line: record.line,
                text: cell_preview(&Value::String(rejected.raw)).to_string(),
                issues: Vec::new(),
                error: Some(match rejected.reason {
                    RejectReason::InvalidUtf8 => "not valid UTF-8".to_string(),
                    RejectReason::TooLarge(bytes) => {
                        format!("{} MiB is more than a document may hold", bytes >> 20)
                    }
                }),
            },
        };
        rows.push(row);
        ControlFlow::Continue(())
    })?;
    let shown = rows.len() as u64;
    let estimated_docs = match (complete, total_bytes) {
        (false, Some(total)) if offset > 0 => (total * shown).div_ceil(offset).max(shown),
        _ => shown,
    };
    Ok(Preview {
        rows,
        complete,
        estimated_docs,
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::transfer::source::Format;

    fn collection(json: &str) -> Collection {
        serde_json::from_str(json).expect("collection")
    }

    fn lines(n: usize) -> String {
        (0..n)
            .map(|i| format!(r#"{{"id":"{i}","t":"row {i}"}}"#))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn jsonl(text: &str, schema: Option<&Collection>) -> Preview {
        let source = Source::Bytes(Arc::from(text.as_bytes()));
        preview(&source, &ReadOptions::new(Format::Jsonl), schema).expect("previews")
    }

    #[test]
    fn preview_reads_20_rows_and_estimates() {
        let text = lines(100);
        let p = jsonl(&text, None);
        assert_eq!(p.rows.len(), PREVIEW_LINES);
        assert!(!p.complete);
        assert_eq!(p.total_bytes, Some(text.len() as u64));
        let off = p.estimated_docs.abs_diff(100);
        assert!(off <= 10, "{}", p.estimated_docs);
        assert_eq!(p.rows[19].line, 20);
    }

    #[test]
    fn preview_of_a_short_source_is_complete_and_exact() {
        let p = jsonl(&lines(5), None);
        assert!(p.complete);
        assert_eq!((p.rows.len(), p.estimated_docs), (5, 5));
        let exactly = jsonl(&lines(PREVIEW_LINES), None);
        assert!(exactly.complete);
        assert_eq!(exactly.estimated_docs, PREVIEW_LINES as u64);
    }

    #[test]
    fn preview_estimates_a_csv_and_a_json_array() {
        let csv: String = std::iter::once("id,t\n".to_string())
            .chain((0..100).map(|i| format!("{i:03},row\n")))
            .collect();
        let source = Source::Bytes(Arc::from(csv.as_bytes()));
        let p = preview(&source, &ReadOptions::new(Format::Csv), None).unwrap();
        assert!(!p.complete);
        assert!(p.estimated_docs.abs_diff(100) <= 10, "{}", p.estimated_docs);
        let array = format!(
            "[{}]",
            (0..100)
                .map(|i| format!(r#"{{"id":"{i:03}"}}"#))
                .collect::<Vec<_>>()
                .join(",\n")
        );
        let source = Source::Bytes(Arc::from(array.as_bytes()));
        let p = preview(&source, &ReadOptions::new(Format::JsonArray), None).unwrap();
        assert!(!p.complete);
        assert!(p.estimated_docs.abs_diff(100) <= 10, "{}", p.estimated_docs);
    }

    #[test]
    fn preview_validates_against_the_schema() {
        let schema = collection(r#"{"name":"c","fields":[{"name":"n","type":"int32"}]}"#);
        let p = jsonl("{\"n\":1}\n{\"n\":\"x\"}\n{}", Some(&schema));
        assert!(p.rows[0].issues.is_empty());
        assert_eq!(p.rows[1].issues[0].key, "document.wrong_type");
        assert_eq!(p.rows[2].issues[0].key, "document.missing_required");
        assert!(jsonl("{\"n\":\"x\"}", None).rows[0].issues.is_empty());
    }

    #[test]
    fn preview_row_with_bad_json_carries_the_message() {
        let p = jsonl("{\"a\":1}\n{\"a\":", None);
        assert_eq!(p.rows[0].error, None);
        let error = p.rows[1].error.as_deref().unwrap();
        assert!(error.contains("EOF"), "{error}");
        assert_eq!(p.rows[1].text, "{\"a\":");
    }

    #[test]
    fn preview_of_rejected_row_says_why() {
        let source = Source::Bytes(Arc::from(&b"{\"a\":\"\xff\"}\n{\"a\":1}"[..]));
        let p = preview(&source, &ReadOptions::new(Format::Jsonl), None).unwrap();
        assert_eq!(p.rows[0].error.as_deref(), Some("not valid UTF-8"));
        assert_eq!(p.rows[1].error, None);
    }

    #[test]
    fn a_read_error_is_returned() {
        let source = Source::Bytes(Arc::from(&b"{\"a\":1}"[..]));
        let e = preview(&source, &ReadOptions::new(Format::JsonArray), None);
        assert!(matches!(e, Err(ReadError::Json { .. })));
    }
}
