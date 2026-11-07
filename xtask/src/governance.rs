// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask governance` and `cargo xtask dco`: what keeps the repository in order.

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::i18n::{flat_keys, rust_files};

const SPDX: &str = "// SPDX-License-Identifier: GPL-3.0-or-later";
const NOTICE: &str = "Tessera is not affiliated with or endorsed by Typesense, Inc.";
const REQUIRED: [&str; 11] = [
    "LICENSE",
    "README.md",
    "CONTRIBUTING.md",
    "CODE_OF_CONDUCT.md",
    "SECURITY.md",
    "CHANGELOG.md",
    ".github/ISSUE_TEMPLATE/bug_report.yml",
    ".github/ISSUE_TEMPLATE/feature_request.yml",
    ".github/ISSUE_TEMPLATE/config.yml",
    ".github/pull_request_template.md",
    ".github/labels.yml",
];

/// Every problem of the tree at `root`, one string each.
pub fn check(root: &Path) -> Vec<String> {
    let read = |path: &str| std::fs::read_to_string(root.join(path));
    let mut problems = Vec::new();

    for file in REQUIRED.iter().filter(|f| !root.join(f).is_file()) {
        problems.push(format!("{file} is missing"));
    }
    // A file that is missing is reported above; one that cannot be read does not hold.
    let holds = |path: &str, needles: &[&str]| match read(path) {
        Ok(text) => needles.iter().all(|n| text.contains(n)),
        Err(e) => e.kind() == ErrorKind::NotFound,
    };
    if !holds(
        "LICENSE",
        &["GNU GENERAL PUBLIC LICENSE", "Version 3, 29 June 2007"],
    ) {
        problems.push("LICENSE is not the GPL-3.0 text".to_string());
    }
    if !holds(
        "CODE_OF_CONDUCT.md",
        &["Contributor Covenant", "version 2.1"],
    ) {
        problems.push("CODE_OF_CONDUCT.md is not the Contributor Covenant 2.1".to_string());
    }
    if !holds("CHANGELOG.md", &["## [Unreleased]"]) {
        problems.push("CHANGELOG.md has no `## [Unreleased]`".to_string());
    }
    if !holds("README.md", &[NOTICE]) {
        problems.push("README.md lacks the trademark notice".to_string());
    }

    let locale = flat_keys(&read_or_report(
        &root.join("assets/locales/en.yml"),
        root,
        &mut problems,
    ));
    if !locale
        .get("welcome.trademark")
        .is_some_and(|v| v.contains(NOTICE))
    {
        problems.push("welcome.trademark lacks the trademark notice".to_string());
    }
    if locale
        .get("app.title")
        .is_none_or(|v| v.contains("Typesense"))
    {
        problems
            .push("the product name in app.title is missing or holds \"Typesense\"".to_string());
    }

    for dir in ["crates", "xtask"] {
        let files = rust_files(&root.join(dir)).unwrap_or_else(|e| {
            problems.push(format!("cannot read {dir}: {e}"));
            Vec::new()
        });
        for path in files {
            let text = read_or_report(&path, root, &mut problems);
            if text.lines().next() != Some(SPDX) {
                let file = path.strip_prefix(root).unwrap_or(&path);
                problems.push(format!("{} has no SPDX line first", file.display()));
            }
        }
    }

    let manifest = read_or_report(&root.join("Cargo.toml"), root, &mut problems);
    let mut pinned_here = BTreeSet::new();
    for (name, value) in dependencies(&manifest, |section| section == "workspace.dependencies") {
        let tight = value.replace(' ', "");
        if !tight.contains("path=") && !tight.contains("\"=") {
            problems.push(format!(
                "workspace dependency `{name}` is not pinned with `=`"
            ));
        }
        pinned_here.insert(name);
    }
    let mut manifests = vec![root.join("xtask/Cargo.toml")];
    match crate_dirs(&root.join("crates")) {
        Ok(dirs) => manifests.extend(dirs.into_iter().map(|d| d.join("Cargo.toml"))),
        Err(e) => problems.push(format!("cannot read crates: {e}")),
    }
    for path in manifests {
        let text = read_or_report(&path, root, &mut problems);
        for (name, value) in dependencies(&text, |section| section.ends_with("dependencies")) {
            if pinned_here.contains(&name) && !value.contains("workspace") {
                let file = path.strip_prefix(root).unwrap_or(&path);
                problems.push(format!(
                    "{} names its own version of `{name}`",
                    file.display()
                ));
            }
        }
    }
    problems
}

/// The text of `path`. A read error is a problem and yields nothing.
fn read_or_report(path: &Path, root: &Path, problems: &mut Vec<String>) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        let file = path.strip_prefix(root).unwrap_or(path);
        problems.push(format!("cannot read {}: {e}", file.display()));
        String::new()
    })
}

/// The directories in `dir`.
fn crate_dirs(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// What the sections `wanted` accepts declare, per dependency: its value, or the joined
/// `key = value` parts of a `[section.name]` table and of `name.key = value` lines.
fn dependencies(manifest: &str, wanted: impl Fn(&str) -> bool) -> BTreeMap<String, String> {
    let mut found = BTreeMap::<String, String>::new();
    let mut add = |name: &str, value: &str| {
        let parts = found.entry(name.trim().to_string()).or_default();
        parts.push_str(value.trim());
        parts.push(' ');
    };
    // The section's dependencies are its lines; a table's lines belong to one dependency.
    let mut inside = false;
    let mut table: Option<String> = None;
    for line in manifest.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = wanted(section);
            table = section
                .rsplit_once('.')
                .filter(|(parent, _)| wanted(parent))
                .map(|(_, name)| name.to_string());
        } else if line.starts_with('#') || !line.contains('=') {
        } else if let Some(name) = &table {
            add(name, line);
        } else if inside && let Some((key, value)) = line.split_once('=') {
            match key.split_once('.') {
                Some((name, part)) => add(name, &format!("{part} = {value}")),
                None => add(key, value),
            }
        }
    }
    found
}

/// The short hashes in `log` without a `Signed-off-by` line. `log` is the output of
/// `git log --no-merges --format=%h%x00%B%x01 <base>..HEAD`.
pub fn dco_missing(log: &str) -> Vec<String> {
    log.split('\u{1}')
        .filter_map(|record| record.trim_start().split_once('\0'))
        .filter(|(_, body)| !body.lines().any(|l| l.starts_with("Signed-off-by: ")))
        .map(|(hash, _)| hash.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::{check, dco_missing};

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    /// A minimal repository that passes.
    fn valid() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let notice = "Tessera is not affiliated with or endorsed by Typesense, Inc.";
        write(
            root,
            "LICENSE",
            "GNU GENERAL PUBLIC LICENSE\nVersion 3, 29 June 2007\n",
        );
        write(root, "README.md", &format!("# Tessera\n\n{notice}\n"));
        write(root, "CONTRIBUTING.md", "# Contributing\n");
        write(root, "SECURITY.md", "# Security\n");
        write(
            root,
            "CODE_OF_CONDUCT.md",
            "Contributor Covenant, version 2.1, available at ...\n",
        );
        write(root, "CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n");
        for file in [
            ".github/ISSUE_TEMPLATE/bug_report.yml",
            ".github/ISSUE_TEMPLATE/feature_request.yml",
            ".github/ISSUE_TEMPLATE/config.yml",
            ".github/pull_request_template.md",
            ".github/labels.yml",
        ] {
            write(root, file, "x\n");
        }
        write(
            root,
            "assets/locales/en.yml",
            &format!("app.title: \"Tessera\"\nwelcome.trademark: \"{notice}\"\n"),
        );
        write(
            root,
            "Cargo.toml",
            "[workspace.dependencies]\n\
             tessera-core = { path = \"crates/tessera-core\" }\n\
             # a comment\n\
             serde = { version = \"=1.0.0\", features = [\"derive\"] }\n\
             anyhow = \"=1.0.0\"\n\
             \n\
             [workspace.lints.rust]\n\
             missing_docs = \"warn\"\n",
        );
        write(
            root,
            "crates/tessera-core/Cargo.toml",
            "[package]\nname = \"tessera-core\"\nversion.workspace = true\n\n\
             [dependencies]\nanyhow.workspace = true\nserde = { workspace = true }\n\
             local = \"1\"\n",
        );
        write(
            root,
            "xtask/Cargo.toml",
            "[dependencies]\nanyhow.workspace = true\n",
        );
        let spdx = "// SPDX-License-Identifier: GPL-3.0-or-later\n";
        write(root, "crates/tessera-core/src/lib.rs", spdx);
        write(root, "xtask/src/main.rs", spdx);
        dir
    }

    fn one_problem(dir: &TempDir, needle: &str) {
        let problems = check(dir.path());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(needle), "{problems:?}");
    }

    #[test]
    fn a_clean_tree_has_no_problem() {
        assert_eq!(check(valid().path()), Vec::<String>::new());
    }

    #[test]
    fn a_missing_file_is_a_problem() {
        for file in [
            "README.md",
            "CONTRIBUTING.md",
            "SECURITY.md",
            ".github/labels.yml",
            ".github/ISSUE_TEMPLATE/bug_report.yml",
            ".github/pull_request_template.md",
        ] {
            let dir = valid();
            std::fs::remove_file(dir.path().join(file)).expect("remove");
            let problems = check(dir.path());
            assert!(
                problems.iter().any(|p| p.contains(file)),
                "{file}: {problems:?}"
            );
        }
    }

    #[test]
    fn a_license_that_is_not_the_gpl_is_a_problem() {
        let dir = valid();
        write(dir.path(), "LICENSE", "MIT License\n");
        one_problem(&dir, "LICENSE");
    }

    #[test]
    fn a_changelog_without_unreleased_is_a_problem() {
        let dir = valid();
        write(dir.path(), "CHANGELOG.md", "# Changelog\n");
        one_problem(&dir, "CHANGELOG.md");
    }

    #[test]
    fn another_code_of_conduct_is_a_problem() {
        let dir = valid();
        write(dir.path(), "CODE_OF_CONDUCT.md", "Be nice.\n");
        one_problem(&dir, "CODE_OF_CONDUCT.md");
    }

    #[test]
    fn a_source_file_without_spdx_is_a_problem() {
        let dir = valid();
        write(
            dir.path(),
            "crates/tessera-core/src/bad.rs",
            "fn main() {}\n",
        );
        one_problem(&dir, "bad.rs");
        let dir = valid();
        write(
            dir.path(),
            "xtask/src/bad.rs",
            "\n// SPDX-License-Identifier: GPL-3.0-or-later\n",
        );
        one_problem(&dir, "bad.rs");
    }

    #[test]
    fn an_unpinned_workspace_dependency_is_a_problem() {
        let dir = valid();
        let manifest = "[workspace.dependencies]\nserde = \"1\"\n";
        write(dir.path(), "Cargo.toml", manifest);
        one_problem(&dir, "serde");
        let dir = valid();
        let manifest = "[workspace.dependencies]\nserde = { version = \"1\", features = [] }\n";
        write(dir.path(), "Cargo.toml", manifest);
        one_problem(&dir, "serde");
    }

    #[test]
    fn a_path_dependency_passes() {
        let dir = valid();
        let manifest = "[workspace.dependencies]\nother = { path = \"crates/other\" }\n";
        write(dir.path(), "Cargo.toml", manifest);
        assert_eq!(check(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn a_path_without_spaces_passes() {
        let dir = valid();
        let manifest = "[workspace.dependencies]\nother = { path=\"crates/other\" }\n";
        write(dir.path(), "Cargo.toml", manifest);
        assert_eq!(check(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn table_and_dotted_workspace_dependencies_are_seen() {
        for manifest in [
            "[workspace.dependencies.foo]\nversion = \"1\"\nfeatures = [\"a\"]\n",
            "[workspace.dependencies]\nfoo.version = \"1\"\n",
        ] {
            let dir = valid();
            write(dir.path(), "Cargo.toml", manifest);
            one_problem(&dir, "`foo` is not pinned");
        }
        let dir = valid();
        let manifest = "[workspace.dependencies.foo]\nversion = \"=1\"\n\n[workspace]\nx = 1\n";
        write(dir.path(), "Cargo.toml", manifest);
        assert_eq!(check(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn a_crate_table_or_dotted_version_of_a_pinned_dependency_is_a_problem() {
        for dependency in [
            "[dependencies.anyhow]\nversion = \"1\"\n",
            "[dependencies]\nanyhow.version = \"1\"\n",
        ] {
            let dir = valid();
            write(dir.path(), "xtask/Cargo.toml", dependency);
            one_problem(&dir, "xtask/Cargo.toml names its own version of `anyhow`");
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_a_problem() {
        let invalid: &[u8] = &[0xff, 0xfe];
        for file in [
            "Cargo.toml",
            "xtask/Cargo.toml",
            "crates/tessera-core/Cargo.toml",
            "crates/tessera-core/src/lib.rs",
            "assets/locales/en.yml",
            "README.md",
        ] {
            let dir = valid();
            std::fs::write(dir.path().join(file), invalid).expect("write");
            let problems = check(dir.path());
            assert!(!problems.is_empty(), "{file}");
        }
        let dir = valid();
        std::fs::remove_file(dir.path().join("Cargo.toml")).expect("remove");
        one_problem(&dir, "cannot read Cargo.toml");
        let dir = valid();
        std::fs::remove_dir_all(dir.path().join("crates")).expect("remove");
        let problems = check(dir.path());
        assert!(problems.iter().any(|p| p.contains("cannot read crates")));
    }

    #[test]
    fn a_crate_naming_its_own_version_is_a_problem() {
        let dir = valid();
        let manifest = "[dependencies]\nanyhow = \"1\"\n";
        write(dir.path(), "crates/tessera-core/Cargo.toml", manifest);
        one_problem(&dir, "anyhow");
        let dir = valid();
        let manifest = "[dev-dependencies]\nserde = { version = \"=1.0.0\" }\n";
        write(dir.path(), "xtask/Cargo.toml", manifest);
        one_problem(&dir, "serde");
        let dir = valid();
        let manifest = "[target.'cfg(windows)'.dependencies]\nanyhow = \"1\"\n";
        write(dir.path(), "xtask/Cargo.toml", manifest);
        one_problem(&dir, "anyhow");
    }

    #[test]
    fn a_missing_notice_is_a_problem() {
        let dir = valid();
        write(dir.path(), "README.md", "# Tessera\n");
        one_problem(&dir, "README.md");
        let dir = valid();
        write(
            dir.path(),
            "assets/locales/en.yml",
            "app.title: \"Tessera\"\n",
        );
        one_problem(&dir, "welcome.trademark");
    }

    #[test]
    fn a_product_name_with_typesense_is_a_problem() {
        let dir = valid();
        let notice = "Tessera is not affiliated with or endorsed by Typesense, Inc.";
        let en = format!("app.title: \"Tessera for Typesense\"\nwelcome.trademark: \"{notice}\"\n");
        write(dir.path(), "assets/locales/en.yml", &en);
        one_problem(&dir, "product name");
    }

    #[test]
    fn dco_finds_a_commit_without_a_trailer() {
        let log = "abc1234\0feat: one\n\nSigned-off-by: A <a@b.c>\n\u{1}\n\
                   def5678\0feat: two\n\nmentions Signed-off-by: in the text\n\u{1}\n";
        assert_eq!(dco_missing(log), ["def5678"]);
    }

    #[test]
    fn dco_passes_a_signed_log_and_an_empty_one() {
        let log =
            "abc1234\0feat: one\n\nSigned-off-by: A <a@b.c>\nCo-Authored-By: B <b@c.d>\n\u{1}\n";
        assert!(dco_missing(log).is_empty());
        assert!(dco_missing("").is_empty());
    }
}
