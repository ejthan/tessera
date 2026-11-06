// SPDX-License-Identifier: GPL-3.0-or-later

//! The `.tar.zst` form of a backup. Blocking: call through `spawn_blocking`.
//!
//! An archive comes from anywhere, so [`unpack`] walks raw entries and copies the bytes of the
//! few it allows into files it names itself. It never calls `Entry::unpack`.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::format::{COLLECTIONS, MANIFEST, MANIFEST_LIMIT, Manifest, ManifestProblem};
use crate::transfer::JobControl;

/// The zstd level of an archive.
const LEVEL: i32 = 3;
/// `pack` looks for a stop this often within a file.
const STOP_EVERY: u64 = 16 << 20;
/// The buffer of `verify` and `unpack`.
const BUFFER: usize = 64 * 1024;
/// The most of an entry's path an error keeps: it comes from the archive.
const NAME_CHARS: usize = 256;

/// Why a backup could not be packed, unpacked or verified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    /// Stopped on request.
    #[error("stopped")]
    Stopped,
    /// The target is a symbolic link.
    #[error("{} is a symbolic link", .0.display())]
    SymlinkTarget(PathBuf),
    /// A part file of an earlier run stands there.
    #[error("{} already exists", .0.display())]
    PartExists(PathBuf),
    /// The first entry is not a regular `manifest.json`.
    #[error("the first entry is not the manifest")]
    ManifestNotFirst,
    /// The manifest is over [`MANIFEST_LIMIT`].
    #[error("the manifest is too large")]
    ManifestTooLarge,
    /// The manifest is refused.
    #[error("the manifest is refused: {0:?}")]
    Manifest(ManifestProblem),
    /// A link, device, folder or anything but a regular file.
    #[error("{0} is not a regular file")]
    NotRegular(String),
    /// A file the manifest does not list.
    #[error("{0} is not listed in the manifest")]
    Unlisted(String),
    /// A path that came before.
    #[error("{0} comes twice")]
    Duplicate(String),
    /// More bytes than declared.
    #[error("{0} is larger than declared")]
    TooLong(String),
    /// Fewer bytes than declared.
    #[error("{0} is smaller than declared")]
    TooShort(String),
    /// A listed file that is not there.
    #[error("{0} is missing")]
    Missing(String),
    /// A digest that differs.
    #[error("{0} does not match its checksum")]
    Digest(String),
    /// The file system or a damaged stream, with the system's message.
    #[error("{0}")]
    Io(String),
}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<ManifestProblem> for ArchiveError {
    fn from(problem: ManifestProblem) -> Self {
        Self::Manifest(problem)
    }
}

fn stopped(cancel: &JobControl) -> bool {
    cancel.stop.is_cancelled() || cancel.abort.is_cancelled()
}

fn stop_check(cancel: &JobControl) -> Result<(), ArchiveError> {
    if stopped(cancel) {
        return Err(ArchiveError::Stopped);
    }
    Ok(())
}

/// A new file only its user can read. Never follows a link at `path`.
pub(super) fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// A new folder only its user can enter.
pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Fails with a stop every [`STOP_EVERY`] bytes, once one was asked for.
struct Stoppable<'a, R> {
    inner: R,
    cancel: &'a JobControl,
    since: u64,
}

impl<R: Read> Read for Stoppable<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.since >= STOP_EVERY {
            self.since = 0;
            if stopped(self.cancel) {
                // Not `Interrupted`, which `io::copy` retries.
                return Err(io::Error::other("stopped"));
            }
        }
        let n = self.inner.read(buf)?;
        self.since += n as u64;
        Ok(n)
    }
}

/// Removes the part file unless it was renamed.
struct PartGuard<'a>(Option<&'a Path>);

impl Drop for PartGuard<'_> {
    fn drop(&mut self) {
        if let Some(path) = self.0 {
            // Nothing to tell: it is a leftover of a run that already failed.
            let _ = fs::remove_file(path);
        }
    }
}

fn same_size(have: u64, declared: u64, path: &str) -> Result<(), ArchiveError> {
    match have.cmp(&declared) {
        Ordering::Greater => Err(ArchiveError::TooLong(path.to_string())),
        Ordering::Less => Err(ArchiveError::TooShort(path.to_string())),
        Ordering::Equal => Ok(()),
    }
}

/// The manifest of `folder`, read and checked.
pub fn read_manifest(folder: &Path) -> Result<Manifest, ArchiveError> {
    let path = folder.join(MANIFEST);
    let meta = fs::symlink_metadata(&path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ArchiveError::Missing(MANIFEST.to_string()),
        _ => e.into(),
    })?;
    if !meta.is_file() {
        return Err(ArchiveError::NotRegular(MANIFEST.to_string()));
    }
    if meta.len() > MANIFEST_LIMIT {
        return Err(ArchiveError::ManifestTooLarge);
    }
    Ok(Manifest::parse(&fs::read(path)?)?)
}

fn tar_header(bytes: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(bytes);
    header.set_mode(0o600);
    header.set_mtime(0);
    header
}

/// Writes the backup in `folder` to `target` as `.tar.zst`, the manifest first, then the files
/// in manifest order. The target appears only when complete, and a stop leaves nothing.
pub fn pack(folder: &Path, target: &Path, cancel: &JobControl) -> Result<(), ArchiveError> {
    if fs::symlink_metadata(target).is_ok_and(|m| m.is_symlink()) {
        return Err(ArchiveError::SymlinkTarget(target.to_path_buf()));
    }
    let manifest = read_manifest(folder)?;
    let mut part = target.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let file = create_private(&part).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => ArchiveError::PartExists(part.clone()),
        _ => e.into(),
    })?;
    let mut guard = PartGuard(Some(&part));

    let mut builder = tar::Builder::new(zstd::stream::write::Encoder::new(file, LEVEL)?);
    let paths = std::iter::once(MANIFEST).chain(manifest.files.iter().map(|f| f.path.as_str()));
    for path in paths {
        stop_check(cancel)?;
        let source = File::open(folder.join(path))?;
        // A tar header states the size, so a file that changed since it was hashed stops here.
        let bytes = source.metadata()?.len();
        if let Some(file) = manifest.files.iter().find(|f| f.path == path) {
            same_size(bytes, file.bytes, path)?;
        }
        let reader = Stoppable {
            inner: source.take(bytes),
            cancel,
            since: 0,
        };
        builder
            .append_data(&mut tar_header(bytes), path, reader)
            .map_err(|e| {
                if stopped(cancel) {
                    ArchiveError::Stopped
                } else {
                    e.into()
                }
            })?;
    }
    let file = builder.into_inner()?.finish()?;
    file.sync_all()?;
    drop(file);
    fs::rename(&part, target)?;
    guard.0 = None;
    Ok(())
}

/// The path of an archive entry, for an error.
fn shown(path: &[u8]) -> String {
    String::from_utf8_lossy(path)
        .chars()
        .take(NAME_CHARS)
        .collect()
}

/// Copies at most `limit` bytes from `reader`, looking for a stop between reads.
fn copy_capped(
    reader: impl Read,
    out: &mut impl Write,
    limit: u64,
    cancel: &JobControl,
) -> Result<u64, ArchiveError> {
    let mut reader = reader.take(limit);
    let mut buffer = vec![0; BUFFER];
    let mut total = 0;
    loop {
        stop_check(cancel)?;
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            return Ok(total);
        }
        out.write_all(&buffer[..n])?;
        total += n as u64;
    }
}

/// Unpacks `archive` into the existing, private folder `into`, entry by entry.
///
/// Refuses, in this order: a first entry that is not `manifest.json`, a manifest over
/// [`MANIFEST_LIMIT`] or one [`Manifest::check`] refuses, then an entry that is not a regular
/// file, is not listed, came before, or holds more or fewer bytes than declared, and at the end
/// a listed file that did not come. So traversal, links, devices and a decompression bomb all
/// end before they cost disk beyond the declared total. What was written stays in `into`, which
/// the caller removes.
pub fn unpack(archive: &Path, into: &Path, cancel: &JobControl) -> Result<Manifest, ArchiveError> {
    let decoder = zstd::stream::read::Decoder::new(File::open(archive)?)?;
    let mut tar = tar::Archive::new(decoder);
    // Raw, because otherwise the crate reads GNU long names and pax headers into memory whole
    // and at any size. Here they are entries that are not regular files.
    let mut entries = tar.entries()?.raw(true);

    let mut first = entries.next().ok_or(ArchiveError::ManifestNotFirst)??;
    if first.path_bytes().as_ref() != MANIFEST.as_bytes()
        || first.header().entry_type() != tar::EntryType::Regular
    {
        return Err(ArchiveError::ManifestNotFirst);
    }
    let mut text = Vec::new();
    (&mut first)
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut text)?;
    if text.len() as u64 > MANIFEST_LIMIT {
        return Err(ArchiveError::ManifestTooLarge);
    }
    drop(first);
    let manifest = Manifest::parse(&text)?;
    create_private(&into.join(MANIFEST))?.write_all(&text)?;
    if !manifest.files.is_empty() {
        match create_private_dir(&into.join(COLLECTIONS)) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => {}
        }
    }

    let declared: HashMap<&[u8], u64> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_bytes(), f.bytes))
        .collect();
    let mut seen: HashSet<Vec<u8>> = HashSet::with_capacity(declared.len());
    for entry in entries {
        stop_check(cancel)?;
        let entry = entry?;
        let path = entry.path_bytes().into_owned();
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(ArchiveError::NotRegular(shown(&path)));
        }
        let Some(&bytes) = declared.get(path.as_slice()) else {
            return Err(ArchiveError::Unlisted(shown(&path)));
        };
        if seen.contains(&path) {
            return Err(ArchiveError::Duplicate(shown(&path)));
        }
        // `declared` holds only paths `allowed_path` passed, so this is valid UTF-8.
        let name = String::from_utf8_lossy(&path);
        let mut out = create_private(&into.join(name.as_ref()))?;
        let copied = copy_capped(entry, &mut out, bytes.saturating_add(1), cancel)?;
        same_size(copied, bytes, &shown(&path))?;
        seen.insert(path);
    }
    if let Some(gone) = manifest
        .files
        .iter()
        .find(|f| !seen.contains(f.path.as_bytes()))
    {
        return Err(ArchiveError::Missing(gone.path.clone()));
    }
    Ok(manifest)
}

/// The size and SHA-256 of a file, looking for a stop between reads.
pub(super) fn digest(path: &Path, cancel: &JobControl) -> Result<(u64, String), ArchiveError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER];
    let mut total = 0;
    loop {
        stop_check(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        total += n as u64;
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((total, hex))
}

/// Checks each listed file of `folder`: a regular file, then its size, then its digest. A file
/// the manifest does not list is ignored.
pub fn verify(folder: &Path, manifest: &Manifest, cancel: &JobControl) -> Result<(), ArchiveError> {
    // A folder comes from anywhere too: a link would read a file of this machine into a restore.
    if fs::symlink_metadata(folder.join(COLLECTIONS)).is_ok_and(|m| m.is_symlink()) {
        return Err(ArchiveError::NotRegular(COLLECTIONS.to_string()));
    }
    for file in &manifest.files {
        stop_check(cancel)?;
        let path = folder.join(&file.path);
        let meta = fs::symlink_metadata(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => ArchiveError::Missing(file.path.clone()),
            _ => e.into(),
        })?;
        if !meta.is_file() {
            return Err(ArchiveError::NotRegular(file.path.clone()));
        }
        same_size(meta.len(), file.bytes, &file.path)?;
        let (_, sha256) = digest(&path, cancel)?;
        if sha256 != file.sha256 {
            return Err(ArchiveError::Digest(file.path.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_stop_ends_a_read_within_sixteen_mib() {
        let cancel = JobControl::default();
        cancel.stop.cancel();
        let mut reader = Stoppable {
            inner: io::repeat(1).take(STOP_EVERY * 2),
            cancel: &cancel,
            since: 0,
        };
        assert!(io::copy(&mut reader, &mut io::sink()).is_err());
        assert!(reader.since < STOP_EVERY);
    }

    #[test]
    fn without_a_stop_all_of_it_comes() {
        let cancel = JobControl::default();
        let mut reader = Stoppable {
            inner: io::repeat(1).take(STOP_EVERY * 2),
            cancel: &cancel,
            since: 0,
        };
        assert_eq!(
            io::copy(&mut reader, &mut io::sink()).unwrap(),
            STOP_EVERY * 2
        );
    }
}
