// SPDX-License-Identifier: GPL-3.0-or-later

//! The import pipeline against a wiremock server.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use tessera_client::{Client, ClientOptions, DirtyValues, Error, ImportAction, ImportParams};
use tessera_core::transfer::import::{ImportJob, ImportOutcome, ImportProgress, run_import};
use tessera_core::transfer::source::{Format, ReadOptions, Source};
use tessera_core::transfer::{JobControl, JobEnd, TransferError};
use tokio::sync::watch;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const IMPORT: &str = "/collections/c/documents/import";

fn client(url: &str) -> Client {
    Client::new(&[url], SecretString::from("k"), ClientOptions::default()).unwrap()
}

fn jsonl(n: usize) -> Vec<u8> {
    (1..=n)
        .map(|i| format!("{{\"id\":\"{i}\"}}\n"))
        .collect::<String>()
        .into_bytes()
}

fn job(bytes: Vec<u8>, format: Format, batch_size: u32) -> ImportJob {
    ImportJob {
        source: Source::Bytes(bytes.into()),
        options: ReadOptions::new(format),
        collection: "c".to_string(),
        params: ImportParams {
            action: ImportAction::Create,
            batch_size,
            dirty_values: DirtyValues::CoerceOrReject,
        },
    }
}

fn answers(request: &Request) -> String {
    let count = String::from_utf8_lossy(&request.body).lines().count();
    "{\"success\":true}\n".repeat(count)
}

fn body_of(request: &Request) -> String {
    String::from_utf8(request.body.clone()).unwrap()
}

/// What the server was sent.
#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<String>>>);

impl Sent {
    fn push(&self, request: &Request) -> usize {
        let mut bodies = self.0.lock().unwrap();
        bodies.push(body_of(request));
        bodies.len()
    }

    fn sizes(&self) -> Vec<usize> {
        let bodies = self.0.lock().unwrap();
        bodies.iter().map(|b| b.lines().count()).collect()
    }

    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

/// A server that stores all it gets, each answer after `delay`.
async fn server(delay: Duration) -> (MockServer, Sent) {
    let server = MockServer::start().await;
    let sent = Sent::default();
    let record = sent.clone();
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            record.push(request);
            ResponseTemplate::new(200)
                .set_body_string(answers(request))
                .set_delay(delay)
        })
        .mount(&server)
        .await;
    (server, sent)
}

fn run(
    url: &str,
    job: ImportJob,
    control: JobControl,
) -> impl Future<Output = ImportOutcome> + use<> {
    let (tx, _rx) = watch::channel(ImportProgress::default());
    run_import(client(url), job, control, tx)
}

async fn until_requests(sent: &Sent, count: usize) {
    while sent.count() < count {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn sends_one_post_per_batch() {
    let (server, sent) = server(Duration::ZERO).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(2500), Format::Jsonl, 1000),
        JobControl::default(),
    )
    .await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    assert_eq!(sent.sizes(), [1000, 1000, 500]);
    assert_eq!(outcome.stopped_at_line, None);
    for request in server.received_requests().await.unwrap() {
        if request.url.path() == IMPORT {
            assert!(request.url.query().unwrap().contains("batch_size=1000"));
        }
    }
}

#[tokio::test]
async fn batch_stops_at_batch_bytes() {
    let (server, sent) = server(Duration::ZERO).await;
    let payload = "x".repeat(6 << 20);
    let bytes: String = (0..7)
        .map(|i| format!("{{\"id\":\"{i}\",\"p\":\"{payload}\"}}\n"))
        .collect();
    let outcome = run(
        &server.uri(),
        job(bytes.into_bytes(), Format::Jsonl, 1000),
        JobControl::default(),
    )
    .await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    assert_eq!(sent.sizes(), [3, 3, 1]);
}

/// A server that refuses the document at `position` of every batch, echoing `id`.
async fn refusing(position: usize, id: Option<&'static str>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            let text: String = body_of(request)
                .lines()
                .enumerate()
                .map(|(i, _)| {
                    if i == position {
                        let id = id.map_or(String::new(), |id| format!(",\"id\":\"{id}\""));
                        format!("{{\"success\":false,\"code\":400,\"error\":\"no\"{id}}}\n")
                    } else {
                        "{\"success\":true}\n".to_string()
                    }
                })
                .collect();
            ResponseTemplate::new(200).set_body_string(text)
        })
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn report_lines_survive_blank_lines() {
    let server = refusing(1, None).await;
    let bytes = b"{\"id\":\"a\"}\n\n{\"id\":\"b\"}\n{\"id\":\"c\"}\n".to_vec();
    let outcome = run(
        &server.uri(),
        job(bytes, Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    let rows = outcome.report.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].line, 3);
    assert_eq!(rows[0].raw, "{\"id\":\"b\"}");
    assert_eq!(rows[0].id.as_deref(), Some("b"));
    assert_eq!(rows[0].code, Some(400));
}

#[tokio::test]
async fn a_bad_json_line_does_not_inherit_the_echoed_id() {
    // Measured on 30.2: a line the server cannot parse is answered with the id before it.
    let server = refusing(1, Some("a")).await;
    let bytes = b"{\"id\":\"a\"}\nnot json\n".to_vec();
    let outcome = run(
        &server.uri(),
        job(bytes, Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    let row = &outcome.report.rows()[0];
    assert_eq!((row.line, row.raw.as_str()), (2, "not json"));
    assert_eq!(row.id, None);
}

#[tokio::test]
async fn rejected_line_is_reported_and_never_sent() {
    let (server, sent) = server(Duration::ZERO).await;
    let bytes = b"{\"id\":\"a\"}\n{\"id\":\"bad\xff\"}\n{\"id\":\"c\"}\n".to_vec();
    let outcome = run(
        &server.uri(),
        job(bytes, Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    assert_eq!(sent.sizes(), [2]);
    assert!(!sent.0.lock().unwrap()[0].contains("bad"));
    let rows = outcome.report.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].line, rows[0].code), (2, None));
    assert!(rows[0].error.contains("UTF-8"), "{}", rows[0].error);
    assert_eq!(outcome.progress.failed, 1);
    assert_eq!(outcome.progress.imported, 2);
    assert_eq!(outcome.progress.sent, 2);
}

#[tokio::test]
async fn only_rejected_batch_sends_nothing() {
    let (server, sent) = server(Duration::ZERO).await;
    let bytes = b"{\"id\":\"\xff\"}\n{\"id\":\"\xfe\"}\n".to_vec();
    let outcome = run(
        &server.uri(),
        job(bytes, Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    assert_eq!(sent.count(), 0);
    assert_eq!(outcome.report.total(), 2);
}

#[tokio::test]
async fn counts_add_up() {
    let server = refusing(3, None).await;
    let mut bytes = jsonl(25);
    bytes.extend_from_slice(b"{\"id\":\"\xff\"}\n");
    let outcome = run(
        &server.uri(),
        job(bytes, Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    let p = &outcome.progress;
    assert_eq!(p.imported + p.failed + p.unconfirmed, 26);
    assert_eq!(p.failed, 4);
    assert_eq!(p.sent, 25);
    assert_eq!(outcome.report.total(), 4);
    assert_eq!(p.fraction(), Some(1.0));
    let lines: Vec<u64> = outcome.report.rows().iter().map(|r| r.line).collect();
    assert_eq!(lines, [4, 14, 24, 26]);
}

#[tokio::test]
async fn first_error_is_published_while_running() {
    let server = MockServer::start().await;
    let hold = Arc::new(AtomicUsize::new(0));
    let calls = hold.clone();
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            let lines = body_of(request).lines().count();
            ResponseTemplate::new(200)
                .set_body_string(
                    "{\"success\":false,\"code\":400,\"error\":\"wrong\"}\n".repeat(lines),
                )
                .set_delay(Duration::from_millis(if first { 0 } else { 2000 }))
        })
        .mount(&server)
        .await;
    let (tx, mut rx) = watch::channel(ImportProgress::default());
    let control = JobControl::default();
    let run = tokio::spawn(run_import(
        client(&server.uri()),
        job(jsonl(20), Format::Jsonl, 10),
        control.clone(),
        tx,
    ));
    let seen = rx.wait_for(|p| p.first_error.is_some()).await.unwrap();
    assert_eq!(seen.first_error.as_deref(), Some("wrong"));
    drop(seen);
    assert!(!run.is_finished(), "the job is still running");
    control.stop.cancel();
    control.abort.cancel();
    run.await.unwrap();
}

#[tokio::test]
async fn stop_starts_no_new_post() {
    let (server, sent) = server(Duration::from_millis(200)).await;
    let control = JobControl::default();
    let (tx, mut rx) = watch::channel(ImportProgress::default());
    let running = tokio::spawn(run_import(
        client(&server.uri()),
        job(jsonl(3000), Format::Jsonl, 1000),
        control.clone(),
        tx,
    ));
    until_requests(&sent, 1).await;
    control.stop.cancel();
    rx.wait_for(|p| p.phase == tessera_core::transfer::Phase::Stopping)
        .await
        .unwrap();
    let outcome = running.await.unwrap();
    assert_eq!(sent.count(), 1);
    assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
    assert_eq!(
        outcome.progress.imported, 1000,
        "the batch in flight counts"
    );
    assert_eq!(outcome.stopped_at_line, Some(1001));
}

#[tokio::test]
async fn stop_before_start_sends_nothing() {
    let (server, sent) = server(Duration::ZERO).await;
    let control = JobControl::default();
    control.stop.cancel();
    let outcome = run(&server.uri(), job(jsonl(10), Format::Jsonl, 5), control).await;
    assert_eq!(sent.count(), 0);
    assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
    assert_eq!(outcome.stopped_at_line, Some(1));
}

#[tokio::test]
async fn abort_counts_unconfirmed() {
    let (server, sent) = server(Duration::from_secs(10)).await;
    let control = JobControl::default();
    let running = tokio::spawn(run(
        &server.uri(),
        job(jsonl(10), Format::Jsonl, 4),
        control.clone(),
    ));
    until_requests(&sent, 1).await;
    control.stop.cancel();
    control.abort.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
    assert_eq!(outcome.progress.unconfirmed, 4);
    assert_eq!(outcome.progress.sent, 4);
    assert_eq!(outcome.report.total(), 4);
    assert_eq!(outcome.stopped_at_line, Some(1));
}

/// Answers `lines(sent)` lines of success to each batch.
async fn answering(lines: fn(usize) -> usize) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            let count = lines(body_of(request).lines().count());
            ResponseTemplate::new(200).set_body_string("{\"success\":true}\n".repeat(count))
        })
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn fewer_answers_are_unconfirmed() {
    let server = answering(|sent| sent - 2).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(5), Format::Jsonl, 5),
        JobControl::default(),
    )
    .await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    assert_eq!(outcome.progress.imported, 3);
    assert_eq!(outcome.progress.unconfirmed, 2);
    let lines: Vec<u64> = outcome.report.rows().iter().map(|r| r.line).collect();
    assert_eq!(lines, [4, 5]);
}

#[tokio::test]
async fn more_answers_fail_the_job() {
    let server = answering(|sent| sent + 1).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(6), Format::Jsonl, 3),
        JobControl::default(),
    )
    .await;
    assert!(
        matches!(
            outcome.end,
            JobEnd::Failed(TransferError::AnswerMismatch {
                sent: 3,
                answered: 4
            })
        ),
        "{:?}",
        outcome.end
    );
    assert_eq!(outcome.progress.imported, 0);
    assert_eq!(outcome.progress.unconfirmed, 3);
    assert_eq!(outcome.stopped_at_line, Some(1));
}

/// Answers the first `good` batches with success, then what `then` says.
async fn turning(good: usize, then: ResponseTemplate) -> (MockServer, Sent) {
    let server = MockServer::start().await;
    let sent = Sent::default();
    let record = sent.clone();
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            if record.push(request) <= good {
                ResponseTemplate::new(200).set_body_string(answers(request))
            } else {
                then.clone()
            }
        })
        .mount(&server)
        .await;
    (server, sent)
}

#[tokio::test]
async fn collection_gone_fails_with_the_message() {
    let gone = ResponseTemplate::new(404).set_body_string("{\"message\":\"Not Found\"}");
    let (server, _) = turning(1, gone).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(10), Format::Jsonl, 5),
        JobControl::default(),
    )
    .await;
    let JobEnd::Failed(TransferError::Client(Error::Http {
        status: 404,
        message,
        ..
    })) = &outcome.end
    else {
        panic!("{:?}", outcome.end);
    };
    assert_eq!(message, "Not Found");
    assert_eq!(outcome.progress.imported, 5);
    assert_eq!(outcome.progress.unconfirmed, 0);
    assert_eq!(outcome.stopped_at_line, Some(6));
}

#[tokio::test]
async fn read_only_fails_the_job() {
    let server = MockServer::start().await;
    let lock = client(&server.uri());
    let locked = lock.clone();
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            locked.set_read_only(true);
            ResponseTemplate::new(200).set_body_string(answers(request))
        })
        .mount(&server)
        .await;
    let (tx, _rx) = watch::channel(ImportProgress::default());
    let outcome = run_import(
        lock,
        job(jsonl(10), Format::Jsonl, 5),
        JobControl::default(),
        tx,
    )
    .await;
    assert!(
        matches!(
            outcome.end,
            JobEnd::Failed(TransferError::Client(Error::ReadOnly { .. }))
        ),
        "{:?}",
        outcome.end
    );
    assert_eq!(outcome.progress.imported, 5);
    assert_eq!(outcome.progress.sent, 5, "the refused batch never left");
}

/// Answers 503 to the first `busy` requests.
async fn busy(busy: usize) -> (MockServer, Sent) {
    let server = MockServer::start().await;
    let sent = Sent::default();
    let record = sent.clone();
    Mock::given(method("POST"))
        .and(path(IMPORT))
        .respond_with(move |request: &Request| {
            if record.push(request) <= busy {
                ResponseTemplate::new(503).set_body_string("{\"message\":\"Not Ready or Lagging\"}")
            } else {
                ResponseTemplate::new(200).set_body_string(answers(request))
            }
        })
        .mount(&server)
        .await;
    (server, sent)
}

// Real waits of 1 s and 2 s: a paused tokio clock would fire the client's timeout too.
#[tokio::test]
async fn busy_server_is_retried() {
    let (server, sent) = busy(2).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(4), Format::Jsonl, 4),
        JobControl::default(),
    )
    .await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    assert_eq!(sent.sizes(), [4, 4, 4], "the same body again");
    assert_eq!(outcome.progress.imported, 4);
    assert_eq!(outcome.progress.sent, 4);
}

#[tokio::test]
async fn stop_during_backoff_ends_stopped() {
    let (server, sent) = busy(100).await;
    let control = JobControl::default();
    let running = tokio::spawn(run(
        &server.uri(),
        job(jsonl(4), Format::Jsonl, 4),
        control.clone(),
    ));
    until_requests(&sent, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    control.stop.cancel();
    let outcome = tokio::time::timeout(Duration::from_millis(500), running)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome.end, JobEnd::Stopped), "{:?}", outcome.end);
    assert_eq!(sent.count(), 1);
    assert_eq!(outcome.progress.sent, 0);
    assert_eq!(outcome.progress.unconfirmed, 0);
    assert_eq!(outcome.stopped_at_line, Some(1));
}

#[tokio::test]
async fn broken_json_array_stops_after_the_good_batches() {
    let (server, sent) = server(Duration::ZERO).await;
    let bytes = b"[{\"id\":\"1\"},{\"id\":\"2\"},{\"id\":\"3\"},{oops".to_vec();
    let outcome = run(
        &server.uri(),
        job(bytes, Format::JsonArray, 2),
        JobControl::default(),
    )
    .await;
    assert_eq!(sent.sizes(), [2, 1]);
    assert!(
        matches!(outcome.end, JobEnd::Failed(TransferError::Read(_))),
        "{:?}",
        outcome.end
    );
    assert_eq!(outcome.progress.imported, 3);
}

/// The reader is fed through a pipe, so what the writer got rid of is what was read.
#[cfg(unix)]
#[tokio::test]
async fn backpressure_keeps_the_reader_close() {
    use std::io::Write as _;

    const DOC_BYTES: usize = 1024;
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("source.jsonl");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let written = Arc::new(AtomicUsize::new(0));
    let counter = written.clone();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let mut pipe = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let line = format!("{{\"id\":\"{}\"}}\n", "x".repeat(DOC_BYTES - 10));
        // Ends with a broken pipe once the job drops the reader.
        while pipe.write_all(line.as_bytes()).is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let (server, sent) = server(Duration::from_secs(10)).await;
    let mut import = job(Vec::new(), Format::Jsonl, 1000);
    import.source = Source::File(fifo);
    let control = JobControl::default();
    let (tx, rx) = watch::channel(ImportProgress::default());
    let running = tokio::spawn(run_import(
        client(&server.uri()),
        import,
        control.clone(),
        tx,
    ));
    until_requests(&sent, 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Three batches, plus what sits in the pipe and the read buffer (at most 128 KiB).
    let slack = 128 * 1024 / DOC_BYTES + 1;
    let read = written.load(Ordering::SeqCst);
    assert!(read >= 3000, "the reader keeps up: {read}");
    assert!(read <= 3000 + slack, "read ahead too far: {read}");
    assert_eq!(rx.borrow().bytes_done, 0);
    control.stop.cancel();
    control.abort.cancel();
    running.await.unwrap();
}

async fn collection(server: &MockServer, documents: u64) {
    let body = format!(
        "{{\"name\":\"c\",\"fields\":[],\"num_documents\":{documents},\"created_at\":0,\
         \"default_sorting_field\":\"\"}}"
    );
    Mock::given(method("GET"))
        .and(path("/collections/c"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn count_check_flags_a_lazy_server() {
    let (server, _) = server(Duration::ZERO).await;
    collection(&server, 7).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(3), Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    assert!(matches!(outcome.end, JobEnd::Finished), "{:?}", outcome.end);
    let count = outcome.count.unwrap();
    assert_eq!((count.before, count.after, count.ok), (7, 7, false));
}

#[tokio::test]
async fn an_unreadable_count_is_no_check() {
    let (server, _) = server(Duration::ZERO).await;
    let outcome = run(
        &server.uri(),
        job(jsonl(3), Format::Jsonl, 10),
        JobControl::default(),
    )
    .await;
    assert!(outcome.count.is_none());
}
