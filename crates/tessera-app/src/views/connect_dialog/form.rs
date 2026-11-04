// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog's fields as plain values, and the rules that turn them into a profile.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::SharedString;
use rust_i18n::t;
use secrecy::SecretString;
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};

/// The texts of one node row.
#[derive(Clone, Debug)]
pub struct NodeValues {
    pub protocol: Protocol,
    pub host: String,
    pub port: String,
    pub path: String,
}

/// Everything the form shows, as typed.
#[derive(Clone, Debug)]
pub struct FormValues {
    /// The edited or new profile: its id and the fields the form does not show.
    pub base: Profile,
    /// Editing a saved profile: an empty key keeps the stored one.
    pub editing: bool,
    pub name: String,
    pub color: ProfileColor,
    /// `Custom(_)` means "other"; the name is `env_name`.
    pub env: Environment,
    pub env_name: String,
    pub nodes: Vec<NodeValues>,
    /// An empty host means no nearest node.
    pub nearest: NodeValues,
    pub key: String,
    pub read_only: bool,
    pub timeout: String,
    pub retries: String,
    pub ca_file: Option<PathBuf>,
    pub allow_insecure: bool,
}

/// Inline errors by the test id of the element that shows them.
pub type FormErrors = HashMap<String, SharedString>;

/// Error ids shown inside the Advanced section, which must open to show them.
pub const ADVANCED_ERRORS: [&str; 3] = [
    "connect.timeout.error",
    "connect.retries.error",
    "connect.nearest.error",
];

/// The profile and the typed key (`None`: keep the stored one), or every field error.
pub fn validate(v: &FormValues) -> Result<(Profile, Option<SecretString>), FormErrors> {
    let mut errors = FormErrors::new();
    let mut profile = v.base.clone();
    profile.nodes = Vec::new();
    for (i, node) in v.nodes.iter().enumerate() {
        match parse_node(node) {
            Ok(node) => profile.nodes.push(node),
            Err(e) => {
                errors.insert(format!("connect.node.{i}.error"), e);
            }
        }
    }
    profile.nearest_node = None;
    if !v.nearest.host.trim().is_empty() {
        match parse_node(&v.nearest) {
            Ok(node) => profile.nearest_node = Some(node),
            Err(e) => {
                errors.insert("connect.nearest.error".into(), e);
            }
        }
    }
    let name = v.name.trim();
    profile.name = match (name.is_empty(), profile.nodes.first()) {
        (true, Some(node)) => node.label(),
        _ => name.to_string(),
    };
    profile.color = v.color;
    profile.environment = match &v.env {
        Environment::Custom(_) => {
            let name = v.env_name.trim();
            if name.is_empty() {
                errors.insert(
                    "connect.env.error".into(),
                    t!("connect.error.env_required").into(),
                );
            }
            Environment::parse(name)
        }
        env => env.clone(),
    };
    profile.read_only = v.read_only;
    match in_range(v.timeout.trim(), 1, 300) {
        Some(secs) => profile.timeout_secs = secs,
        None => {
            errors.insert(
                "connect.timeout.error".into(),
                t!("connect.error.timeout_range").into(),
            );
        }
    }
    match in_range(v.retries.trim(), 0, 10) {
        Some(n) => profile.retries = n,
        None => {
            errors.insert(
                "connect.retries.error".into(),
                t!("connect.error.retries_range").into(),
            );
        }
    }
    profile.tls.ca_file = v.ca_file.clone();
    profile.tls.allow_insecure = v.allow_insecure;
    let key = v.key.trim();
    let key = (!key.is_empty()).then(|| SecretString::from(key.to_string()));
    if key.is_none() && !v.editing {
        errors.insert(
            "connect.key.error".into(),
            t!("connect.error.key_required").into(),
        );
    }
    if errors.is_empty() {
        Ok((profile, key))
    } else {
        Err(errors)
    }
}

/// Builds a node from the row's texts, with the dialog's error text on failure.
pub fn parse_node(v: &NodeValues) -> Result<Node, SharedString> {
    let host = v.host.trim();
    if host.is_empty() {
        return Err(t!("connect.error.host_required").into());
    }
    let port = match v.port.trim().parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(t!("connect.error.port_range").into()),
    };
    let node = Node {
        protocol: v.protocol,
        host: host.to_string(),
        port,
        path: v.path.trim().to_string(),
    };
    // Rejects a scheme, a path or whitespace in the host (Review Focus 2). The port is
    // checked above and every path is accepted, so a failure is always the host.
    node.validate()
        .map_err(|_| SharedString::from(t!("connect.error.host_format").to_string()))?;
    Ok(node)
}

/// `text` as a number within `lo..=hi`.
fn in_range(text: &str, lo: u32, hi: u32) -> Option<u32> {
    text.parse().ok().filter(|v| (lo..=hi).contains(v))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use secrecy::ExposeSecret as _;

    use super::*;

    fn row(host: &str, port: &str) -> NodeValues {
        NodeValues {
            protocol: Protocol::Http,
            host: host.into(),
            port: port.into(),
            path: String::new(),
        }
    }

    fn values() -> FormValues {
        rust_i18n::set_locale("en");
        FormValues {
            base: Profile::new("", Vec::new()),
            editing: false,
            name: "n".into(),
            color: ProfileColor::Blue,
            env: Environment::Dev,
            env_name: String::new(),
            nodes: vec![row("localhost", "8108")],
            nearest: row("", "8108"),
            key: "k".into(),
            read_only: false,
            timeout: "5".into(),
            retries: "2".into(),
            ca_file: None,
            allow_insecure: false,
        }
    }

    fn errors(v: &FormValues) -> Vec<String> {
        let mut ids: Vec<_> = validate(v).unwrap_err().into_keys().collect();
        ids.sort();
        ids
    }

    #[test]
    fn valid_form_builds_the_profile() {
        let mut v = values();
        v.ca_file = Some("/etc/ca.pem".into());
        v.allow_insecure = true;
        let (p, key) = validate(&v).unwrap();
        assert_eq!(p.id, v.base.id);
        assert_eq!((p.name.as_str(), p.color), ("n", ProfileColor::Blue));
        assert_eq!(p.nodes[0].label(), "localhost:8108");
        assert_eq!(p.nearest_node, None, "empty host means none");
        assert_eq!(
            p.tls.ca_file.as_deref(),
            Some(std::path::Path::new("/etc/ca.pem"))
        );
        assert!(p.tls.allow_insecure);
        assert_eq!(key.unwrap().expose_secret(), "k");
    }

    #[test]
    fn empty_name_falls_back_to_the_first_node() {
        let mut v = values();
        v.name = "  ".into();
        assert_eq!(validate(&v).unwrap().0.name, "localhost:8108");
    }

    #[test]
    fn environment_rules() {
        let mut v = values();
        v.env = Environment::Custom(String::new());
        v.env_name = " ".into();
        assert_eq!(errors(&v), ["connect.env.error"]);
        v.env_name = " PROD ".into();
        assert_eq!(validate(&v).unwrap().0.environment, Environment::Prod);
        v.env_name = "qa-eu".into();
        assert_eq!(
            validate(&v).unwrap().0.environment,
            Environment::Custom("qa-eu".into())
        );
    }

    #[test]
    fn key_rules() {
        let mut v = values();
        v.key = " ".into();
        assert_eq!(errors(&v), ["connect.key.error"]);
        v.editing = true;
        assert!(validate(&v).unwrap().1.is_none(), "keep the stored key");
    }

    #[test]
    fn range_rules() {
        let mut v = values();
        for (timeout, retries, ok) in [
            ("1", "0", true),
            ("300", "10", true),
            ("0", "0", false),
            ("301", "0", false),
            ("5", "11", false),
            ("5", "-1", false),
            ("", "", false),
        ] {
            v.timeout = timeout.into();
            v.retries = retries.into();
            assert_eq!(validate(&v).is_ok(), ok, "{timeout:?} {retries:?}");
        }
        v.timeout = "0".into();
        v.retries = "11".into();
        assert_eq!(
            errors(&v),
            ["connect.retries.error", "connect.timeout.error"]
        );
    }

    #[test]
    fn host_with_scheme_or_path_is_rejected() {
        rust_i18n::set_locale("en");
        for host in ["https://ts.example.com/", "ts.example.com/x", "a b"] {
            let err = parse_node(&row(host, "443")).expect_err(host);
            assert_eq!(err, "Enter the host without protocol or path.");
        }
        assert_eq!(parse_node(&row("::1", "8108")).unwrap().label(), "::1:8108");
    }

    #[test]
    fn port_must_be_in_range() {
        rust_i18n::set_locale("en");
        for port in ["0", "65536", "", "x"] {
            assert!(parse_node(&row("localhost", port)).is_err(), "{port:?}");
        }
        assert!(parse_node(&row("localhost", "65535")).is_ok());
    }

    #[test]
    fn nearest_node_is_validated() {
        let mut v = values();
        v.nearest = row("https://near", "443");
        assert_eq!(errors(&v), ["connect.nearest.error"]);
        v.nearest = row("near.example.com", "443");
        assert_eq!(
            validate(&v).unwrap().0.nearest_node.unwrap().label(),
            "near.example.com:443"
        );
    }
}
