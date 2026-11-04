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
        matches!(self, Environment::Prod)
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

impl Serialize for Environment {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "local" => Environment::Local,
            "dev" => Environment::Dev,
            "staging" => Environment::Staging,
            "prod" => Environment::Prod,
            _ => Environment::Custom(s),
        })
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
    /// Base URL of the node. IPv6 hosts are bracketed and the path always ends with `/`.
    ///
    /// An unparseable host falls back to `http://localhost/`; input is validated before saving.
    pub fn url(&self) -> Url {
        let scheme = match self.protocol {
            Protocol::Http => "http",
            Protocol::Https => "https",
        };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let path: Vec<&str> = self.path.split('/').filter(|s| !s.is_empty()).collect();
        let mut raw = format!("{scheme}://{host}:{}/", self.port);
        for seg in path {
            raw.push_str(seg);
            raw.push('/');
        }
        // shortcut: invalid hosts fall back to localhost, the dialog validates input before saving.
        Url::parse(&raw).unwrap_or_else(|_| Url::parse("http://localhost/").expect("static URL"))
    }

    /// Short `host:port` label.
    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Whether the node runs on this machine.
    pub fn is_local(&self) -> bool {
        matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "::1")
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
        assert_eq!(n.url().as_str(), "http://[::1]:8108/");
    }
    #[test]
    fn node_url_keeps_path_prefix() {
        let n = Node {
            protocol: Protocol::Https,
            host: "ts.example.com".into(),
            port: 443,
            path: "/typesense".into(),
        };
        assert_eq!(n.url().as_str(), "https://ts.example.com/typesense/");
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
    fn local_hosts_and_duplicate_slashes() {
        let n = Node {
            protocol: Protocol::Http,
            host: "localhost".into(),
            port: 1,
            path: "//a//b/".into(),
        };
        assert!(n.is_local());
        assert_eq!(n.url().as_str(), "http://localhost:1/a/b/");
    }
}
