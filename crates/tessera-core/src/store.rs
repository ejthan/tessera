// SPDX-License-Identifier: GPL-3.0-or-later

//! Persistence of connection profiles in `connections.json`. Keys never end up here.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::Profile;

/// Schema version written by this build.
pub const SCHEMA_VERSION: u32 = 1;

/// Content of `connections.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConnectionsFile {
    /// File format version; files from a newer version are refused.
    pub schema_version: u32,
    /// All saved connection profiles.
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// The profile used last, if any.
    #[serde(default)]
    pub last_used: Option<Uuid>,
    /// Whether to reopen the last used profile on start.
    #[serde(default)]
    pub reopen_last: bool,
}

impl Default for ConnectionsFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profiles: Vec::new(),
            last_used: None,
            reopen_last: false,
        }
    }
}

/// Why loading or saving `connections.json` failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Reading, writing or renaming failed.
    #[error("cannot access connections file: {0}")]
    Io(#[from] std::io::Error),
    /// The file exists but is not valid; it is left untouched.
    #[error("cannot parse {}: {message}", path.display())]
    Parse {
        /// The file that failed to parse.
        path: PathBuf,
        /// The parser's message.
        message: String,
    },
    /// The file was written by a newer Tessera.
    #[error("connections file uses schema version {0}, which this version does not understand")]
    NewerSchema(u32),
}

/// Platform config directory of Tessera, if the home directory is known.
pub fn config_dir() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .config_dir()
            .to_path_buf(),
    )
}

/// Loads the file at `path`. A missing file yields an empty default.
pub fn load(path: &Path) -> Result<ConnectionsFile, StoreError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ConnectionsFile::default()),
        Err(e) => return Err(e.into()),
    };
    let parse_err = |e: serde_json::Error| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    // Check the version first: a newer file may not match today's shape.
    let value: serde_json::Value = serde_json::from_str(&text).map_err(parse_err)?;
    if let Some(v) = value.get("schema_version").and_then(|v| v.as_u64())
        && v > u64::from(SCHEMA_VERSION)
    {
        return Err(StoreError::NewerSchema(
            u32::try_from(v).unwrap_or(u32::MAX),
        ));
    }
    serde_json::from_value(value).map_err(parse_err)
}

/// Writes `file` as pretty JSON via [`write_atomic`].
pub fn save(path: &Path, file: &ConnectionsFile) -> Result<(), StoreError> {
    let json = serde_json::to_string_pretty(file).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    write_atomic(path, json.as_bytes())
}

/// Writes `bytes` via a synced temporary file and a rename, creating parent directories.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    // Synced before the rename, so a power loss cannot leave an empty file in its place.
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("sub").join("connections.json")
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = load(&path(&dir)).expect("load");
        assert_eq!(f, ConnectionsFile::default());
        assert!(f.profiles.is_empty());
        assert_eq!(f.schema_version, 1);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = path(&dir);
        let mut f = ConnectionsFile::default();
        let profile = Profile::new("local", vec![]);
        f.last_used = Some(profile.id);
        f.reopen_last = true;
        f.profiles.push(profile);
        save(&p, &f).expect("save");
        assert_eq!(load(&p).expect("load"), f);
    }

    #[test]
    fn save_is_atomic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = path(&dir);
        let f = ConnectionsFile::default();
        save(&p, &f).expect("first save");
        let before = std::fs::read_to_string(&p).expect("read");
        // A directory where the temp file goes makes the write fail.
        std::fs::create_dir(p.with_extension("json.tmp")).expect("mkdir");
        let mut g = f.clone();
        g.reopen_last = true;
        assert!(save(&p, &g).is_err());
        assert_eq!(std::fs::read_to_string(&p).expect("read"), before);
    }

    #[test]
    fn unparseable_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("connections.json");
        std::fs::write(&p, "{not json").expect("write");
        match load(&p) {
            Err(StoreError::Parse { path, message }) => {
                assert_eq!(path, p);
                assert!(!message.is_empty());
            }
            other => panic!("expected Parse, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&p).expect("read"), "{not json");
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("connections.json");
        std::fs::write(
            &p,
            r#"{"schema_version": 2, "profiles": [], "future": true}"#,
        )
        .expect("write");
        assert!(matches!(load(&p), Err(StoreError::NewerSchema(2))));
    }
}
