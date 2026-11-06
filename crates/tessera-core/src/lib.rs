// SPDX-License-Identifier: GPL-3.0-or-later

//! Domain logic for Tessera that does not depend on the UI.

pub mod ai;
pub mod analytics;
pub mod cli;
pub mod curl;
pub mod documents;
pub mod import;
pub mod keymap;
pub mod keys;
pub mod log;
pub mod metrics;
pub mod openapi;
pub mod profile;
pub mod raw;
pub mod redact;
pub mod relevance;
pub mod schema;
pub mod search;
pub mod secrets;
pub mod stemming;
pub mod store;
pub mod transfer;

use semver::{Prerelease, Version};

/// Oldest Typesense release (major, minor) that Tessera fully supports.
pub const MIN_SUPPORTED: (u64, u64) = (30, 0);

/// What Tessera knows about a server, derived from its reported version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCapabilities {
    /// The parsed server version, or `None` if the string was not understood.
    pub version: Option<Version>,
    /// Whether the server meets [`MIN_SUPPORTED`]. Unknown versions count as supported,
    /// because Tessera cannot tell and should not block the user.
    pub supported: bool,
}

impl ServerCapabilities {
    /// Derives capabilities from the version string reported by `GET /debug`.
    ///
    /// Parsing is lenient: a leading `v` is ignored, missing minor or patch parts
    /// become `0`, and a trailing non-numeric part (`30.0.rc1`) becomes a pre-release tag.
    pub fn from_version(raw: &str) -> Self {
        let version = parse_lenient(raw);
        // Compare major/minor only, so pre-releases of 30.0 count as 30.0.
        let supported = version
            .as_ref()
            .is_none_or(|v| (v.major, v.minor) >= MIN_SUPPORTED);
        Self { version, supported }
    }

    /// Whether curation rules take `rule.stem` and `rule.synonyms`: from 30.2 on, while 30.0
    /// drops them silently. An unknown version counts as yes.
    pub fn rule_flags(&self) -> bool {
        self.version
            .as_ref()
            .is_none_or(|v| (v.major, v.minor) >= (30, 2))
    }
}

fn parse_lenient(raw: &str) -> Option<Version> {
    let s = raw.trim().trim_start_matches('v');
    if let Ok(v) = Version::parse(s) {
        return Some(v);
    }
    let mut numbers = Vec::new();
    let mut rest = Vec::new();
    for part in s.split('.') {
        match part.parse::<u64>() {
            Ok(n) if rest.is_empty() && numbers.len() < 3 => numbers.push(n),
            _ => rest.push(part),
        }
    }
    let major = *numbers.first()?;
    let mut v = Version::new(
        major,
        numbers.get(1).copied().unwrap_or(0),
        numbers.get(2).copied().unwrap_or(0),
    );
    if !rest.is_empty() {
        v.pre = Prerelease::new(&rest.join(".")).ok()?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    #[test]
    fn parses_full_version() {
        let c = ServerCapabilities::from_version("30.2.0");
        assert_eq!(c.version, Some(Version::new(30, 2, 0)));
        assert!(c.supported);
    }

    #[test]
    fn fills_missing_patch() {
        assert_eq!(
            ServerCapabilities::from_version("30.2").version,
            Some(Version::new(30, 2, 0))
        );
    }

    #[test]
    fn strips_v_and_keeps_rc_as_prerelease() {
        let c = ServerCapabilities::from_version("v30.0.rc1");
        let v = c.version.clone().expect("version parses");
        assert_eq!((v.major, v.minor, v.patch), (30, 0, 0));
        assert_eq!(v.pre.as_str(), "rc1");
        assert!(c.supported);
    }

    #[test]
    fn old_version_is_unsupported() {
        assert!(!ServerCapabilities::from_version("29.0").supported);
    }

    #[test]
    fn rule_flags_need_30_2() {
        for (version, flags) in [
            ("29.0", false),
            ("30.0", false),
            ("30.1", false),
            ("30.2", true),
            ("31.0", true),
            ("nightly", true),
        ] {
            assert_eq!(
                ServerCapabilities::from_version(version).rule_flags(),
                flags,
                "{version}"
            );
        }
    }

    #[test]
    fn garbage_is_unknown_but_supported() {
        let c = ServerCapabilities::from_version("nightly");
        assert_eq!(c.version, None);
        assert!(c.supported);
    }
}
