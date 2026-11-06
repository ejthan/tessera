// SPDX-License-Identifier: GPL-3.0-or-later

//! Facet clicks as edits of the `filter_by` text, which is the only filter state.

/// The element type of a field type: `int32[]` is `int32`.
fn base_type(field_type: &str) -> &str {
    field_type.strip_suffix("[]").unwrap_or(field_type)
}

fn is_numeric(base: &str) -> bool {
    matches!(base, "int32" | "int64" | "float")
}

/// Whether `value` of a field of `field_type` can be written into `filter_by` and read back
/// as the same value. Typesense mishandles a backslash or a backtick inside backticks and
/// refuses `*`, so strings with them are not offered, rather than quoted wrongly.
pub fn facet_value_ok(field_type: &str, value: &str) -> bool {
    match base_type(field_type) {
        "int32" | "int64" => value.parse::<i64>().is_ok_and(|n| n.to_string() == value),
        "float" => {
            value.parse::<f64>().is_ok_and(f64::is_finite)
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
        }
        "bool" => matches!(value, "true" | "false"),
        _ => {
            !value.trim().is_empty()
                && value == value.trim()
                && value != "*"
                && !value
                    .chars()
                    .any(|c| c == '`' || c == '\\' || c.is_control())
        }
    }
}

/// A field name that cannot change the structure of a filter.
fn field_ok(field: &str) -> bool {
    !field.is_empty()
        && !field
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "`:&|()[],=<>!\\".contains(c))
}

fn literal(base: &str, value: &str) -> String {
    if is_numeric(base) || base == "bool" {
        value.to_string()
    } else {
        format!("`{value}`")
    }
}

/// `` field:=`a` ``, or `` field:=[`a`,`b`] `` for several values; numbers and booleans are
/// not quoted. `None` for no value, or when any value or the field name is not allowed.
pub fn facet_clause(field: &str, field_type: &str, values: &[&str]) -> Option<String> {
    if values.is_empty()
        || !field_ok(field)
        || !values.iter().all(|v| facet_value_ok(field_type, v))
    {
        return None;
    }
    let base = base_type(field_type);
    let literals: Vec<String> = values.iter().map(|v| literal(base, v)).collect();
    Some(match literals.as_slice() {
        [one] => format!("{field}:={one}"),
        many => format!("{field}:=[{}]", many.join(",")),
    })
}

/// Splits `text` at `sep` outside backticks, parentheses and brackets. `None` when they do
/// not balance.
fn split_top<'a>(text: &'a str, sep: &str) -> Option<Vec<&'a str>> {
    let (mut depth, mut tick, mut start, mut i) = (0usize, false, 0, 0);
    let mut parts = Vec::new();
    while i < text.len() {
        let c = text[i..].chars().next()?;
        if c == '`' {
            tick = !tick;
        } else if !tick {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.checked_sub(1)?,
                _ if depth == 0 && text[i..].starts_with(sep) => {
                    parts.push(&text[start..i]);
                    i += sep.len();
                    start = i;
                    continue;
                }
                _ => {}
            }
        }
        i += c.len_utf8();
    }
    if tick || depth != 0 {
        return None;
    }
    parts.push(&text[start..]);
    Some(parts)
}

/// The top-level `&&` clauses of `filter`, trimmed. `None` for a filter this module will not
/// edit: unbalanced, or with a top-level `||`.
fn and_clauses(filter: &str) -> Option<Vec<&str>> {
    if split_top(filter, "||")?.len() > 1 {
        return None;
    }
    Some(
        split_top(filter, "&&")?
            .into_iter()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect(),
    )
}

/// The values of a clause this module wrote for `field`, in `` field:=`a` `` or
/// `` field:=[`a`,`b`] `` form. `None` for anything else, or a value it would not write.
fn own_values(clause: &str, field: &str) -> Option<Vec<String>> {
    let rest = clause.strip_prefix(field)?.strip_prefix(":=")?;
    let items = match rest.strip_prefix('[') {
        Some(inner) => split_top(inner.strip_suffix(']')?, ",")?,
        None => vec![rest],
    };
    items
        .into_iter()
        .map(|item| {
            let item = item.trim();
            let value = match item.strip_prefix('`') {
                Some(quoted) => quoted.strip_suffix('`')?,
                None => item,
            };
            // Only a value this module could have written, so a rewrite keeps it.
            facet_value_ok("string", value).then(|| value.to_string())
        })
        .collect()
}

/// Where `field`'s own clause sits among `clauses`, with its values.
fn find_own(clauses: &[&str], field: &str) -> Option<(usize, Vec<String>)> {
    clauses
        .iter()
        .enumerate()
        .find_map(|(i, c)| own_values(c, field).map(|values| (i, values)))
}

/// Adds `value` to `field`'s clause of `filter`, or removes it when it is already there. The
/// clause is created, rewritten as a list, or dropped with its `&&` when its last value goes.
/// A filter with a top-level `||` is wrapped in parentheses first; a clause this module cannot
/// read stays as it is and the new one is appended. `None` when `value` or `field` is not
/// allowed, or the filter's brackets do not balance.
pub fn toggle_facet(filter: &str, field: &str, field_type: &str, value: &str) -> Option<String> {
    if !facet_value_ok(field_type, value) || !field_ok(field) {
        return None;
    }
    let filter = filter.trim();
    let wrapped;
    let filter = if filter.is_empty() || split_top(filter, "||")?.len() == 1 {
        filter
    } else {
        wrapped = format!("({filter})");
        &wrapped
    };
    let mut clauses: Vec<String> = and_clauses(filter)?.into_iter().map(String::from).collect();
    let refs: Vec<&str> = clauses.iter().map(String::as_str).collect();
    match find_own(&refs, field) {
        Some((at, mut values)) => {
            match values.iter().position(|v| v == value) {
                Some(i) => {
                    values.remove(i);
                }
                None => values.push(value.to_string()),
            }
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            if values.is_empty() {
                clauses.remove(at);
            } else {
                clauses[at] = facet_clause(field, field_type, &values)?;
            }
        }
        None => clauses.push(facet_clause(field, field_type, &[value])?),
    }
    Some(clauses.join(" && "))
}

/// Whether `value` is among the values of `field`'s own clause of `filter`.
pub fn facet_active(filter: &str, field: &str, value: &str) -> bool {
    and_clauses(filter)
        .and_then(|clauses| find_own(&clauses, field))
        .is_some_and(|(_, values)| values.iter().any(|v| v == value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facet_value_ok_table() {
        let cases = [
            ("string", "sci fi", true),
            ("string", "a,b", true),
            ("string", "(p)", true),
            ("string", "x&&y", true),
            ("string", "*", false),
            ("string", "back\\", false),
            ("string", "a`b", false),
            ("string", " a", false),
            ("string", "a ", false),
            ("string", "", false),
            ("string[]", "tag", true),
            ("int32", "1965", true),
            ("int32", "19x", false),
            ("int32", "1.5", false),
            ("int64[]", "-7", true),
            ("float", "1.5", true),
            ("float", "NaN", false),
            ("float", "1e5", false),
            ("bool", "true", true),
            ("bool", "yes", false),
        ];
        for (ty, value, want) in cases {
            assert_eq!(facet_value_ok(ty, value), want, "{ty} {value:?}");
        }
    }

    #[test]
    fn facet_clause_forms() {
        assert_eq!(
            facet_clause("genre", "string", &["sci fi"]).as_deref(),
            Some("genre:=`sci fi`")
        );
        assert_eq!(
            facet_clause("genre", "string", &["a", "b"]).as_deref(),
            Some("genre:=[`a`,`b`]")
        );
        assert_eq!(
            facet_clause("year", "int32", &["1965"]).as_deref(),
            Some("year:=1965")
        );
        assert_eq!(
            facet_clause("year", "int32[]", &["1", "2"]).as_deref(),
            Some("year:=[1,2]")
        );
        assert_eq!(
            facet_clause("ok", "bool", &["true"]).as_deref(),
            Some("ok:=true")
        );
        assert_eq!(facet_clause("genre", "string", &["a", "b\\"]), None);
        assert_eq!(facet_clause("genre", "string", &[]), None);
        assert_eq!(facet_clause("a b", "string", &["x"]), None);
    }

    /// Toggles in turn and checks the filter and `facet_active` after each step.
    #[test]
    fn toggle_facet_table() {
        let t = |filter: &str, field: &str, ty: &str, value: &str| {
            toggle_facet(filter, field, ty, value).unwrap()
        };
        let one = t("", "genre", "string", "sci fi");
        assert_eq!(one, "genre:=`sci fi`");
        assert!(facet_active(&one, "genre", "sci fi"));
        let two = t(&one, "genre", "string", "crime");
        assert_eq!(two, "genre:=[`sci fi`,`crime`]");
        assert!(facet_active(&two, "genre", "sci fi") && facet_active(&two, "genre", "crime"));
        let back = t(&two, "genre", "string", "sci fi");
        assert_eq!(back, "genre:=`crime`");
        assert!(!facet_active(&back, "genre", "sci fi"));
        assert_eq!(t(&back, "genre", "string", "crime"), "");
    }

    #[test]
    fn removing_the_last_value_drops_the_clause_and_its_join() {
        let t = |f: &str| toggle_facet(f, "genre", "string", "x").unwrap();
        assert_eq!(t("year:>1900 && genre:=`x`"), "year:>1900");
        assert_eq!(t("genre:=`x` && year:>1900"), "year:>1900");
        assert_eq!(t("a:1 && genre:=`x` && b:2"), "a:1 && b:2");
    }

    #[test]
    fn another_fields_clause_stays() {
        let got = toggle_facet("year:=1965", "genre", "string", "x").unwrap();
        assert_eq!(got, "year:=1965 && genre:=`x`");
        let got = toggle_facet("year:=[1965,1970] && genre:=`x`", "year", "int32", "1970").unwrap();
        assert_eq!(got, "year:=1965 && genre:=`x`");
        assert!(facet_active(&got, "year", "1965") && !facet_active(&got, "year", "1970"));
    }

    #[test]
    fn a_top_level_or_is_wrapped() {
        let got = toggle_facet("a:1 || b:2", "genre", "string", "x").unwrap();
        assert_eq!(got, "(a:1 || b:2) && genre:=`x`");
        // The wrapped filter has no top-level `||`, so the next click edits our clause.
        let again = toggle_facet(&got, "genre", "string", "y").unwrap();
        assert_eq!(again, "(a:1 || b:2) && genre:=[`x`,`y`]");
        assert!(!facet_active("a:1 || genre:=`x`", "genre", "x"));
    }

    #[test]
    fn a_backticked_join_in_a_value_does_not_split() {
        let filter = "genre:=[`x&&y`,`b||c`]";
        assert!(facet_active(filter, "genre", "x&&y"));
        assert_eq!(
            toggle_facet(filter, "genre", "string", "b||c").unwrap(),
            "genre:=`x&&y`"
        );
        assert_eq!(
            toggle_facet("genre:=`x&&y`", "year", "int32", "1").unwrap(),
            "genre:=`x&&y` && year:=1"
        );
    }

    #[test]
    fn a_clause_it_cannot_read_stays_and_the_new_one_is_appended() {
        let got = toggle_facet("(genre:=x)", "genre", "string", "y").unwrap();
        assert_eq!(got, "(genre:=x) && genre:=`y`");
        let got = toggle_facet("genre:!=`x`", "genre", "string", "y").unwrap();
        assert_eq!(got, "genre:!=`x` && genre:=`y`");
    }

    #[test]
    fn unsafe_input_is_refused() {
        assert_eq!(toggle_facet("", "genre", "string", "*"), None);
        assert_eq!(toggle_facet("", "genre", "string", "a\\"), None);
        assert_eq!(toggle_facet("", "genre", "string", "a`b"), None);
        assert_eq!(toggle_facet("", "year", "int32", "x"), None);
        assert_eq!(toggle_facet("(a:1", "genre", "string", "x"), None);
        assert_eq!(toggle_facet("", "g`:=1 || x", "string", "x"), None);
    }

    #[test]
    fn numbers_and_bools_stay_unquoted() {
        let got = toggle_facet("", "year", "int32", "1965").unwrap();
        assert_eq!(got, "year:=1965");
        assert!(facet_active(&got, "year", "1965"));
        let got = toggle_facet(&got, "year", "int32", "1970").unwrap();
        assert_eq!(got, "year:=[1965,1970]");
        assert_eq!(toggle_facet("", "ok", "bool", "true").unwrap(), "ok:=true");
    }
}
