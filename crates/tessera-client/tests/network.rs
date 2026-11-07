// SPDX-License-Identifier: GPL-3.0-or-later

//! Source scans for two promises of spec §9, which no request test can cover.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// Each `src` file of the workspace, with `#[cfg(test)]` modules cut off, and its path from the
/// root.
fn workspace() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in fs::read_dir(root.join("crates")).unwrap().flatten() {
        sources(&dir.path().join("src"), &mut files);
    }
    files
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap().to_string();
            let name = path.strip_prefix(&root).unwrap();
            (name.display().to_string(), code)
        })
        .collect()
}

#[test]
fn only_two_places_build_an_http_client() {
    let mut found: Vec<String> = workspace()
        .into_iter()
        .filter(|(_, code)| code.contains("reqwest::Client::builder"))
        .map(|(name, _)| name)
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            "crates/tessera-client/src/lib.rs",
            "crates/tessera-client/src/update.rs",
        ]
    );
}

#[test]
fn no_key_is_copied_into_a_plain_string() {
    let mut copies = Vec::new();
    for (name, code) in workspace() {
        let lines: Vec<&str> = code.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let before = i.checked_sub(1).map_or("", |i| lines[i]);
            if line.contains("expose_secret().to_string()")
                && !line.contains("Zeroizing::new")
                && !before.contains("Zeroizing::new")
            {
                copies.push(format!("{name}: {}", line.trim()));
            }
        }
    }
    assert_eq!(copies, Vec::<String>::new());
}
