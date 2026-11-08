// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask i18n`: locale files and hardcoded UI strings (NF8).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;

/// One problem, printed as `file:line: message`.
#[derive(Debug)]
pub struct Finding {
    /// The file, relative to the workspace root.
    pub file: PathBuf,
    /// The 1-based line.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

const EN: &str = "assets/locales/en.yml";
const DE: &str = "assets/locales/de.yml";
const SAME: &str = "xtask/i18n-same.txt";
const UI_SOURCES: [&str; 2] = ["crates/tessera-app/src", "crates/tessera-widgets/src"];
const SINKS: [&str; 14] = [
    ".child(",
    ".label(",
    ".tooltip(",
    ".tooltip_with_action(",
    ".placeholder(",
    ".title(",
    ".aria_label(",
    ".aria_description(",
    ".accessibility_label(",
    ".message(",
    ".description(",
    "Label::new(",
    ".push_notification(",
    ".set_window_title(",
];

/// Runs every rule of the lint on the tree at `root`.
pub fn run(root: &Path) -> anyhow::Result<Vec<Finding>> {
    let read = |path: &Path| {
        std::fs::read_to_string(root.join(path))
            .with_context(|| format!("reading {}", path.display()))
    };
    let (en_yaml, de_yaml) = (read(Path::new(EN))?, read(Path::new(DE))?);
    let (en, de) = (flat_keys(&en_yaml), flat_keys(&de_yaml));
    let same_text = read(Path::new(SAME))?;
    let same: BTreeSet<&str> = same_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let mut findings = Vec::new();
    let mut report = |file: &str, line, message| {
        findings.push(Finding {
            file: file.into(),
            line,
            message,
        });
    };
    for key in en.keys().filter(|k| !de.contains_key(*k)) {
        report(
            EN,
            line_of(&en_yaml, key),
            format!("`{key}` is missing in de.yml"),
        );
    }
    for key in de.keys().filter(|k| !en.contains_key(*k)) {
        report(
            DE,
            line_of(&de_yaml, key),
            format!("`{key}` is missing in en.yml"),
        );
    }
    for (key, value) in en.iter().filter_map(|(k, v)| Some((k, (v, de.get(k)?)))) {
        let line = line_of(&de_yaml, key);
        if placeholders(value.0) != placeholders(value.1) {
            let message = format!("`{key}` has other placeholders than in en.yml");
            report(DE, line, message);
        }
        if value.0 == value.1 && !same.contains(key.as_str()) {
            let message = format!("`{key}` equals English: translate it or list it in {SAME}");
            report(DE, line, message);
        }
    }

    for key in same {
        let line = same_text.lines().position(|l| l.trim() == key).unwrap_or(0) + 1;
        let stale = match (en.get(key), de.get(key)) {
            (None, _) => Some("is in no locale file"),
            (Some(a), Some(b)) if a != b => Some("differs in de.yml"),
            _ => None,
        };
        if let Some(stale) = stale {
            report(
                SAME,
                line,
                format!("`{key}` {stale}: remove it from {SAME}"),
            );
        }
    }

    for path in rust_files(&root.join("crates"))? {
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let file = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        for (line, key) in t_keys(&source)
            .into_iter()
            .filter(|(_, k)| !en.contains_key(k))
        {
            findings.push(Finding {
                file: file.clone(),
                line,
                message: format!("t!(\"{key}\") is not in en.yml"),
            });
        }
        if UI_SOURCES.iter().any(|dir| file.starts_with(dir)) {
            findings.extend(
                sink_literals(&source)
                    .into_iter()
                    .map(|(line, text)| Finding {
                        file: file.clone(),
                        line,
                        message: format!(
                            "hardcoded UI string \"{text}\": use t!() or `// i18n-ok: <reason>`"
                        ),
                    }),
            );
        }
    }
    Ok(findings)
}

/// The `key: "value"` pairs of a locale file, values as written.
pub fn flat_keys(yaml: &str) -> BTreeMap<String, String> {
    yaml.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('_'))
        .filter_map(|l| l.split_once(':'))
        .map(|(key, value)| {
            let value = value.trim();
            let inner = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'));
            (key.trim().to_string(), inner.unwrap_or(value).to_string())
        })
        .collect()
}

/// The names of the `%{name}` placeholders of a value.
pub fn placeholders(value: &str) -> BTreeSet<&str> {
    value
        .split("%{")
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .collect()
}

/// Letter-bearing literals in a UI position, as `(line, literal)`.
pub fn sink_literals(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    // Only a test module ends the code: `#[cfg(test)]` on one item leaves what follows in.
    let end = source
        .match_indices("#[cfg(test)]")
        .find(|(at, attr)| source[at + attr.len()..].trim_start().starts_with("mod "))
        .map_or(source.len(), |(at, _)| at);
    let code = blank(&source[..end], false);
    let bare = blank(&source[..end], true);
    let mut hits = Vec::new();
    for sink in SINKS {
        let hits_of_sink = bare.match_indices(sink);
        for (at, _) in
            hits_of_sink.filter(|(at, _)| sink.starts_with('.') || !after_ident(&code, *at))
        {
            let Some(literal) = literal_after(&code, at + sink.len()) else {
                continue;
            };
            let hardcoded = if literal.is_format {
                has_words(literal.text)
            } else {
                has_letter(literal.text)
            };
            let line = line_at(&code, literal.start);
            let above = line.checked_sub(2).and_then(|i| lines.get(i));
            let reasoned = lines.get(line - 1).is_some_and(|l| excused(l))
                || above.is_some_and(|l| excused(l));
            if hardcoded && !reasoned {
                hits.push((literal.start, literal.text.to_string()));
            }
        }
    }
    hits.sort();
    hits.into_iter()
        .map(|(start, text)| (line_at(&code, start), text))
        .collect()
}

/// The literal keys of every `t!("key")`, as `(line, key)`.
pub fn t_keys(source: &str) -> Vec<(usize, String)> {
    let code = blank(source, false);
    blank(source, true)
        .match_indices("t!(")
        .filter(|(at, _)| !after_ident(&code, *at))
        .filter_map(|(at, _)| literal_after(&code, at + 3))
        .filter(|literal| !literal.is_format)
        .map(|literal| (line_at(&code, literal.start), literal.text.to_string()))
        .collect()
}

struct Literal<'a> {
    start: usize,
    text: &'a str,
    is_format: bool,
}

/// The `"…"` or `format!("…"` that `code` holds at `at`, past any whitespace.
fn literal_after(code: &str, at: usize) -> Option<Literal<'_>> {
    let skip_space = |at: usize| at + code[at..].len() - code[at..].trim_start().len();
    let mut start = skip_space(at);
    let is_format = code[start..].starts_with("format!(");
    if is_format {
        start = skip_space(start + "format!(".len());
    }
    if !code[start..].starts_with('"') {
        return None;
    }
    let close = closing_quote(code.as_bytes(), start + 1);
    Some(Literal {
        start,
        text: code.get(start + 1..close)?,
        is_format,
    })
}

fn after_ident(code: &str, at: usize) -> bool {
    code[..at]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

fn line_at(code: &str, offset: usize) -> usize {
    code[..offset].matches('\n').count() + 1
}

fn excused(line: &str) -> bool {
    line.split_once("// i18n-ok:")
        .is_some_and(|(_, reason)| !reason.trim().is_empty())
}

/// A literal as source text, without its `\n` and `\u{…}` escapes.
fn without_escapes(raw: &str) -> String {
    let mut text = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
        } else if chars.next() == Some('u') {
            chars.by_ref().find(|c| *c == '}');
        }
    }
    text
}

fn has_letter(raw: &str) -> bool {
    without_escapes(raw).chars().any(char::is_alphabetic)
}

/// Two letters in a row outside `{…}`.
fn has_words(raw: &str) -> bool {
    let text = without_escapes(raw).replace("{{", " ").replace("}}", " ");
    let (mut in_value, mut after_letter) = (false, false);
    for c in text.chars() {
        in_value = (in_value || c == '{') && c != '}';
        let letter = !in_value && c.is_alphabetic();
        if letter && after_letter {
            return true;
        }
        after_letter = letter;
    }
    false
}

/// The index of the `"` that ends a string whose text starts at `i`.
fn closing_quote(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// `source` with its comments, and on request the text of its strings, as
/// spaces, so every offset and line stays.
fn blank(source: &str, strings: bool) -> String {
    let bytes = source.as_bytes();
    let mut out = source.to_string().into_bytes();
    let mut clear = |range: std::ops::Range<usize>| {
        for b in &mut out[range] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        let rest = &source[i..];
        if rest.starts_with("//") {
            let end = rest.find('\n').map_or(bytes.len(), |n| i + n);
            clear(i..end);
            i = end;
        } else if rest.starts_with("/*") {
            let end = rest.find("*/").map_or(bytes.len(), |n| i + n + 2);
            clear(i..end);
            i = end;
        } else if rest.starts_with('"') {
            let end = closing_quote(bytes, i + 1).min(bytes.len());
            if strings {
                clear(i + 1..end);
            }
            i = end + 1;
        } else if let Some(end) = raw_string_end(source, i) {
            if strings {
                clear(i..end);
            }
            i = end;
        } else if rest.starts_with('\'') {
            i += char_literal_len(rest);
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

/// The offset past the raw string that starts at `at`, if one does.
fn raw_string_end(source: &str, at: usize) -> Option<usize> {
    if after_ident(source, at) {
        return None;
    }
    let rest = source[at..].strip_prefix('r')?;
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    let body = rest[hashes..].strip_prefix('"')?;
    let close = format!("\"{}", "#".repeat(hashes));
    let end = body.find(&close).map_or(body.len(), |n| n + close.len());
    Some(at + 1 + hashes + 1 + end)
}

/// `'"'`, `'\''` and the like, so that a quote in one opens no string.
fn char_literal_len(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    match (bytes.get(1), bytes.get(2), bytes.get(3)) {
        (Some(b'\\'), Some(_), Some(b'\'')) => 4,
        (Some(_), Some(b'\''), _) => 3,
        _ => 1,
    }
}

fn line_of(yaml: &str, key: &str) -> usize {
    yaml.lines()
        .position(|l| l.split_once(':').is_some_and(|(k, _)| k.trim() == key))
        .map_or(1, |i| i + 1)
}

/// Every `.rs` file under `dir`, sorted.
pub fn rust_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::{flat_keys, placeholders, run, sink_literals, t_keys};

    fn found(source: &str) -> Vec<(usize, String)> {
        sink_literals(source)
    }

    #[test]
    fn a_plain_sink_is_found() {
        assert_eq!(found(r#"x.child("Save")"#), [(1, "Save".to_string())]);
    }

    #[test]
    fn a_multi_line_sink_is_found_at_the_line_of_the_literal() {
        let source = "x\n    .child(\n        \"Save\",\n    )\n";
        assert_eq!(found(source), [(3, "Save".to_string())]);
    }

    #[test]
    fn every_sink_of_the_design_is_covered() {
        for sink in [
            ".child(",
            ".label(",
            ".tooltip(",
            ".tooltip_with_action(",
            ".placeholder(",
            ".title(",
            ".aria_label(",
            ".aria_description(",
            ".accessibility_label(",
            ".message(",
            ".description(",
            "Label::new(",
            ".push_notification(",
            ".set_window_title(",
        ] {
            assert_eq!(found(&format!("{sink}\"Oops\")")).len(), 1, "{sink}");
        }
    }

    #[test]
    fn no_letter_is_no_finding() {
        assert!(found(r#"x.child("  ·  ")"#).is_empty());
        assert!(found(r#"x.child("\n")"#).is_empty());
        assert!(found(r#"x.child("\u{2026}")"#).is_empty());
    }

    #[test]
    fn other_methods_are_not_sinks() {
        assert!(found(r#"x.id("documents.table")"#).is_empty());
        assert!(found(r#"MyLabel::new("x")"#).is_empty());
    }

    #[test]
    fn test_code_is_cut() {
        let source = "x.child(\"Real\")\n#[cfg(test)]\nmod tests { y.child(\"Save\") }\n";
        assert_eq!(found(source), [(1, "Real".to_string())]);
    }

    #[test]
    fn a_test_attribute_on_one_item_cuts_nothing() {
        let source = "#[cfg(test)]\nfn helper() {}\nx.child(\"Real\")\n";
        assert_eq!(found(source), [(3, "Real".to_string())]);
        let source = "#[cfg(test)]\n\nmod tests { y.child(\"Save\") }\n";
        assert_eq!(found(source), []);
    }

    #[test]
    fn comments_are_skipped_and_a_url_is_no_comment() {
        assert!(found("// x.child(\"Save\")\n").is_empty());
        assert!(found("/* x.child(\"Save\") */\n").is_empty());
        assert_eq!(
            found(r#"x.child("http://a").child("Save")"#),
            [(1, "http://a".to_string()), (1, "Save".to_string())]
        );
        assert_eq!(
            found("x.child(\n    // why\n    \"Save\",\n)"),
            [(3, "Save".to_string())]
        );
    }

    #[test]
    fn strings_that_look_like_quotes_do_not_derail_the_scan() {
        let source = "let a = '\"';\nlet b = r#\"\"// \"#;\nx.child(\"Sa\\\"ve\")\n";
        assert_eq!(found(source), [(3, "Sa\\\"ve".to_string())]);
    }

    #[test]
    fn a_format_with_words_is_found() {
        assert_eq!(
            found(r#"x.child(format!("{} files", n))"#),
            [(1, "{} files".to_string())]
        );
        assert_eq!(
            found("x.child(\n    format!(\n        \"{n} files\"\n    ))").len(),
            1
        );
    }

    #[test]
    fn a_format_of_only_values_is_no_finding() {
        assert!(found(r#"x.child(format!("{a}.{b}"))"#).is_empty());
        assert!(found(r#"x.child(format!("v{}", n))"#).is_empty());
        assert!(found(r#"x.child(format!("{a:?} · {b:>4}"))"#).is_empty());
    }

    #[test]
    fn escaped_braces_are_text() {
        assert_eq!(found(r#"x.child(format!("{{files}} {n}"))"#).len(), 1);
    }

    #[test]
    fn i18n_ok_needs_a_reason() {
        assert!(found("x.child(\"id\") // i18n-ok: API name\n").is_empty());
        assert!(found("// i18n-ok: API name\nx.child(\"id\")\n").is_empty());
        assert_eq!(found("x.child(\"id\") // i18n-ok:\n").len(), 1);
        assert_eq!(found("// i18n-ok:   \nx.child(\"id\")\n").len(), 1);
        assert_eq!(found("// i18n-ok\nx.child(\"id\")\n").len(), 1);
    }

    #[test]
    fn i18n_ok_two_lines_up_does_not_pass() {
        assert_eq!(found("// i18n-ok: name\n\nx.child(\"id\")\n").len(), 1);
    }

    #[test]
    fn flat_keys_read_a_locale_file() {
        let yaml = "_version: 1\n# a comment\na.b: \"x: y\"\nc: \"say \\\"hi\\\"\"\n\n";
        let keys = flat_keys(yaml);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys["a.b"], "x: y");
        assert_eq!(keys["c"], "say \\\"hi\\\"");
    }

    #[test]
    fn placeholders_are_named() {
        assert_eq!(
            placeholders("%{a} and %{b}, 100% {c}")
                .into_iter()
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_ne!(placeholders("%{a}"), placeholders("%{b}"));
    }

    #[test]
    fn t_keys_are_literals_only() {
        let source = "t!(\"a.b\");\nt!(\n    \"c\",\n    n = 1,\n);\nt!(key);\nformat!(\"d\");\n";
        assert_eq!(
            t_keys(source),
            [(1, "a.b".to_string()), (3, "c".to_string())]
        );
    }

    #[test]
    fn t_keys_skip_comments_and_strings() {
        assert!(t_keys("// t!(\"a\")\n").is_empty());
        assert!(t_keys("l.contains(\"t!(\");\nl.split('\"');\n").is_empty());
    }

    #[test]
    fn a_sink_inside_a_string_is_no_sink() {
        assert!(found(r#"let a = "x.child(\"Save\")";"#).is_empty());
    }

    fn tree(en: &str, de: &str, same: &str, view: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        };
        write("assets/locales/en.yml", en);
        write("assets/locales/de.yml", de);
        write("xtask/i18n-same.txt", same);
        write("crates/tessera-app/src/view.rs", view);
        dir
    }

    fn messages(dir: &tempfile::TempDir) -> Vec<String> {
        run(dir.path())
            .expect("run")
            .into_iter()
            .map(|f| format!("{}:{}: {}", f.file.display(), f.line, f.message))
            .collect()
    }

    #[test]
    fn a_clean_tree_has_no_finding() {
        let dir = tree(
            "a: \"Save\"\nb: \"Status\"\n",
            "a: \"Sichern\"\nb: \"Status\"\n",
            "# same in both\nb\n",
            "x.child(t!(\"a\"))\n",
        );
        assert_eq!(messages(&dir), Vec::<String>::new());
    }

    #[test]
    fn a_key_in_one_file_is_reported() {
        let dir = tree("a: \"A\"\nb: \"B\"\n", "a: \"X\"\nc: \"C\"\n", "", "");
        let got = messages(&dir);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            got[0].contains("en.yml:2") && got[0].contains("`b`"),
            "{got:?}"
        );
        assert!(
            got[1].contains("de.yml:2") && got[1].contains("`c`"),
            "{got:?}"
        );
    }

    #[test]
    fn a_placeholder_mismatch_is_reported() {
        let dir = tree("a: \"%{a}\"\n", "a: \"X %{b}\"\n", "", "");
        let got = messages(&dir);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].contains("de.yml:1") && got[0].contains("placeholder"),
            "{got:?}"
        );
    }

    #[test]
    fn an_unknown_key_is_reported() {
        let dir = tree("a: \"A\"\n", "a: \"X\"\n", "", "t!(\"no.such\")\n");
        let got = messages(&dir);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].contains("view.rs:1") && got[0].contains("no.such"),
            "{got:?}"
        );
    }

    #[test]
    fn a_dynamic_key_is_skipped() {
        let dir = tree("a: \"A\"\n", "a: \"X\"\n", "", "t!(key)\n");
        assert_eq!(messages(&dir), Vec::<String>::new());
    }

    #[test]
    fn a_hardcoded_string_is_reported() {
        let dir = tree("a: \"A\"\n", "a: \"X\"\n", "", "\nx.child(\"Oops\")\n");
        let got = messages(&dir);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].contains("view.rs:2") && got[0].contains("Oops"),
            "{got:?}"
        );
    }

    #[test]
    fn a_stale_entry_of_the_same_list_is_reported() {
        let dir = tree(
            "a: \"Status\"\nb: \"Save\"\n",
            "a: \"Status\"\nb: \"Sichern\"\n",
            "# same in both\na\nb\ngone\n",
            "",
        );
        let got = messages(&dir);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].contains("i18n-same.txt:3") && got[0].contains("`b` differs"));
        assert!(got[1].contains("i18n-same.txt:4") && got[1].contains("`gone` is in no"));
    }

    #[test]
    fn an_equal_value_needs_its_key_listed() {
        let dir = tree("a: \"Same\"\n", "a: \"Same\"\n", "", "");
        let got = messages(&dir);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].contains("de.yml:1") && got[0].contains("i18n-same"),
            "{got:?}"
        );
    }
}
