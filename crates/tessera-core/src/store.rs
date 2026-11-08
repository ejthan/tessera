// SPDX-License-Identifier: GPL-3.0-or-later

//! Persistence of connection profiles in `connections.json`. Keys never end up here.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// Why loading or saving a JSON file failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Reading, writing or renaming failed.
    #[error("cannot access file: {0}")]
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
    #[error("file uses schema version {0}, which this version does not understand")]
    NewerSchema(u32),
    /// A migration failed; the file is left untouched.
    #[error("cannot migrate the file: {0}")]
    Migration(String),
}

impl StoreError {
    /// Whether the file on disk must not be written over. Only a file that does not parse may
    /// be, by the two files that are cheap to lose: `document_views.json` and the layout.
    pub fn blocks_saving(&self) -> bool {
        !matches!(self, Self::Parse { .. })
    }
}

/// One step of a file's format: `migrations[n]` lifts level `n + 1` to `n + 2`.
pub(crate) type Migration = fn(Value) -> Result<Value, String>;

/// Platform config directory of Tessera, if the home directory is known.
pub fn config_dir() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .config_dir()
            .to_path_buf(),
    )
}

/// The platform data directory.
pub fn data_dir() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .data_dir()
            .to_path_buf(),
    )
}

/// Where the log files go.
pub fn logs_dir() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("org", "tessera", "Tessera")?
            .data_local_dir()
            .join("logs"),
    )
}

/// Loads the file at `path`. A missing file yields an empty default.
pub fn load(path: &Path) -> Result<ConnectionsFile, StoreError> {
    // No migrations yet: `connections.json` has only ever had version 1.
    load_migrated(path, SCHEMA_VERSION, &[])
}

/// `<name>.bak`, which holds the file from before the last migration.
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    name.into()
}

/// Loads a file with a `schema_version` up to `version`, lifting an older one through
/// `migrations`: first a copy to `.bak`, then each step, then an atomic write of the new
/// version. A missing file yields the default. A bad or newer one, or a failed step, is an
/// error that leaves the file, and the caller must not write over it.
pub(crate) fn load_migrated<T: Default + Serialize + DeserializeOwned>(
    path: &Path,
    version: u32,
    migrations: &[Migration],
) -> Result<T, StoreError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(e.into()),
    };
    let parse_err = |e: serde_json::Error| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    // Check the version first: a newer file may not match today's shape.
    let mut value: Value = serde_json::from_str(&text).map_err(parse_err)?;
    let found = value.get("schema_version").and_then(Value::as_u64);
    if let Some(v) = found
        && v > u64::from(version)
    {
        return Err(StoreError::NewerSchema(
            u32::try_from(v).unwrap_or(u32::MAX),
        ));
    }
    let Some(found) = found.filter(|v| *v < u64::from(version)) else {
        return serde_json::from_value(value).map_err(parse_err);
    };
    write_atomic(&backup_path(path), text.as_bytes())?;
    for level in found..u64::from(version) {
        let step = usize::try_from(level)
            .ok()
            .and_then(|level| migrations.get(level.checked_sub(1)?))
            .ok_or_else(|| StoreError::Migration(format!("no step from version {level}")))?;
        value = step(value).map_err(StoreError::Migration)?;
    }
    let Some(object) = value.as_object_mut() else {
        return Err(StoreError::Migration("the result is no object".into()));
    };
    object.insert("schema_version".into(), version.into());
    let file: T =
        serde_json::from_value(value).map_err(|e| StoreError::Migration(e.to_string()))?;
    save_pretty(path, &file)?;
    Ok(file)
}

/// Writes `file` as pretty JSON via [`write_atomic`].
pub fn save(path: &Path, file: &ConnectionsFile) -> Result<(), StoreError> {
    save_pretty(path, file)
}

/// Writes `file` as pretty JSON via [`write_atomic`].
pub(crate) fn save_pretty<T: Serialize>(path: &Path, file: &T) -> Result<(), StoreError> {
    let json = serde_json::to_string_pretty(file).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    write_atomic(path, json.as_bytes())
}

/// Makes `dir` if needed, readable by this user alone where the system has modes.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `bytes` via a synced temporary file and a rename, creating parent directories.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
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

    #[cfg(unix)]
    #[test]
    fn a_private_dir_is_this_users_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("temp dir");
        let logs = dir.path().join("a").join("logs");
        private_dir(&logs).expect("made");
        let open = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&logs, open).expect("widened");
        private_dir(&logs).expect("again");
        let found = std::fs::metadata(&logs).expect("metadata");
        assert_eq!(found.permissions().mode() & 0o777, 0o700);
    }

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
        let text = r#"{"schema_version": 2, "profiles": [], "future": true}"#;
        assert_eq!(std::fs::read_to_string(&p).expect("read"), text);
    }

    #[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
    struct Fake {
        schema_version: u32,
        #[serde(default)]
        a: u32,
        #[serde(default)]
        b: u32,
    }

    const OLD: &str = "{ \"schema_version\": 1,   \"keep\": true }\n";

    fn fake_file(dir: &tempfile::TempDir, text: &str) -> PathBuf {
        let p = dir.path().join("fake.json");
        std::fs::write(&p, text).expect("write");
        p
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).expect("read")
    }

    fn step_a(mut v: Value) -> Result<Value, String> {
        v["a"] = 1.into();
        Ok(v)
    }

    fn step_b(mut v: Value) -> Result<Value, String> {
        v["b"] = (v["a"].as_u64().unwrap_or(0) + 1).into();
        Ok(v)
    }

    fn fail(_: Value) -> Result<Value, String> {
        Err("step failed".into())
    }

    #[test]
    fn migrates_from_level_1_through_both_steps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = fake_file(&dir, OLD);
        let got: Fake = load_migrated(&p, 3, &[step_a, step_b]).expect("load");
        let want = Fake {
            schema_version: 3,
            a: 1,
            b: 2,
        };
        assert_eq!(got, want);
        assert_eq!(read(&backup_path(&p)), OLD);
        assert_eq!(serde_json::from_str::<Fake>(&read(&p)).expect("json"), want);
        // Current now: nothing runs, and the backup stays.
        let again: Fake = load_migrated(&p, 3, &[fail, fail]).expect("load");
        assert_eq!(again, want);
        assert_eq!(read(&backup_path(&p)), OLD);
    }

    #[test]
    fn migrates_from_level_2_through_the_last_step() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = r#"{"schema_version":2,"a":5}"#;
        let p = fake_file(&dir, text);
        let got: Fake = load_migrated(&p, 3, &[fail, step_b]).expect("load");
        assert_eq!((got.schema_version, got.a, got.b), (3, 5, 6));
        assert_eq!(read(&backup_path(&p)), text);
    }

    #[test]
    fn a_later_migration_replaces_the_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = fake_file(&dir, OLD);
        let _: Fake = load_migrated(&p, 2, &[step_a]).expect("load");
        let second = read(&p);
        let _: Fake = load_migrated(&p, 3, &[step_a, step_b]).expect("load");
        assert_eq!(read(&backup_path(&p)), second);
    }

    #[test]
    fn a_failing_step_leaves_the_file_and_the_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = fake_file(&dir, OLD);
        let got = load_migrated::<Fake>(&p, 3, &[step_a, fail]);
        assert!(matches!(got, Err(StoreError::Migration(m)) if m == "step failed"));
        assert_eq!(read(&p), OLD);
        assert_eq!(read(&backup_path(&p)), OLD);
    }

    #[test]
    fn a_missing_step_or_a_bad_result_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = fake_file(&dir, OLD);
        let got = load_migrated::<Fake>(&p, 3, &[step_a]);
        assert!(matches!(got, Err(StoreError::Migration(_))));
        assert_eq!(read(&p), OLD);
        fn to_list(_: Value) -> Result<Value, String> {
            Ok(serde_json::json!([]))
        }
        let got = load_migrated::<Fake>(&p, 2, &[to_list]);
        assert!(matches!(got, Err(StoreError::Migration(_))));
        fn wrong_shape(mut v: Value) -> Result<Value, String> {
            v["a"] = "text".into();
            Ok(v)
        }
        let got = load_migrated::<Fake>(&p, 2, &[wrong_shape]);
        assert!(matches!(got, Err(StoreError::Migration(_))));
        assert_eq!(read(&p), OLD);
        let zero = fake_file(&dir, r#"{"schema_version":0}"#);
        assert!(matches!(
            load_migrated::<Fake>(&zero, 2, &[step_a]),
            Err(StoreError::Migration(_))
        ));
    }

    #[test]
    fn a_backup_that_cannot_be_written_stops_the_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = fake_file(&dir, OLD);
        std::fs::create_dir(backup_path(&p)).expect("mkdir");
        assert!(load_migrated::<Fake>(&p, 2, &[step_a]).is_err());
        assert_eq!(read(&p), OLD);
    }

    #[test]
    fn a_newer_file_writes_no_backup_and_stays() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = r#"{"schema_version":4}"#;
        let p = fake_file(&dir, text);
        let got = load_migrated::<Fake>(&p, 3, &[step_a, step_b]);
        assert!(matches!(got, Err(StoreError::NewerSchema(4))));
        assert_eq!(read(&p), text);
        assert!(!backup_path(&p).exists());
    }

    #[test]
    fn a_current_file_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = r#"{"schema_version":3,"a":1}"#;
        let p = fake_file(&dir, text);
        let _: Fake = load_migrated(&p, 3, &[fail, fail]).expect("load");
        assert_eq!(read(&p), text);
        assert!(!backup_path(&p).exists());
    }
}
