// SPDX-License-Identifier: GPL-3.0-or-later

//! Sizes of the window's docks, in `layout/main.json`. Cheap to lose: a corrupt file gives the
//! default and is replaced, but one that is newer or failed to migrate is left alone.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::store::{self, StoreError};

/// Schema version written by this build.
const SCHEMA_VERSION: u32 = 1;

/// The tab the bottom dock shows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BottomTab {
    /// The request log.
    #[default]
    Log,
    /// The jobs list.
    Jobs,
}

/// Content of `layout/main.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Layout {
    /// File format version.
    pub schema_version: u32,
    /// Width of the left dock, 160..=600.
    pub left_width: f32,
    /// Height of the bottom dock, 120..=800.
    pub bottom_height: f32,
    /// Whether the bottom dock is open.
    pub bottom_open: bool,
    /// The bottom dock's tab.
    pub bottom_tab: BottomTab,
    /// The file on disk must stay: [`save`] writes nothing.
    #[serde(skip)]
    pub save_blocked: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            left_width: 240.,
            bottom_height: 240.,
            bottom_open: false,
            bottom_tab: BottomTab::default(),
            save_blocked: false,
        }
    }
}

impl Layout {
    /// This layout with its sizes in range.
    pub fn clamped(self) -> Layout {
        Layout {
            left_width: self.left_width.clamp(160., 600.),
            bottom_height: self.bottom_height.clamp(120., 800.),
            ..self
        }
    }
}

/// `layout/main.json` in the platform data directory.
pub fn layout_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .data_dir()
            .join("layout")
            .join("main.json"),
    )
}

/// Loads the file, clamped. A missing or bad one gives the default, with a warning.
pub fn load(path: &Path) -> Layout {
    // No migrations yet: `layout/main.json` has only ever had version 1.
    match store::load_migrated::<Layout>(path, SCHEMA_VERSION, &[]) {
        Ok(layout) => layout.clamped(),
        Err(e) => {
            tracing::warn!("ignoring {}: {e}", path.display());
            Layout {
                save_blocked: e.blocks_saving(),
                ..Layout::default()
            }
        }
    }
}

/// Writes the file atomically, unless the file on disk must stay.
pub fn save(path: &Path, layout: &Layout) -> Result<(), StoreError> {
    if layout.save_blocked {
        return Ok(());
    }
    store::save_pretty(path, layout)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("none.json")), Layout::default());
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout").join("main.json");
        let layout = Layout {
            left_width: 300.,
            bottom_height: 400.,
            bottom_open: true,
            bottom_tab: BottomTab::Jobs,
            ..Layout::default()
        };
        save(&path, &layout).unwrap();
        assert_eq!(load(&path), layout);
    }

    #[test]
    fn sizes_are_clamped_at_both_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.json");
        for (given, left, bottom) in [(1., 160., 120.), (9000., 600., 800.)] {
            let text =
                format!(r#"{{"schema_version":1,"left_width":{given},"bottom_height":{given}}}"#);
            std::fs::write(&path, text).unwrap();
            let layout = load(&path);
            assert_eq!((layout.left_width, layout.bottom_height), (left, bottom));
        }
    }

    #[test]
    fn a_bad_file_gives_the_default_and_can_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.json");
        std::fs::write(&path, "{not json").unwrap();
        let layout = load(&path);
        assert_eq!(layout, Layout::default());
        save(&path, &layout).unwrap();
        assert_eq!(load(&path), Layout::default());
        assert_ne!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn a_newer_file_is_refused_and_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.json");
        let text = r#"{"schema_version":2,"left_width":300}"#;
        std::fs::write(&path, text).unwrap();
        let layout = load(&path);
        assert_eq!(layout.left_width, 240.);
        save(&path, &layout).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
