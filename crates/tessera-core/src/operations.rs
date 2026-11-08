// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Ops tab checks before it sends anything, and how each operation confirms (D85).

use serde_json::Value;

/// The longest snapshot path, in bytes.
pub const SNAPSHOT_PATH_MAX: usize = 1024;

/// Why a snapshot path is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathProblem {
    /// Nothing typed: the server would answer success and export nothing.
    Empty,
    /// Does not start with `/`.
    NotAbsolute,
    /// Only slashes and `.` segments.
    Root,
    /// A segment is `..`.
    Climbs,
    /// A control character.
    Control,
    /// Above [`SNAPSHOT_PATH_MAX`] bytes.
    TooLong,
}

/// Whether `path` may go to the server. Tessera cannot see the server's disk, so this checks
/// the text alone.
pub fn check_snapshot_path(path: &str) -> Result<(), PathProblem> {
    if path.is_empty() {
        return Err(PathProblem::Empty);
    }
    if !path.starts_with('/') {
        return Err(PathProblem::NotAbsolute);
    }
    if path.split('/').all(|segment| matches!(segment, "" | ".")) {
        return Err(PathProblem::Root);
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err(PathProblem::Climbs);
    }
    if path.chars().any(char::is_control) {
        return Err(PathProblem::Control);
    }
    if path.len() > SNAPSHOT_PATH_MAX {
        return Err(PathProblem::TooLong);
    }
    Ok(())
}

/// The type of a runtime config value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKind {
    /// An integer from `min` up to `i32::MAX`.
    Integer {
        /// The lowest value offered.
        min: Option<i64>,
    },
    /// `true` or `false`.
    Bool,
}

/// A key of `POST /config` that Tessera offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKey {
    /// The name the server takes.
    pub name: &'static str,
    /// The type of its value.
    pub kind: ConfigKind,
}

const fn integer(name: &'static str, min: i64) -> ConfigKey {
    ConfigKey {
        name,
        kind: ConfigKind::Integer { min: Some(min) },
    }
}

/// The closed list of keys (D86): the server accepts any name, and `skip-writes` is left out.
pub const CONFIG_KEYS: [ConfigKey; 7] = [
    integer("log-slow-requests-time-ms", -1),
    integer("log-slow-searches-time-ms", -1),
    ConfigKey {
        name: "enable-search-logging",
        kind: ConfigKind::Bool,
    },
    integer("cache-num-entries", 1),
    integer("embedding-cache-num-entries", 1),
    integer("healthy-read-lag", 1),
    integer("healthy-write-lag", 1),
];

/// Why a config value is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigProblem {
    /// Not a whole number.
    NotInteger,
    /// Below the key's lowest value.
    TooLow(i64),
    /// Above `i32::MAX`.
    TooHigh,
    /// Neither `true` nor `false`.
    NotBool,
}

/// The JSON value `text` stands for under `key`.
pub fn parse_config(key: &ConfigKey, text: &str) -> Result<Value, ConfigProblem> {
    match key.kind {
        ConfigKind::Bool => match text {
            "true" => Ok(true.into()),
            "false" => Ok(false.into()),
            _ => Err(ConfigProblem::NotBool),
        },
        ConfigKind::Integer { min } => {
            let value: i64 = text.parse().map_err(|e: std::num::ParseIntError| {
                // A number too long for i64 is a number, only too high or too low.
                match e.kind() {
                    std::num::IntErrorKind::PosOverflow => ConfigProblem::TooHigh,
                    std::num::IntErrorKind::NegOverflow => {
                        ConfigProblem::TooLow(min.unwrap_or(i64::MIN))
                    }
                    _ => ConfigProblem::NotInteger,
                }
            })?;
            if let Some(min) = min.filter(|min| value < *min) {
                return Err(ConfigProblem::TooLow(min));
            }
            if value > i64::from(i32::MAX) {
                return Err(ConfigProblem::TooHigh);
            }
            Ok(value.into())
        }
    }
}

/// An operation of the Ops tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Copies the data to a directory on the node.
    Snapshot,
    /// Asks the node to become leader.
    Vote,
    /// Compacts the on-disk database.
    Compact,
    /// Clears the search response cache.
    ClearCache,
    /// Sets one runtime config key.
    Config,
}

/// How an operation is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    /// It runs at once.
    None,
    /// A plain confirmation.
    Plain,
    /// The profile name is typed.
    Typed,
}

/// D85's table.
pub fn confirm_for(op: Operation, prod: bool) -> Confirm {
    match (op, prod) {
        (Operation::Snapshot | Operation::Vote, true) => Confirm::Typed,
        (Operation::Config, false) => Confirm::None,
        _ => Confirm::Plain,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn snapshot_path_table() {
        use PathProblem::*;
        let long = format!("/{}", "a".repeat(SNAPSHOT_PATH_MAX - 1));
        let too_long = format!("{long}a");
        let multibyte = format!("/{}", "ü".repeat(SNAPSHOT_PATH_MAX / 2));
        for (path, expected) in [
            ("", Err(Empty)),
            ("relative/x", Err(NotAbsolute)),
            ("./x", Err(NotAbsolute)),
            ("/", Err(Root)),
            ("///", Err(Root)),
            ("/.", Err(Root)),
            ("/./", Err(Root)),
            ("//.//./", Err(Root)),
            ("/a/../b", Err(Climbs)),
            ("/..", Err(Climbs)),
            ("/a\nb", Err(Control)),
            ("/a\0b", Err(Control)),
            ("/a\u{7f}", Err(Control)),
            (too_long.as_str(), Err(TooLong)),
            (multibyte.as_str(), Err(TooLong)),
            (long.as_str(), Ok(())),
            ("/var/backups/ts", Ok(())),
            ("/var/my backups/ü", Ok(())),
            ("/a/..b/c..", Ok(())),
            ("/a/./b", Ok(())),
            ("/a/", Ok(())),
        ] {
            assert_eq!(check_snapshot_path(path), expected, "{path:?}");
        }
    }

    #[test]
    fn config_keys_in_order_without_skip_writes() {
        let names: Vec<_> = CONFIG_KEYS.iter().map(|k| k.name).collect();
        assert_eq!(
            names,
            [
                "log-slow-requests-time-ms",
                "log-slow-searches-time-ms",
                "enable-search-logging",
                "cache-num-entries",
                "embedding-cache-num-entries",
                "healthy-read-lag",
                "healthy-write-lag",
            ]
        );
        assert!(!names.contains(&"skip-writes"));
    }

    #[test]
    fn parse_config_table() {
        use ConfigProblem::*;
        let (log, cache, flag) = (&CONFIG_KEYS[0], &CONFIG_KEYS[3], &CONFIG_KEYS[2]);
        for (key, text, expected) in [
            (log, "-1", Ok(json!(-1))),
            (log, "0", Ok(json!(0))),
            (log, "-2", Err(TooLow(-1))),
            (log, "2147483647", Ok(json!(2147483647))),
            (log, "2147483648", Err(TooHigh)),
            (log, "99999999999999999999", Err(TooHigh)),
            (log, "-99999999999999999999", Err(TooLow(-1))),
            (cache, "1", Ok(json!(1))),
            (cache, "0", Err(TooLow(1))),
            (cache, "1e3", Err(NotInteger)),
            (cache, "1.0", Err(NotInteger)),
            (cache, "", Err(NotInteger)),
            (cache, " 1", Err(NotInteger)),
            (cache, "abc", Err(NotInteger)),
            (flag, "true", Ok(json!(true))),
            (flag, "false", Ok(json!(false))),
            (flag, "True", Err(NotBool)),
            (flag, "1", Err(NotBool)),
            (flag, "", Err(NotBool)),
        ] {
            assert_eq!(parse_config(key, text), expected, "{} {text:?}", key.name);
        }
    }

    #[test]
    fn confirm_table() {
        use Operation::*;
        for (op, not_prod, prod) in [
            (Snapshot, Confirm::Plain, Confirm::Typed),
            (Vote, Confirm::Plain, Confirm::Typed),
            (Compact, Confirm::Plain, Confirm::Plain),
            (ClearCache, Confirm::Plain, Confirm::Plain),
            (Config, Confirm::None, Confirm::Plain),
        ] {
            assert_eq!(confirm_for(op, false), not_prod, "{op:?}");
            assert_eq!(confirm_for(op, true), prod, "{op:?} on prod");
        }
    }
}
