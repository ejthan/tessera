// SPDX-License-Identifier: GPL-3.0-or-later

//! CSV and JSON files of a synonym set (D67).

use std::path::Path;

use serde_json::{Value, json};

use super::changes::{ItemIssue, check_items};
use super::synonyms::SynonymDraft;
use crate::transfer::CLIPBOARD_LIMIT;

/// Files and pastes are read whole, up to this size.
pub const SET_FILE_LIMIT: usize = CLIPBOARD_LIMIT;

/// How a set file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetFormat {
    /// `{"items":[…]}` or a bare array.
    Json,
    /// `id,root,synonyms,locale,symbols_to_index`.
    Csv,
}

/// `.csv` and `.tsv` are CSV, anything else JSON.
pub fn format_of(path: &Path) -> SetFormat {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    match ext.as_deref() {
        Some("csv" | "tsv") => SetFormat::Csv,
        _ => SetFormat::Json,
    }
}

/// Why a file is not a set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetFileError {
    /// Not JSON, or not the expected shape.
    Json(String),
    /// A CSV problem; `row` is the 1-based data row, 0 for the header.
    Csv {
        /// The data row.
        row: u64,
        /// What is wrong.
        message: String,
    },
    /// The items parse, and some cannot be sent.
    Items(Vec<ItemIssue>),
    /// More than [`SET_FILE_LIMIT`] bytes; holds the size.
    TooLarge(u64),
}

/// `{"items":[…]}`, pretty.
pub fn synonyms_to_json(items: &[Value]) -> String {
    serde_json::to_string_pretty(&json!({ "items": items })).unwrap_or_default()
}

fn list_cell(list: &[String]) -> String {
    let joined = list.join(";");
    // A value that would split, lose its edges or open as JSON is written as JSON.
    let lossy = list
        .iter()
        .any(|v| v.is_empty() || v.contains(';') || v.trim() != v)
        || joined.starts_with('[');
    if lossy {
        json!(list).to_string()
    } else {
        joined
    }
}

/// The CSV of a set. Cells stay verbatim: a spreadsheet may run one that starts with `=`.
pub fn synonyms_to_csv(items: &[SynonymDraft]) -> String {
    let mut writer = csv::Writer::from_writer(Vec::new());
    let mut write = |row: [&str; 5]| {
        // A write to memory cannot fail.
        let _ = writer.write_record(row);
    };
    write(["id", "root", "synonyms", "locale", "symbols_to_index"]);
    for item in items {
        write([
            &item.id,
            &item.root,
            &list_cell(&item.synonyms),
            &item.locale,
            &list_cell(&item.symbols_to_index),
        ]);
    }
    String::from_utf8(writer.into_inner().unwrap_or_default()).unwrap_or_default()
}

fn parse_cell(cell: &str, row: u64) -> Result<Vec<String>, SetFileError> {
    if cell.starts_with('[') {
        return serde_json::from_str(cell).map_err(|e| SetFileError::Csv {
            row,
            message: format!("a list that starts with [ must be a JSON array of strings: {e}"),
        });
    }
    Ok(cell
        .split(';')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect())
}

fn parse_csv(text: &str) -> Result<Vec<Value>, SetFileError> {
    let header_error = |message: String| SetFileError::Csv { row: 0, message };
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| header_error(e.to_string()))?
        .clone();
    let column = |name: &str| headers.iter().position(|h| h.trim() == name);
    let (Some(id), Some(synonyms)) = (column("id"), column("synonyms")) else {
        return Err(header_error(
            "the header needs the columns id and synonyms".into(),
        ));
    };
    let (root, locale, symbols) = (column("root"), column("locale"), column("symbols_to_index"));
    let mut items = Vec::new();
    for (i, record) in reader.records().enumerate() {
        let row = i as u64 + 1;
        let record = record.map_err(|e| SetFileError::Csv {
            row,
            message: e.to_string(),
        })?;
        let cell = |at: Option<usize>| at.and_then(|n| record.get(n)).unwrap_or_default();
        let draft = SynonymDraft {
            id: match cell(Some(id)) {
                "" => format!("row_{row}"),
                given => given.to_string(),
            },
            root: cell(root).to_string(),
            synonyms: parse_cell(cell(Some(synonyms)), row)?,
            locale: cell(locale).to_string(),
            symbols_to_index: parse_cell(cell(symbols), row)?,
            extra: Default::default(),
        };
        items.push(draft.to_value());
    }
    Ok(items)
}

/// The items of a file or paste. Duplicate ids are refused with both rows, because the server
/// keeps the last one without a word.
pub fn parse_synonyms(text: &str, format: SetFormat) -> Result<Vec<Value>, SetFileError> {
    if text.len() > SET_FILE_LIMIT {
        return Err(SetFileError::TooLarge(text.len() as u64));
    }
    let items = match format {
        SetFormat::Csv => parse_csv(text)?,
        SetFormat::Json => {
            let value: Value =
                serde_json::from_str(text).map_err(|e| SetFileError::Json(e.to_string()))?;
            match value {
                Value::Array(list) => list,
                Value::Object(mut map) => match map.remove("items") {
                    Some(Value::Array(list)) => list,
                    _ => {
                        return Err(SetFileError::Json(
                            "expected {\"items\":[…]} or an array".into(),
                        ));
                    }
                },
                _ => {
                    return Err(SetFileError::Json(
                        "expected {\"items\":[…]} or an array".into(),
                    ));
                }
            }
        }
    };
    let issues = check_items(&items);
    if issues.is_empty() {
        Ok(items)
    } else {
        Err(SetFileError::Items(issues))
    }
}

/// `current` with `incoming` laid over it by id: a match is replaced in place, the rest is
/// appended.
pub fn merged(current: &[Value], incoming: &[Value]) -> Vec<Value> {
    let id = |v: &Value| v.get("id").and_then(Value::as_str).map(String::from);
    let mut out = current.to_vec();
    for item in incoming {
        let at =
            id(item).and_then(|wanted| out.iter().position(|v| id(v).as_deref() == Some(&wanted)));
        match at {
            Some(at) => out[at] = item.clone(),
            None => out.push(item.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(id: &str, root: &str, words: &[&str]) -> SynonymDraft {
        SynonymDraft {
            id: id.into(),
            root: root.into(),
            synonyms: words.iter().map(ToString::to_string).collect(),
            ..SynonymDraft::default()
        }
    }

    fn csv_round_trip(drafts: &[SynonymDraft]) -> Vec<SynonymDraft> {
        let text = synonyms_to_csv(drafts);
        parse_synonyms(&text, SetFormat::Csv)
            .unwrap_or_else(|e| panic!("{e:?}\n{text}"))
            .iter()
            .map(|v| SynonymDraft::from_value(v).unwrap())
            .collect()
    }

    #[test]
    fn the_csv_has_the_header_and_joins_lists_with_a_semicolon() {
        let text = synonyms_to_csv(&[draft("a", "", &["x", "y"])]);
        assert_eq!(text, "id,root,synonyms,locale,symbols_to_index\na,,x;y,,\n");
    }

    #[test]
    fn csv_round_trips_awkward_values() {
        let mut awkward = draft("q", "r", &["a;b", "c"]);
        awkward.locale = "en".into();
        awkward.symbols_to_index = vec!["+".into(), "#".into()];
        let drafts = [
            draft("plain", "", &["x", "y"]),
            awkward,
            draft("bracket", "", &["[x", "y"]),
            draft("quotes", "", &["say \"hi\"", "a,b"]),
            draft("newline", "", &["line\none", "two"]),
            draft("edges", "", &[" pad ", "z"]),
            draft("empty", "", &["", "z"]),
        ];
        assert_eq!(csv_round_trip(&drafts), drafts);
    }

    #[test]
    fn json_cells_are_read_as_json() {
        let text = "id,synonyms\na,\"[\"\"x;y\"\",\"\"z\"\"]\"\n";
        let items = parse_synonyms(text, SetFormat::Csv).unwrap();
        assert_eq!(items[0]["synonyms"], json!(["x;y", "z"]));
        let bad = "id,synonyms\na,[nope\n";
        assert!(matches!(
            parse_synonyms(bad, SetFormat::Csv),
            Err(SetFileError::Csv { row: 1, .. })
        ));
    }

    #[test]
    fn csv_columns_may_come_in_any_order_and_a_blank_id_gets_a_row_name() {
        let text = "synonyms,locale,id\nx;y,en,\nz;w,,named\n";
        let items = parse_synonyms(text, SetFormat::Csv).unwrap();
        assert_eq!(items[0]["id"], "row_1");
        assert_eq!(items[0]["locale"], "en");
        assert_eq!(items[1]["id"], "named");
    }

    #[test]
    fn short_rows_are_accepted_and_missing_columns_named() {
        let items = parse_synonyms("id,synonyms,root\na\nb,x;y\n", SetFormat::Csv).unwrap();
        assert_eq!(items[0]["synonyms"], json!([]));
        assert_eq!(items[1]["synonyms"], json!(["x", "y"]));
        let error = parse_synonyms("id,words\na,b\n", SetFormat::Csv).unwrap_err();
        assert!(matches!(error, SetFileError::Csv { row: 0, .. }));
    }

    #[test]
    fn json_takes_items_or_a_bare_array() {
        let item = json!({"id": "a", "synonyms": ["x", "y"], "kept": 1});
        let text = synonyms_to_json(std::slice::from_ref(&item));
        assert_eq!(
            parse_synonyms(&text, SetFormat::Json).unwrap(),
            vec![item.clone()]
        );
        let bare = json!([item]).to_string();
        assert_eq!(parse_synonyms(&bare, SetFormat::Json).unwrap().len(), 1);
        for bad in ["nope", "1", "{}", "{\"items\":1}"] {
            assert!(
                matches!(
                    parse_synonyms(bad, SetFormat::Json),
                    Err(SetFileError::Json(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_duplicate_id_reports_both_rows() {
        let json = r#"[{"id":"a","synonyms":["x","y"]},{"id":"b"},{"id":"a"}]"#;
        let expected = Err(SetFileError::Items(vec![ItemIssue::DuplicateId {
            id: "a".into(),
            rows: (1, 3),
        }]));
        assert_eq!(parse_synonyms(json, SetFormat::Json), expected);
        let csv = "id,synonyms\na,x;y\nb,x;y\na,z;w\n";
        assert_eq!(parse_synonyms(csv, SetFormat::Csv), expected);
    }

    #[test]
    fn a_file_over_the_limit_is_refused() {
        let big = " ".repeat(SET_FILE_LIMIT + 1);
        assert_eq!(
            parse_synonyms(&big, SetFormat::Json),
            Err(SetFileError::TooLarge(SET_FILE_LIMIT as u64 + 1))
        );
    }

    #[test]
    fn the_format_follows_the_extension() {
        for (name, format) in [
            ("a.csv", SetFormat::Csv),
            ("a.TSV", SetFormat::Csv),
            ("a.json", SetFormat::Json),
            ("a.txt", SetFormat::Json),
            ("a", SetFormat::Json),
        ] {
            assert_eq!(format_of(Path::new(name)), format, "{name}");
        }
    }

    #[test]
    fn merged_replaces_in_place_and_appends() {
        let current = [json!({"id": "a", "v": 1}), json!({"id": "b", "v": 1})];
        let incoming = [json!({"id": "c", "v": 2}), json!({"id": "a", "v": 2})];
        assert_eq!(
            merged(&current, &incoming),
            [
                json!({"id": "a", "v": 2}),
                json!({"id": "b", "v": 1}),
                json!({"id": "c", "v": 2})
            ]
        );
    }
}
