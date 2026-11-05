// SPDX-License-Identifier: GPL-3.0-or-later

//! Column preferences per connection and collection, in `document_views.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::{StoreError, write_atomic};

/// Schema version written by this build.
const SCHEMA_VERSION: u32 = 1;

fn default_pinned() -> Vec<String> {
    vec!["id".into()]
}

/// Which columns are hidden, pinned and how wide. Unknown keys are kept, so a re-added
/// field keeps its width.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CollectionView {
    /// Hidden column keys.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Pinned column keys.
    #[serde(default = "default_pinned")]
    pub pinned: Vec<String>,
    /// Column widths in pixels.
    #[serde(default)]
    pub widths: BTreeMap<String, f32>,
}

impl Default for CollectionView {
    fn default() -> Self {
        Self {
            hidden: Vec::new(),
            pinned: default_pinned(),
            widths: BTreeMap::new(),
        }
    }
}

/// Content of `document_views.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ViewsFile {
    /// File format version.
    pub schema_version: u32,
    /// Views by profile id, then collection name.
    #[serde(default)]
    pub profiles: BTreeMap<Uuid, BTreeMap<String, CollectionView>>,
}

impl Default for ViewsFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profiles: BTreeMap::new(),
        }
    }
}

impl ViewsFile {
    /// The saved view, or the default.
    pub fn view(&self, profile: Uuid, collection: &str) -> CollectionView {
        self.profiles
            .get(&profile)
            .and_then(|c| c.get(collection))
            .cloned()
            .unwrap_or_default()
    }

    /// Stores the view.
    pub fn set_view(&mut self, profile: Uuid, collection: &str, view: CollectionView) {
        self.profiles
            .entry(profile)
            .or_default()
            .insert(collection.to_string(), view);
    }
}

/// `document_views.json` in the platform data directory.
pub fn views_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .data_dir()
            .join("document_views.json"),
    )
}

/// Loads the file; a missing one gives the default, an unreadable one too (with a warning).
pub fn load(path: &Path) -> ViewsFile {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            ViewsFile::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ViewsFile::default(),
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", path.display());
            ViewsFile::default()
        }
    }
}

/// Writes the file atomically.
pub fn save(path: &Path, file: &ViewsFile) -> Result<(), StoreError> {
    let json = serde_json::to_string_pretty(file).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    write_atomic(path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load(&dir.path().join("none.json")), ViewsFile::default());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("sub").join("document_views.json");
        let id = Uuid::new_v4();
        let mut f = ViewsFile::default();
        let mut v = CollectionView::default();
        v.hidden.push("price".into());
        v.widths.insert("title".into(), 180.5);
        f.set_view(id, "products", v.clone());
        save(&p, &f).expect("save");
        let loaded = load(&p);
        assert_eq!(loaded, f);
        assert_eq!(loaded.view(id, "products"), v);
        assert_eq!(loaded.view(id, "other"), CollectionView::default());
    }

    #[test]
    fn default_view_pins_id() {
        assert_eq!(CollectionView::default().pinned, ["id"]);
        let v: CollectionView = serde_json::from_str("{}").expect("parse");
        assert_eq!(v.pinned, ["id"]);
    }

    #[test]
    fn corrupt_file_is_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("document_views.json");
        std::fs::write(&p, "{not json").expect("write");
        assert_eq!(load(&p), ViewsFile::default());
    }
}
