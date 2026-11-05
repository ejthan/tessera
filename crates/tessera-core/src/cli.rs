// SPDX-License-Identifier: GPL-3.0-or-later

//! Command-line flags and `TYPESENSE_*` variables: decides how the app launches.

use std::ffi::OsString;
use std::fmt;

use secrecy::SecretString;

use crate::profile::{Environment, Node, Profile, Protocol};

/// Usage text printed for `--help`. CLI output is not UI, so it is not localized.
pub const HELP: &str = "\
Usage: tessera [OPTIONS]

Connection (or set TYPESENSE_HOST, TYPESENSE_PORT, TYPESENSE_PROTOCOL, TYPESENSE_PATH):
  --host <host>          Open a temporary connection to this host
  --port <port>          Port (default 8108)
  --protocol <http|https>  Default: http for localhost, otherwise https
  --path <path>          URL path prefix
  --env <local|dev|staging|prod>
                         Environment (default: local for localhost, otherwise dev)
  --read-only            Block write requests (always on for --env prod)
  --api-key-stdin        Read the API key as one line from stdin
                         (TYPESENSE_API_KEY is also read; a key is never a flag value)

Other:
  --profile <name>       Open a saved profile (cannot be combined with connection options)
  --help                 Show this text
";

/// A command-line error. The message is printed before exiting with code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

/// How the app starts.
pub enum Launch {
    /// Open the normal window with no connection.
    Normal,
    /// Open this saved profile.
    Profile(String),
    /// Open a temporary connection that is never saved.
    Temporary {
        /// The in-memory profile.
        profile: Profile,
        /// Read one line from stdin as the API key (done by the app).
        key_from_stdin: bool,
        /// Key from `TYPESENSE_API_KEY`.
        key_from_env: Option<SecretString>,
    },
    /// Print [`HELP`] and exit.
    Help,
}

impl fmt::Debug for Launch {
    // Hand-written so the key can never leak through `{:?}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Launch::Normal => f.write_str("Normal"),
            Launch::Profile(n) => f.debug_tuple("Profile").field(n).finish(),
            Launch::Help => f.write_str("Help"),
            Launch::Temporary {
                profile,
                key_from_stdin,
                key_from_env,
            } => f
                .debug_struct("Temporary")
                .field("profile", profile)
                .field("key_from_stdin", key_from_stdin)
                .field("key_from_env", &key_from_env.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

/// Converts the arguments (without argv[0]) to UTF-8. The error names only the position:
/// the argument itself may be anything.
pub fn utf8_args(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, CliError> {
    args.into_iter()
        .enumerate()
        .map(|(i, a)| {
            a.into_string()
                .map_err(|_| CliError(format!("argument {} is not valid UTF-8", i + 1)))
        })
        .collect()
}

/// Parses `args` (without argv[0]) and the environment into a [`Launch`].
pub fn parse(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Launch, CliError> {
    if args.iter().any(|a| a == "--help") {
        return Ok(Launch::Help);
    }
    let (mut host, mut port, mut protocol, mut path, mut env_name) = (None, None, None, None, None);
    let (mut profile, mut read_only, mut key_from_stdin) = (None, false, false);
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
            _ => (arg.as_str(), None),
        };
        if matches!(name, "--read-only" | "--api-key-stdin") {
            if inline.is_some() {
                return Err(CliError(format!("{name} does not take a value")));
            }
            if name == "--read-only" {
                read_only = true;
            } else {
                key_from_stdin = true;
            }
            continue;
        }
        let slot = match name {
            "--host" => &mut host,
            "--port" => &mut port,
            "--protocol" => &mut protocol,
            "--path" => &mut path,
            "--env" => &mut env_name,
            "--profile" => &mut profile,
            "--api-key" => {
                return Err(CliError(
                    "a key is never accepted as a flag value (it would show in the process list); use --api-key-stdin or TYPESENSE_API_KEY".into(),
                ));
            }
            // Only the name is echoed: a mistyped `--apikey=SECRET` must not reach stderr.
            _ if name.starts_with("--") => {
                return Err(CliError(format!("unknown option {name} (see --help)")));
            }
            _ => return Err(CliError("unexpected argument (see --help)".into())),
        };
        let value = match inline {
            Some(v) => v,
            None => match args.get(i) {
                Some(next) if !next.starts_with("--") => {
                    i += 1;
                    next.clone()
                }
                _ => return Err(CliError(format!("{name} needs a value"))),
            },
        };
        if value.trim().is_empty() && matches!(name, "--host" | "--profile") {
            return Err(CliError(format!("{name} needs a value")));
        }
        *slot = Some(value);
    }

    let connection_flag = host.is_some()
        || port.is_some()
        || protocol.is_some()
        || path.is_some()
        || env_name.is_some()
        || read_only
        || key_from_stdin;
    if let Some(name) = profile {
        if connection_flag {
            return Err(CliError(
                "--profile cannot be combined with connection options".into(),
            ));
        }
        return Ok(Launch::Profile(name));
    }

    let var = |k: &str| {
        env(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let Some(host) = host.or_else(|| var("TYPESENSE_HOST")) else {
        if connection_flag {
            return Err(CliError(
                "connection options need --host or TYPESENSE_HOST".into(),
            ));
        }
        return Ok(Launch::Normal);
    };
    let port = match port.or_else(|| var("TYPESENSE_PORT")) {
        None => 8108,
        Some(p) => p
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| CliError(format!("port {p:?} is not a number from 1 to 65535")))?,
    };
    let explicit_protocol = match protocol.or_else(|| var("TYPESENSE_PROTOCOL")) {
        None => None,
        Some(p) => Some(match p.to_ascii_lowercase().as_str() {
            "http" => Protocol::Http,
            "https" => Protocol::Https,
            _ => return Err(CliError(format!("protocol {p:?} must be http or https"))),
        }),
    };
    let mut node = Node {
        protocol: Protocol::Https,
        host: host.trim().to_owned(),
        port,
        path: path.or_else(|| var("TYPESENSE_PATH")).unwrap_or_default(),
    };
    node.protocol = explicit_protocol.unwrap_or(if node.is_local() {
        Protocol::Http
    } else {
        Protocol::Https
    });
    node.validate().map_err(|e| CliError(e.to_string()))?;

    let environment = match env_name {
        None if node.is_local() => Environment::Local,
        None => Environment::Dev,
        Some(e) => match Environment::parse(&e) {
            Environment::Custom(_) => {
                return Err(CliError(format!(
                    "--env {e:?} must be local, dev, staging or prod"
                )));
            }
            known => known,
        },
    };
    let mut profile = Profile::new(node.label(), vec![node]);
    profile.read_only = read_only || environment.is_prod();
    profile.environment = environment;
    Ok(Launch::Temporary {
        profile,
        key_from_stdin,
        key_from_env: var("TYPESENSE_API_KEY").map(SecretString::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn run(args: &[&str], vars: &[(&str, &str)]) -> Result<Launch, CliError> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let env = |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        };
        parse(&args, &env)
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_argument_is_an_error_without_its_text() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;
        let args = [
            OsString::from("--host"),
            OsString::from_vec(b"k\xffsecret".to_vec()),
        ];
        let e = utf8_args(args).unwrap_err().0;
        assert_eq!(e, "argument 2 is not valid UTF-8");
        let ok = utf8_args([OsString::from("--read-only")]).unwrap();
        assert_eq!(ok, ["--read-only"]);
    }

    fn temp(args: &[&str], vars: &[(&str, &str)]) -> (Profile, bool, Option<SecretString>) {
        match run(args, vars) {
            Ok(Launch::Temporary {
                profile,
                key_from_stdin,
                key_from_env,
            }) => (profile, key_from_stdin, key_from_env),
            other => panic!("expected Temporary, got {other:?}"),
        }
    }

    fn err(args: &[&str], vars: &[(&str, &str)]) -> String {
        run(args, vars).expect_err("expected an error").0
    }

    #[test]
    fn no_args_no_env_is_normal() {
        assert!(matches!(run(&[], &[]), Ok(Launch::Normal)));
    }

    #[test]
    fn host_from_env() {
        let (p, stdin, key) = temp(&[], &[("TYPESENSE_HOST", "ts.example.com")]);
        let n = &p.nodes[0];
        assert_eq!((n.protocol, n.port), (Protocol::Https, 8108));
        assert_eq!(p.environment, Environment::Dev);
        assert_eq!(p.name, "ts.example.com:8108");
        assert!(!p.read_only && !stdin && key.is_none());
    }

    #[test]
    fn flag_wins_over_env() {
        let (p, ..) = temp(
            &["--host", "a.io", "--port=9000"],
            &[("TYPESENSE_HOST", "b.io"), ("TYPESENSE_PORT", "8108")],
        );
        assert_eq!((p.nodes[0].host.as_str(), p.nodes[0].port), ("a.io", 9000));
    }

    #[test]
    fn localhost_defaults() {
        let (p, ..) = temp(&["--host", "localhost"], &[]);
        assert_eq!(p.nodes[0].protocol, Protocol::Http);
        assert_eq!(p.environment, Environment::Local);
    }

    #[test]
    fn env_prod_sets_read_only() {
        let (p, ..) = temp(&["--host", "x.io", "--env", "PROD"], &[]);
        assert_eq!(p.environment, Environment::Prod);
        assert!(p.read_only);
    }

    #[test]
    fn read_only_flag_and_explicit_options() {
        let (p, ..) = temp(
            &[
                "--host=x.io",
                "--read-only",
                "--protocol",
                "http",
                "--path",
                "ts",
            ],
            &[],
        );
        assert!(p.read_only);
        assert_eq!(p.nodes[0].protocol, Protocol::Http);
        assert_eq!(p.nodes[0].path, "ts");
    }

    #[test]
    fn profile_flag() {
        assert!(
            matches!(run(&["--profile", "prod-ch"], &[]), Ok(Launch::Profile(n)) if n == "prod-ch")
        );
        assert!(matches!(run(&["--profile=a"], &[]), Ok(Launch::Profile(n)) if n == "a"));
    }

    #[test]
    fn profile_with_connection_flag_is_error() {
        for extra in [
            &["--host", "x"][..],
            &["--port", "1"],
            &["--protocol", "http"],
            &["--path", "p"],
            &["--env", "dev"],
            &["--read-only"],
            &["--api-key-stdin"],
        ] {
            let mut a = vec!["--profile", "p"];
            a.extend_from_slice(extra);
            assert!(err(&a, &[]).contains("--profile"), "{extra:?}");
        }
    }

    #[test]
    fn unknown_flag_is_error() {
        assert!(err(&["--bogus"], &[]).contains("--bogus"));
    }

    #[test]
    fn bad_values_are_errors() {
        assert!(err(&["--host", "x", "--port", "abc"], &[]).contains("port"));
        assert!(err(&["--host", "x", "--port", "0"], &[]).contains("port"));
        assert!(err(&[], &[("TYPESENSE_HOST", "x"), ("TYPESENSE_PORT", "99999")]).contains("port"));
        assert!(err(&["--host", "x", "--protocol", "ftp"], &[]).contains("protocol"));
        assert!(err(&["--host", "x", "--env", "qa"], &[]).contains("--env"));
        assert!(err(&["--host", "https://x/"], &[]).contains("host"));
        assert!(err(&["--host"], &[]).contains("--host"));
    }

    #[test]
    fn unknown_option_error_does_not_echo_value() {
        for a in [
            "--apikey=sk-live-test-1234",
            "--api_key=sk-live-test-1234",
            "sk-live-test-1234=x",
        ] {
            assert!(!err(&[a], &[]).contains("sk-live-test-1234"), "{a}");
        }
    }

    #[test]
    fn missing_or_empty_values_are_errors() {
        assert!(err(&["--host", "--read-only"], &[]).contains("--host needs a value"));
        assert!(err(&["--profile="], &[]).contains("--profile"));
        assert!(err(&["--profile", "  "], &[]).contains("--profile"));
        assert!(err(&["--host="], &[]).contains("--host"));
    }

    #[test]
    fn key_flag_is_rejected() {
        assert!(err(&["--api-key", "x"], &[]).contains("--api-key-stdin"));
        assert!(err(&["--api-key=x"], &[]).contains("--api-key-stdin"));
    }

    #[test]
    fn api_key_env_alone_is_normal() {
        assert!(matches!(
            run(&[], &[("TYPESENSE_API_KEY", "k")]),
            Ok(Launch::Normal)
        ));
    }

    #[test]
    fn connection_flag_without_host_is_error() {
        assert!(err(&["--read-only"], &[]).contains("--host"));
    }

    #[test]
    fn help_anywhere() {
        assert!(matches!(
            run(&["--host", "x", "--help"], &[]),
            Ok(Launch::Help)
        ));
        assert!(matches!(run(&["--bogus", "--help"], &[]), Ok(Launch::Help)));
    }

    #[test]
    fn keys_are_carried_but_not_debug_printed() {
        let (_, stdin, key) = temp(
            &["--host", "x.io", "--api-key-stdin"],
            &[("TYPESENSE_API_KEY", "s3cret-key")],
        );
        assert!(stdin);
        assert_eq!(key.expect("key").expose_secret(), "s3cret-key");
        let l = run(&["--host", "x.io"], &[("TYPESENSE_API_KEY", "s3cret-key")]).expect("ok");
        assert!(!format!("{l:?}").contains("s3cret-key"));
    }

    #[test]
    fn help_text_mentions_every_flag() {
        for f in [
            "--host",
            "--port",
            "--protocol",
            "--path",
            "--api-key-stdin",
            "--env",
            "--read-only",
            "--profile",
            "--help",
        ] {
            assert!(HELP.contains(f), "{f}");
        }
    }
}
