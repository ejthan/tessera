// SPDX-License-Identifier: GPL-3.0-or-later

//! The backup format: a manifest, four JSON files and numbered collection files. A collection's
//! name can hold anything the server accepts, so it never becomes a path.
//!
//! A manifest is untrusted input, because a backup comes from anywhere.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// The format this version writes, and the newest it reads.
pub const FORMAT: u32 = 1;
/// The manifest's name, in a folder and in an archive.
pub const MANIFEST: &str = "manifest.json";
/// The aliases of the selected collections.
pub const ALIASES: &str = "aliases.json";
/// All stopword sets.
pub const STOPWORDS: &str = "stopwords.json";
/// The synonym sets the selected collections attach.
pub const SYNONYM_SETS: &str = "synonym_sets.json";
/// The curation sets the selected collections attach.
pub const CURATION_SETS: &str = "curation_sets.json";
/// The folder of the numbered collection files.
pub const COLLECTIONS: &str = "collections";
/// The largest manifest read (64 MiB).
pub const MANIFEST_LIMIT: u64 = 64 << 20;
/// The most collections a manifest may list.
pub const MAX_COLLECTIONS: usize = 100_000;
/// The most files a manifest may list: two per collection, and some room.
pub const MAX_FILES: usize = 200_010;

const TOP_LEVEL: [&str; 4] = [ALIASES, STOPWORDS, SYNONYM_SETS, CURATION_SETS];

/// What a backup holds. Every file but the manifest itself is listed with its size and digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The format, see [`FORMAT`].
    pub format: u32,
    /// Creation time, Unix seconds.
    pub created_at: i64,
    /// The Tessera that wrote it.
    pub tessera_version: String,
    /// The server it was read from.
    pub server_version: String,
    /// The collections, in backup order.
    pub collections: Vec<ManifestCollection>,
    /// Every file but the manifest.
    pub files: Vec<ManifestFile>,
}

/// One collection of a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestCollection {
    /// The number of its files.
    pub file: u32,
    /// Its name on the server.
    pub name: String,
    /// Documents the export wrote.
    pub documents: u64,
    /// Fields with `store: false`, which an export cannot hold.
    pub unstored: Vec<String>,
    /// Provider credentials blanked in its schema.
    pub blanked_secrets: usize,
    /// Stemming dictionaries it uses, which a backup does not hold.
    pub dictionaries: Vec<String>,
}

/// One file of a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Its path, relative and exact, see [`allowed_path`].
    pub path: String,
    /// Its size.
    pub bytes: u64,
    /// Its SHA-256, 64 lowercase hex digits.
    pub sha256: String,
}

/// What is wrong with a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestProblem {
    /// It is not the JSON of a manifest; holds the parser's text.
    Unreadable(String),
    /// A format this version does not read.
    Format(u32),
    /// More than [`MAX_COLLECTIONS`] collections.
    TooManyCollections,
    /// More than [`MAX_FILES`] files.
    TooManyFiles,
    /// A path outside [`allowed_path`].
    BadPath(String),
    /// A path listed twice.
    DuplicatePath(String),
    /// A digest that is not 64 lowercase hex digits; holds the file's path.
    BadDigest(String),
    /// Two collections with one file number.
    DuplicateFile(u32),
    /// A file a collection needs and `files` does not list.
    MissingFile(String),
}

/// Whether `path` is one a backup may hold, the manifest aside: exactly one of the four
/// top-level names, or `collections/`, four to nine ASCII digits and `.schema.json` or `.jsonl`.
pub fn allowed_path(path: &str) -> bool {
    if TOP_LEVEL.contains(&path) {
        return true;
    }
    let Some(name) = path
        .strip_prefix(COLLECTIONS)
        .and_then(|p| p.strip_prefix('/'))
    else {
        return false;
    };
    let digits = name
        .strip_suffix(".schema.json")
        .or_else(|| name.strip_suffix(".jsonl"));
    digits.is_some_and(|d| (4..=9).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
}

impl ManifestCollection {
    /// The path of its schema.
    pub fn schema_path(&self) -> String {
        format!("{COLLECTIONS}/{:04}.schema.json", self.file)
    }

    /// The path of its documents.
    pub fn documents_path(&self) -> String {
        format!("{COLLECTIONS}/{:04}.jsonl", self.file)
    }
}

impl Manifest {
    /// Reads and checks manifest text.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestProblem> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|e| ManifestProblem::Unreadable(e.to_string()))?;
        manifest.check()?;
        Ok(manifest)
    }

    /// Refuses a manifest that must not be followed. Runs before any of it is used.
    pub fn check(&self) -> Result<(), ManifestProblem> {
        if !(1..=FORMAT).contains(&self.format) {
            return Err(ManifestProblem::Format(self.format));
        }
        if self.collections.len() > MAX_COLLECTIONS {
            return Err(ManifestProblem::TooManyCollections);
        }
        if self.files.len() > MAX_FILES {
            return Err(ManifestProblem::TooManyFiles);
        }
        let mut paths = HashSet::with_capacity(self.files.len());
        for file in &self.files {
            if !allowed_path(&file.path) {
                return Err(ManifestProblem::BadPath(file.path.clone()));
            }
            if !paths.insert(file.path.as_str()) {
                return Err(ManifestProblem::DuplicatePath(file.path.clone()));
            }
            if !is_digest(&file.sha256) {
                return Err(ManifestProblem::BadDigest(file.path.clone()));
            }
        }
        let mut numbers = HashSet::with_capacity(self.collections.len());
        for collection in &self.collections {
            if !numbers.insert(collection.file) {
                return Err(ManifestProblem::DuplicateFile(collection.file));
            }
            for path in [collection.schema_path(), collection.documents_path()] {
                if !paths.contains(path.as_str()) {
                    return Err(ManifestProblem::MissingFile(path));
                }
            }
        }
        Ok(())
    }
}

fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> ManifestFile {
        ManifestFile {
            path: path.into(),
            bytes: 0,
            sha256: "a".repeat(64),
        }
    }

    fn collection(number: u32) -> ManifestCollection {
        ManifestCollection {
            file: number,
            name: "c".into(),
            documents: 0,
            unstored: Vec::new(),
            blanked_secrets: 0,
            dictionaries: Vec::new(),
        }
    }

    fn manifest() -> Manifest {
        Manifest {
            format: FORMAT,
            created_at: 0,
            tessera_version: "0.1.0".into(),
            server_version: "30.2".into(),
            collections: vec![collection(1)],
            files: vec![
                file(ALIASES),
                file("collections/0001.schema.json"),
                file("collections/0001.jsonl"),
            ],
        }
    }

    #[test]
    fn allowed_path_is_exact() {
        for good in [
            "aliases.json",
            "stopwords.json",
            "synonym_sets.json",
            "curation_sets.json",
            "collections/0001.jsonl",
            "collections/0001.schema.json",
            "collections/123456789.jsonl",
        ] {
            assert!(allowed_path(good), "{good}");
        }
        for bad in [
            "",
            "manifest.json",
            "../aliases.json",
            "/aliases.json",
            "./aliases.json",
            "Aliases.json",
            "aliases.json ",
            "collections/",
            "collections/.jsonl",
            "collections/001.jsonl",
            "collections/1234567890.jsonl",
            "collections/0001.json",
            "collections/0001.schema.jsonl",
            "collections//0001.jsonl",
            "collections/../0001.jsonl",
            "collections/0001/0001.jsonl",
            "collections\\0001.jsonl",
            "collections/٠٠٠١.jsonl",
            "collections/0001.jsonl\0",
        ] {
            assert!(!allowed_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_good_manifest_passes_and_round_trips() {
        let m = manifest();
        assert_eq!(m.check(), Ok(()));
        let text = serde_json::to_vec(&m).unwrap();
        assert_eq!(Manifest::parse(&text), Ok(m));
    }

    #[test]
    fn a_number_above_four_digits_keeps_its_width() {
        let mut m = manifest();
        m.collections[0].file = 12345;
        m.files[1].path = "collections/12345.schema.json".into();
        m.files[2].path = "collections/12345.jsonl".into();
        assert_eq!(m.check(), Ok(()));
    }

    #[test]
    fn the_format_must_be_known() {
        for format in [0, FORMAT + 1] {
            let mut m = manifest();
            m.format = format;
            assert_eq!(m.check(), Err(ManifestProblem::Format(format)));
        }
    }

    #[test]
    fn a_path_outside_the_list_is_refused() {
        let mut m = manifest();
        m.files.push(file("../x"));
        assert_eq!(m.check(), Err(ManifestProblem::BadPath("../x".into())));
    }

    #[test]
    fn a_path_listed_twice_is_refused() {
        let mut m = manifest();
        m.files.push(file(ALIASES));
        assert_eq!(
            m.check(),
            Err(ManifestProblem::DuplicatePath(ALIASES.into()))
        );
    }

    #[test]
    fn a_digest_must_be_lowercase_hex_of_full_length() {
        for digest in ["a".repeat(63), "A".repeat(64), "g".repeat(64)] {
            let mut m = manifest();
            m.files[0].sha256 = digest;
            assert_eq!(m.check(), Err(ManifestProblem::BadDigest(ALIASES.into())));
        }
    }

    #[test]
    fn a_file_number_is_used_once() {
        let mut m = manifest();
        m.collections.push(collection(1));
        assert_eq!(m.check(), Err(ManifestProblem::DuplicateFile(1)));
    }

    #[test]
    fn both_files_of_a_collection_are_listed() {
        for at in [1, 2] {
            let mut m = manifest();
            let gone = m.files.remove(at).path;
            assert_eq!(m.check(), Err(ManifestProblem::MissingFile(gone)));
        }
    }

    #[test]
    fn the_counts_are_limited() {
        let mut m = manifest();
        m.collections = vec![collection(1); MAX_COLLECTIONS + 1];
        assert_eq!(m.check(), Err(ManifestProblem::TooManyCollections));
        let mut m = manifest();
        m.files = vec![file(ALIASES); MAX_FILES + 1];
        assert_eq!(m.check(), Err(ManifestProblem::TooManyFiles));
    }

    #[test]
    fn text_that_is_no_manifest_is_refused() {
        assert!(matches!(
            Manifest::parse(b"{\"format\":1}"),
            Err(ManifestProblem::Unreadable(_))
        ));
    }
}
