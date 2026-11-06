// SPDX-License-Identifier: GPL-3.0-or-later

//! The export pipeline against a wiremock server, and a raw socket where the answer must
//! break off.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use secrecy::SecretString;
use serde_json::Value;
use tessera_client::{Client, ClientOptions};
use tessera_core::transfer::export::{
    ExportFormat, ExportJob, ExportOutcome, ExportProgress, ExportScope, FieldPick, run_export,
};
use tessera_core::transfer::source::ReadError;
use tessera_core::transfer::{JobControl, JobEnd, TransferError};
use tokio::sync::watch;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EXPORT: &str = "/collections/c/documents/export";

fn client(url: &str) -> Client {
    Client::new(&[url], SecretString::from("k"), ClientOptions::default()).unwrap()
}

fn job(target: &Path, format: ExportFormat) -> ExportJob {
    ExportJob {
        collection: "c".to_string(),
        scope: ExportScope::All,
        fields: FieldPick::All,
        format,
        target: target.to_path_buf(),
        expected: None,
    }
}

async fn serving(body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(EXPORT))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    server
}

async fn run(url: &str, job: ExportJob) -> ExportOutcome {
    let (tx, _rx) = watch::channel(ExportProgress::default());
    run_export(client(url), job, JobControl::default(), tx).await
}

fn target() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out");
    (dir, path)
}

fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

async fn finished(url: &str, job: ExportJob) -> ExportOutcome {
    let outcome = run(url, job).await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    outcome
}

/// Starts a chunked answer with `lines` on a raw socket. With `hold` the connection stays
/// open until the returned sender goes; without it, it closes, cutting the answer off.
fn partial_answer(lines: String, hold: bool) -> (String, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        write!(
            socket,
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{lines}\r\n",
            lines.len()
        )
        .unwrap();
        socket.flush().unwrap();
        if hold {
            // Ends when the test drops the sender.
            let _ = released.recv();
        }
    });
    (url, release)
}

#[tokio::test]
async fn jsonl_is_written_as_it_came() {
    let (_dir, target) = target();
    let body = "{\"id\":\"1\", \"z\":1}\n{\"id\":\"2\"}";
    let server = serving(body).await;
    let outcome = finished(&server.uri(), job(&target, ExportFormat::Jsonl)).await;
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        format!("{body}\n")
    );
    assert_eq!(outcome.progress.docs, 2);
    assert_eq!(outcome.progress.bytes, body.len() as u64 + 1);
}

#[tokio::test]
async fn json_array_parses() {
    for (body, count) in [("", 0), ("{\"a\":1}", 1), ("{\"a\":1}\n{\"a\":2}", 2)] {
        let (_dir, target) = target();
        let server = serving(body).await;
        finished(&server.uri(), job(&target, ExportFormat::JsonArray)).await;
        let bytes = std::fs::read(&target).unwrap();
        let parsed: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.len(), count);
    }
}

#[tokio::test]
async fn csv_flattens_and_quotes() {
    let (_dir, target) = target();
    let server = serving("{\"id\":\"1\",\"a\":{\"b\":\"x,y\"},\"t\":[\"p\"]}").await;
    finished(&server.uri(), job(&target, ExportFormat::Csv)).await;
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "id,a.b,t\n1,\"x,y\",\"[\"\"p\"\"]\"\n"
    );
}

#[tokio::test]
async fn csv_puts_id_first_and_writes_nothing_for_nothing() {
    let (_dir, target) = target();
    let server = serving("{\"a\":1,\"id\":\"x\"}").await;
    finished(&server.uri(), job(&target, ExportFormat::Csv)).await;
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "id,a\nx,1\n");

    let (_dir, target) = self::target();
    let server = serving("").await;
    finished(&server.uri(), job(&target, ExportFormat::Csv)).await;
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
}

#[tokio::test]
async fn an_empty_result_without_a_target_keeps_nothing() {
    let (_dir, target) = target();
    let server = serving("").await;
    assert!(
        !finished(&server.uri(), job(&target, ExportFormat::Jsonl))
            .await
            .kept_existing
    );
}

#[tokio::test]
async fn csv_header_comes_from_the_first_1000() {
    let (_dir, target) = target();
    let mut body: String = (0..1000).map(|i| format!("{{\"id\":\"{i}\"}}\n")).collect();
    body.push_str("{\"id\":\"late\",\"z\":1}\n{\"id\":\"last\"}");
    let server = serving(&body).await;
    let outcome = finished(&server.uri(), job(&target, ExportFormat::Csv)).await;
    assert_eq!(outcome.unlisted_values, 1);
    let text = std::fs::read_to_string(&target).unwrap();
    assert_eq!(text.lines().count(), 1003);
    assert!(text.starts_with("id\n0\n"));
    assert!(text.ends_with("late\nlast\n"));
}

#[tokio::test]
async fn csv_counts_a_key_that_flattens_onto_another() {
    let (_dir, target) = target();
    let server = serving("{\"id\":\"1\",\"a.b\":1,\"a\":{\"b\":2}}").await;
    let outcome = finished(&server.uri(), job(&target, ExportFormat::Csv)).await;
    assert_eq!(outcome.unlisted_values, 1);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "id,a.b\n1,1\n");
}

#[tokio::test]
async fn csv_cells_stay_verbatim() {
    let (_dir, target) = target();
    let server = serving("{\"id\":\"1\",\"f\":\"=1+1\"}").await;
    finished(&server.uri(), job(&target, ExportFormat::Csv)).await;
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "id,f\n1,=1+1\n");
}

#[tokio::test]
async fn a_line_that_is_no_object_fails_the_csv_with_a_capped_text() {
    let (dir, target) = target();
    let long = format!("not json {}", "é".repeat(5000));
    let server = serving(&format!("{{\"id\":\"1\"}}\n{long}")).await;
    let outcome = run(&server.uri(), job(&target, ExportFormat::Csv)).await;
    let JobEnd::Failed(TransferError::Read(ReadError::Json { line, message, .. })) = &outcome.end
    else {
        panic!("{:?}", outcome.end);
    };
    assert_eq!(*line, 2);
    assert!(message.contains("not json"), "{message}");
    assert!(message.len() < 3000, "{}", message.len());
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn include_and_exclude_reach_the_query() {
    let (_dir, target) = target();
    let server = MockServer::start().await;
    Mock::given(query_param("include_fields", "a,b.c"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(query_param("exclude_fields", "z"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut include = job(&target, ExportFormat::Jsonl);
    include.fields = FieldPick::Include(vec!["a".to_string(), "b.c".to_string()]);
    finished(&server.uri(), include).await;
    let mut exclude = job(&target, ExportFormat::Jsonl);
    exclude.fields = FieldPick::Exclude(vec!["z".to_string()]);
    finished(&server.uri(), exclude).await;
}

#[tokio::test]
async fn filter_reaches_the_query() {
    let (_dir, target) = target();
    let server = MockServer::start().await;
    Mock::given(query_param("filter_by", "n:>1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut filtered = job(&target, ExportFormat::Jsonl);
    filtered.scope = ExportScope::Filter("n:>1".to_string());
    finished(&server.uri(), filtered).await;
}

#[tokio::test]
async fn unsafe_field_is_refused_and_sends_nothing() {
    let (dir, target) = target();
    let server = MockServer::start().await;
    let mut unsafe_job = job(&target, ExportFormat::Jsonl);
    unsafe_job.fields = FieldPick::Exclude(vec!["a,b".to_string()]);
    let outcome = run(&server.uri(), unsafe_job).await;
    assert!(
        matches!(&outcome.end, JobEnd::Failed(TransferError::UnsafeField(name)) if name == "a,b"),
        "{:?}",
        outcome.end
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn an_include_of_nothing_is_refused() {
    let (dir, target) = target();
    let server = MockServer::start().await;
    let mut empty = job(&target, ExportFormat::Jsonl);
    empty.fields = FieldPick::Include(Vec::new());
    let outcome = run(&server.uri(), empty).await;
    assert!(
        matches!(outcome.end, JobEnd::Failed(TransferError::UnsafeField(_))),
        "{:?}",
        outcome.end
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn selection_runs_one_stream_per_filter_and_single_for_backticks() {
    let (_dir, target) = target();
    let server = MockServer::start().await;
    Mock::given(query_param("filter_by", "id:[`a`]"))
        .and(path(EXPORT))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"id\":\"a\",\"t\":1}"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/collections/c/documents/b.*c$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("{\"id\":\"b`c\",\"t\":2,\"x\":3}"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/collections/c/documents/g.*e$"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{\"message\":\"gone\"}"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/collections/c"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"name\":\"c\"}"))
        .expect(1)
        .mount(&server)
        .await;
    let mut selected = job(&target, ExportFormat::Jsonl);
    selected.scope =
        ExportScope::Selection(vec!["a".to_string(), "b`c".to_string(), "g`e".to_string()]);
    selected.fields = FieldPick::Include(vec!["t".to_string()]);
    let outcome = finished(&server.uri(), selected).await;
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "{\"id\":\"a\",\"t\":1}\n{\"t\":2}\n"
    );
    assert_eq!(outcome.progress.docs, 2);
    assert_eq!(outcome.progress.expected, Some(3), "the gone id shows here");
}

fn single(target: &Path) -> ExportJob {
    let mut selected = job(target, ExportFormat::Jsonl);
    selected.scope = ExportScope::Selection(vec!["b`c".to_string()]);
    selected
}

#[tokio::test]
async fn a_gone_collection_is_no_skipped_document() {
    let (_dir, target) = target();
    std::fs::write(&target, "old").unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{\"message\":\"Not Found\"}"))
        .mount(&server)
        .await;
    let outcome = run(&server.uri(), single(&target)).await;
    assert!(
        matches!(
            outcome.end,
            JobEnd::Failed(TransferError::Client(tessera_client::Error::Http {
                status: 404,
                ..
            }))
        ),
        "{:?}",
        outcome.end
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
}

#[tokio::test]
async fn stop_does_not_wait_for_a_single_document() {
    let (dir, target) = target();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let control = JobControl::default();
    let (tx, _rx) = watch::channel(ExportProgress::default());
    let running = tokio::spawn(run_export(
        client(&server.uri()),
        single(&target),
        control.clone(),
        tx,
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    control.stop.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn an_empty_result_keeps_the_target() {
    for format in [
        ExportFormat::Jsonl,
        ExportFormat::JsonArray,
        ExportFormat::Csv,
    ] {
        let (dir, target) = target();
        std::fs::write(&target, "old").unwrap();
        let server = serving("").await;
        let outcome = finished(&server.uri(), job(&target, format)).await;
        assert_eq!(outcome.progress.docs, 0);
        assert!(outcome.kept_existing, "{format:?}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "old",
            "{format:?}"
        );
        assert_eq!(names(dir.path()), ["out"], "{format:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn export_keeps_the_permissions_of_the_target() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_dir, target) = target();
    std::fs::write(&target, "old").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let server = serving("{\"id\":\"1\"}").await;
    finished(&server.uri(), job(&target, ExportFormat::Jsonl)).await;
    let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[tokio::test]
async fn export_refuses_a_symbolic_link_before_any_request() {
    let (dir, target) = target();
    let real = dir.path().join("real");
    std::fs::write(&real, "old").unwrap();
    std::os::unix::fs::symlink(&real, &target).unwrap();
    let server = MockServer::start().await;
    let outcome = run(&server.uri(), job(&target, ExportFormat::Jsonl)).await;
    assert!(
        matches!(outcome.end, JobEnd::Failed(TransferError::Io(_))),
        "{:?}",
        outcome.end
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(std::fs::symlink_metadata(&target).unwrap().is_symlink());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "old");
}

#[tokio::test]
async fn stop_removes_the_part_and_keeps_the_target() {
    let (dir, target) = target();
    std::fs::write(&target, "old").unwrap();
    let lines: String = (0..300).map(|i| format!("{{\"id\":\"{i}\"}}\n")).collect();
    let (url, _hold) = partial_answer(lines, true);
    let control = JobControl::default();
    let (tx, mut rx) = watch::channel(ExportProgress::default());
    let running = tokio::spawn(run_export(
        client(&url),
        job(&target, ExportFormat::Jsonl),
        control.clone(),
        tx,
    ));
    rx.wait_for(|p| p.docs >= 256).await.unwrap();
    control.stop.cancel();
    let outcome = running.await.unwrap();
    assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    assert_eq!(names(dir.path()), ["out"]);
}

#[tokio::test]
async fn failure_midway_removes_the_part_and_keeps_the_target() {
    let (dir, target) = target();
    std::fs::write(&target, "old").unwrap();
    let (url, _hold) = partial_answer("{\"id\":\"1\"}\n".to_string(), false);
    let outcome = run(&url, job(&target, ExportFormat::Jsonl)).await;
    assert!(
        matches!(outcome.end, JobEnd::Failed(TransferError::Client(_))),
        "{:?}",
        outcome.end
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    assert_eq!(names(dir.path()), ["out"]);
}

#[tokio::test]
async fn success_replaces_the_target() {
    let (dir, target) = target();
    std::fs::write(&target, "old").unwrap();
    let server = serving("{\"id\":\"1\"}").await;
    finished(&server.uri(), job(&target, ExportFormat::Jsonl)).await;
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "{\"id\":\"1\"}\n"
    );
    assert_eq!(names(dir.path()), ["out"]);
}

#[tokio::test]
async fn missing_directory_fails_before_any_request() {
    let (dir, _) = target();
    let server = MockServer::start().await;
    let target = dir.path().join("gone").join("out");
    let outcome = run(&server.uri(), job(&target, ExportFormat::Jsonl)).await;
    assert!(
        matches!(outcome.end, JobEnd::Failed(TransferError::Io(_))),
        "{:?}",
        outcome.end
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn expected_mismatch_is_reported() {
    let (_dir, target) = target();
    let server = serving("{\"id\":\"1\"}").await;
    let mut counted = job(&target, ExportFormat::Jsonl);
    counted.expected = Some(5);
    let outcome = finished(&server.uri(), counted).await;
    assert_eq!(outcome.progress.docs, 1);
    assert_eq!(outcome.progress.expected, Some(5));
}
