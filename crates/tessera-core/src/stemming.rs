// SPDX-License-Identifier: GPL-3.0-or-later

//! Stemming dictionary files. The server stores the lines before a bad one, so a file is
//! checked whole before anything is sent. Words nothing, the app translates the enums.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use tessera_client::{Collection, StemmingWord};

use crate::relevance::synonym_file::SET_FILE_LIMIT;

/// A file or paste is read whole, up to this size.
pub const STEMMING_FILE_LIMIT: usize = SET_FILE_LIMIT;

/// The most bad lines reported.
const MAX_BAD_LINES: usize = 100;

/// What is wrong with a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineProblem {
    /// Not JSON.
    NotJson,
    /// JSON, but not an object.
    NotObject,
    /// No `word`.
    MissingWord,
    /// No `root`.
    MissingRoot,
    /// A `word` or `root` that is not text.
    NotText,
    /// An empty or blank `word` or `root`, which the server would store.
    Empty,
}

/// A line that stops the import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadLine {
    /// 1-based line number.
    pub line: usize,
    /// What is wrong.
    pub problem: LineProblem,
}

/// The words of a JSON Lines file, blank lines skipped, or up to 100 bad lines. Unknown keys
/// are ignored, as on the server, and the first of a repeated word wins there.
pub fn parse_dictionary(text: &str) -> Result<Vec<StemmingWord>, Vec<BadLine>> {
    let mut words = Vec::new();
    let mut bad = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Ok(word) => words.push(word),
            Err(problem) => {
                bad.push(BadLine {
                    line: i + 1,
                    problem,
                });
                if bad.len() == MAX_BAD_LINES {
                    break;
                }
            }
        }
    }
    if bad.is_empty() { Ok(words) } else { Err(bad) }
}

fn parse_line(line: &str) -> Result<StemmingWord, LineProblem> {
    let value: Value = serde_json::from_str(line).map_err(|_| LineProblem::NotJson)?;
    let object = value.as_object().ok_or(LineProblem::NotObject)?;
    let word = object.get("word").ok_or(LineProblem::MissingWord)?;
    let root = object.get("root").ok_or(LineProblem::MissingRoot)?;
    let text = |v: &Value| match v.as_str() {
        None => Err(LineProblem::NotText),
        Some(s) if s.trim().is_empty() => Err(LineProblem::Empty),
        Some(s) => Ok(s.to_string()),
    };
    Ok(StemmingWord {
        word: text(word)?,
        root: text(root)?,
    })
}

/// One JSON object per line, no final newline.
pub fn to_jsonl(words: &[StemmingWord]) -> String {
    words
        .iter()
        .filter_map(|w| serde_json::to_string(w).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

/// What an import did, read back from the dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCheck {
    /// Lines sent.
    pub sent: usize,
    /// Sent words that are not stored with the root they should have: the root they had, or
    /// else the first one sent.
    pub missing: Vec<String>,
    /// Words of the dictionary before the import that were sent with another root and keep
    /// the one they have.
    pub kept_root: usize,
}

fn roots(words: &[StemmingWord]) -> HashMap<&str, &str> {
    words
        .iter()
        .map(|w| (w.word.as_str(), w.root.as_str()))
        .collect()
}

/// Compares what was sent with what is stored. The server keeps the root a word has, from an
/// earlier import or an earlier line, so `kept_root` is what the user should hear about.
pub fn check_import(
    before: &[StemmingWord],
    sent: &[StemmingWord],
    stored: &[StemmingWord],
) -> ImportCheck {
    let before_roots = roots(before);
    let stored_roots = roots(stored);
    let mut seen = HashSet::new();
    let mut missing = Vec::new();
    let mut kept_root = 0;
    for w in sent {
        let word = w.word.as_str();
        if !seen.insert(word) {
            continue;
        }
        let had = before_roots.get(word).copied();
        if stored_roots.get(word).copied() != Some(had.unwrap_or(&w.root)) {
            missing.push(w.word.clone());
        }
        kept_root += usize::from(had.is_some_and(|root| root != w.root));
    }
    ImportCheck {
        sent: sent.len(),
        missing,
        kept_root,
    }
}

/// The dictionaries the fields of `collection` name, each once.
pub fn dictionaries_used(collection: &Collection) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for field in &collection.fields {
        let name = field.attr("stem_dictionary").and_then(Value::as_str);
        if let Some(name) = name.filter(|n| !n.is_empty() && !names.iter().any(|known| known == n))
        {
            names.push(name.to_string());
        }
    }
    names
}

/// The collections with a field that names dictionary `id`.
pub fn collections_using(collections: &[Collection], id: &str) -> Vec<String> {
    collections
        .iter()
        .filter(|c| {
            c.fields
                .iter()
                .any(|f| f.attr("stem_dictionary").and_then(Value::as_str) == Some(id))
        })
        .map(|c| c.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dictionaries_used_lists_each_once_and_no_blank() {
        let collection: Collection = serde_json::from_value(json!({
            "name": "c",
            "fields": [
                {"name": "a", "type": "string", "stem_dictionary": "d"},
                {"name": "b", "type": "string", "stem_dictionary": "d"},
                {"name": "c", "type": "string", "stem_dictionary": ""},
                {"name": "e", "type": "string"},
                {"name": "f", "type": "string", "stem_dictionary": "g"},
            ],
        }))
        .unwrap();
        assert_eq!(dictionaries_used(&collection), ["d", "g"]);
    }

    fn w(word: &str, root: &str) -> StemmingWord {
        StemmingWord {
            word: word.into(),
            root: root.into(),
        }
    }

    #[test]
    fn parse_accepts_jsonl() {
        let text = "{\"word\":\"people\",\"root\":\"person\"}\n\n  \n{\"root\":\"mouse\",\"word\":\"mice\",\"x\":1}\n";
        assert_eq!(
            parse_dictionary(text),
            Ok(vec![w("people", "person"), w("mice", "mouse")])
        );
        assert_eq!(parse_dictionary(""), Ok(vec![]));
    }

    #[test]
    fn parse_names_every_bad_line() {
        let text = [
            "{\"word\":\"a\",\"root\":\"b\"}",
            "not json",
            "[1]",
            "{\"root\":\"b\"}",
            "{\"word\":\"a\"}",
            "",
            "{\"word\":\"a\",\"root\":\"b\"}",
        ]
        .join("\n");
        let bad = |line, problem| BadLine { line, problem };
        assert_eq!(
            parse_dictionary(&text),
            Err(vec![
                bad(2, LineProblem::NotJson),
                bad(3, LineProblem::NotObject),
                bad(4, LineProblem::MissingWord),
                bad(5, LineProblem::MissingRoot),
            ])
        );
    }

    #[test]
    fn parse_stops_collecting_at_100() {
        let text = "x\n".repeat(250);
        let Err(bad) = parse_dictionary(&text) else {
            panic!("bad lines");
        };
        assert_eq!(bad.len(), 100);
        assert_eq!(bad[99].line, 100);
    }

    #[test]
    fn empty_word_is_refused() {
        for line in [r#"{"word":"","root":"b"}"#, r#"{"word":"a","root":"  "}"#] {
            assert_eq!(
                parse_dictionary(line),
                Err(vec![BadLine {
                    line: 1,
                    problem: LineProblem::Empty
                }])
            );
        }
    }

    #[test]
    fn numeric_word_is_refused() {
        for line in [r#"{"word":5,"root":"b"}"#, r#"{"word":"a","root":null}"#] {
            assert_eq!(
                parse_dictionary(line),
                Err(vec![BadLine {
                    line: 1,
                    problem: LineProblem::NotText
                }])
            );
        }
    }

    #[test]
    fn jsonl_round_trips_without_a_final_newline() {
        let words = vec![w("a\"b", "c\nd"), w("é", "e")];
        let text = to_jsonl(&words);
        assert!(!text.ends_with('\n') && text.lines().count() == 2);
        assert_eq!(parse_dictionary(&text), Ok(words));
        assert_eq!(to_jsonl(&[]), "");
    }

    #[test]
    fn check_import_counts_a_kept_root() {
        let before = [w("a", "x"), w("b", "y")];
        let sent = [w("a", "z"), w("b", "y"), w("c", "q")];
        let stored = [w("a", "x"), w("b", "y"), w("c", "q")];
        assert_eq!(
            check_import(&before, &sent, &stored),
            ImportCheck {
                sent: 3,
                missing: vec![],
                kept_root: 1
            }
        );
    }

    #[test]
    fn check_import_finds_a_missing_word() {
        let check = check_import(&[], &[w("a", "x"), w("b", "y")], &[w("a", "x")]);
        assert_eq!(check.missing, ["b"]);
        let wrong_root = check_import(&[], &[w("a", "x")], &[w("a", "other")]);
        assert_eq!(wrong_root.missing, ["a"]);
    }

    #[test]
    fn first_duplicate_wins_in_the_check() {
        let sent = [w("a", "x"), w("a", "y")];
        let check = check_import(&[], &sent, &[w("a", "x")]);
        assert!(check.missing.is_empty() && check.kept_root == 0);
        assert_eq!(check_import(&[], &sent, &[w("a", "y")]).missing, ["a"]);
    }

    #[test]
    fn a_stored_word_is_compared_with_its_old_root() {
        let before = [w("a", "x")];
        let moved = check_import(&before, &[w("a", "y")], &[w("a", "y")]);
        assert_eq!(moved.missing, ["a"]);
        let repeated = [w("a", "y"), w("a", "x")];
        assert_eq!(check_import(&before, &repeated, &before).kept_root, 1);
    }

    #[test]
    fn collections_using_compares_the_field_attribute() {
        let collection = |name: &str, dict: Option<&str>| -> Collection {
            let mut field = json!({"name": "t", "type": "string"});
            if let Some(d) = dict {
                field["stem_dictionary"] = d.into();
            }
            serde_json::from_value(json!({"name": name, "fields": [field]})).expect("collection")
        };
        let all = [
            collection("a", Some("en")),
            collection("b", Some("english")),
            collection("c", None),
            collection("d", Some("en")),
        ];
        assert_eq!(collections_using(&all, "en"), ["a", "d"]);
        assert!(collections_using(&all, "nope").is_empty());
    }
}
