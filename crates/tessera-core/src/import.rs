// SPDX-License-Identifier: GPL-3.0-or-later

//! Import of typesense-dashboard `config.json` files and export of profiles.

use std::fmt;

use secrecy::SecretString;
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use crate::profile::{Node, Profile, Protocol};
use crate::store::SCHEMA_VERSION;

/// Why a `config.json` could not be read at all.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The text is not valid JSON or has fields of the wrong type. Carries serde's message.
    #[error("invalid config.json: {0}")]
    Json(String),
}

/// A remark the import preview shows next to a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportNote {
    /// Entries of one `clusterTag` had different API keys; the first one was taken.
    DifferingClusterKeys,
    /// The entry used `host: "SAME"`, which only makes sense in the web version.
    SkippedSameHost,
    /// A node failed [`Node::validate`]; the row stays visible but unselected.
    InvalidNode,
}

/// One profile the user may import, with its key and preview state.
#[derive(Clone)]
pub struct ImportCandidate {
    /// The profile to create.
    pub profile: Profile,
    /// The API key to store in the keychain for it.
    pub key: SecretString,
    /// Remarks for the preview.
    pub notes: Vec<ImportNote>,
    /// The existing profile with the same nodes, if any.
    pub duplicate_of: Option<Uuid>,
    /// Whether the row is checked by default.
    pub selected: bool,
}

impl fmt::Debug for ImportCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportCandidate")
            .field("profile", &self.profile)
            .field("key", &"[redacted]")
            .field("notes", &self.notes)
            .field("duplicate_of", &self.duplicate_of)
            .field("selected", &self.selected)
            .finish()
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    #[serde(rename = "apiKey")]
    api_key: String,
    node: Option<RawNode>,
    #[serde(rename = "connectionTimeoutSeconds")]
    timeout: Option<u32>,
    history: Vec<RawEntry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawEntry {
    #[serde(rename = "apiKey")]
    api_key: String,
    node: RawNode,
    #[serde(rename = "clusterTag")]
    cluster_tag: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawNode {
    host: String,
    #[serde(deserialize_with = "lenient_port")]
    port: Option<u16>,
    protocol: Option<String>,
    path: Option<String>,
    tls: Option<bool>,
}

impl RawNode {
    /// Protocol comes from `protocol`, else from `tls`, else `http`; port defaults to 8108.
    fn node(&self) -> Node {
        let protocol = match (self.protocol.as_deref(), self.tls) {
            (Some(p), _) if p.eq_ignore_ascii_case("https") => Protocol::Https,
            (Some(p), _) if p.eq_ignore_ascii_case("http") => Protocol::Http,
            (_, Some(true)) => Protocol::Https,
            _ => Protocol::Http,
        };
        Node {
            protocol,
            host: self.host.trim().to_owned(),
            port: self.port.unwrap_or(8108),
            path: self.path.clone().unwrap_or_default(),
        }
    }
}

/// Accepts `"8108"`, `8108`, an empty string or null.
fn lenient_port<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum P {
        Num(u16),
        Str(String),
    }
    match Option::<P>::deserialize(d)? {
        None => Ok(None),
        Some(P::Num(n)) => Ok(Some(n)),
        Some(P::Str(s)) if s.trim().is_empty() => Ok(None),
        Some(P::Str(s)) => s.trim().parse().map(Some).map_err(serde::de::Error::custom),
    }
}

struct Group {
    name: String,
    tag: Option<String>,
    rows: Vec<(Node, String)>,
    same: bool,
}

/// Turns a typesense-dashboard `config.json` into import candidates.
///
/// The top-level node and every history entry become a profile; entries sharing a
/// `clusterTag` become one profile with several nodes. Candidates equal to a profile in
/// `existing` are unselected duplicates. `host: "SAME"` entries and nodes that fail
/// [`Node::validate`] are kept as unselected rows with a note, so the user can see them.
pub fn dashboard_config(
    json: &str,
    existing: &[Profile],
) -> Result<Vec<ImportCandidate>, ImportError> {
    let raw: RawConfig =
        serde_json::from_str(json).map_err(|e| ImportError::Json(e.to_string()))?;

    let top = raw.node.map(|node| RawEntry {
        api_key: raw.api_key,
        node,
        cluster_tag: None,
    });
    let mut groups: Vec<Group> = Vec::new();
    for e in top.into_iter().chain(raw.history) {
        let node = e.node.node();
        let same = node.host == "SAME";
        let tag = e.cluster_tag.filter(|t| !t.trim().is_empty());
        let joined = tag
            .as_ref()
            .filter(|_| !same)
            .and_then(|t| groups.iter_mut().find(|g| g.tag.as_ref() == Some(t)));
        if let Some(g) = joined {
            g.rows.push((node, e.api_key));
        } else {
            groups.push(Group {
                name: tag.clone().unwrap_or_else(|| node.host.clone()),
                tag: tag.filter(|_| !same),
                rows: vec![(node, e.api_key)],
                same,
            });
        }
    }

    Ok(groups
        .into_iter()
        .map(|g| {
            let mut notes = Vec::new();
            let first_key = g.rows[0].1.clone();
            if g.same {
                notes.push(ImportNote::SkippedSameHost);
            } else {
                if g.rows.iter().any(|r| r.1 != first_key) {
                    notes.push(ImportNote::DifferingClusterKeys);
                }
                if g.rows.iter().any(|r| r.0.validate().is_err()) {
                    notes.push(ImportNote::InvalidNode);
                }
            }
            let nodes: Vec<Node> = g.rows.into_iter().map(|r| r.0).collect();
            let duplicate_of = existing.iter().find(|p| p.nodes == nodes).map(|p| p.id);
            let mut profile = Profile::new(g.name, nodes);
            if let Some(t) = raw.timeout {
                profile.timeout_secs = t;
            }
            // Differing keys are only a remark; every other note or a duplicate unchecks the row.
            let selected = duplicate_of.is_none()
                && notes.iter().all(|n| *n == ImportNote::DifferingClusterKeys);
            ImportCandidate {
                profile,
                key: SecretString::from(first_key),
                notes,
                duplicate_of,
                selected,
            }
        })
        .collect())
}

/// Serializes profiles as pretty JSON `{ "schema_version", "profiles" }`. Keys are never
/// included because [`Profile`] does not hold one.
pub fn export_profiles(profiles: &[Profile]) -> String {
    // Serializing plain structs with string keys cannot fail.
    serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "profiles": profiles,
    }))
    .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::profile::Environment;
    use secrecy::ExposeSecret;

    const SAMPLE: &str = r#"{
      "apiKey": "abc",
      "node": {"host": "localhost", "port": "8108", "protocol": "http", "path": "", "tls": false},
      "connectionTimeoutSeconds": 10,
      "ui": {"hideProjectInfo": false}
    }"#;

    fn entry(host: &str, key: &str, tag: Option<&str>) -> String {
        let tag = tag.map_or(String::new(), |t| format!(r#","clusterTag":"{t}""#));
        format!(
            r#"{{"apiKey":"{key}","node":{{"host":"{host}","port":"443","protocol":"https","path":""}}{tag}}}"#
        )
    }

    fn with_history(entries: &[String]) -> String {
        format!(
            r#"{{"apiKey":"abc","node":{{"host":"localhost","port":"8108","protocol":"http","path":""}},"history":[{}]}}"#,
            entries.join(",")
        )
    }

    #[test]
    fn top_level_node_becomes_profile() {
        let c = dashboard_config(SAMPLE, &[]).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].profile.name, "localhost");
        assert_eq!(c[0].profile.timeout_secs, 10);
        assert_eq!(c[0].profile.environment, Environment::Dev);
        assert_eq!(c[0].key.expose_secret(), "abc");
        assert!(c[0].selected && c[0].notes.is_empty());
    }

    #[test]
    fn history_entries_become_profiles() {
        let j = with_history(&[
            entry("a.example", "def", None),
            entry("b.example", "ghi", None),
        ]);
        assert_eq!(dashboard_config(&j, &[]).unwrap().len(), 3);
    }

    #[test]
    fn cluster_tag_groups_nodes() {
        let j = with_history(&[
            entry("a.example", "def", Some("prod-cluster")),
            entry("b.example", "def", Some("prod-cluster")),
        ]);
        let c = dashboard_config(&j, &[]).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].profile.name, "prod-cluster");
        assert_eq!(c[1].profile.nodes.len(), 2);
        assert!(c[1].notes.is_empty());
    }

    #[test]
    fn differing_cluster_keys_are_noted() {
        let j = with_history(&[
            entry("a.example", "def", Some("t")),
            entry("b.example", "ghi", Some("t")),
        ]);
        let c = dashboard_config(&j, &[]).unwrap();
        assert_eq!(c[1].key.expose_secret(), "def");
        assert_eq!(c[1].notes, [ImportNote::DifferingClusterKeys]);
    }

    #[test]
    fn same_host_is_skipped() {
        let j = with_history(&[entry("SAME", "def", None)]);
        let c = dashboard_config(&j, &[]).unwrap();
        assert_eq!(c.len(), 2);
        let s = c.iter().find(|c| c.profile.name == "SAME").unwrap();
        assert_eq!(s.notes, [ImportNote::SkippedSameHost]);
        assert!(!s.selected);
    }

    #[test]
    fn duplicate_is_marked_and_unselected() {
        let first = dashboard_config(SAMPLE, &[]).unwrap();
        let existing = Profile::new("mine", first[0].profile.nodes.clone());
        let c = dashboard_config(SAMPLE, std::slice::from_ref(&existing)).unwrap();
        assert_eq!(c[0].duplicate_of, Some(existing.id));
        assert!(!c[0].selected);
    }

    #[test]
    fn lenient_node_fields() {
        let j = r#"{"apiKey":"abc","node":{"host":"ts.example","port":8108,"tls":true}}"#;
        let n = &dashboard_config(j, &[]).unwrap()[0].profile.nodes[0];
        assert_eq!(
            (n.protocol, n.port, n.path.as_str()),
            (Protocol::Https, 8108, "")
        );
        let j = r#"{"apiKey":"abc","node":{"host":"ts.example"}}"#;
        let n = &dashboard_config(j, &[]).unwrap()[0].profile.nodes[0];
        assert_eq!((n.protocol, n.port), (Protocol::Http, 8108));
    }

    #[test]
    fn invalid_node_is_kept_unselected() {
        let j = r#"{"apiKey":"abc","node":{"host":"https://x.example/"}}"#;
        let c = dashboard_config(j, &[]).unwrap();
        assert_eq!(c[0].notes, [ImportNote::InvalidNode]);
        assert!(!c[0].selected);
    }

    #[test]
    fn bad_json_is_an_error_and_debug_hides_key() {
        assert!(matches!(
            dashboard_config("{", &[]),
            Err(ImportError::Json(_))
        ));
        let c = dashboard_config(SAMPLE, &[]).unwrap();
        assert!(!format!("{:?}", c[0]).contains("abc"));
    }

    #[test]
    fn export_has_no_keys() {
        let c = dashboard_config(SAMPLE, &[]).unwrap();
        let out = export_profiles(&[c[0].profile.clone()]);
        assert!(!out.contains("abc"));
        assert!(out.contains("\"schema_version\": 1"));
    }
}
