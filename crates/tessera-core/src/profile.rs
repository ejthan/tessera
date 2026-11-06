// SPDX-License-Identifier: GPL-3.0-or-later

//! Connection profile model.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;
use uuid::Uuid;

/// A saved connection to a Typesense cluster. Holds no secrets.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    /// Stable identifier, also the keychain account name.
    pub id: Uuid,
    /// Display name.
    pub name: String,
    /// Accent color shown in the sidebar and tabs.
    #[serde(default)]
    pub color: ProfileColor,
    /// Kind of environment this connection points at.
    #[serde(default)]
    pub environment: Environment,
    /// Cluster nodes, tried in order.
    pub nodes: Vec<Node>,
    /// Optional nearest node for geo-distributed clusters.
    #[serde(default)]
    pub nearest_node: Option<Node>,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
    /// Retries per request.
    #[serde(default = "default_retries")]
    pub retries: u32,
    /// Block all write operations.
    #[serde(default)]
    pub read_only: bool,
    /// TLS settings.
    #[serde(default)]
    pub tls: TlsOptions,
    /// Names of the collections the user starred.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub favorite_collections: Vec<String>,
}

fn default_timeout() -> u32 {
    5
}

fn default_retries() -> u32 {
    2
}

impl Profile {
    /// Creates a profile with a fresh id and default settings.
    pub fn new(name: impl Into<String>, nodes: Vec<Node>) -> Profile {
        Profile {
            id: Uuid::new_v4(),
            name: name.into(),
            color: ProfileColor::default(),
            environment: Environment::default(),
            nodes,
            nearest_node: None,
            timeout_secs: default_timeout(),
            retries: default_retries(),
            read_only: false,
            tls: TlsOptions::default(),
            favorite_collections: Vec::new(),
        }
    }
}

/// Accent color of a profile.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum ProfileColor {
    #[default]
    Gray,
    Blue,
    Teal,
    Violet,
    Amber,
    Red,
}

/// Kind of environment a profile points at. Serialized as a plain string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Environment {
    /// A server on this machine.
    Local,
    /// A development server.
    #[default]
    Dev,
    /// A staging server.
    Staging,
    /// A production server.
    Prod,
    /// A user-defined environment name.
    Custom(String),
}

impl Environment {
    /// Whether this is the production environment.
    pub fn is_prod(&self) -> bool {
        matches!(Environment::parse(self.label()), Environment::Prod)
    }

    /// The text stored on disk and shown in the UI.
    pub fn label(&self) -> &str {
        match self {
            Environment::Local => "local",
            Environment::Dev => "dev",
            Environment::Staging => "staging",
            Environment::Prod => "prod",
            Environment::Custom(s) => s,
        }
    }
}

impl Environment {
    /// Parses a stored name: trimmed and case-insensitive for the built-ins, else `Custom`.
    pub fn parse(raw: &str) -> Environment {
        let t = raw.trim();
        match t.to_ascii_lowercase().as_str() {
            "local" => Environment::Local,
            "dev" => Environment::Dev,
            "staging" => Environment::Staging,
            "prod" => Environment::Prod,
            _ => Environment::Custom(t.to_owned()),
        }
    }
}

impl Serialize for Environment {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Normalize so `Custom("prod")` is written as the built-in and reads back as `Prod`.
        s.serialize_str(Environment::parse(self.label()).label())
    }
}

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Environment::parse(&String::deserialize(d)?))
    }
}

/// URL scheme of a node.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// Plain HTTP.
    Http,
    /// HTTP over TLS.
    Https,
}

/// A node that cannot be turned into a URL. The text is English diagnostics, not UI text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidNode(pub String);

impl std::fmt::Display for InvalidNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid node: {}", self.0)
    }
}

impl std::error::Error for InvalidNode {}

/// One Typesense node.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// URL scheme.
    pub protocol: Protocol,
    /// Host name or IP address, without brackets.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Optional path prefix, for example `/typesense`.
    pub path: String,
}

impl Node {
    /// Checks that the node can be turned into a URL.
    pub fn validate(&self) -> Result<(), InvalidNode> {
        self.url().map(drop)
    }

    /// Base URL of the node. IPv6 hosts are bracketed and the path always ends with `/`.
    pub fn url(&self) -> Result<Url, InvalidNode> {
        let host = self.host.as_str();
        if host.is_empty()
            || host.contains(char::is_whitespace)
            || host.contains(['/', '?', '#', '@'])
            || host.contains("://")
        {
            return Err(InvalidNode(format!(
                "host {host:?} is not a plain host name"
            )));
        }
        let scheme = match self.protocol {
            Protocol::Http => "http",
            Protocol::Https => "https",
        };
        let bad_host = |_| InvalidNode(format!("host {host:?} is not valid"));
        let mut url = Url::parse(&format!("{scheme}://localhost/")).map_err(bad_host)?;
        if host.contains(':') && !host.starts_with('[') {
            url.set_host(Some(&format!("[{host}]")))
        } else {
            url.set_host(Some(host))
        }
        .map_err(bad_host)?;
        url.set_port(Some(self.port))
            .map_err(|()| InvalidNode("port is not valid".into()))?;
        let mut segs = url
            .path_segments_mut()
            .map_err(|()| InvalidNode("path is not valid".into()))?;
        segs.clear();
        segs.extend(self.path.split('/').filter(|s| !s.is_empty()));
        segs.push("");
        drop(segs);
        Ok(url)
    }

    /// Short `host:port` label.
    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Whether the node runs on this machine.
    pub fn is_local(&self) -> bool {
        matches!(
            self.host.to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "::1" | "[::1]"
        )
    }
}

/// What makes a connection readable or changeable on the way (D98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportRisk {
    /// `http://` to a host that is not this machine.
    PlainHttp {
        /// The node, as `host:port`.
        host: String,
    },
    /// Any server certificate is accepted.
    InsecureCertificates,
}

impl Profile {
    /// How many different nodes the profile can reach: its nodes and the nearest node, once each.
    pub fn distinct_nodes(&self) -> usize {
        let nodes = self.nodes.iter().chain(&self.nearest_node);
        let urls: std::collections::BTreeSet<_> = nodes.filter_map(|n| n.url().ok()).collect();
        urls.len()
    }

    /// Every risk of this profile: each remote `http` node, the nearest node too, then the
    /// certificates.
    pub fn transport_risks(&self) -> Vec<TransportRisk> {
        let plain = self
            .nodes
            .iter()
            .chain(&self.nearest_node)
            .filter(|n| n.protocol == Protocol::Http && !n.is_local())
            .map(|n| TransportRisk::PlainHttp { host: n.label() });
        let insecure = self
            .tls
            .allow_insecure
            .then_some(TransportRisk::InsecureCertificates);
        plain.chain(insecure).collect()
    }
}

/// TLS settings of a profile.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TlsOptions {
    /// Extra CA certificate to trust.
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    /// Skip certificate verification.
    #[serde(default)]
    pub allow_insecure: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn node_url_brackets_ipv6() {
        let n = Node {
            protocol: Protocol::Http,
            host: "::1".into(),
            port: 8108,
            path: String::new(),
        };
        assert_eq!(n.url().unwrap().as_str(), "http://[::1]:8108/");
    }
    #[test]
    fn node_url_keeps_path_prefix() {
        let n = Node {
            protocol: Protocol::Https,
            host: "ts.example.com".into(),
            port: 443,
            path: "/typesense".into(),
        };
        assert_eq!(
            n.url().unwrap().as_str(),
            "https://ts.example.com/typesense/"
        );
        assert_eq!(n.label(), "ts.example.com:443");
    }
    #[test]
    fn environment_serializes_as_plain_string() {
        assert_eq!(
            serde_json::to_string(&Environment::Prod).unwrap(),
            r#""prod""#
        );
        assert_eq!(
            serde_json::from_str::<Environment>(r#""qa-eu""#).unwrap(),
            Environment::Custom("qa-eu".into())
        );
        assert!(!Environment::Custom("prod-like".into()).is_prod());
    }
    #[test]
    fn profile_round_trips_and_fills_defaults() {
        let p: Profile = serde_json::from_str(r#"{"id":"6f1c2a4e-8d3b-4c1a-9e2f-0b7d5a3c9e11","name":"local","nodes":[{"protocol":"http","host":"localhost","port":8108,"path":""}]}"#).unwrap();
        assert_eq!((p.timeout_secs, p.retries, p.read_only), (5, 2, false));
        assert_eq!(
            serde_json::from_str::<Profile>(&serde_json::to_string(&p).unwrap()).unwrap(),
            p
        );
    }
    #[test]
    fn transport_risks_table() {
        use TransportRisk::*;
        let node = |protocol, host: &str| Node {
            protocol,
            host: host.into(),
            port: 8108,
            path: String::new(),
        };
        let plain = |host: &str| PlainHttp {
            host: format!("{host}:8108"),
        };
        for (host, protocol, expected) in [
            ("localhost", Protocol::Http, vec![]),
            ("127.0.0.1", Protocol::Http, vec![]),
            ("::1", Protocol::Http, vec![]),
            ("[::1]", Protocol::Http, vec![]),
            (
                "localhost.evil.com",
                Protocol::Http,
                vec![plain("localhost.evil.com")],
            ),
            (
                "search.example",
                Protocol::Http,
                vec![plain("search.example")],
            ),
            ("search.example", Protocol::Https, vec![]),
        ] {
            let p = Profile::new("p", vec![node(protocol, host)]);
            assert_eq!(p.transport_risks(), expected, "{host}");
        }

        let mut p = Profile::new("p", vec![node(Protocol::Https, "a.example")]);
        p.nearest_node = Some(node(Protocol::Http, "near.example"));
        p.tls.allow_insecure = true;
        assert_eq!(
            p.transport_risks(),
            [plain("near.example"), InsecureCertificates]
        );
    }

    #[test]
    fn distinct_nodes_counts_the_nearest_node_once() {
        let node = |host: &str, path: &str| Node {
            protocol: Protocol::Http,
            host: host.into(),
            port: 8108,
            path: path.into(),
        };
        let mut p = Profile::new("p", vec![node("a", "")]);
        assert_eq!(p.distinct_nodes(), 1);
        p.nearest_node = Some(node("a", "/"));
        assert_eq!(p.distinct_nodes(), 1);
        p.nearest_node = Some(node("near", ""));
        assert_eq!(p.distinct_nodes(), 2);
        p.nodes = vec![node("a", ""), node("a", "/two"), node("a", "")];
        p.nearest_node = None;
        assert_eq!(p.distinct_nodes(), 2);
    }

    #[test]
    fn local_hosts_and_duplicate_slashes() {
        let n = Node {
            protocol: Protocol::Http,
            host: "localhost".into(),
            port: 1,
            path: "//a//b/".into(),
        };
        assert!(n.is_local());
        assert_eq!(n.url().unwrap().as_str(), "http://localhost:1/a/b/");
    }

    #[test]
    fn bad_hosts_are_rejected() {
        for h in ["https://x.com", "a.com/b", "", "a b", "u@h"] {
            let n = Node {
                protocol: Protocol::Http,
                host: h.into(),
                port: 1,
                path: String::new(),
            };
            assert!(n.validate().is_err(), "{h:?}");
        }
    }
    #[test]
    fn prebracketed_ipv6_works() {
        let n = Node {
            protocol: Protocol::Http,
            host: "[::1]".into(),
            port: 8108,
            path: String::new(),
        };
        assert_eq!(n.url().unwrap().as_str(), "http://[::1]:8108/");
        assert!(n.is_local());
    }
    #[test]
    fn environment_is_case_insensitive_and_round_trips() {
        assert_eq!(
            serde_json::from_str::<Environment>(r#""PROD""#).unwrap(),
            Environment::Prod
        );
        assert_eq!(
            serde_json::from_str::<Environment>(r#"" Staging ""#).unwrap(),
            Environment::Staging
        );
        let c = Environment::Custom("prod".into());
        assert!(c.is_prod());
        assert_eq!(serde_json::to_string(&c).unwrap(), r#""prod""#);
    }

    #[test]
    fn m1_file_loads_without_favorites() {
        let json = r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8","name":"old","nodes":[]}"#;
        let p: Profile = serde_json::from_str(json).unwrap();
        assert!(p.favorite_collections.is_empty());
        assert!(!serde_json::to_string(&p).unwrap().contains("favorite"));
    }
}
