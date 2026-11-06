// SPDX-License-Identifier: GPL-3.0-or-later

//! Packing, unpacking and verifying a backup, and hostile archives.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tessera_core::backup::archive::{ArchiveError, pack, read_manifest, unpack, verify};
use tessera_core::backup::format::{
    FORMAT, MANIFEST_LIMIT, Manifest, ManifestCollection, ManifestFile, ManifestProblem,
};
use tessera_core::transfer::JobControl;

const SCHEMA: &str = "collections/0001.schema.json";
const DOCS: &str = "collections/0001.jsonl";

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn listed(path: &str, bytes: &[u8]) -> ManifestFile {
    ManifestFile {
        path: path.into(),
        bytes: bytes.len() as u64,
        sha256: sha(bytes),
    }
}

/// A manifest for `files`, with collection 1 when both of its files are there.
fn manifest_of(files: &[(&str, &[u8])]) -> Manifest {
    let paths: Vec<&str> = files.iter().map(|f| f.0).collect();
    let collections = (paths.contains(&SCHEMA) && paths.contains(&DOCS))
        .then(|| ManifestCollection {
            file: 1,
            name: "books".into(),
            documents: 2,
            unstored: Vec::new(),
            blanked_secrets: 0,
            dictionaries: Vec::new(),
        })
        .into_iter()
        .collect();
    Manifest {
        format: FORMAT,
        created_at: 1_700_000_000,
        tessera_version: "0.1.0".into(),
        server_version: "30.2".into(),
        collections,
        files: files.iter().map(|(p, b)| listed(p, b)).collect(),
    }
}

const FILES: [(&str, &[u8]); 3] = [
    ("aliases.json", b"[]"),
    (SCHEMA, b"{\"name\":\"books\"}"),
    (DOCS, b"{\"id\":\"1\"}\n{\"id\":\"2\"}\n"),
];

/// A backup folder, in a folder of its own beside the place to unpack to.
fn folder(root: &Path) -> (PathBuf, Manifest) {
    let dir = root.join("backup");
    fs::create_dir_all(dir.join("collections")).unwrap();
    for (path, bytes) in FILES {
        fs::write(dir.join(path), bytes).unwrap();
    }
    let manifest = manifest_of(&FILES);
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    (dir, manifest)
}

/// One raw entry. The name goes straight into the header, past what `Header::set_path` refuses.
struct Raw<'a> {
    name: &'a str,
    kind: u8,
    link: &'a str,
    size: Option<u64>,
    data: &'a [u8],
}

fn regular<'a>(name: &'a str, data: &'a [u8]) -> Raw<'a> {
    Raw {
        name,
        kind: b'0',
        link: "",
        size: None,
        data,
    }
}

fn special<'a>(name: &'a str, kind: u8, link: &'a str) -> Raw<'a> {
    Raw {
        name,
        kind,
        link,
        size: Some(0),
        data: b"",
    }
}

fn write_entry(out: &mut Vec<u8>, entry: &Raw) {
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o600);
    header.set_size(entry.size.unwrap_or(entry.data.len() as u64));
    header.set_mtime(0);
    let gnu = header.as_gnu_mut().unwrap();
    gnu.name[..entry.name.len()].copy_from_slice(entry.name.as_bytes());
    gnu.linkname[..entry.link.len()].copy_from_slice(entry.link.as_bytes());
    gnu.typeflag[0] = entry.kind;
    header.set_cksum();
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(entry.data);
    out.resize(out.len().next_multiple_of(512), 0);
}

fn archive_of(path: &Path, entries: &[Raw]) {
    let mut tar = Vec::new();
    for entry in entries {
        write_entry(&mut tar, entry);
    }
    tar.resize(tar.len() + 1024, 0);
    let file = fs::File::create(path).unwrap();
    let mut encoder = zstd::stream::write::Encoder::new(file, 3).unwrap();
    encoder.write_all(&tar).unwrap();
    encoder.finish().unwrap();
}

/// Everything below `root`, relative, sorted.
fn tree(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// Unpacks `entries`, and asserts that nothing appeared outside `into`.
fn unpack_hostile(entries: &[Raw]) -> Result<Manifest, ArchiveError> {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("a.tar.zst");
    archive_of(&archive, entries);
    let into = root.path().join("sandbox").join("into");
    fs::create_dir_all(&into).unwrap();
    let got = unpack(&archive, &into, &JobControl::default());
    let outside: Vec<String> = tree(root.path())
        .into_iter()
        .filter(|p| !p.starts_with("sandbox/into") && p != "sandbox" && p != "a.tar.zst")
        .collect();
    assert_eq!(outside, Vec::<String>::new());
    got
}

fn manifest_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    serde_json::to_vec(&manifest_of(files)).unwrap()
}

const ONE: [(&str, &[u8]); 1] = [("aliases.json", b"[]")];

#[test]
fn a_folder_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    let control = JobControl::default();

    pack(&dir, &target, &control).unwrap();

    assert!(!root.path().join("b.tar.zst.part").exists());
    let into = root.path().join("into");
    fs::create_dir(&into).unwrap();
    let read = unpack(&target, &into, &control).unwrap();
    assert_eq!(read, manifest);
    verify(&into, &read, &control).unwrap();
    assert_eq!(read_manifest(&into).unwrap(), manifest);
    for (path, bytes) in FILES {
        assert_eq!(fs::read(into.join(path)).unwrap(), bytes, "{path}");
    }
}

#[cfg(unix)]
#[test]
fn what_is_written_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    pack(&dir, &target, &JobControl::default()).unwrap();
    assert_eq!(mode(&target), 0o600);

    let into = root.path().join("into");
    fs::create_dir(&into).unwrap();
    unpack(&target, &into, &JobControl::default()).unwrap();
    assert_eq!(mode(&into.join("collections")), 0o700);
    assert_eq!(mode(&into.join("manifest.json")), 0o600);
    assert_eq!(mode(&into.join(DOCS)), 0o600);
}

#[test]
fn an_archive_starts_with_the_manifest_and_holds_plain_entries() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    pack(&dir, &target, &JobControl::default()).unwrap();

    let decoder = zstd::stream::read::Decoder::new(fs::File::open(&target).unwrap()).unwrap();
    let mut tar = tar::Archive::new(decoder);
    let mut names = Vec::new();
    for entry in tar.entries().unwrap() {
        let entry = entry.unwrap();
        assert_eq!(entry.header().mode().unwrap(), 0o600);
        assert_eq!(entry.header().mtime().unwrap(), 0);
        assert_eq!(entry.header().entry_type(), tar::EntryType::Regular);
        names.push(entry.path().unwrap().to_string_lossy().into_owned());
    }
    assert_eq!(names, ["manifest.json", "aliases.json", SCHEMA, DOCS]);
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_as_target_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let real = root.path().join("real");
    fs::write(&real, b"keep").unwrap();
    let link = root.path().join("b.tar.zst");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert_eq!(
        pack(&dir, &link, &JobControl::default()),
        Err(ArchiveError::SymlinkTarget(link.clone()))
    );
    assert_eq!(fs::read(&real).unwrap(), b"keep");
}

#[test]
fn a_part_file_that_stands_there_is_named() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    let part = root.path().join("b.tar.zst.part");
    fs::write(&part, b"old").unwrap();

    assert_eq!(
        pack(&dir, &target, &JobControl::default()),
        Err(ArchiveError::PartExists(part.clone()))
    );
    assert_eq!(fs::read(&part).unwrap(), b"old");
}

#[test]
fn a_stop_leaves_no_part_file() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    let control = JobControl::default();
    control.stop.cancel();

    assert_eq!(pack(&dir, &target, &control), Err(ArchiveError::Stopped));

    assert!(!target.exists());
    assert!(!root.path().join("b.tar.zst.part").exists());
}

#[test]
fn a_failed_pack_leaves_no_part_file() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    fs::write(dir.join(DOCS), b"changed since it was hashed").unwrap();
    let target = root.path().join("b.tar.zst");

    assert_eq!(
        pack(&dir, &target, &JobControl::default()),
        Err(ArchiveError::TooLong(DOCS.into()))
    );

    assert!(!target.exists());
    assert!(!root.path().join("b.tar.zst.part").exists());
}

#[test]
fn a_stop_ends_an_unpack() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    pack(&dir, &target, &JobControl::default()).unwrap();
    let into = root.path().join("into");
    fs::create_dir(&into).unwrap();
    let control = JobControl::default();
    control.stop.cancel();

    assert_eq!(unpack(&target, &into, &control), Err(ArchiveError::Stopped));
}

#[test]
fn the_first_entry_must_be_the_manifest() {
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[
        regular("aliases.json", b"[]"),
        regular("manifest.json", &manifest),
    ]);
    assert_eq!(got, Err(ArchiveError::ManifestNotFirst));
}

#[test]
fn an_empty_archive_has_no_manifest() {
    assert_eq!(unpack_hostile(&[]), Err(ArchiveError::ManifestNotFirst));
}

#[test]
fn a_link_named_manifest_is_no_manifest() {
    let got = unpack_hostile(&[special("manifest.json", b'2', "/etc/passwd")]);
    assert_eq!(got, Err(ArchiveError::ManifestNotFirst));
}

#[test]
fn a_manifest_over_the_limit_is_refused() {
    let big = vec![b' '; MANIFEST_LIMIT as usize + 1];
    let got = unpack_hostile(&[regular("manifest.json", &big)]);
    assert_eq!(got, Err(ArchiveError::ManifestTooLarge));
}

#[test]
fn a_newer_format_is_refused() {
    let mut manifest = manifest_of(&ONE);
    manifest.format = FORMAT + 1;
    let text = serde_json::to_vec(&manifest).unwrap();
    let got = unpack_hostile(&[
        regular("manifest.json", &text),
        regular("aliases.json", b"[]"),
    ]);
    assert_eq!(
        got,
        Err(ArchiveError::Manifest(ManifestProblem::Format(FORMAT + 1)))
    );
}

#[test]
fn a_manifest_listing_a_bad_path_is_refused() {
    let mut manifest = manifest_of(&ONE);
    manifest.files[0].path = "../aliases.json".into();
    let text = serde_json::to_vec(&manifest).unwrap();
    let got = unpack_hostile(&[regular("manifest.json", &text)]);
    assert_eq!(
        got,
        Err(ArchiveError::Manifest(ManifestProblem::BadPath(
            "../aliases.json".into()
        )))
    );
}

#[test]
fn traversal_is_refused() {
    let manifest = manifest_bytes(&ONE);
    for path in ["../x", "/etc/x", "collections/../../x", "./aliases.json"] {
        let got = unpack_hostile(&[regular("manifest.json", &manifest), regular(path, b"x")]);
        assert_eq!(got, Err(ArchiveError::Unlisted(path.into())), "{path}");
    }
}

#[test]
fn entries_that_are_no_regular_files_are_refused() {
    let manifest = manifest_bytes(&ONE);
    let cases: [(u8, &str, &str); 8] = [
        (b'2', "symbolic link", "/etc/passwd"),
        (b'1', "hard link", "manifest.json"),
        (b'3', "character device", ""),
        (b'4', "block device", ""),
        (b'5', "folder", ""),
        (b'6', "FIFO", ""),
        (b'L', "GNU long name", ""),
        (b'x', "pax header", ""),
    ];
    for (kind, label, link) in cases {
        let got = unpack_hostile(&[
            regular("manifest.json", &manifest),
            special("aliases.json", kind, link),
        ]);
        assert_eq!(
            got,
            Err(ArchiveError::NotRegular("aliases.json".into())),
            "{label}"
        );
    }
}

#[test]
fn a_long_name_entry_cannot_rename_a_file() {
    // Without raw entries this would set the next entry's path to `../x` first.
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[
        regular("manifest.json", &manifest),
        Raw {
            kind: b'L',
            ..regular("././@LongLink", b"../x\0")
        },
        regular("aliases.json", b"[]"),
    ]);
    assert_eq!(got, Err(ArchiveError::NotRegular("././@LongLink".into())));
}

#[test]
fn a_duplicate_is_refused() {
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[
        regular("manifest.json", &manifest),
        regular("aliases.json", b"[]"),
        regular("aliases.json", b"[]"),
    ]);
    assert_eq!(got, Err(ArchiveError::Duplicate("aliases.json".into())));
}

#[test]
fn an_unlisted_file_is_refused() {
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[
        regular("manifest.json", &manifest),
        regular("stopwords.json", b"[]"),
    ]);
    assert_eq!(got, Err(ArchiveError::Unlisted("stopwords.json".into())));
}

#[test]
fn a_second_manifest_is_refused() {
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[
        regular("manifest.json", &manifest),
        regular("manifest.json", &manifest),
    ]);
    assert_eq!(got, Err(ArchiveError::Unlisted("manifest.json".into())));
}

#[test]
fn more_bytes_than_declared_stop_at_the_declared_size() {
    let manifest = manifest_bytes(&ONE);
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("a.tar.zst");
    let bomb = vec![0; 8 << 20];
    archive_of(
        &archive,
        &[
            regular("manifest.json", &manifest),
            regular("aliases.json", &bomb),
        ],
    );
    let into = root.path().join("into");
    fs::create_dir(&into).unwrap();

    assert_eq!(
        unpack(&archive, &into, &JobControl::default()),
        Err(ArchiveError::TooLong("aliases.json".into()))
    );
    assert_eq!(fs::metadata(into.join("aliases.json")).unwrap().len(), 3);
}

#[test]
fn fewer_bytes_than_declared_are_refused() {
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[
        regular("manifest.json", &manifest),
        regular("aliases.json", b"["),
    ]);
    assert_eq!(got, Err(ArchiveError::TooShort("aliases.json".into())));
}

#[test]
fn a_listed_file_must_come() {
    let manifest = manifest_bytes(&ONE);
    let got = unpack_hostile(&[regular("manifest.json", &manifest)]);
    assert_eq!(got, Err(ArchiveError::Missing("aliases.json".into())));
}

#[test]
fn an_archive_cut_short_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    let target = root.path().join("b.tar.zst");
    pack(&dir, &target, &JobControl::default()).unwrap();
    let bytes = fs::read(&target).unwrap();
    fs::write(&target, &bytes[..bytes.len() / 2]).unwrap();
    let into = root.path().join("into");
    fs::create_dir(&into).unwrap();

    assert!(unpack(&target, &into, &JobControl::default()).is_err());
}

#[test]
fn garbage_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("b.tar.zst");
    fs::write(&target, b"this is no archive").unwrap();

    assert!(matches!(
        unpack(&target, root.path(), &JobControl::default()),
        Err(ArchiveError::Io(_))
    ));
}

#[test]
fn verify_finds_a_flipped_byte() {
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    let mut bytes = fs::read(dir.join(DOCS)).unwrap();
    bytes[3] ^= 1;
    fs::write(dir.join(DOCS), bytes).unwrap();

    assert_eq!(
        verify(&dir, &manifest, &JobControl::default()),
        Err(ArchiveError::Digest(DOCS.into()))
    );
}

#[test]
fn verify_finds_a_truncated_file() {
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    fs::write(dir.join(DOCS), b"{").unwrap();

    assert_eq!(
        verify(&dir, &manifest, &JobControl::default()),
        Err(ArchiveError::TooShort(DOCS.into()))
    );
}

#[test]
fn verify_finds_a_missing_file() {
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    fs::remove_file(dir.join(DOCS)).unwrap();

    assert_eq!(
        verify(&dir, &manifest, &JobControl::default()),
        Err(ArchiveError::Missing(DOCS.into()))
    );
}

#[test]
fn verify_ignores_an_extra_file() {
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    fs::write(dir.join("notes.txt"), b"mine").unwrap();

    verify(&dir, &manifest, &JobControl::default()).unwrap();
}

#[test]
fn verify_stops() {
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    let control = JobControl::default();
    control.stop.cancel();

    assert_eq!(
        verify(&dir, &manifest, &control),
        Err(ArchiveError::Stopped)
    );
}

#[cfg(unix)]
#[test]
fn verify_refuses_a_link_in_a_folder() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let (dir, manifest) = folder(root.path());
    let elsewhere = root.path().join("elsewhere");
    fs::copy(dir.join(DOCS), &elsewhere).unwrap();
    fs::remove_file(dir.join(DOCS)).unwrap();
    symlink(&elsewhere, dir.join(DOCS)).unwrap();
    assert_eq!(
        verify(&dir, &manifest, &JobControl::default()),
        Err(ArchiveError::NotRegular(DOCS.into()))
    );

    fs::remove_file(dir.join(DOCS)).unwrap();
    fs::copy(&elsewhere, dir.join(DOCS)).unwrap();
    fs::rename(dir.join("collections"), root.path().join("moved")).unwrap();
    symlink(root.path().join("moved"), dir.join("collections")).unwrap();
    assert_eq!(
        verify(&dir, &manifest, &JobControl::default()),
        Err(ArchiveError::NotRegular("collections".into()))
    );
}

#[cfg(unix)]
#[test]
fn a_linked_manifest_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let (dir, _) = folder(root.path());
    fs::rename(dir.join("manifest.json"), root.path().join("m")).unwrap();
    std::os::unix::fs::symlink(root.path().join("m"), dir.join("manifest.json")).unwrap();

    assert_eq!(
        read_manifest(&dir),
        Err(ArchiveError::NotRegular("manifest.json".into()))
    );
}

#[test]
fn a_folder_without_a_manifest_is_incomplete() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        read_manifest(root.path()),
        Err(ArchiveError::Missing("manifest.json".into()))
    );
}
