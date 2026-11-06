// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of import and export jobs: they outlive their views, and their progress is throttled.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::connection::{CollectionChanged, ConnectionState};
use tessera_app::jobs::{JobId, JobStatus, Jobs, JobsGlobal, Live, RunningChanged};
use tessera_app::state::AppState;
use tessera_app::views::root::RootView;
use tessera_client::{DirtyValues, Error, ImportAction, ImportParams};
use tessera_core::profile::Profile;
use tessera_core::transfer::JobEnd;
use tessera_core::transfer::TransferError;
use tessera_core::transfer::export::{ExportFormat, ExportJob, ExportScope, FieldPick};
use tessera_core::transfer::import::{CountCheck, ImportJob, ImportProgress};
use tessera_core::transfer::report::ReportRow;
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

const TOGGLE_JOBS: &str = key("cmd-shift-j", "ctrl-shift-j");
const TOGGLE_LOG: &str = key("cmd-j", "ctrl-j");
const CANCEL_JOB: &str = key("cmd-.", "ctrl-.");

impl Fixture {
    fn press(&self, cx: &mut TestAppContext, keys: &str) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press(keys, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(id.to_string(), cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn text(&self, cx: &mut TestAppContext, id: &str) -> Option<String> {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find(id.to_string())
                .and_then(|e| e.label().map(str::to_string))
        })
        .unwrap()
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id.to_string()).is_some()
        })
        .unwrap()
    }

    fn panel_focus(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("jobs.row.0".to_string(), cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn toasts(&self, cx: &mut TestAppContext) -> usize {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.notifications(cx).len()
        })
        .unwrap()
    }

    fn root(&self, cx: &mut TestAppContext) -> Entity<RootView> {
        self.handle
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<RootView>().unwrap()
            })
            .unwrap()
    }
}

/// [`connected`] with the jobs tab open and one running import.
async fn running(cx: &mut TestAppContext) -> (Fixture, JobId) {
    let f = connected(cx).await;
    let id = f.start(cx);
    f.press(cx, TOGGLE_JOBS);
    (f, id)
}

fn failed_row(line: u64) -> ReportRow {
    ReportRow {
        line,
        code: Some(400),
        error: format!("Field n of line {line} must be an int32."),
        id: None,
        raw: format!("{{\"n\":\"{line}\"}}"),
        lossy: false,
    }
}

#[gpui_kit::test]
async fn running_import_shows_bar_rate_eta_and_errors(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    f.fake.progress(
        id,
        ImportProgress {
            bytes_done: 500,
            total_bytes: Some(1000),
            imported: 1200,
            failed: 3,
            elapsed: Duration::from_secs(10),
            ..Default::default()
        },
    );
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert_eq!(f.text(cx, "jobs.progress.0").as_deref(), Some("50%"));
    let text = f.text(cx, "jobs.text.0").unwrap();
    assert!(text.contains("1,203"), "{text}");
    assert!(text.contains("120/s"), "{text}");
    assert!(text.contains("ETA 0:10"), "{text}");
    assert!(text.contains("3 errors"), "{text}");
}

#[gpui_kit::test]
async fn unknown_total_shows_no_bar(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    f.fake.progress(id, update(5));
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert!(f.has(cx, "jobs.text.0"));
    assert!(!f.has(cx, "jobs.progress.0"));
}

#[gpui_kit::test]
async fn row_shows_connection_and_environment(cx: &mut TestAppContext) {
    let (f, _) = running(cx).await;
    let row = f.text(cx, "jobs.row.0").unwrap();
    assert!(row.contains("staging-eu"), "{row}");
    assert!(row.contains("STAGING"), "{row}");
    assert!(row.contains("books.jsonl"), "{row}");
}

#[gpui_kit::test]
async fn first_error_shows_in_full(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    let long = "e".repeat(900);
    f.fake.progress(
        id,
        ImportProgress {
            first_error: Some(long.clone().into()),
            ..Default::default()
        },
    );
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert_eq!(f.text(cx, "jobs.error.0"), Some(long));
}

#[gpui_kit::test]
async fn cancel_button_and_shortcut_stop_the_job(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    f.click(cx, "jobs.cancel");
    assert_eq!(f.status(cx, id), JobStatus::Stopping);

    let second = f.start(cx);
    f.panel_focus(cx);
    f.click(cx, "jobs.row.1");
    assert!(f.has(cx, "jobs.cancel"));
    f.press(cx, CANCEL_JOB);
    assert_eq!(f.status(cx, second), JobStatus::Stopping);
}

#[gpui_kit::test]
async fn stopping_offers_stop_waiting(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    f.click(cx, "jobs.cancel");
    assert!(!f.has(cx, "jobs.cancel"));
    let tokens = cx.read(|cx| f.jobs.read(cx).jobs()[0].control.clone());
    assert!(!tokens.abort.is_cancelled());
    f.click(cx, "jobs.stop_waiting");
    assert!(tokens.abort.is_cancelled());
    assert_eq!(f.status(cx, id), JobStatus::Stopping);
}

#[gpui_kit::test]
async fn finished_shows_the_counts_and_the_note(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    let progress = ImportProgress {
        imported: 411_997,
        failed: 3,
        ..Default::default()
    };
    f.fake
        .finish(id, import_outcome(JobEnd::Finished, progress));
    cx.run_until_parked();
    let text = f.text(cx, "jobs.text.0").unwrap();
    assert!(text.contains("Imported 411,997"), "{text}");
    assert!(text.contains("Failed 3"), "{text}");
    assert!(f.has(cx, "jobs.stay_note"));
    assert!(f.has(cx, "jobs.count_unchecked"));
}

#[gpui_kit::test]
async fn count_warning_shows(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    let mut outcome = import_outcome(JobEnd::Finished, update(5));
    outcome.count = Some(CountCheck {
        before: 10,
        after: 19,
        ok: false,
    });
    f.fake.finish(id, outcome);
    cx.run_until_parked();
    let text = f.text(cx, "jobs.count_warning").unwrap();
    assert!(text.contains("grew by 9"), "{text}");
    assert!(text.contains("reported 5"), "{text}");
    assert!(!f.has(cx, "jobs.count_unchecked"));
}

#[gpui_kit::test]
async fn report_table_shows_rows_and_the_more_note(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    let rows = (1..=1005).map(failed_row).collect();
    let mut outcome = import_outcome(JobEnd::Finished, update(0));
    outcome.report = report_with(rows);
    f.fake.finish(id, outcome);
    cx.run_until_parked();
    let row = f.text(cx, "jobs.report.row.0").unwrap();
    assert!(row.contains("Field n of line 1 must"), "{row}");
    let more = f.text(cx, "jobs.report_more").unwrap();
    assert!(more.contains("1,000 of 1,005"), "{more}");
}

#[gpui_kit::test]
async fn failed_job_shows_error_inline(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    let err = TransferError::Io("disk full".into());
    let mut outcome = import_outcome(JobEnd::Failed(err), update(0));
    outcome.stopped_at_line = Some(42);
    f.fake.finish(id, outcome);
    cx.run_until_parked();
    let text = f.text(cx, "jobs.error.0").unwrap();
    assert!(text.contains("disk full"), "{text}");
    let line = f.text(cx, "jobs.stopped_at").unwrap();
    assert!(line.contains("42"), "{line}");
}

async fn finished_with_a_report(cx: &mut TestAppContext) -> (Fixture, JobId) {
    let (f, id) = running(cx).await;
    let mut outcome = import_outcome(JobEnd::Finished, update(0));
    outcome.report = report_with(vec![failed_row(1)]);
    f.fake.finish(id, outcome);
    cx.run_until_parked();
    (f, id)
}

#[gpui_kit::test]
async fn dismiss_of_unsaved_report_asks_with_focus_on_cancel(cx: &mut TestAppContext) {
    let (f, _) = finished_with_a_report(cx).await;
    f.click(cx, "jobs.dismiss");
    let focused = cx
        .update_window(f.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("confirm.cancel").focused()
        })
        .unwrap();
    assert_eq!(focused, Some(true));
    assert_eq!(cx.read(|cx| f.jobs.read(cx).jobs().len()), 1);
    f.click(cx, "confirm.cancel");
    assert_eq!(cx.read(|cx| f.jobs.read(cx).jobs().len()), 1);
    f.click(cx, "jobs.dismiss");
    f.click(cx, "confirm.ok");
    assert_eq!(cx.read(|cx| f.jobs.read(cx).jobs().len()), 0);
    assert!(f.has(cx, "jobs.empty"));
}

#[gpui_kit::test]
async fn dismiss_of_saved_job_does_not_ask(cx: &mut TestAppContext) {
    let (f, _) = finished_with_a_report(cx).await;
    *f.fake.saved_lines.lock().unwrap() = 1;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    f.click(cx, "jobs.save_report");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    f.click(cx, "jobs.dismiss");
    assert!(!f.has(cx, "confirm.cancel"));
    assert_eq!(cx.read(|cx| f.jobs.read(cx).jobs().len()), 0);
}

#[gpui_kit::test]
async fn no_toast_while_the_tab_is_visible(cx: &mut TestAppContext) {
    let (f, id) = running(cx).await;
    f.fake.finish(id, finished());
    cx.run_until_parked();
    assert_eq!(f.toasts(cx), 0);
}

#[gpui_kit::test]
async fn toast_when_it_is_hidden(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    f.fake.finish(id, finished());
    cx.run_until_parked();
    assert_eq!(f.toasts(cx), 1);
}

#[gpui_kit::test]
async fn save_report_uses_the_picked_path(cx: &mut TestAppContext) {
    let (f, _) = finished_with_a_report(cx).await;
    *f.fake.saved_lines.lock().unwrap() = 1;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    f.click(cx, "jobs.save_report");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert_eq!(*f.fake.saves.lock().unwrap(), [(path.clone(), false)]);
    let text = f.text(cx, "jobs.saved").unwrap();
    assert!(text.contains("Saved 1 lines"), "{text}");
    assert!(text.contains("r.jsonl"), "{text}");
}

#[gpui_kit::test]
async fn save_failure_shows_inline(cx: &mut TestAppContext) {
    let (f, _) = finished_with_a_report(cx).await;
    *f.fake.save_error.lock().unwrap() = Some("no room left".into());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    f.click(cx, "jobs.save_failed");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert_eq!(*f.fake.saves.lock().unwrap(), [(path, true)]);
    let text = f.text(cx, "jobs.saved").unwrap();
    assert!(text.contains("no room left"), "{text}");
}

#[gpui_kit::test]
async fn command_j_still_toggles_the_log(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.press(cx, TOGGLE_LOG);
    assert!(f.has(cx, "dock.bottom"));
    assert!(!f.has(cx, "jobs.panel"));
    f.press(cx, TOGGLE_LOG);
    assert!(!f.has(cx, "dock.bottom"));
}

#[gpui_kit::test]
async fn shift_command_j_opens_jobs(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.press(cx, TOGGLE_JOBS);
    assert!(f.has(cx, "jobs.panel"));
    assert!(f.has(cx, "jobs.empty"));
    assert!(!f.has(cx, "dock.bottom"));
}

#[gpui_kit::test]
async fn shift_command_j_closes_jobs(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.press(cx, TOGGLE_JOBS);
    f.press(cx, TOGGLE_JOBS);
    assert!(!f.has(cx, "jobs.panel"));
}

#[gpui_kit::test]
async fn command_j_on_jobs_tab_selects_the_log(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.press(cx, TOGGLE_JOBS);
    f.press(cx, TOGGLE_LOG);
    assert!(!f.has(cx, "jobs.panel"));
    assert!(f.has(cx, "dock.bottom"));
}

#[gpui_kit::test]
async fn start_opens_jobs_without_moving_focus(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    f.start(cx);
    let root = f.root(cx);
    let (before, after) = cx
        .update_window(f.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let before = window.focused(cx);
            root.update(cx, |root, cx| root.show_jobs(window, cx));
            window.render_frame(cx);
            (before, window.focused(cx))
        })
        .unwrap();
    assert_eq!(before, after);
    assert!(f.has(cx, "jobs.panel"));
}

#[gpui_kit::test]
async fn no_entry_without_jobs(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    assert!(!f.has(cx, "status.jobs"));
}

#[gpui_kit::test]
async fn count_shows_with_the_shortcut(cx: &mut TestAppContext) {
    let f = connected(cx).await;
    let id = f.start(cx);
    assert!(f.has(cx, "status.jobs"));
    let text = f.text(cx, "status.jobs").unwrap();
    assert!(text.contains("1 jobs"), "{text}");
    f.click(cx, "status.jobs");
    assert!(f.has(cx, "jobs.panel"));
    f.fake.finish(id, finished());
    cx.run_until_parked();
    f.click(cx, "jobs.dismiss");
    assert!(!f.has(cx, "status.jobs"));
}

#[gpui_kit::test]
async fn saved_failed_lines_offer_import(cx: &mut TestAppContext) {
    let (f, _) = finished_with_a_report(cx).await;
    assert!(!f.has(cx, "jobs.import_saved"));
    let (_dir, path) = {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("books.failed.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        (dir, path)
    };
    f.click(cx, "jobs.save_report");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert!(!f.has(cx, "jobs.import_saved"), "only for the failed lines");
    f.click(cx, "jobs.save_failed");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    f.click(cx, "jobs.import_saved");
    cx.wait_for(f.handle.into(), Duration::from_millis(5000), |window, _| {
        let source = window.try_find("transfer.import.source");
        source.is_some_and(|e| e.label().is_some_and(|l| l.contains("books.failed")))
    })
    .await;
}

async fn export_running(cx: &mut TestAppContext) -> (Fixture, JobId) {
    let f = connected(cx).await;
    let job = ExportJob {
        collection: "books".into(),
        scope: ExportScope::All,
        fields: FieldPick::All,
        format: ExportFormat::Csv,
        target: "books.csv".into(),
        expected: Some(10),
    };
    let conn = f.conn.clone();
    let id = cx.update(|cx| {
        f.jobs.update(cx, |jobs, cx| {
            jobs.start_export(&conn, job, "books.csv".into(), cx)
        })
    });
    cx.run_until_parked();
    f.press(cx, TOGGLE_JOBS);
    (f, id)
}

#[gpui_kit::test]
async fn finished_export_shows_unlisted_and_mismatch_warnings(cx: &mut TestAppContext) {
    let (f, id) = export_running(cx).await;
    f.fake
        .finish_export(id, export_outcome(JobEnd::Finished, 9, Some(10), 4));
    cx.run_until_parked();
    let unlisted = f.text(cx, "jobs.export_unlisted").unwrap();
    assert!(unlisted.starts_with("4 values were left out"), "{unlisted}");
    let mismatch = f.text(cx, "jobs.export_mismatch").unwrap();
    assert_eq!(
        mismatch,
        "Exported 9 of 10. Others may have written at the same time."
    );
}

#[gpui_kit::test]
async fn matching_export_shows_no_warning(cx: &mut TestAppContext) {
    let (f, id) = export_running(cx).await;
    f.fake
        .finish_export(id, export_outcome(JobEnd::Finished, 10, Some(10), 0));
    cx.run_until_parked();
    assert!(!f.has(cx, "jobs.export_unlisted"));
    assert!(!f.has(cx, "jobs.export_mismatch"));
}

#[gpui_kit::test]
async fn stopped_export_is_not_a_mismatch(cx: &mut TestAppContext) {
    let (f, id) = export_running(cx).await;
    f.fake
        .finish_export(id, export_outcome(JobEnd::Stopped, 3, Some(10), 0));
    cx.run_until_parked();
    assert!(!f.has(cx, "jobs.export_mismatch"));
}
