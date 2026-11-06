// SPDX-License-Identifier: GPL-3.0-or-later

//! Facet clicks as edits of the `filter_by` text, which is the only filter state.

use super::split_top;

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

/// The form `facet_clause` writes for a value: backticks around a string, nothing around a
/// number or a boolean. `kind` is the field's base type, or `None` when it is not known, which
/// accepts either form.
fn own_item(item: &str, kind: Option<&str>) -> Option<String> {
    let item = item.trim();
    let quoted = item
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'));
    let string_ok = |v: &str| facet_value_ok("string", v);
    let literal_type = |base: &str| is_numeric(base) || base == "bool";
    let bare_ok = |v: &str| match kind {
        Some(base) => literal_type(base) && facet_value_ok(base, v),
        None => ["int64", "float", "bool"]
            .iter()
            .any(|t| facet_value_ok(t, v)),
    };
    let wants_string = kind.is_none_or(|base| !literal_type(base));
    match quoted {
        Some(v) if wants_string && string_ok(v) => Some(v.to_string()),
        Some(_) => None,
        None => bare_ok(item).then(|| item.to_string()),
    }
}

/// The values of a clause this module wrote for `field` in `` field:=`a` `` or
/// `` field:=[`a`,`b`] `` form. `None` for anything else: a range, a prefix match, `!=`, or a
/// value of the wrong form for the type.
fn own_values(clause: &str, field: &str, kind: Option<&str>) -> Option<Vec<String>> {
    let rest = clause.strip_prefix(field)?.strip_prefix(":=")?;
    let items = match rest.strip_prefix('[') {
        Some(inner) => split_top(inner.strip_suffix(']')?, ",")?,
        None => vec![rest],
    };
    items.into_iter().map(|i| own_item(i, kind)).collect()
}

/// The positions of `field`'s own clauses among `clauses`, with all their values.
fn find_own(
    clauses: &[&str],
    field: &str,
    kind: Option<&str>,
) -> Option<(Vec<usize>, Vec<String>)> {
    let (mut at, mut values) = (Vec::new(), Vec::<String>::new());
    for (i, clause) in clauses.iter().enumerate() {
        if let Some(found) = own_values(clause, field, kind) {
            at.push(i);
            for v in found {
                if !values.contains(&v) {
                    values.push(v);
                }
            }
        }
    }
    (!at.is_empty()).then_some((at, values))
}

/// Adds `value` to `field`'s clause of `filter`, or removes it when it is already there. The
/// clause is created, rewritten as a list, or dropped with its `&&` when its last value goes.
/// Two own clauses of one field are merged into the first. A filter with a top-level `||` is
/// wrapped in parentheses first; a clause this module cannot read (a range, a prefix match, a
/// negation) stays as it is and the new one is appended. `None` when `value` or `field` is not
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
    let clauses: Vec<String> = and_clauses(filter)?.into_iter().map(String::from).collect();
    let refs: Vec<&str> = clauses.iter().map(String::as_str).collect();
    let mut out = clauses.clone();
    match find_own(&refs, field, Some(base_type(field_type))) {
        Some((at, mut values)) => {
            match values.iter().position(|v| v == value) {
                Some(i) => {
                    values.remove(i);
                }
                None => values.push(value.to_string()),
            }
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            if values.is_empty() {
                out.remove(at[0]);
            } else {
                out[at[0]] = facet_clause(field, field_type, &values)?;
            }
            for i in at[1..].iter().rev() {
                out.remove(*i);
            }
        }
        None => out.push(facet_clause(field, field_type, &[value])?),
    }
    Some(out.join(" && "))
}

/// Whether `value` is among the values of `field`'s own clauses of `filter`.
pub fn facet_active(filter: &str, field: &str, value: &str) -> bool {
    and_clauses(filter)
        .and_then(|clauses| find_own(&clauses, field, None))
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
    fn clauses_of_another_form_are_not_ours() {
        let t = |f: &str, ty: &str, v: &str| toggle_facet(f, f.split(':').next().unwrap(), ty, v);
        assert_eq!(
            t("year:=[1900..1950]", "int32", "1965").unwrap(),
            "year:=[1900..1950] && year:=1965"
        );
        assert_eq!(
            t("year:=>1920", "int32", "1965").unwrap(),
            "year:=>1920 && year:=1965"
        );
        // A prefix match stays a prefix match.
        assert_eq!(
            t("genre:=foo*", "string", "bar").unwrap(),
            "genre:=foo* && genre:=`bar`"
        );
        // A bare word is not the form written for a string; backticks around a number are not
        // the form written for an int.
        assert_eq!(
            t("genre:=bar", "string", "x").unwrap(),
            "genre:=bar && genre:=`x`"
        );
        assert_eq!(
            t("year:=`1965`", "int32", "1970").unwrap(),
            "year:=`1965` && year:=1970"
        );
        for f in [
            "year:=[1900..1950]",
            "year:=>1920",
            "genre:=foo*",
            "genre:=bar",
        ] {
            let field = f.split(':').next().unwrap();
            assert!(!facet_active(f, field, "foo*") && !facet_active(f, field, "1950"));
        }
        assert!(facet_active("year:=1965", "year", "1965"));
        assert!(facet_active("ok:=true", "ok", "true"));
        assert!(!facet_active("genre:=bar", "genre", "bar"));
    }

    #[test]
    fn two_own_clauses_merge_into_one() {
        let got = toggle_facet("genre:=`a` && x:1 && genre:=`b`", "genre", "string", "c").unwrap();
        assert_eq!(got, "genre:=[`a`,`b`,`c`] && x:1");
        let got = toggle_facet("genre:=`a` && genre:=`a`", "genre", "string", "b").unwrap();
        assert_eq!(got, "genre:=[`a`,`b`]");
        let got = toggle_facet("genre:=`a` && genre:=`b`", "genre", "string", "a").unwrap();
        assert_eq!(got, "genre:=`b`");
        assert!(facet_active("genre:=`a` && genre:=`b`", "genre", "b"));
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
