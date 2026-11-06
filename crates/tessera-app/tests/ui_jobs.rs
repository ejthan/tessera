// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of import and export jobs: they outlive their views, and their progress is throttled.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::connection::{CollectionChanged, ConnectionState};
use tessera_app::jobs::{JobId, JobStatus, Jobs, JobsGlobal, Live, RunningChanged};
use tessera_app::state::AppState;
use tessera_app::views::root::RootView;
use tessera_client::{DirtyValues, Error, ImportAction, ImportParams};
use tessera_core::profile::Profile;
use tessera_core::transfer::JobEnd;
use tessera_core::transfer::TransferError;
use tessera_core::transfer::import::{ImportJob, ImportProgress};
use tessera_core::transfer::source::{Format, ReadOptions, Source};

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    profile: Profile,
    fake: Arc<FakeTransfer>,
    jobs: Entity<Jobs>,
    conn: Entity<ConnectionState>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    fn start(&self, cx: &mut TestAppContext) -> JobId {
        let conn = self.conn.clone();
        let id = cx.update(|cx| {
            self.jobs.update(cx, |jobs, cx| {
                jobs.start_import(&conn, job(), "books.jsonl".into(), cx)
            })
        });
        cx.run_until_parked();
        id
    }

    fn status(&self, cx: &TestAppContext, id: JobId) -> JobStatus {
        cx.read(|cx| {
            let jobs = self.jobs.read(cx).jobs();
            let job = jobs.iter().find(|j| j.id == id).unwrap();
            job.status.clone()
        })
    }
}

fn job() -> ImportJob {
    ImportJob {
        source: Source::Bytes(Arc::from(&b"{}\n"[..])),
        options: ReadOptions::new(Format::Jsonl),
        collection: "books".into(),
        params: ImportParams {
            action: ImportAction::Create,
            batch_size: 1000,
            dirty_values: DirtyValues::CoerceOrReject,
        },
    }
}

async fn connected(cx: &mut TestAppContext) -> Fixture {
    fake_backend(cx, Some((Vec::new(), Vec::new())));
    fake_connector(cx, |p| Ok(server_for(p)));
    let fake = fake_transfer(cx);
    let (staging, _, secrets) = two_profiles();
    let (handle, state, rt) = open_app(cx, vec![staging.clone()], secrets);
    cx.update(|cx| state.update(cx, |s, cx| s.connect(staging.id, None, cx)));
    let id = staging.id;
    cx.condition(&state, |s, cx| {
        s.connection(id, cx)
            .is_some_and(|c| c.read(cx).connected.is_some())
    })
    .await;
    let conn = cx.read(|cx| state.read(cx).connection(id, cx).unwrap());
    let jobs = cx.read(|cx| cx.global::<JobsGlobal>().0.clone());
    Fixture {
        handle,
        state,
        profile: staging,
        fake,
        jobs,
        conn,
        _rt: rt,
    }
}

fn finished() -> tessera_core::transfer::import::ImportOutcome {
    import_outcome(JobEnd::Finished, ImportProgress::default())
}

#[gpui_kit::test]
async fn job_survives_its_starter(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    struct Starter;
    let conn = f.conn.clone();
    let id = cx.update(|cx| {
        let view = cx.new(|_| Starter);
        view.update(cx, |_, cx| {
            f.jobs.update(cx, |jobs, cx| {
                jobs.start_import(&conn, job(), "x".into(), cx)
            })
        })
    });
    cx.run_until_parked();
    f.fake.finish(id, finished());
    cx.run_until_parked();
    assert_eq!(f.status(cx, id), JobStatus::Finished);
}

#[gpui_kit::test]
async fn disconnect_does_not_stop_a_job(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    let profile = f.profile.id;
    cx.update(|cx| f.state.update(cx, |s, cx| s.disconnect(profile, cx)));
    cx.run_until_parked();
    assert_eq!(f.status(cx, id), JobStatus::Running);
    let stopped = cx.read(|cx| f.jobs.read(cx).jobs()[0].control.stop.is_cancelled());
    assert!(!stopped);
    f.fake.finish(id, finished());
    cx.run_until_parked();
    assert_eq!(f.status(cx, id), JobStatus::Finished);
}

fn update(imported: u64) -> ImportProgress {
    ImportProgress {
        imported,
        ..Default::default()
    }
}

#[gpui_kit::test]
async fn progress_is_throttled(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    let seen = Arc::new(Mutex::new(0));
    let count = seen.clone();
    let _watch = cx.update(|cx| {
        cx.observe(&f.jobs, move |_, _| {
            *count.lock().unwrap() += 1;
        })
    });
    for i in 0..1000 {
        f.fake.progress(id, update(i));
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
    }
    let seen = *seen.lock().unwrap();
    assert!((2..=30).contains(&seen), "{seen} updates");
}

#[gpui_kit::test]
async fn newest_progress_wins(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    for i in 0..10 {
        f.fake.progress(id, update(i));
    }
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    let shown = cx.read(|cx| match &f.jobs.read(cx).jobs()[0].live {
        Live::Import(p) => p.imported,
        Live::Export(_) => panic!("an import"),
    });
    assert_eq!(shown, 9);
}

#[gpui_kit::test]
async fn progress_does_not_notify_the_root(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let root = f
        .handle
        .update(cx, |root, _, _| {
            root.view().clone().downcast::<RootView>().unwrap()
        })
        .unwrap();
    let notified = Arc::new(Mutex::new(0));
    let count = notified.clone();
    let _root = cx.update(|cx| cx.observe(&root, move |_, _| *count.lock().unwrap() += 1));
    let changes = Arc::new(Mutex::new(Vec::new()));
    let record = changes.clone();
    let _changes = cx.update(|cx| {
        cx.subscribe(&f.jobs, move |_, event: &RunningChanged, _| {
            record.lock().unwrap().push(event.0);
        })
    });
    let id = f.start(cx);
    let after_start = *notified.lock().unwrap();
    for i in 0..50 {
        f.fake.progress(id, update(i));
        cx.executor().advance_clock(Duration::from_millis(40));
        cx.run_until_parked();
    }
    assert_eq!(*notified.lock().unwrap(), after_start);
    assert_eq!(*changes.lock().unwrap(), [1]);
    f.fake.finish(id, finished());
    cx.run_until_parked();
    assert_eq!(*changes.lock().unwrap(), [1, 0]);
}

#[gpui_kit::test]
async fn stop_then_abort(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    let tokens = cx.read(|cx| f.jobs.read(cx).jobs()[0].control.clone());
    cx.update(|cx| f.jobs.update(cx, |jobs, cx| jobs.abort(id, cx)));
    assert!(!tokens.abort.is_cancelled(), "abort only while stopping");
    cx.update(|cx| f.jobs.update(cx, |jobs, cx| jobs.stop(id, cx)));
    assert!(tokens.stop.is_cancelled());
    assert!(!tokens.abort.is_cancelled());
    assert_eq!(f.status(cx, id), JobStatus::Stopping);
    cx.update(|cx| f.jobs.update(cx, |jobs, cx| jobs.abort(id, cx)));
    assert!(tokens.abort.is_cancelled());
    f.fake.finish(
        id,
        import_outcome(JobEnd::Stopped, ImportProgress::default()),
    );
    cx.run_until_parked();
    assert_eq!(f.status(cx, id), JobStatus::Stopped);
}

#[gpui_kit::test]
async fn finish_reloads_the_collection(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    let changed = Arc::new(Mutex::new(Vec::new()));
    let record = changed.clone();
    let _changed = cx.update(|cx| {
        cx.subscribe(&f.conn, move |_, event: &CollectionChanged, _| {
            record.lock().unwrap().push(event.0.clone());
        })
    });
    f.fake.finish(id, finished());
    cx.run_until_parked();
    assert_eq!(*changed.lock().unwrap(), ["books"]);
}

#[gpui_kit::test]
async fn failure_text_is_kept_whole(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    let message = "x".repeat(4000);
    let err = TransferError::Client(Error::Http {
        status: 400,
        message: message.clone(),
        body: String::new(),
    });
    f.fake.finish(
        id,
        import_outcome(JobEnd::Failed(err), ImportProgress::default()),
    );
    cx.run_until_parked();
    match f.status(cx, id) {
        JobStatus::Failed(text) => assert!(text.contains(&message), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[gpui_kit::test]
async fn dismiss_refuses_a_running_job(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    cx.update(|cx| f.jobs.update(cx, |jobs, cx| jobs.dismiss(id, cx)));
    assert_eq!(cx.read(|cx| f.jobs.read(cx).jobs().len()), 1);
    f.fake.finish(id, finished());
    cx.run_until_parked();
    cx.update(|cx| f.jobs.update(cx, |jobs, cx| jobs.dismiss(id, cx)));
    assert_eq!(cx.read(|cx| f.jobs.read(cx).jobs().len()), 0);
}

#[gpui_kit::test]
async fn quit_stops_running_jobs(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start(cx);
    let tokens = cx.read(|cx| f.jobs.read(cx).jobs()[0].control.clone());
    cx.quit();
    assert!(tokens.stop.is_cancelled());
}
