// SPDX-License-Identifier: GPL-3.0-or-later

//! The user's preferences, in `settings.toml`. A file that does not parse changes nothing and
//! is not overwritten, like `keymap.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::documents::{CONFIRM_ABOVE, PER_PAGE};
use crate::store::{self, StoreError};
use crate::transfer::{DEFAULT_BATCH, MAX_BATCH};

/// The poll intervals the overview offers, in seconds.
pub const POLL_CHOICES: [u32; 4] = [2, 5, 10, 30];

/// How tight data tables are.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// The kit's size.
    #[default]
    Comfortable,
    /// Smaller rows.
    Compact,
}

/// The interface language.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Follow the system.
    #[default]
    System,
    /// English.
    En,
    /// German.
    De,
}

/// Content of `settings.toml`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// `system`, `light`, `dark`, or a theme name.
    pub theme: String,
    /// Font size in pixels, 12..=20; `None` is the kit's default.
    pub font_size: Option<u8>,
    /// Data table density.
    pub density: Density,
    /// Interface language.
    pub language: Language,
    /// Documents per page, 10..=[`PER_PAGE`].
    pub per_page: u32,
    /// Documents per import batch, 1..=[`MAX_BATCH`].
    pub batch_size: u32,
    /// Poll interval of a new connection, one of [`POLL_CHOICES`].
    pub poll_seconds: u32,
    /// Bulk actions above this many documents need the typed name, 1..=100,000.
    pub confirm_above: u64,
    /// Whether to look for a new version at start.
    pub update_check: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            font_size: None,
            density: Density::default(),
            language: Language::default(),
            per_page: PER_PAGE,
            batch_size: DEFAULT_BATCH,
            poll_seconds: 5,
            confirm_above: CONFIRM_ABOVE,
            update_check: false,
        }
    }
}

impl Settings {
    /// These settings with every number in range; a poll interval takes the nearest choice.
    pub fn clamped(self) -> Settings {
        let poll_seconds = self.poll_seconds;
        Settings {
            font_size: self.font_size.map(|n| n.clamp(12, 20)),
            per_page: self.per_page.clamp(10, PER_PAGE),
            batch_size: self.batch_size.clamp(1, MAX_BATCH),
            poll_seconds: POLL_CHOICES
                .into_iter()
                .min_by_key(|n| n.abs_diff(poll_seconds))
                .unwrap_or(5),
            confirm_above: self.confirm_above.clamp(1, 100_000),
            ..self
        }
    }
}

/// Why `settings.toml` could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// Reading failed.
    #[error("cannot read the settings: {0}")]
    Io(#[from] std::io::Error),
    /// The file does not parse; it is left untouched.
    #[error("{0}")]
    Parse(String),
}

/// `settings.toml` in the platform config directory.
pub fn settings_path() -> Option<PathBuf> {
    Some(store::config_dir()?.join("settings.toml"))
}

/// Loads the file at `path`, clamped. A missing file yields the default; a bad one is an error,
/// and the caller must not write over it.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e.into()),
    };
    toml::from_str::<Settings>(&text)
        .map(Settings::clamped)
        .map_err(|e| SettingsError::Parse(e.to_string()))
}

/// Writes `settings` atomically.
pub fn save(path: &Path, settings: &Settings) -> Result<(), StoreError> {
    let text = toml::to_string_pretty(settings).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    store::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn write(dir: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load(&dir.path().join("none.toml")).unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.per_page, 250);
        assert_eq!(settings.batch_size, 1000);
        assert_eq!(settings.poll_seconds, 5);
        assert_eq!(settings.confirm_above, 1000);
        assert!(!settings.update_check);
    }

    #[test]
    fn round_trips_and_keeps_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.toml");
        let mut settings = Settings::default();
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), settings);
        settings.font_size = Some(16);
        settings.density = Density::Compact;
        settings.language = Language::De;
        settings.theme = "Catppuccin".into();
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn unknown_keys_are_ignored_and_missing_keys_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "future = 1\ndensity = \"compact\"\n[more]\na = 2\n");
        let settings = load(&path).unwrap();
        assert_eq!(settings.density, Density::Compact);
        assert_eq!(settings.per_page, 250);
    }

    #[test]
    fn a_bad_file_is_an_error_and_stays() {
        let dir = tempfile::tempdir().unwrap();
        for text in ["not toml =", "density = \"tight\"", "per_page = \"many\""] {
            let path = write(&dir, text);
            assert!(
                matches!(load(&path), Err(SettingsError::Parse(_))),
                "{text}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[test]
    fn clamps_both_ends_of_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let low =
            "font_size = 1\nper_page = 0\nbatch_size = 0\npoll_seconds = 0\nconfirm_above = 0";
        let settings = load(&write(&dir, low)).unwrap();
        assert_eq!(settings.font_size, Some(12));
        assert_eq!(settings.per_page, 10);
        assert_eq!(settings.batch_size, 1);
        assert_eq!(settings.poll_seconds, 2);
        assert_eq!(settings.confirm_above, 1);
        let high = "font_size = 200\nper_page = 9999\nbatch_size = 9999999\npoll_seconds = 999\n\
                    confirm_above = 9999999";
        let settings = load(&write(&dir, high)).unwrap();
        assert_eq!(settings.font_size, Some(20));
        assert_eq!(settings.per_page, 250);
        assert_eq!(settings.batch_size, MAX_BATCH);
        assert_eq!(settings.poll_seconds, 30);
        assert_eq!(settings.confirm_above, 100_000);
    }

    #[test]
    fn a_poll_interval_takes_the_nearest_choice() {
        for (given, kept) in [(2, 2), (3, 2), (5, 5), (7, 5), (10, 10), (21, 30), (30, 30)] {
            let settings = Settings {
                poll_seconds: given,
                ..Settings::default()
            };
            assert_eq!(settings.clamped().poll_seconds, kept, "{given}");
        }
    }
}
