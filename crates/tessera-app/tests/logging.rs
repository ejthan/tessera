// SPDX-License-Identifier: GPL-3.0-or-later

//! Log files: masked in both layers, proved on real logs written by a real session.

use std::path::Path;
use std::process::Command;

use secrecy::SecretString;
use serde_json::json;
use tessera_app::logging;
use tessera_client::{Client, ClientOptions, Method, RawBody, RawRequest};
use tracing_subscriber::EnvFilter;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "SESSION-KEY-7f3a";
const PROVIDER_KEY: &str = "PROVIDER-KEY-91c2";
const BODY_MARK: &str = "BODY-MARK-44d0";
/// Set for the child of `a_real_session_logs_no_key_and_no_body`.
const CHILD_DIR: &str = "TESSERA_TEST_LOG_DIR";

fn log_files(dir: &Path) -> Vec<(String, String)> {
    std::fs::read_dir(dir)
        .expect("log dir")
        .map(|entry| {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(entry.path()).expect("read"))
        })
        .collect()
}

/// Runs `events` under a local subscriber and returns the file it wrote, once flushed.
fn logged(events: impl FnOnce()) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let (writer, guard) = logging::file_writer(dir.path()).expect("writer");
    let dispatch = logging::dispatch(Some(writer), EnvFilter::new("debug"));
    tracing::dispatcher::with_default(&dispatch, events);
    drop(dispatch);
    drop(guard);
    let files = log_files(dir.path());
    assert_eq!(files.len(), 1, "{files:?}");
    files.into_iter().next().expect("one file").1
}

#[test]
fn a_file_has_the_prefix_and_no_secret() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (writer, guard) = logging::file_writer(dir.path()).expect("writer");
    let dispatch = logging::dispatch(Some(writer), EnvFilter::new("info"));
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("GET http://h/x?x-typesense-api-key=abc&a=1 failed");
        tracing::info!(r#"body {{"api_key":"abc","n":1}}"#);
        tracing::info!("X-Typesense-Api-Key: abc");
        tracing::info!(api_key = "abc", "a field");
        tracing::info!(body = ?r#"{"api_key":"abc"}"#, "a debug field");
    });
    drop(guard);
    let files = log_files(dir.path());
    assert_eq!(files.len(), 1);
    let (name, text) = &files[0];
    assert!(name.starts_with("tessera.log"), "{name}");
    assert!(!text.contains("abc"), "{text}");
    assert_eq!(text.matches("a=1").count(), 1, "{text}");
    assert!(!text.contains('\x1b'), "no colour in a file: {text}");
}

#[test]
fn newlines_stay() {
    let text = logged(|| tracing::info!("one\ntwo"));
    assert!(text.contains("one\ntwo\n"), "{text}");
    assert_eq!(text.lines().count(), 2);
}

#[test]
fn the_debug_line_of_send_never_carries_a_key() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let server = runtime.block_on(MockServer::start());
    runtime.block_on(
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server),
    );
    let client = Client::new(
        &[&server.uri()],
        SecretString::from(KEY),
        ClientOptions::default(),
    )
    .expect("client");
    let text = logged(|| {
        runtime.block_on(async {
            client.request(Method::GET, "/health").await.expect("send");
        });
    });
    let line = text
        .lines()
        .find(|l| l.contains("typesense request"))
        .unwrap_or_else(|| panic!("no debug line in {text}"));
    assert!(line.contains("path=/health"), "{line}");
    assert!(!text.contains(KEY), "{text}");
}

#[test]
fn a_directory_that_cannot_be_made_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("file");
    std::fs::write(&file, "").expect("write");
    assert!(logging::file_writer(&file.join("logs")).is_err());
}

/// One session of the real `init`, with everything on. Runs only as a child, because `init`
/// takes the process and `RUST_LOG`.
#[test]
fn child_session() {
    let Ok(dir) = std::env::var(CHILD_DIR) else {
        return;
    };
    let guard = logging::init(Some(Path::new(&dir)));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "c"})))
            .mount(&server)
            .await;
        let options = ClientOptions::default;
        let key = || SecretString::from(KEY);
        let client = Client::new(&[&server.uri()], key(), options()).expect("client");
        let schema = json!({"name": "c", "note": BODY_MARK, "fields": [{
            "name": "e", "type": "float[]",
            "embed": {"from": ["t"], "model_config": {"api_key": PROVIDER_KEY}},
        }]});
        let _ = client.create_collection(&schema).await;
        let _ = client
            .raw(RawRequest {
                method: Method::POST,
                path: "/multi_search".into(),
                query: Some(format!("x-typesense-api-key={KEY}&q=a")),
                body: Some(RawBody::Text(BODY_MARK.into())),
            })
            .await;
        // A refused connection takes the error path.
        let gone = Client::new(&["http://127.0.0.1:9"], key(), options()).expect("client");
        let _ = gone.request(Method::GET, "/health").await;
    });
    tracing::info!("session over");
    drop(guard);
}

#[test]
fn a_failed_directory_warns_and_the_session_goes_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("file");
    std::fs::write(&file, "").expect("write");
    let output = Command::new(std::env::current_exe().expect("exe"))
        .args(["--exact", "child_session", "--nocapture"])
        .env(CHILD_DIR, file.join("logs"))
        .env("RUST_LOG", "info")
        .output()
        .expect("child");
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("logging to stderr only"), "{stderr}");
    assert!(stderr.contains("session over"), "{stderr}");
}

#[test]
fn a_real_session_logs_no_key_and_no_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(std::env::current_exe().expect("exe"))
        .args(["--exact", "child_session", "--nocapture"])
        .env(CHILD_DIR, dir.path())
        .env("RUST_LOG", "trace")
        .output()
        .expect("child");
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let files = log_files(dir.path());
    assert_eq!(files.len(), 1, "{files:?}");
    let (name, file) = &files[0];
    assert!(name.starts_with("tessera.log"), "{name}");
    assert!(file.contains("typesense request"), "not a session: {file}");
    assert!(file.contains("session over"), "not flushed: {file}");
    assert!(stderr.contains("typesense request"), "{stderr}");
    for (layer, text) in [("file", file), ("stderr", &stderr)] {
        for secret in [KEY, PROVIDER_KEY, BODY_MARK] {
            let hit = text.lines().find(|l| l.contains(secret));
            assert!(hit.is_none(), "{layer} holds {secret}: {hit:?}");
        }
    }
}
