// SPDX-License-Identifier: GPL-3.0-or-later

//! The user's `keymap.toml`.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::store;

/// One `[[binding]]` of `keymap.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBinding {
    /// The chord, in GPUI's notation, such as `cmd-shift-p`.
    pub key: String,
    /// The action name, such as `tessera::OpenPalette`. `None` unbinds the chord.
    pub action: Option<String>,
    /// The key context it applies in, everywhere when `None`.
    pub context: Option<String>,
}

// Unknown fields are refused: `acton = "…"` would otherwise read as an unbind.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeymapFile {
    #[serde(default)]
    binding: Vec<UserBinding>,
}

/// The bindings in `text`. An error is `toml`'s, with the line and column.
pub fn parse_keymap(text: &str) -> Result<Vec<UserBinding>, String> {
    let file: KeymapFile = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(file.binding)
}

/// `keymap.toml` in the platform config directory.
pub fn keymap_path() -> Option<PathBuf> {
    Some(store::config_dir()?.join("keymap.toml"))
}

/// The bindings of the file at `path`. A missing file is no bindings.
pub fn read_keymap(path: &Path) -> Result<Vec<UserBinding>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_keymap(&text),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_bindings() {
        let bindings = parse_keymap(
            r#"
            [[binding]]
            key = "cmd-shift-p"
            action = "tessera::OpenPalette"

            [[binding]]
            key = "cmd-r"
            action = "tessera::Refresh"
            context = "Workbench"
            "#,
        )
        .unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].key, "cmd-shift-p");
        assert_eq!(bindings[0].action.as_deref(), Some("tessera::OpenPalette"));
        assert_eq!(bindings[0].context, None);
        assert_eq!(bindings[1].context.as_deref(), Some("Workbench"));
    }

    #[test]
    fn no_action_is_an_unbind() {
        let bindings = parse_keymap("[[binding]]\nkey = \"cmd-k\"\ncontext = \"Workbench\"\n");
        assert_eq!(bindings.unwrap()[0].action, None);
    }

    #[test]
    fn bad_toml_names_the_line() {
        let error = parse_keymap("[[binding]]\nkey = \"cmd-k\"\nkey = nope\n").unwrap_err();
        assert!(error.contains("line 3"), "{error}");
    }

    #[test]
    fn a_misspelt_field_is_an_error_not_an_unbind() {
        let error = parse_keymap("[[binding]]\nkey = \"cmd-k\"\nacton = \"x\"\n").unwrap_err();
        assert!(error.contains("line 3"), "{error}");
    }

    #[test]
    fn missing_table_is_empty() {
        assert_eq!(parse_keymap(""), Ok(Vec::new()));
        assert_eq!(parse_keymap("# nothing yet\n"), Ok(Vec::new()));
    }

    #[test]
    fn a_missing_file_is_no_bindings() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_keymap(&dir.path().join("keymap.toml")), Ok(Vec::new()));
        assert!(read_keymap(dir.path()).is_err(), "a directory");
    }
}
