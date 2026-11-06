// SPDX-License-Identifier: GPL-3.0-or-later

//! Running imports and exports. [`Jobs`] is a global entity, not part of a view or a
//! connection, so closing a dialog or a tab or disconnecting never drops a job.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
    WeakEntity,
};
use rust_i18n::t;
use tessera_client::Client;
use tessera_core::profile::{Environment, Profile};
use tessera_core::transfer::export::{ExportJob, ExportOutcome, ExportProgress};
use tessera_core::transfer::import::{ImportJob, ImportOutcome, ImportProgress};
use tessera_core::transfer::report::Report;
use tessera_core::transfer::{JobControl, JobEnd, TransferError};
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

use crate::collections::BoxFuture;
use crate::connection::{ConnectionState, long_error_text};
use crate::transfer::TransferBackendGlobal;

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
}

/// The newest numbers of a job.
#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    /// Of an import.
    Import(ImportProgress),
    /// Of an export.
    Export(ExportProgress),
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
    match err {
        TransferError::Client(e) => long_error_text(e, profile),
        TransferError::Read(e) => t!("jobs.error.read", detail = e.to_string()).to_string(),
        TransferError::Report(detail) => t!("jobs.error.report", detail = detail).to_string(),
        TransferError::AnswerMismatch { sent, answered } => t!(
            "jobs.error.answer_mismatch",
            sent = sent,
            answered = answered
        )
        .to_string(),
        TransferError::UnsafeField(name) => t!("jobs.error.unsafe_field", name = name).to_string(),
        TransferError::Io(detail) => t!("jobs.error.io", detail = detail).to_string(),
    }
    .into()
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
                &outcome.end
            }
            JobOutcome::Export(outcome) => {
                job.live = Live::Export(outcome.progress.clone());
                &outcome.end
            }
        };
        job.status = match end {
            JobEnd::Finished => JobStatus::Finished,
            JobEnd::Stopped => JobStatus::Stopped,
            JobEnd::Failed(err) => JobStatus::Failed(failure_text(err, &job.profile)),
        };
        job.outcome = Some(outcome);
        // Also after a stop or a failure: what arrived stays on the server.
        let reload = (job.kind == JobKind::Import)
            .then(|| job.state.upgrade().map(|c| (c, job.collection.clone())))
            .flatten();
        if let Some((connection, name)) = reload {
            connection.update(cx, |c, cx| c.collection_changed(name, cx));
        }
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
