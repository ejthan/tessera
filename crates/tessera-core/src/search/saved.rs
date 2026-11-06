// SPDX-License-Identifier: GPL-3.0-or-later

//! Saved queries and console favorites, kept in `saved_queries.json`. They are the user's work,
//! so a corrupt or newer file is left alone and nothing is written, like `connections.json`.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::search::history::HistoryItem;
use crate::search::params::SearchDraft;
use crate::store::{self, StoreError};

/// Schema version written by this build.
pub const SCHEMA_VERSION: u32 = 1;

/// Content of `saved_queries.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedFile {
    /// File format version; files from a newer version are refused.
    pub schema_version: u32,
    /// The saved queries.
    #[serde(default)]
    pub queries: Vec<SavedQuery>,
    /// Console requests kept past the history's cap.
    #[serde(default)]
    pub favorites: Vec<HistoryItem>,
}

impl Default for SavedFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            queries: Vec::new(),
            favorites: Vec::new(),
        }
    }
}

/// A named set of searches.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedQuery {
    /// Identity across renames.
    pub id: Uuid,
    /// The connection it belongs to.
    pub profile: Uuid,
    /// Its name.
    pub name: String,
    /// Its tags.
    pub tags: Vec<String>,
    /// The searches run as one merged list.
    pub union: bool,
    /// The drafts.
    pub searches: Vec<SearchDraft>,
}

/// `saved_queries.json` in the platform data directory.
pub fn saved_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .data_dir()
            .join("saved_queries.json"),
    )
}

/// Loads the file at `path`. A missing file yields an empty default.
pub fn load(path: &Path) -> Result<SavedFile, StoreError> {
    store::load_versioned(path, SCHEMA_VERSION)
}

/// Writes `file` atomically.
pub fn save(path: &Path, file: &SavedFile) -> Result<(), StoreError> {
    store::save_pretty(path, file)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::search::history::raw_entry;

    fn file() -> SavedFile {
        let profile = Uuid::new_v4();
        SavedFile {
            schema_version: SCHEMA_VERSION,
            queries: vec![SavedQuery {
                id: Uuid::new_v4(),
                profile,
                name: "dune".into(),
                tags: vec!["books".into()],
                union: false,
                searches: vec![SearchDraft::new("books")],
            }],
            favorites: vec![HistoryItem {
                at: 1,
                profile,
                entry: raw_entry("GET", "health", None, None),
            }],
        }
    }

    #[test]
    fn missing_saved_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("saved_queries.json")).unwrap();
        assert_eq!(loaded, SavedFile::default());
        assert_eq!(loaded.schema_version, 1);
    }

    #[test]
    fn corrupt_saved_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved_queries.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(load(&path), Err(StoreError::Parse { .. })));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn newer_saved_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved_queries.json");
        std::fs::write(&path, r#"{"schema_version": 2, "future": true}"#).unwrap();
        assert!(matches!(load(&path), Err(StoreError::NewerSchema(2))));
    }

    #[test]
    fn saved_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("saved_queries.json");
        let f = file();
        save(&path, &f).unwrap();
        assert_eq!(load(&path).unwrap(), f);
    }
}
