// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask dist` and `cargo xtask release-notes`: the packages of this OS, and their notes.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, ensure};
use sha2::{Digest as _, Sha256};

use crate::governance::section_value;

/// NF5.
const SIZE_LIMIT: u64 = 41_943_040;
const INSTALL_PACKAGER: &str =
    "cargo install cargo-packager --locked --version 0.11.8 --root target/tools";
const INSTALL_RPM: &str =
    "cargo install cargo-generate-rpm --locked --version 0.21.1 --root target/tools";

/// The `--formats` of `cargo packager` on `os`.
pub fn formats_for(os: &str) -> &'static str {
    match os {
        "macos" => "app,dmg",
        "linux" => "deb,appimage",
        "windows" => "wix",
        _ => "",
    }
}

/// What `run` puts into `target/dist` on `os`, as the tools name it.
pub fn package_names(os: &str, arch: &str, version: &str) -> Vec<String> {
    let short = if arch == "x86_64" { "x64" } else { arch };
    let debian = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    match os {
        "macos" => vec![format!("Tessera_{version}_{short}.dmg")],
        "linux" => vec![
            format!("tessera_{version}_{debian}.deb"),
            format!("tessera_{version}_{arch}.AppImage"),
            format!("tessera-{version}-1.{arch}.rpm"),
        ],
        "windows" => vec![
            format!("tessera_{version}_{short}_en-US.msi"),
            format!("tessera_{version}_{short}.zip"),
        ],
        _ => Vec::new(),
    }
}

/// `SHA256SUMS` of `paths`, sorted by file name, read in pieces because a `.dmg` is large.
pub fn sha256sums(paths: &[PathBuf]) -> std::io::Result<String> {
    let mut lines = Vec::new();
    for path in paths {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        let hex: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        lines.push(format!("{hex}  {name}\n"));
    }
    lines.sort_by(|a, b| a[64..].cmp(&b[64..]));
    Ok(lines.concat())
}

/// The section of `version` in `changelog`, from its heading to the next.
pub fn release_notes(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## [{version}]");
    let mut lines = changelog.lines().skip_while(|l| !l.starts_with(&heading));
    let first = lines.next()?;
    let rest = lines.take_while(|l| !l.starts_with("## ["));
    let section = std::iter::once(first).chain(rest).collect::<Vec<_>>();
    Some(section.join("\n").trim_end().to_string() + "\n")
}

/// The install commands of the tools `path` does not hold.
fn missing_tools(os: &str, path: &OsStr) -> Vec<&'static str> {
    let found = |tool: &str| {
        let file = format!("{tool}{}", std::env::consts::EXE_SUFFIX);
        std::env::split_paths(path).any(|dir| dir.join(&file).is_file())
    };
    let mut missing = Vec::new();
    if !found("cargo-packager") {
        missing.push(INSTALL_PACKAGER);
    }
    if os == "linux" && !found("cargo-generate-rpm") {
        missing.push(INSTALL_RPM);
    }
    missing
}

/// Builds this OS's packages into `target/dist`. `Ok(false)` means a tool is missing.
pub fn run(root: &Path) -> anyhow::Result<bool> {
    use std::env::consts::{ARCH, EXE_SUFFIX, OS};

    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let tools = std::iter::once(root.join("target/tools/bin"))
        .chain(std::env::split_paths(&inherited))
        .collect::<Vec<_>>();
    let path: OsString = std::env::join_paths(tools)?;
    let missing = missing_tools(OS, &path);
    for line in &missing {
        eprintln!("{line}");
    }
    if !missing.is_empty() {
        return Ok(false);
    }

    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let version = section_value(&manifest, "workspace.package", "version")
        .context("Cargo.toml names no workspace version")?;
    let names = package_names(OS, ARCH, version);
    ensure!(!names.is_empty(), "no packages are defined for {OS}");

    let cargo = |args: &[&str]| -> anyhow::Result<()> {
        let mut command = Command::new("cargo");
        command.current_dir(root).env("PATH", &path).args(args);
        if OS == "macos" {
            // `CI` makes the dmg step skip its Finder automation.
            command
                .env("MACOSX_DEPLOYMENT_TARGET", "13.0")
                .env("CI", "true");
        }
        let status = command.status().context("cannot run cargo")?;
        ensure!(status.success(), "cargo {} failed", args.join(" "));
        Ok(())
    };
    let formats = formats_for(OS);
    cargo(&[
        "packager",
        "-p",
        "tessera-app",
        "--release",
        "--formats",
        formats,
    ])?;

    let release = root.join("target/release");
    // shortcut: the rpm and the Windows zip never run on macOS, so only release.yml proves them;
    // correct them after its first run.
    if OS == "linux" {
        // The flag says "crate name", and the tool reads it as a folder under the root.
        cargo(&["generate-rpm", "-p", "crates/tessera-app"])?;
    }
    if let Some(zip) = names.iter().find(|name| name.ends_with(".zip")) {
        let status = Command::new("tar")
            .current_dir(root)
            .args(["-a", "-c", "-f"])
            .arg(release.join(zip))
            .args(["-C", "target/release", "tessera.exe"])
            .args(["-C", "../..", "LICENSE", "THIRD_PARTY_NOTICES.md"])
            .status()
            .context("cannot run tar")?;
        ensure!(status.success(), "tar failed");
    }

    let binary = release.join(format!("tessera{EXE_SUFFIX}"));
    let size = std::fs::metadata(&binary)
        .with_context(|| format!("read {}", binary.display()))?
        .len();
    ensure!(size < SIZE_LIMIT, "the binary is {size} bytes (NF5)");
    println!("{}: {size} bytes", binary.display());

    let dist = root.join("target/dist");
    if dist.exists() {
        std::fs::remove_dir_all(&dist)?;
    }
    std::fs::create_dir_all(&dist)?;
    let mut copies = Vec::new();
    for name in &names {
        let from = if name.ends_with(".rpm") {
            root.join("target/generate-rpm").join(name)
        } else {
            release.join(name)
        };
        let to = dist.join(name);
        std::fs::copy(&from, &to).with_context(|| format!("copy {}", from.display()))?;
        copies.push(to);
    }
    std::fs::write(dist.join("SHA256SUMS"), sha256sums(&copies)?)?;
    println!("{}", dist.display());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{
        INSTALL_PACKAGER, INSTALL_RPM, formats_for, missing_tools, package_names, release_notes,
        sha256sums,
    };

    #[test]
    fn each_os_has_its_formats() {
        assert_eq!(formats_for("macos"), "app,dmg");
        assert_eq!(formats_for("linux"), "deb,appimage");
        assert_eq!(formats_for("windows"), "wix");
    }

    #[test]
    fn packages_carry_the_names_of_the_tools() {
        assert_eq!(
            package_names("macos", "aarch64", "1.0.0"),
            ["Tessera_1.0.0_aarch64.dmg"]
        );
        assert_eq!(
            package_names("macos", "x86_64", "1.0.0"),
            ["Tessera_1.0.0_x64.dmg"]
        );
        assert_eq!(
            package_names("linux", "x86_64", "1.0.0"),
            [
                "tessera_1.0.0_amd64.deb",
                "tessera_1.0.0_x86_64.AppImage",
                "tessera-1.0.0-1.x86_64.rpm"
            ]
        );
        assert_eq!(
            package_names("windows", "x86_64", "1.0.0"),
            ["tessera_1.0.0_x64_en-US.msi", "tessera_1.0.0_x64.zip"]
        );
    }

    #[test]
    fn sums_are_in_shasum_format_and_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (b, a) = (dir.path().join("b.dmg"), dir.path().join("a.dmg"));
        std::fs::write(&b, "abc").expect("write");
        std::fs::write(&a, "").expect("write");
        assert_eq!(
            sha256sums(&[b, a]).expect("sums"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  a.dmg\n\
             ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  b.dmg\n"
        );
    }

    const CHANGELOG: &str = "# Changelog\n\n## [Unreleased]\n\n- soon\n\n\
                             ## [1.0.0] - 2026-11-07\n\n### Added\n\n- all\n\n\
                             ## [0.9.0]\n\n- old\n";

    #[test]
    fn release_notes_stop_at_the_next_heading() {
        assert_eq!(
            release_notes(CHANGELOG, "1.0.0").as_deref(),
            Some("## [1.0.0] - 2026-11-07\n\n### Added\n\n- all\n")
        );
        assert_eq!(
            release_notes(CHANGELOG, "0.9.0").as_deref(),
            Some("## [0.9.0]\n\n- old\n")
        );
    }

    #[test]
    fn an_unknown_version_has_no_notes() {
        assert_eq!(release_notes(CHANGELOG, "1.0"), None);
    }

    #[test]
    fn a_missing_tool_names_its_install_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = OsString::from(dir.path());
        assert_eq!(missing_tools("macos", &path), [INSTALL_PACKAGER]);
        assert_eq!(
            missing_tools("linux", &path),
            [INSTALL_PACKAGER, INSTALL_RPM]
        );
        let tool = format!("cargo-packager{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(dir.path().join(tool), "").expect("write");
        assert_eq!(missing_tools("macos", &path), Vec::<&str>::new());
    }
}
