// SPDX-License-Identifier: GPL-3.0-or-later

//! What a cursor inside a `filter_by` expression can be completed with.

/// What the text before the cursor expects next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterContext {
    /// A field name.
    Field {
        /// Byte offset a completion replaces from.
        start: usize,
        /// The name so far.
        prefix: String,
    },
    /// The operator of `field`.
    Operator {
        /// Byte offset of the operator's `:`.
        start: usize,
        /// Name of the field.
        field: String,
    },
    /// A value of `field`; inside a list, the current element.
    Value {
        /// Byte offset a completion replaces from, the opening backtick if there is one.
        start: usize,
        /// Name of the field.
        field: String,
        /// The value so far, without the backtick.
        prefix: String,
    },
    /// After a finished clause: `&&` or `||`.
    Join {
        /// Byte offset a completion replaces from.
        start: usize,
    },
}

/// Where the forward scan stands.
#[derive(Clone, Copy)]
enum Scan {
    Field,
    Operator,
    /// Before a value or list element: spaces are skipped and `[` or a backtick may open it.
    BeforeValue,
    Value,
    Quoted,
    Join,
}

/// The context at byte `offset` of `text`. An offset outside the text reads all of it.
pub fn context_at(text: &str, offset: usize) -> FilterContext {
    let head = text.get(..offset).unwrap_or(text);
    let mut scan = Scan::Field;
    let mut start = 0;
    let mut field = "";
    let mut in_list = false;
    let mut i = 0;
    while let Some(c) = head[i..].chars().next() {
        let next = i + c.len_utf8();
        let joins = head[i..].starts_with("&&") || head[i..].starts_with("||");
        match scan {
            Scan::Field => match c {
                ':' => {
                    field = &head[start..i];
                    start = i;
                    scan = Scan::Operator;
                }
                ')' => {
                    start = next;
                    scan = Scan::Join;
                }
                '(' => start = next,
                c if c.is_whitespace() => start = next,
                _ => {}
            },
            Scan::Operator => {
                if !matches!(c, ':' | '=' | '!' | '<' | '>') {
                    start = i;
                    scan = Scan::BeforeValue;
                    continue;
                }
            }
            Scan::BeforeValue => match c {
                c if c.is_whitespace() => start = next,
                '[' if !in_list => {
                    in_list = true;
                    start = next;
                }
                '`' => scan = Scan::Quoted,
                _ => {
                    scan = Scan::Value;
                    continue;
                }
            },
            Scan::Value if in_list => match c {
                ',' => {
                    start = next;
                    scan = Scan::BeforeValue;
                }
                ']' => {
                    in_list = false;
                    start = next;
                    scan = Scan::Join;
                }
                _ => {}
            },
            Scan::Value if joins => {
                scan = Scan::Join;
                continue;
            }
            Scan::Value => {
                if c == ')' || c.is_whitespace() {
                    start = next;
                    scan = Scan::Join;
                }
            }
            Scan::Quoted => {
                if c == '`' {
                    if in_list {
                        scan = Scan::Value;
                    } else {
                        start = next;
                        scan = Scan::Join;
                    }
                }
            }
            Scan::Join if joins => {
                i += 2;
                start = i;
                scan = Scan::Field;
                continue;
            }
            Scan::Join => {
                if c == ')' || c.is_whitespace() {
                    start = next;
                }
            }
        }
        i = next;
    }
    match scan {
        Scan::Field => FilterContext::Field {
            start,
            prefix: head[start..].to_string(),
        },
        Scan::Operator => FilterContext::Operator {
            start,
            field: field.to_string(),
        },
        Scan::BeforeValue | Scan::Value => FilterContext::Value {
            start,
            field: field.to_string(),
            prefix: head[start..].to_string(),
        },
        Scan::Quoted => FilterContext::Value {
            start,
            field: field.to_string(),
            prefix: head[start + 1..].to_string(),
        },
        Scan::Join => FilterContext::Join { start },
    }
}

/// The operators of a field of `field_type`.
pub fn operators_for(field_type: &str) -> &'static [&'static str] {
    match field_type.trim_end_matches("[]") {
        "int32" | "int64" | "float" => &[":=", ":>", ":>=", ":<", ":<=", ":!=", ":["],
        "bool" => &[":=", ":!="],
        _ => &[":", ":=", ":!="],
    }
}

/// `value` as `filter_by` needs it written. `None` for a value with a backtick: `filter_by`
/// has no escape, and such a value matches nothing however it is written.
pub fn quote_value(value: &str) -> Option<String> {
    if value.contains('`') {
        None
    } else if value.contains([' ', ',', '[', ']', '(', ')', '&', '|']) {
        Some(format!("`{value}`"))
    } else {
        Some(value.to_string())
    }
}

/// Whether `filter`, the target of a bulk write, names every document instead of some. The
/// server treats these spellings differently (design §4.4), so none of them may reach it: a
/// blank filter counts everything and writes nothing, `*` has no count and updates everything,
/// and a `*` among the values of `id` counts and writes everything.
///
/// Everything behind `id:` is searched for a lone `*`, because a quoted value can hold `]` or
/// `&`, so the end of the list cannot be told without parsing it. A `*` in a later clause is
/// refused too, which costs only that clause.
pub fn names_everything(filter: &str) -> bool {
    let bare: String = filter
        .chars()
        .filter(|c| !c.is_whitespace() && !"`()".contains(*c))
        .collect();
    let own_field =
        |at: usize| !filter[..at].ends_with(|c: char| c.is_alphanumeric() || "_.".contains(c));
    let lone_star = |values: &str| {
        !values.trim_start().starts_with("!=")
            && values
                .split(|c: char| c.is_whitespace() || ",[]&|=`()".contains(c))
                .any(|value| value == "*")
    };
    bare.is_empty()
        || bare == "*"
        || filter.match_indices("id").any(|(at, _)| {
            let values = filter[at + "id".len()..].trim_start().strip_prefix(':');
            own_field(at) && values.is_some_and(lone_star)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use FilterContext::{Field, Join, Operator, Value};

    fn field(start: usize, prefix: &str) -> FilterContext {
        Field {
            start,
            prefix: prefix.into(),
        }
    }

    fn operator(start: usize, field: &str) -> FilterContext {
        Operator {
            start,
            field: field.into(),
        }
    }

    fn value(start: usize, field: &str, prefix: &str) -> FilterContext {
        Value {
            start,
            field: field.into(),
            prefix: prefix.into(),
        }
    }

    #[test]
    fn context_at_the_end_of_the_text() {
        let cases = [
            ("", field(0, "")),
            ("pri", field(0, "pri")),
            ("price:", operator(5, "price")),
            ("price:>", operator(5, "price")),
            ("price:>1", value(7, "price", "1")),
            ("genre:=cr", value(7, "genre", "cr")),
            ("genre:[a,b", value(9, "genre", "b")),
            ("genre:`science fi", value(6, "genre", "science fi")),
            ("a:1 ", Join { start: 4 }),
            ("a:1 &", Join { start: 4 }),
            ("a:1 && pr", field(7, "pr")),
            ("a:1&&pr", field(5, "pr")),
            ("(a:1 || b", field(8, "b")),
            ("a:1 && (b:2)", Join { start: 12 }),
            ("a:[1, 2] ", Join { start: 9 }),
            ("a:`x y` ", Join { start: 8 }),
            ("a:[`x y`, b", value(10, "a", "b")),
            ("a:[", value(3, "a", "")),
            ("a:= ", value(4, "a", "")),
        ];
        for (text, want) in cases {
            assert_eq!(context_at(text, text.len()), want, "{text}");
        }
    }

    #[test]
    fn only_the_text_before_the_cursor_counts() {
        assert_eq!(context_at("year:>2000 && gen", 4), field(0, "year"));
        assert_eq!(context_at("gen && a:1", 3), field(0, "gen"));
    }

    #[test]
    fn an_offset_outside_the_text_reads_all_of_it() {
        assert_eq!(context_at("pri", 99), field(0, "pri"));
    }

    #[test]
    fn multibyte_values_keep_byte_offsets() {
        let text = "genre:=mü && pr";
        assert_eq!(context_at(text, text.len()), field(14, "pr"));
    }

    #[test]
    fn operators_follow_the_type() {
        assert_eq!(operators_for("string").len(), 3);
        assert_eq!(operators_for("int32").len(), 7);
        assert_eq!(operators_for("bool").len(), 2);
        assert_eq!(operators_for("string[]"), operators_for("string"));
        assert_eq!(operators_for("float[]"), operators_for("int64"));
    }

    #[test]
    fn values_are_quoted_when_they_need_it() {
        let quoted = |value| quote_value(value).unwrap();
        assert_eq!(quoted("crime"), "crime");
        assert_eq!(quoted("science fiction"), "`science fiction`");
        assert_eq!(quoted("a,b"), "`a,b`");
        assert_eq!(quoted("a[1]"), "`a[1]`");
        assert_eq!(quoted("(a)"), "`(a)`");
        assert_eq!(quoted("a&b"), "`a&b`");
        assert_eq!(quoted("a|b"), "`a|b`");
    }

    #[test]
    fn a_backtick_cannot_be_quoted() {
        assert_eq!(quote_value("a`b"), None);
    }

    #[test]
    fn names_everything_catches_each_spelling_the_server_takes_for_all() {
        for filter in [
            "",
            "  ",
            "\t",
            "*",
            " * ",
            "()",
            "id:*",
            "id: *",
            "id:=*",
            "id:= *",
            "id:`*`",
            "id:= `*`",
            "id:[*]",
            "id:[ * ]",
            "id:[`*`]",
            "id:=[*]",
            "id:[a,*]",
            "(id:*)",
            "n:>1 || id:*",
            "id:* || n:>1",
        ] {
            assert!(names_everything(filter), "{filter:?}");
        }
    }

    #[test]
    fn names_everything_leaves_real_filters_alone() {
        for filter in [
            "n:>1",
            "id:a",
            "id:[a,b]",
            "id:*a",
            "id:a*",
            "id:!=*",
            "id:!=[*]",
            "valid:*",
            "a.id:*",
            "uuid:=*",
            "title:=`*`",
            "n:>=0",
        ] {
            assert!(!names_everything(filter), "{filter:?}");
        }
    }
}
