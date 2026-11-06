// SPDX-License-Identifier: GPL-3.0-or-later

//! Sends a source to the server batch by batch and accounts for every document.

use std::mem;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tessera_client::{Client, Error, ImportAction, ImportLine, ImportParams};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::report::{Report, ReportRow};
use super::source::{ReadError, ReadOptions, Record, RejectReason, Source, read_documents};
use super::{BATCH_BYTES, BUSY_RETRIES, CHANNEL_BATCHES};

/// How a running job is told to end. Clones share the tokens.
#[derive(Clone, Default)]
pub struct JobControl {
    /// Start nothing new, wait for what is in flight.
    pub stop: CancellationToken,
    /// Stop waiting for what is in flight.
    pub abort: CancellationToken,
}

/// Why a transfer failed.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The server or the connection.
    #[error(transparent)]
    Client(#[from] tessera_client::Error),
    /// The source.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The report spool.
    #[error("cannot write the report: {0}")]
    Report(String),
    /// The server answered more lines than it was sent.
    #[error("the server answered {answered} lines to {sent} documents")]
    AnswerMismatch {
        /// Documents sent.
        sent: usize,
        /// Lines answered.
        answered: usize,
    },
    /// A field name cannot be expressed in a request.
    #[error("the field name {0:?} cannot be sent")]
    UnsafeField(String),
    /// The target file.
    #[error("cannot write the file: {0}")]
    Io(String),
}

/// What to import.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportJob {
    /// Where the documents come from.
    pub source: Source,
    /// How to read them.
    pub options: ReadOptions,
    /// The target collection.
    pub collection: String,
    /// What the server does with them.
    pub params: ImportParams,
}

/// Whether a job still takes on new work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Working.
    #[default]
    Running,
    /// Stop was asked for; what is in flight is awaited.
    Stopping,
}

/// How far an import is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportProgress {
    /// The phase.
    pub phase: Phase,
    /// Source bytes up to the last batch evaluated.
    pub bytes_done: u64,
    /// Size of the source, if known.
    pub total_bytes: Option<u64>,
    /// Documents handed to a request that got an answer or was aborted.
    pub sent: u64,
    /// Documents stored.
    pub imported: u64,
    /// Documents refused, by the server or locally.
    pub failed: u64,
    /// Documents sent whose fate nobody knows.
    pub unconfirmed: u64,
    /// Time since the start.
    pub elapsed: Duration,
    /// The first reason a document failed, for showing while the job runs.
    pub first_error: Option<Arc<str>>,
}

impl ImportProgress {
    /// Documents per second.
    pub fn rate(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds > 0.0 {
            (self.imported + self.failed) as f64 / seconds
        } else {
            0.0
        }
    }

    /// Time left, by bytes. `None` before the first batch or without a total.
    pub fn eta(&self) -> Option<Duration> {
        let total = self.total_bytes?;
        if self.bytes_done == 0 {
            return None;
        }
        let left = total.saturating_sub(self.bytes_done) as f64;
        Some(self.elapsed.mul_f64(left / self.bytes_done as f64))
    }

    /// Share of the source done. `None` without a total.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total_bytes.filter(|total| *total > 0)?;
        Some((self.bytes_done as f32 / total as f32).clamp(0.0, 1.0))
    }
}

/// How a job ended.
#[derive(Debug)]
pub enum JobEnd<E> {
    /// Everything was processed.
    Finished,
    /// Stopped on request.
    Stopped,
    /// Stopped by an error; what was done before it stays.
    Failed(E),
}

/// `num_documents` around a run, because a 200 can have done nothing or too much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountCheck {
    /// Before the first batch.
    pub before: u64,
    /// After the last batch.
    pub after: u64,
    /// Whether the change fits what was imported. Others may write at the same time, so
    /// `false` is a warning.
    pub ok: bool,
}

/// What an import did.
#[derive(Debug)]
pub struct ImportOutcome {
    /// How it ended.
    pub end: JobEnd<TransferError>,
    /// The final numbers.
    pub progress: ImportProgress,
    /// The failed documents.
    pub report: Report,
    /// The first line of the first batch not answered, unless finished.
    pub stopped_at_line: Option<u64>,
    /// `None` when not finished or when the count could not be read.
    pub count: Option<CountCheck>,
}

#[derive(Default)]
struct Batch {
    /// The documents, each with a `\n`.
    body: String,
    /// Source line of each document in `body`.
    lines: Vec<u64>,
    /// Records of this stretch that must not be sent.
    rejected: Vec<ReportRow>,
    /// Source offset after the last record.
    end_offset: u64,
    /// Bytes held, `rejected` included.
    bytes: usize,
}

impl Batch {
    fn add(&mut self, record: Record) {
        self.end_offset = record.offset;
        match record.doc {
            Ok(text) => {
                self.bytes += text.len() + 1;
                self.body.push_str(&text);
                self.body.push('\n');
                self.lines.push(record.line);
            }
            Err(rejected) => {
                self.bytes += rejected.raw.len();
                self.rejected.push(ReportRow {
                    line: record.line,
                    code: None,
                    error: match rejected.reason {
                        RejectReason::InvalidUtf8 => {
                            "not valid UTF-8; never sent, the text here is approximate".to_string()
                        }
                        RejectReason::TooLarge(bytes) => {
                            format!("{bytes} bytes is too large; never sent, the text here is cut")
                        }
                    },
                    id: None,
                    raw: rejected.raw,
                });
            }
        }
    }

    fn is_full(&self, batch_size: u32) -> bool {
        self.lines.len() + self.rejected.len() >= batch_size as usize || self.bytes >= BATCH_BYTES
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.rejected.is_empty()
    }

    fn first_line(&self) -> Option<u64> {
        let rejected = self.rejected.first().map(|row| row.line);
        match (self.lines.first().copied(), rejected) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn last_line(&self) -> Option<u64> {
        let rejected = self.rejected.last().map(|row| row.line);
        self.lines.last().copied().max(rejected)
    }

    fn docs(&self) -> impl Iterator<Item = (u64, &str)> {
        self.lines
            .iter()
            .copied()
            .zip(self.body.split_terminator('\n'))
    }
}

enum Message {
    Batch(Batch),
    End(Result<(), ReadError>),
}

/// Reads the source into batches until it ends or nobody listens. Blocking.
fn read_batches(job: &ImportJob, tx: &mpsc::Sender<Message>) {
    let mut batch = Batch::default();
    let result = read_documents(&job.source, &job.options, |record| {
        batch.add(record);
        if batch.is_full(job.params.batch_size)
            && tx
                .blocking_send(Message::Batch(mem::take(&mut batch)))
                .is_err()
        {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    if !batch.is_empty() && tx.blocking_send(Message::Batch(batch)).is_err() {
        return;
    }
    // Nobody left to tell when this fails.
    let _ = tx.blocking_send(Message::End(result));
}

enum Sent {
    /// The server answered every line of a 2xx, or refused the batch.
    Answer(Result<Vec<ImportLine>, Error>),
    /// Stopped while the server was busy: nothing of the batch was applied.
    Withdrawn,
    /// The request was dropped mid-way.
    Aborted,
}

/// Sends `body`, again while the server says it is busy.
async fn send(client: &Client, job: &ImportJob, control: &JobControl, body: &str) -> Sent {
    let mut retries = 0;
    loop {
        let answer = tokio::select! {
            answer = client.import_batch(&job.collection, &job.params, body) => answer,
            () = control.abort.cancelled() => return Sent::Aborted,
        };
        match answer {
            Err(Error::Http { status: 503, .. }) if retries < BUSY_RETRIES => {
                let wait = Duration::from_secs(1 << retries);
                retries += 1;
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = control.stop.cancelled() => return Sent::Withdrawn,
                    () = control.abort.cancelled() => return Sent::Withdrawn,
                }
            }
            answer => return Sent::Answer(answer),
        }
    }
}

/// The string `id` of a line that was sent.
fn id_of(raw: &str) -> Option<String> {
    match serde_json::from_str::<Value>(raw).ok()?.get("id")? {
        Value::String(id) => Some(id.clone()),
        _ => None,
    }
}

fn unconfirmed_row(line: u64, raw: &str, error: &str) -> ReportRow {
    ReportRow {
        line,
        code: None,
        error: error.to_string(),
        id: id_of(raw),
        raw: raw.to_string(),
    }
}

/// Pairs `answers` with the documents of `batch` by position: a line the server cannot read
/// has no id of its own in the answer.
fn evaluate(
    batch: &Batch,
    answers: Vec<ImportLine>,
    state: &mut ImportProgress,
    rows: &mut Vec<ReportRow>,
) {
    let mut answers = answers.into_iter();
    for (line, raw) in batch.docs() {
        match answers.next() {
            Some(answer) if answer.success => state.imported += 1,
            Some(answer) => {
                state.failed += 1;
                rows.push(ReportRow {
                    line,
                    code: answer.code,
                    error: answer.error.unwrap_or_default(),
                    id: id_of(raw),
                    raw: raw.to_string(),
                });
            }
            None => {
                state.unconfirmed += 1;
                let error = "the server gave no answer for this document";
                rows.push(unconfirmed_row(line, raw, error));
            }
        }
    }
}

/// Whether `num_documents` moved as `action` and `imported` say it should.
fn count_fits(action: ImportAction, before: u64, after: u64, imported: u64) -> bool {
    let growth = after.checked_sub(before);
    match action {
        ImportAction::Create => growth == Some(imported),
        ImportAction::Upsert | ImportAction::Emplace => growth.is_some_and(|g| g <= imported),
        ImportAction::Update => growth == Some(0),
    }
}

/// One import from start to end. Nothing starts after `stop`, and the batch in flight is
/// awaited unless `abort` drops it, so the counts stay true.
pub async fn run_import(
    client: Client,
    job: ImportJob,
    control: JobControl,
    progress: watch::Sender<ImportProgress>,
) -> ImportOutcome {
    let started = Instant::now();
    let mut state = ImportProgress {
        total_bytes: match &job.source {
            Source::File(path) => tokio::fs::metadata(path).await.ok().map(|m| m.len()),
            Source::Bytes(bytes) => Some(bytes.len() as u64),
        },
        ..ImportProgress::default()
    };
    let mut report = match Report::new().await {
        Ok(report) => report,
        Err(e) => {
            return ImportOutcome {
                end: JobEnd::Failed(TransferError::Report(e.to_string())),
                progress: state,
                report: Report::without_spool(),
                stopped_at_line: None,
                count: None,
            };
        }
    };
    progress.send_replace(state.clone());

    let before = client
        .collection(&job.collection)
        .await
        .ok()
        .map(|c| c.num_documents);

    // The queue plus the batch that waits to enter it plus the one in flight: three batches
    // at most, so the constant's "one being filled" is not a slot of the queue.
    let (tx, mut rx) = mpsc::channel(CHANNEL_BATCHES - 1);
    let reader_job = job.clone();
    tokio::task::spawn_blocking(move || read_batches(&reader_job, &tx));

    let stopping = tokio::spawn({
        let (stop, progress) = (control.stop.clone(), progress.clone());
        async move {
            stop.cancelled().await;
            progress.send_modify(|p| p.phase = Phase::Stopping);
        }
    });

    // Where a resumed run starts, if no batch is waiting to tell.
    let mut next_line = 1;
    let mut pending = None;
    let end = loop {
        // A waiting message wins over `stop`, so a run that is done ends as done.
        let message = tokio::select! {
            biased;
            message = rx.recv() => message,
            () = control.stop.cancelled() => break JobEnd::Stopped,
        };
        let mut batch = match message {
            Some(Message::Batch(batch)) => batch,
            Some(Message::End(Ok(()))) => break JobEnd::Finished,
            Some(Message::End(Err(e))) => break JobEnd::Failed(e.into()),
            None => {
                let e = ReadError::Io("the reader ended early".to_string());
                break JobEnd::Failed(e.into());
            }
        };
        if control.stop.is_cancelled() {
            pending = batch.first_line();
            break JobEnd::Stopped;
        }

        let (first_line, last_line) = (batch.first_line(), batch.last_line());
        let mut rows = mem::take(&mut batch.rejected);
        let rejected = rows.len() as u64;
        let docs = batch.lines.len() as u64;
        let mut ended = None;
        if !batch.body.is_empty() {
            let mut unconfirmed = |rows: &mut Vec<ReportRow>, error: &str| {
                state.sent += docs;
                state.unconfirmed += docs;
                rows.extend(batch.docs().map(|(l, raw)| unconfirmed_row(l, raw, error)));
            };
            match send(&client, &job, &control, &batch.body).await {
                Sent::Withdrawn => {
                    pending = first_line;
                    break JobEnd::Stopped;
                }
                Sent::Aborted => {
                    unconfirmed(&mut rows, "aborted; the server may have stored this");
                    ended = Some(JobEnd::Stopped);
                }
                Sent::Answer(Ok(answers)) if answers.len() > batch.lines.len() => {
                    unconfirmed(&mut rows, "the server answered more lines than it was sent");
                    ended = Some(JobEnd::Failed(TransferError::AnswerMismatch {
                        sent: batch.lines.len(),
                        answered: answers.len(),
                    }));
                }
                Sent::Answer(Ok(answers)) => {
                    state.sent += docs;
                    evaluate(&batch, answers, &mut state, &mut rows);
                }
                // A broken connection says nothing about what the server stored.
                Sent::Answer(Err(e @ (Error::Network(_) | Error::Timeout))) => {
                    unconfirmed(
                        &mut rows,
                        "the connection failed; the server may have stored this",
                    );
                    ended = Some(JobEnd::Failed(e.into()));
                }
                Sent::Answer(Err(e)) => {
                    if matches!(e, Error::Http { .. }) {
                        state.sent += docs;
                    }
                    ended = Some(JobEnd::Failed(e.into()));
                }
            }
        }
        state.failed += rejected;

        rows.sort_by_key(|row| row.line);
        if state.first_error.is_none() {
            state.first_error = rows.first().map(|row| Arc::from(row.error.as_str()));
        }
        let mut spool_failed = None;
        for row in rows {
            if let Err(e) = report.add(row).await {
                spool_failed = Some(e);
                break;
            }
        }
        if ended.is_some() {
            pending = first_line;
        } else {
            next_line = last_line.map_or(next_line, |line| line + 1);
            state.bytes_done = batch.end_offset;
        }
        publish(&mut state, started, &control, &progress);
        if let Some(e) = spool_failed {
            break JobEnd::Failed(TransferError::Report(e.to_string()));
        }
        if let Some(end) = ended {
            break end;
        }
    };
    stopping.abort();
    drop(rx);

    let end = match report.finish().await {
        Err(e) if matches!(end, JobEnd::Finished) => {
            JobEnd::Failed(TransferError::Report(e.to_string()))
        }
        _ => end,
    };
    let count = match (&end, before) {
        (JobEnd::Finished, Some(before)) => {
            client
                .collection(&job.collection)
                .await
                .ok()
                .map(|c| CountCheck {
                    before,
                    after: c.num_documents,
                    ok: count_fits(job.params.action, before, c.num_documents, state.imported),
                })
        }
        _ => None,
    };
    let stopped_at_line = match end {
        JobEnd::Finished => None,
        _ => Some(pending.unwrap_or(next_line)),
    };
    publish(&mut state, started, &control, &progress);
    ImportOutcome {
        end,
        progress: state,
        report,
        stopped_at_line,
        count,
    }
}

fn publish(
    state: &mut ImportProgress,
    started: Instant,
    control: &JobControl,
    progress: &watch::Sender<ImportProgress>,
) {
    state.elapsed = started.elapsed();
    if control.stop.is_cancelled() {
        state.phase = Phase::Stopping;
    }
    progress.send_replace(state.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fit_per_action() {
        assert!(count_fits(ImportAction::Create, 5, 8, 3));
        assert!(!count_fits(ImportAction::Create, 5, 5, 3));
        assert!(count_fits(ImportAction::Upsert, 5, 6, 3));
        assert!(!count_fits(ImportAction::Upsert, 5, 9, 3));
        assert!(!count_fits(ImportAction::Emplace, 5, 4, 3));
        assert!(count_fits(ImportAction::Update, 5, 5, 3));
        assert!(!count_fits(ImportAction::Update, 5, 6, 3));
    }

    #[test]
    fn progress_figures() {
        let p = ImportProgress {
            bytes_done: 25,
            total_bytes: Some(100),
            imported: 8,
            failed: 2,
            elapsed: Duration::from_secs(5),
            ..ImportProgress::default()
        };
        assert_eq!(p.rate(), 2.0);
        assert_eq!(p.eta(), Some(Duration::from_secs(15)));
        assert_eq!(p.fraction(), Some(0.25));
        assert_eq!(ImportProgress::default().eta(), None);
        assert_eq!(ImportProgress::default().fraction(), None);
    }
}
