// SPDX-License-Identifier: GPL-3.0-or-later

//! Column preferences per connection and collection, in `document_views.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::{self, StoreError};

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
    /// The file on disk must stay: [`save`] writes nothing.
    #[serde(skip)]
    pub save_blocked: bool,
}

impl Default for ViewsFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profiles: BTreeMap::new(),
            save_blocked: false,
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
/// One that is newer or failed to migrate also stops [`save`].
pub fn load(path: &Path) -> ViewsFile {
    // No migrations yet: `document_views.json` has only ever had version 1.
    store::load_migrated(path, SCHEMA_VERSION, &[]).unwrap_or_else(|e: StoreError| {
        tracing::warn!("ignoring {}: {e}", path.display());
        ViewsFile {
            save_blocked: e.blocks_saving(),
            ..ViewsFile::default()
        }
    })
}

/// Writes the file atomically, unless the file on disk must stay.
pub fn save(path: &Path, file: &ViewsFile) -> Result<(), StoreError> {
    if file.save_blocked {
        return Ok(());
    }
    store::save_pretty(path, file)
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

    #[test]
    fn newer_file_is_refused_and_never_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("document_views.json");
        let text = r#"{"schema_version":2,"future":true}"#;
        std::fs::write(&p, text).expect("write");
        let mut f = load(&p);
        assert!(f.save_blocked);
        assert!(f.profiles.is_empty());
        f.set_view(Uuid::new_v4(), "books", CollectionView::default());
        save(&p, &f).expect("no error");
        assert_eq!(std::fs::read_to_string(&p).expect("read"), text);
    }
}
