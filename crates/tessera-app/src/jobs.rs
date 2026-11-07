// SPDX-License-Identifier: GPL-3.0-or-later

//! Running imports, exports, backups and restores. [`Jobs`] is a global entity, not part of a view or a
//! connection, so closing a dialog or a tab or disconnecting never drops a job. A disconnect
//! does lock the client a job shares, which stops a write on a read-only profile.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
    WeakEntity,
};
use rust_i18n::t;
use tessera_client::{Client, Error};
use tessera_core::backup::restore::{
    RestoreError, RestoreJob, RestoreOutcome, RestoreProgress, RestoreRow, RowFailure, RowKind,
    RowResult,
};
use tessera_core::backup::run::{BackupError, BackupOutcome, BackupPlan, BackupProgress};
use tessera_core::profile::{Environment, Profile};
use tessera_core::transfer::export::{ExportJob, ExportOutcome, ExportProgress};
use tessera_core::transfer::import::{ImportJob, ImportOutcome, ImportProgress};
use tessera_core::transfer::report::Report;
use tessera_core::transfer::{JobControl, JobEnd, TransferError, cap_error};
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

use crate::collections::BoxFuture;
use crate::connection::{ConnectionState, long_error_text};
use crate::transfer::{TransferBackendGlobal, archive_error_text, read_error_text};

/// The shortest time between two progress updates: at most 30 a second.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(34);

/// Identifies a job for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(pub u64);

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Writes a source into a collection.
    Import,
    /// Writes a collection to a file.
    Export,
    /// Writes collections and what they use to a folder or an archive.
    Backup,
    /// Writes a backup to the server.
    Restore,
}

/// The newest numbers of a job.
#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    /// Of an import.
    Import(ImportProgress),
    /// Of an export.
    Export(ExportProgress),
    /// Of a backup.
    Backup(BackupProgress),
    /// Of a restore.
    Restore(RestoreProgress),
}

/// Where a job stands.
#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    /// Working.
    Running,
    /// Stop was asked for; what is in flight is awaited.
    Stopping,
    /// Everything was processed.
    Finished,
    /// Stopped on request.
    Stopped,
    /// Ended by an error, in full.
    Failed(SharedString),
}

/// What a finished job did.
#[derive(Debug)]
pub enum JobOutcome {
    /// Of an import.
    Import(ImportOutcome),
    /// Of an export.
    Export(ExportOutcome),
    /// Of a backup.
    Backup(BackupOutcome),
    /// Of a restore.
    Restore(RestoreOutcome),
}

/// One import or export of the session.
pub struct Job {
    /// Its id.
    pub id: JobId,
    /// What it does.
    pub kind: JobKind,
    /// The profile it runs on.
    pub connection: Uuid,
    /// Name of that profile.
    pub connection_name: SharedString,
    /// Environment of that profile.
    pub environment: Environment,
    /// The collection.
    pub collection: String,
    /// What the row calls it: the file, usually.
    pub label: SharedString,
    /// Where it stands.
    pub status: JobStatus,
    /// The newest numbers.
    pub live: Live,
    /// Stops it.
    pub control: JobControl,
    /// What it did, once it ended.
    pub outcome: Option<JobOutcome>,
    /// The report was saved, so dismissing it loses nothing.
    pub report_saved: bool,
    profile: Profile,
    /// Tells whether a refused write came from a disconnect.
    state: WeakEntity<ConnectionState>,
    /// Fires when the backend is done, also while the foreground executor stands still.
    done: Option<oneshot::Receiver<()>>,
    outcome_task: Option<Task<()>>,
    throttle: Option<Task<()>>,
}

impl Job {
    /// Whether the job is working still.
    pub fn is_running(&self) -> bool {
        matches!(self.status, JobStatus::Running | JobStatus::Stopping)
    }

    /// The toast for the job having ended; it names the connection, as UX rule 1 asks.
    pub fn toast_text(&self) -> String {
        let (connection, name) = (self.connection_name.clone(), self.label.clone());
        let grouped = crate::views::overview::grouped;
        match (&self.status, &self.live) {
            (JobStatus::Failed(_), _) => {
                t!("jobs.toast_failed", connection = connection, name = name)
            }
            (_, Live::Import(p)) => t!(
                "jobs.toast_import",
                connection = connection,
                name = name,
                imported = grouped(p.imported),
                failed = grouped(p.failed)
            ),
            (_, Live::Export(p)) => t!(
                "jobs.toast_export",
                connection = connection,
                name = name,
                docs = grouped(p.docs)
            ),
            (_, Live::Backup(_)) => t!("jobs.toast_backup", connection = connection, name = name),
            (_, Live::Restore(_)) => t!("jobs.toast_restore", connection = connection, name = name),
        }
        .to_string()
    }

    /// The profile it runs on.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The report of a finished import.
    pub fn report(&self) -> Option<&Arc<Report>> {
        match &self.outcome {
            Some(JobOutcome::Import(outcome)) => Some(&outcome.report),
            _ => None,
        }
    }
}

/// All jobs of the session, in start order.
#[derive(Default)]
pub struct Jobs {
    jobs: Vec<Job>,
    next_id: u64,
    quit: Option<Subscription>,
}

/// The app's jobs.
pub struct JobsGlobal(pub Entity<Jobs>);

impl Global for JobsGlobal {}

/// The number of running jobs changed, or the list became empty or stopped being so.
pub struct RunningChanged(pub usize);

/// A job ended.
pub struct JobFinished(pub JobId);

impl EventEmitter<RunningChanged> for Jobs {}
impl EventEmitter<JobFinished> for Jobs {}

/// Installs the jobs entity.
pub fn init(cx: &mut App) {
    let jobs = cx.new(|_| Jobs::default());
    cx.set_global(JobsGlobal(jobs));
}

/// The sentence for a failed job.
fn failure_text(err: &TransferError, profile: &Profile) -> SharedString {
    let text = match err {
        TransferError::Client(e) => long_error_text(e, profile),
        TransferError::Read(e) => read_error_text(e),
        TransferError::Report(detail) => t!("jobs.error.report", detail = detail).to_string(),
        TransferError::AnswerMismatch { sent, answered } => t!(
            "jobs.error.answer_mismatch",
            sent = sent,
            answered = answered
        )
        .to_string(),
        TransferError::UnsafeField(name) => t!("jobs.error.unsafe_field", name = name).to_string(),
        TransferError::Io(detail) => t!("jobs.error.io", detail = detail).to_string(),
    };
    cap_error(&text).to_string().into()
}

fn backup_failure(err: &BackupError, profile: &Profile) -> SharedString {
    let text = match err {
        BackupError::Client(e) => long_error_text(e, profile),
        BackupError::UnknownCollection(name) => t!("backup.error.unknown", name = name).to_string(),
        BackupError::Exists(path) => {
            t!("backup.target_exists", path = path.display().to_string()).to_string()
        }
        BackupError::Export { collection, source } => t!(
            "backup.error.export",
            collection = collection,
            detail = failure_text(source, profile)
        )
        .to_string(),
        BackupError::TooLarge(name) => t!("backup.error.too_large", name = name).to_string(),
        BackupError::Archive(e) => archive_error_text(e),
        BackupError::Io(detail) => t!("jobs.error.io", detail = detail).to_string(),
    };
    cap_error(&text).to_string().into()
}

/// What went wrong with a row of a restore.
fn row_failure_text(failure: &RowFailure, profile: &Profile) -> String {
    let grouped = crate::views::overview::grouped;
    match failure {
        RowFailure::Error(e) => failure_text(&e.0, profile).to_string(),
        RowFailure::Count { expected, found } => t!(
            "restore.failure.count",
            expected = grouped(*expected),
            found = found.map_or_else(|| t!("figure.missing").to_string(), grouped)
        )
        .to_string(),
        RowFailure::Lines {
            failed,
            unconfirmed,
            first_error,
        } => {
            let counts = t!(
                "restore.failure.lines",
                failed = grouped(*failed),
                unconfirmed = grouped(*unconfirmed)
            );
            match first_error {
                Some(reason) => format!("{counts} {reason}"),
                None => counts.to_string(),
            }
        }
        RowFailure::Stopped => t!("restore.failure.stopped").to_string(),
    }
}

/// One row after a restore in a sentence, and whether it failed.
pub fn row_result_text(row: &RestoreRow, profile: &Profile) -> (String, bool) {
    let text = match &row.result {
        RowResult::Done => t!("restore.result.done").to_string(),
        RowResult::Kept => t!("restore.result.kept").to_string(),
        RowResult::Skipped => t!("restore.result.skipped").to_string(),
        RowResult::Failed(failure) => row_failure_text(failure, profile),
    };
    let what = t!(row_kind_key(row.kind));
    (
        format!("{what} {}: {text}", row.name),
        matches!(row.result, RowResult::Failed(_)),
    )
}

/// The ticked collections and what they miss, as a list of `collection → target`.
pub fn unmet_text(unmet: &[(String, String)]) -> String {
    let pairs = unmet.iter().map(|(from, to)| format!("{from} → {to}"));
    pairs.collect::<Vec<_>>().join(", ")
}

fn restore_failure(err: &RestoreError, rows: &[RestoreRow], profile: &Profile) -> SharedString {
    let text = match err {
        RestoreError::Read(e) => long_error_text(e, profile),
        RestoreError::NowExists(names) => {
            t!("restore.error.now_exists", names = names.join(", ")).to_string()
        }
        RestoreError::Unmet(unmet) => {
            t!("restore.error.unmet", names = unmet_text(unmet)).to_string()
        }
        RestoreError::Row { kind, name } => {
            let result = rows.iter().find(|r| r.kind == *kind && r.name == *name);
            let detail = match result.map(|r| &r.result) {
                Some(RowResult::Failed(failure)) => row_failure_text(failure, profile),
                _ => String::new(),
            };
            let what = t!(row_kind_key(*kind));
            t!(
                "restore.error.row",
                what = what,
                name = name,
                detail = detail
            )
            .to_string()
        }
    };
    cap_error(&text).to_string().into()
}

/// The locale key of a row's kind.
pub fn row_kind_key(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Stopwords => "restore.kind.stopwords",
        RowKind::SynonymSet => "restore.kind.synonyms",
        RowKind::CurationSet => "restore.kind.curations",
        RowKind::Collection => "restore.kind.collection",
        RowKind::Alias => "restore.kind.alias",
    }
}

/// Whether the read-only guard refused a write of this job.
fn refused_write(outcome: &JobOutcome) -> bool {
    let refused = |e: &TransferError| matches!(e, TransferError::Client(Error::ReadOnly { .. }));
    match outcome {
        JobOutcome::Import(o) => matches!(&o.end, JobEnd::Failed(e) if refused(e)),
        JobOutcome::Export(o) => matches!(&o.end, JobEnd::Failed(e) if refused(e)),
        JobOutcome::Backup(_) => false,
        JobOutcome::Restore(o) => o
            .rows
            .iter()
            .any(|r| matches!(&r.result, RowResult::Failed(RowFailure::Error(e)) if refused(&e.0))),
    }
}

fn status_of<E>(end: &JobEnd<E>, failed: impl FnOnce(&E) -> SharedString) -> JobStatus {
    match end {
        JobEnd::Finished => JobStatus::Finished,
        JobEnd::Stopped => JobStatus::Stopped,
        JobEnd::Failed(e) => JobStatus::Failed(failed(e)),
    }
}

/// What `start` needs to know of a job.
struct Spec {
    kind: JobKind,
    collection: String,
    label: SharedString,
    live: Live,
    control: JobControl,
}

impl Jobs {
    /// Starts an import on `connection`'s client, whose flags are shared, so its unlock state
    /// stays live. Its numbers arrive through `Jobs`, however long the starter lives.
    pub fn start_import(
        &mut self,
        connection: &Entity<ConnectionState>,
        job: ImportJob,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> JobId {
        let (sender, receiver) = watch::channel(ImportProgress::default());
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        let control = JobControl::default();
        let spec = Spec {
            kind: JobKind::Import,
            collection: job.collection.clone(),
            label,
            live: Live::Import(ImportProgress::default()),
            control: control.clone(),
        };
        let run = move |client| -> BoxFuture<JobOutcome> {
            let run = backend.import(client, job, control, sender);
            Box::pin(async move { JobOutcome::Import(run.await) })
        };
        self.start(connection, spec, run, receiver, Live::Import, cx)
    }

    /// Starts an export on `connection`'s client.
    pub fn start_export(
        &mut self,
        connection: &Entity<ConnectionState>,
        job: ExportJob,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> JobId {
        let (sender, receiver) = watch::channel(ExportProgress::default());
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        let control = JobControl::default();
        let spec = Spec {
            kind: JobKind::Export,
            collection: job.collection.clone(),
            label,
            live: Live::Export(ExportProgress::default()),
            control: control.clone(),
        };
        let run = move |client| -> BoxFuture<JobOutcome> {
            let run = backend.export(client, job, control, sender);
            Box::pin(async move { JobOutcome::Export(run.await) })
        };
        self.start(connection, spec, run, receiver, Live::Export, cx)
    }

    /// Starts a backup on `connection`'s client. It reads, so a read-only connection stays locked.
    pub fn start_backup(
        &mut self,
        connection: &Entity<ConnectionState>,
        plan: BackupPlan,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> JobId {
        let (sender, receiver) = watch::channel(BackupProgress::default());
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        let control = JobControl::default();
        let spec = Spec {
            kind: JobKind::Backup,
            collection: plan.collections.first().cloned().unwrap_or_default(),
            label,
            live: Live::Backup(BackupProgress::default()),
            control: control.clone(),
        };
        let run = move |client| -> BoxFuture<JobOutcome> {
            let run = backend.backup(client, plan, control, sender);
            Box::pin(async move { JobOutcome::Backup(run.await) })
        };
        self.start(connection, spec, run, receiver, Live::Backup, cx)
    }

    /// Starts a restore on `connection`'s client. `temp` is the unpacked archive, which stays
    /// until the run ended, whatever becomes of the view that made it.
    pub fn start_restore(
        &mut self,
        connection: &Entity<ConnectionState>,
        job: RestoreJob,
        temp: Option<Arc<tempfile::TempDir>>,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> JobId {
        let (sender, receiver) = watch::channel(RestoreProgress::default());
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        let control = JobControl::default();
        let spec = Spec {
            kind: JobKind::Restore,
            collection: job
                .loaded
                .schemas
                .first()
                .map(|s| s.name.clone())
                .unwrap_or_default(),
            label,
            live: Live::Restore(RestoreProgress::default()),
            control: control.clone(),
        };
        let run = move |client| -> BoxFuture<JobOutcome> {
            let run = backend.restore(client, job, control, sender);
            Box::pin(async move {
                let outcome = run.await;
                drop(temp);
                JobOutcome::Restore(outcome)
            })
        };
        self.start(connection, spec, run, receiver, Live::Restore, cx)
    }

    fn start<P: Clone + Send + Sync + 'static>(
        &mut self,
        connection: &Entity<ConnectionState>,
        spec: Spec,
        run: impl FnOnce(Client) -> BoxFuture<JobOutcome>,
        mut progress: watch::Receiver<P>,
        live: fn(P) -> Live,
        cx: &mut Context<Self>,
    ) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        let conn = connection.read(cx);
        let client = conn.connected.as_ref().map(|c| c.client.clone());
        let profile = conn.profile.clone();
        let mut job = Job {
            id,
            kind: spec.kind,
            connection: profile.id,
            connection_name: profile.name.clone().into(),
            environment: profile.environment.clone(),
            collection: spec.collection,
            label: spec.label,
            status: JobStatus::Running,
            live: spec.live,
            control: spec.control,
            outcome: None,
            report_saved: false,
            profile,
            state: connection.downgrade(),
            done: None,
            outcome_task: None,
            throttle: None,
        };
        let Some(client) = client else {
            job.status = JobStatus::Failed(t!("jobs.error.disconnected").to_string().into());
            self.jobs.push(job);
            cx.emit(RunningChanged(self.running()));
            cx.emit(JobFinished(id));
            cx.notify();
            return id;
        };
        let (done, finished) = oneshot::channel();
        job.done = Some(finished);
        let run = run(client);
        // Not on the foreground: on quit that one stands still while the handler waits.
        let outcome = cx.background_spawn(async move {
            let outcome = run.await;
            done.send(()).ok();
            outcome
        });
        job.outcome_task = Some(cx.spawn(async move |this, cx| {
            let outcome = outcome.await;
            this.update(cx, |jobs, cx| jobs.finish(id, outcome, cx))
                .ok();
        }));
        // `watch` keeps only the newest value, so a slow UI skips states and never queues them.
        job.throttle = Some(cx.spawn(async move |this, cx| {
            while progress.changed().await.is_ok() {
                let snapshot = live(progress.borrow_and_update().clone());
                let shown = this.update(cx, |jobs, cx| jobs.set_live(id, snapshot, cx));
                if shown.is_err() {
                    break;
                }
                cx.background_executor().timer(PROGRESS_INTERVAL).await;
            }
        }));
        self.jobs.push(job);
        if self.quit.is_none() {
            self.quit = Some(cx.on_app_quit(|jobs, _| jobs.stop_all()));
        }
        cx.emit(RunningChanged(self.running()));
        cx.notify();
        id
    }

    /// Tells every job to stop; the returned future ends when they have.
    fn stop_all(&mut self) -> impl Future<Output = ()> + use<> {
        let mut waiting = Vec::new();
        for job in &mut self.jobs {
            job.control.stop.cancel();
            waiting.extend(job.done.take());
        }
        async move {
            for done in waiting {
                done.await.ok();
            }
        }
    }

    fn job_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    fn set_live(&mut self, id: JobId, live: Live, cx: &mut Context<Self>) {
        if let Some(job) = self.job_mut(id) {
            job.live = live;
            cx.notify();
        }
    }

    fn finish(&mut self, id: JobId, outcome: JobOutcome, cx: &mut Context<Self>) {
        let Some(job) = self.job_mut(id) else {
            return;
        };
        job.throttle = None;
        job.done = None;
        let end = match &outcome {
            JobOutcome::Import(outcome) => {
                job.live = Live::Import(outcome.progress.clone());
                Some(&outcome.end)
            }
            JobOutcome::Export(outcome) => {
                job.live = Live::Export(outcome.progress.clone());
                Some(&outcome.end)
            }
            JobOutcome::Backup(_) | JobOutcome::Restore(_) => None,
        };
        // A disconnect locks the client this job shares (spec 7.1).
        let locked =
            refused_write(&outcome) && job.state.upgrade().is_none_or(|c| c.read(cx).closed);
        job.status = match (end, &outcome) {
            _ if locked => {
                JobStatus::Failed(t!("jobs.error.locked_by_disconnect").to_string().into())
            }
            (Some(end), _) => status_of(end, |e| failure_text(e, &job.profile)),
            (None, JobOutcome::Backup(o)) => status_of(&o.end, |e| backup_failure(e, &job.profile)),
            (None, JobOutcome::Restore(o)) => {
                status_of(&o.end, |e| restore_failure(e, &o.rows, &job.profile))
            }
            (None, _) => JobStatus::Finished,
        };
        job.outcome = Some(outcome);
        cx.emit(RunningChanged(self.running()));
        cx.emit(JobFinished(id));
        cx.notify();
    }

    /// Asks a running job to stop: it starts nothing new and finishes what is in flight.
    pub fn stop(&mut self, id: JobId, cx: &mut Context<Self>) {
        if let Some(job) = self.job_mut(id).filter(|j| j.status == JobStatus::Running) {
            job.status = JobStatus::Stopping;
            job.control.stop.cancel();
            cx.notify();
        }
    }

    /// Stops waiting for what a stopping job has in flight.
    pub fn abort(&mut self, id: JobId, cx: &mut Context<Self>) {
        if let Some(job) = self.job_mut(id).filter(|j| j.status == JobStatus::Stopping) {
            job.control.abort.cancel();
            cx.notify();
        }
    }

    /// Removes a finished job, and with it its report.
    pub fn dismiss(&mut self, id: JobId, cx: &mut Context<Self>) {
        if self.job_mut(id).is_some_and(|job| !job.is_running()) {
            self.jobs.retain(|job| job.id != id);
            cx.emit(RunningChanged(self.running()));
            cx.notify();
        }
    }

    /// Marks the report of job `id` as saved.
    pub fn set_report_saved(&mut self, id: JobId, cx: &mut Context<Self>) {
        if let Some(job) = self.job_mut(id) {
            job.report_saved = true;
            cx.notify();
        }
    }

    /// How many jobs are working.
    pub fn running(&self) -> usize {
        self.jobs.iter().filter(|job| job.is_running()).count()
    }

    /// All jobs, oldest first.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }
}
