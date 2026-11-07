// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the Backup tab, through fake backends.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::json;
use std::sync::Arc;
use tessera_app::actions::{OpenSettings, ToggleJobs};
use tessera_app::jobs::{JobKind, JobStatus, Jobs, JobsGlobal};
use tessera_app::state::{AppState, ResourceKind};
use tessera_client::{Collection, SetKind, StopwordSet};
use tessera_core::backup::archive::ArchiveError;
use tessera_core::backup::format::{FORMAT, Manifest, ManifestCollection, ManifestProblem};
use tessera_core::backup::restore::{
    Loaded, RestoreError, RestoreOutcome, RestoreRow, RestoreView, RestoreWarning, RowFailure,
    RowKind, RowResult,
};
use tessera_core::backup::run::{
    BackupOutcome, BackupProgress, BackupStep, BackupTarget, BackupWarning,
};
use tessera_core::profile::Profile;
use tessera_core::schema::SchemaDraft;
use tessera_core::transfer::JobEnd;
use uuid::Uuid;

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    jobs: Entity<Jobs>,
    fake: Arc<FakeTransfer>,
    data: Arc<FakeData>,
    id: Uuid,
    _rt: tokio::runtime::Runtime,
}

fn collections() -> Vec<Collection> {
    let books = json!({"name": "books", "num_documents": 3, "fields": [
        {"name": "title", "type": "string"},
    ]});
    let hidden = json!({"name": "hidden", "num_documents": 1, "fields": [
        {"name": "secret_note", "type": "string", "store": false},
    ]});
    let embedded = json!({"name": "embedded", "num_documents": 1, "fields": [
        {"name": "v", "type": "float[]", "embed": {
            "from": ["title"],
            "model_config": {"model_name": "openai/x", "api_key": "sk-x"},
        }},
    ]});
    let linked = json!({"name": "linked", "num_documents": 1, "fields": [
        {"name": "author", "type": "string", "reference": "authors.id"},
    ]});
    [books, hidden, embedded, linked]
        .into_iter()
        .map(|c| serde_json::from_value(c).unwrap())
        .collect()
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, read_only: bool) -> Self {
        let data = fake_backend(cx, Some((collections(), Vec::new())));
        fake_documents(cx);
        install_search_fake(cx);
        fake_admin(cx);
        fake_connector(cx, |profile| Ok(server_for(profile)));
        let fake = fake_transfer(cx);
        let (staging, prod, secrets) = two_profiles();
        let profile: Profile = if read_only { prod } else { staging };
        let id = profile.id;
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        cx.update(|cx| {
            state.update(cx, |s, cx| s.open_resource(id, ResourceKind::Backup, cx));
        });
        wait_for_element(cx, handle, "backup.start".into(), true).await;
        wait_for_element(cx, handle, "backup.row.books".into(), true).await;
        let jobs = cx.read(|cx| cx.global::<JobsGlobal>().0.clone());
        Self {
            handle,
            state,
            jobs,
            fake,
            data,
            id,
            _rt: rt,
        }
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.click(id.to_string(), cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            window.try_find(id.to_string()).is_some()
        })
        .unwrap()
    }

    fn label(&self, cx: &mut TestAppContext, id: &str) -> String {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            let found = window.try_find(id.to_string());
            found.and_then(|e| e.label().map(ToString::to_string))
        })
        .unwrap()
        .unwrap_or_default()
    }

    /// Every warning on screen.
    fn warnings(&self, cx: &mut TestAppContext) -> String {
        let lines = (0..8).map(|i| self.label(cx, &format!("backup.warning.{i}")));
        lines.collect::<Vec<_>>().join("\n")
    }

    fn start_to(&self, cx: &mut TestAppContext, path: PathBuf) {
        self.click(cx, "backup.start");
        cx.simulate_new_path_selection(move |_| Some(path));
        self.settle(cx);
    }

    fn close_tab(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.dispatch_action(Box::new(gpui_kit::component::dock::ClosePanel), cx);
        })
        .unwrap();
        self.settle(cx);
    }
}

#[gpui_kit::test]
async fn start_without_a_selection_does_nothing(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.click(cx, "backup.start");
    assert!(!cx.did_prompt_for_paths());
    f.settle(cx);
    assert!(f.fake.backups.lock().unwrap().is_empty());

    f.click(cx, "backup.row.books");
    f.click(cx, "backup.row.books");
    f.click(cx, "backup.start");
    assert!(f.fake.backups.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn all_ticks_every_collection(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.click(cx, "backup.all");
    let dir = tempfile::tempdir().unwrap();
    f.start_to(cx, dir.path().join("b"));
    let plans = f.fake.backups.lock().unwrap();
    assert_eq!(plans[0].collections.len(), 4);
    assert_eq!(plans[0].target, BackupTarget::Folder(dir.path().join("b")));
}

#[gpui_kit::test]
async fn each_warning_shows_for_its_selection(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    assert_eq!(f.warnings(cx), "\n\n\n\n\n\n\n");

    f.click(cx, "backup.row.hidden");
    assert!(f.warnings(cx).contains("secret_note"));
    f.click(cx, "backup.row.hidden");

    f.click(cx, "backup.row.embedded");
    let text = f.warnings(cx);
    assert!(
        text.contains("embedded") && text.contains("blocked"),
        "{text}"
    );
    assert!(!text.contains("sk-x"), "{text}");
    f.click(cx, "backup.row.embedded");

    f.click(cx, "backup.row.linked");
    assert!(f.warnings(cx).contains("authors"));
}

#[gpui_kit::test]
async fn the_archive_format_goes_to_the_job(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.click(cx, "backup.row.books");
    f.settle(cx);
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.within("backup.format").click(1, cx);
    })
    .unwrap();
    f.settle(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b.tar.zst");
    f.start_to(cx, path.clone());
    let plans = f.fake.backups.lock().unwrap();
    assert_eq!(plans[0].target, BackupTarget::Archive(path));
}

#[gpui_kit::test]
async fn a_target_that_exists_is_refused_inline(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.click(cx, "backup.row.books");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("taken");
    std::fs::write(&path, "x").unwrap();
    f.start_to(cx, path);
    assert!(f.label(cx, "backup.error").contains("already exists"));
    assert!(f.fake.backups.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[gpui_kit::test]
async fn a_link_as_target_is_refused_inline(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.click(cx, "backup.row.books");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("nowhere"), &path).unwrap();
    f.start_to(cx, path);
    assert!(f.label(cx, "backup.error").contains("already exists"));
    assert!(f.fake.backups.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn a_job_goes_on_with_the_tab_closed(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.click(cx, "backup.row.books");
    let dir = tempfile::tempdir().unwrap();
    f.start_to(cx, dir.path().join("b"));
    let job = cx.read(|cx| {
        let jobs = f.jobs.read(cx).jobs();
        assert_eq!(jobs[0].kind, JobKind::Backup);
        assert_eq!(jobs[0].collection, "books");
        jobs[0].id
    });

    f.close_tab(cx);
    assert!(!f.has(cx, "backup.start"));
    f.fake.backup_progress(
        job,
        BackupProgress {
            step: BackupStep::Collections,
            done: 0,
            total: 1,
            collection: "books".into(),
            ..BackupProgress::default()
        },
    );
    cx.executor().advance_clock(Duration::from_millis(100));
    f.settle(cx);
    cx.read(|cx| assert!(f.jobs.read(cx).jobs()[0].is_running()));
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.dispatch_action(Box::new(ToggleJobs), cx);
    })
    .unwrap();
    wait_for_element(cx, f.handle, "jobs.text.0".into(), true).await;
    let row = f.label(cx, "jobs.text.0");
    assert!(row.contains("Exporting · books · 0 of 1"), "{row}");

    cx.update(|cx| f.jobs.update(cx, |jobs, cx| jobs.stop(job, cx)));
    cx.read(|cx| assert_eq!(f.jobs.read(cx).jobs()[0].status, JobStatus::Stopping));
    f.fake.finish_backup(
        job,
        BackupOutcome {
            end: JobEnd::Stopped,
            collections: Vec::new(),
            warnings: vec![BackupWarning::BlankedSetSecrets {
                kind: SetKind::Curation,
                name: "cur".into(),
                count: 1,
            }],
        },
    );
    f.settle(cx);
    cx.read(|cx| assert_eq!(f.jobs.read(cx).jobs()[0].status, JobStatus::Stopped));
    f.click(cx, "jobs.row.0");
    assert!(f.label(cx, "jobs.backup_warning.0").contains("cur"));
}

#[gpui_kit::test]
async fn a_read_only_connection_needs_no_unlock(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, true).await;
    f.click(cx, "backup.row.books");
    let dir = tempfile::tempdir().unwrap();
    f.start_to(cx, dir.path().join("b"));
    assert_eq!(f.fake.backups.lock().unwrap().len(), 1);
    assert!(!f.has(cx, "unlocked.banner"));
    cx.read(|cx| {
        let connection = f.state.read(cx).connection(f.id, cx).unwrap();
        assert!(connection.read(cx).writes_locked());
    });
}

// The Restore view.

/// A folder that `verify` passes, a manifest of `names` and their schemas.
struct BackupFiles {
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// The backend answers with a backup of `names`, on a server that holds `server`.
    fn offer(&self, names: &[&str], blanked: &[&str], server: RestoreView) -> BackupFiles {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<ManifestCollection> = names
            .iter()
            .enumerate()
            .map(|(i, name)| ManifestCollection {
                file: i as u32 + 1,
                name: name.to_string(),
                documents: 7,
                unstored: Vec::new(),
                blanked_secrets: usize::from(blanked.contains(name)),
                dictionaries: Vec::new(),
            })
            .collect();
        let schemas = names.iter().map(|name| {
            SchemaDraft::from_value(&json!({"name": name, "fields": [
                {"name": "t", "type": "string"},
            ]}))
            .unwrap()
        });
        let manifest = Manifest {
            format: FORMAT,
            created_at: 0,
            tessera_version: "0".into(),
            server_version: "30.2".into(),
            collections: entries,
            files: Vec::new(),
        };
        *self.fake.open_answer.lock().unwrap() = Some(Ok((dir.path().to_path_buf(), manifest)));
        *self.fake.load_answer.lock().unwrap() = Some(Ok(Loaded {
            schemas: schemas.collect(),
            stopwords: vec![StopwordSet {
                id: "s".into(),
                stopwords: vec!["a".into()],
                locale: None,
            }],
            ..Loaded::default()
        }));
        *self.fake.server.lock().unwrap() = server;
        BackupFiles { _dir: dir }
    }

    /// Opens the Restore view and chooses a backup.
    fn choose(&self, cx: &mut TestAppContext) {
        self.click(cx, "backup.view.restore");
        self.click(cx, "restore.choose");
        cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/b.tar.zst")]));
        self.settle(cx);
    }

    fn restores(&self) -> usize {
        self.fake.restores.lock().unwrap().len()
    }

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            fill(window, id, text, cx);
        })
        .unwrap();
        self.settle(cx);
    }
}

fn differing() -> RestoreView {
    RestoreView {
        stopwords: vec![StopwordSet {
            id: "s".into(),
            stopwords: vec!["other".into()],
            locale: None,
        }],
        ..RestoreView::default()
    }
}

#[gpui_kit::test]
async fn the_views_and_choose_answer_their_keys(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    assert!(!f.has(cx, "restore.choose"));
    let press = |f: &Fixture, cx: &mut TestAppContext, keys: &str| {
        cx.update_window(f.handle.into(), |_, window, cx| window.press(keys, cx))
            .unwrap();
        f.settle(cx);
    };
    press(&f, cx, key("cmd-2", "ctrl-2"));
    assert!(f.has(cx, "restore.choose"));
    assert!(!f.has(cx, "backup.start"));
    press(&f, cx, key("cmd-o", "ctrl-o"));
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/b.tar.zst")]));
    f.settle(cx);
    assert_eq!(f.fake.opened_paths.lock().unwrap().len(), 1);
    press(&f, cx, key("cmd-1", "ctrl-1"));
    assert!(f.has(cx, "backup.start"));
}

#[gpui_kit::test]
async fn a_blocked_collection_cannot_be_ticked(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let server = RestoreView {
        collections: vec!["books".into()],
        ..RestoreView::default()
    };
    let _files = f.offer(&["books", "fresh"], &[], server);
    f.choose(cx);
    assert!(
        f.label(cx, "restore.row.Collection.books")
            .contains("server")
    );
    assert!(!f.has(cx, "restore.row.Collection.books.tick"));
    assert!(f.has(cx, "restore.row.Collection.fresh.tick"));
}

#[gpui_kit::test]
async fn blanked_keys_say_plainly_what_the_user_has_to_do(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["embedded"], &["embedded"], RestoreView::default());
    f.choose(cx);
    let text = f.label(cx, "restore.row.Collection.embedded");
    assert!(text.contains("stays blocked"), "{text}");
    assert!(text.contains("no way to enter"), "{text}");
    assert!(text.contains("collections/0001.schema.json"), "{text}");
    assert!(text.contains("collections/0001.jsonl"), "{text}");
    assert!(!f.has(cx, "restore.row.Collection.embedded.tick"));
}

#[gpui_kit::test]
async fn replace_off_keeps_a_differing_row(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], differing());
    f.choose(cx);
    f.click(cx, "restore.row.Stopwords.s.tick");
    f.click(cx, "restore.start");
    f.click(cx, "confirm.ok");
    let jobs = f.fake.restores.lock().unwrap();
    assert!(jobs[0].replace.is_empty());
    assert!(
        !jobs[0]
            .ticked
            .contains(&(RowKind::Stopwords, "s".to_string()))
    );
}

#[gpui_kit::test]
async fn replace_ticks_what_differs_and_needs_the_name(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], differing());
    f.choose(cx);
    f.click(cx, "restore.replace");
    f.click(cx, "restore.start");
    assert!(f.has(cx, "danger.input"), "a replace is typed");
    f.click(cx, "danger.ok");
    assert_eq!(f.restores(), 0);
    let name = cx.read(|cx| {
        let connection = f.state.read(cx).connection(f.id, cx).unwrap();
        connection.read(cx).profile.name.clone()
    });
    f.fill(cx, "danger.input", &name);
    f.click(cx, "danger.ok");
    let jobs = f.fake.restores.lock().unwrap();
    let approved = (RowKind::Stopwords, "s".to_string());
    assert!(jobs[0].replace.contains(&approved));
    assert!(
        jobs[0]
            .ticked
            .contains(&(RowKind::Stopwords, "s".to_string()))
    );
}

#[gpui_kit::test]
async fn a_plain_restore_counts_what_it_does_and_cancel_writes_nothing(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    f.choose(cx);
    f.click(cx, "restore.start");
    assert!(!f.has(cx, "danger.input"));
    assert!(f.has(cx, "confirm.ok"));
    f.click(cx, "confirm.cancel");
    assert_eq!(f.restores(), 0);
    assert!(!f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn read_only_unlocks_first_and_prod_is_typed(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, true).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    f.choose(cx);
    f.click(cx, "restore.start");
    assert!(f.has(cx, "danger.input"), "the unlock dialog opens");
    assert_eq!(f.restores(), 0);
    f.fill(cx, "danger.input", "prod-ch");
    f.click(cx, "danger.ok");
    assert!(f.has(cx, "danger.input"), "prod asks for the name as well");
    assert_eq!(f.restores(), 0);
    f.fill(cx, "danger.input", "prod-ch");
    f.click(cx, "danger.ok");
    assert_eq!(f.restores(), 1);
}

#[gpui_kit::test]
async fn a_bad_manifest_shows_inline_in_full(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    *f.fake.open_answer.lock().unwrap() =
        Some(Err(ArchiveError::Manifest(ManifestProblem::Format(9))));
    f.choose(cx);
    assert!(f.label(cx, "restore.error").contains("format 9"));
    assert!(!f.has(cx, "restore.start"));
}

#[gpui_kit::test]
async fn a_stage_shows_and_stop_ends_it(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    f.choose(cx);
    assert!(f.label(cx, "restore.stage").contains("Unpacking"));
    f.click(cx, "restore.stop");
    assert!(!f.has(cx, "restore.stage"));
    assert!(f.has(cx, "restore.choose"));
}

#[gpui_kit::test]
async fn the_job_survives_the_tab_and_the_tab_shows_what_it_did(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    f.choose(cx);
    f.click(cx, "restore.start");
    f.click(cx, "confirm.ok");
    let job = cx.read(|cx| f.jobs.read(cx).jobs()[0].id);
    let outcome = || RestoreOutcome {
        end: JobEnd::Failed(RestoreError::Row {
            kind: RowKind::Collection,
            name: "fresh".into(),
        }),
        rows: vec![RestoreRow {
            kind: RowKind::Collection,
            name: "fresh".into(),
            result: RowResult::Failed(RowFailure::Lines {
                failed: 2,
                unconfirmed: 0,
                first_error: Some("Referenced collection not found.".into()),
            }),
        }],
        to_delete: vec!["fresh".into()],
        warnings: Vec::new(),
    };
    f.fake.finish_restore(job, outcome());
    f.settle(cx);
    let text = f.label(cx, "restore.result.Collection.fresh");
    assert!(text.contains("Referenced collection not found."), "{text}");
    assert!(f.label(cx, "restore.to_delete").contains("fresh"));

    f.click(cx, "restore.start");
    f.click(cx, "confirm.ok");
    let job = cx.read(|cx| f.jobs.read(cx).jobs()[1].id);
    f.close_tab(cx);
    assert!(!f.has(cx, "restore.start"));
    cx.read(|cx| assert!(f.jobs.read(cx).jobs()[1].is_running()));
    f.fake.finish_restore(job, outcome());
    f.settle(cx);
    cx.read(|cx| {
        let status = &f.jobs.read(cx).jobs()[1].status;
        assert!(
            matches!(status, JobStatus::Failed(t) if t.contains("not found")),
            "{status:?}"
        );
    });
}

/// What a finished run reports.
fn finished_run() -> RestoreOutcome {
    RestoreOutcome {
        end: JobEnd::Finished,
        rows: vec![RestoreRow {
            kind: RowKind::Collection,
            name: "fresh".into(),
            result: RowResult::Done,
        }],
        to_delete: vec!["fresh".into()],
        warnings: vec![RestoreWarning::AliasShadowed("shop".into())],
    }
}

impl Fixture {
    /// Starts a restore of `fresh`, and returns its job.
    fn start_restore(&self, cx: &mut TestAppContext) -> tessera_app::jobs::JobId {
        self.choose(cx);
        self.click(cx, "restore.start");
        self.click(cx, "confirm.ok");
        cx.read(|cx| self.jobs.read(cx).jobs().last().unwrap().id)
    }

    fn open_jobs(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.dispatch_action(Box::new(ToggleJobs), cx);
        })
        .unwrap();
        self.settle(cx);
        self.click(cx, "jobs.row.0");
    }
}

#[gpui_kit::test]
async fn a_finished_run_does_not_ask_to_delete_what_it_made(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let job = f.start_restore(cx);
    f.fake.finish_restore(job, finished_run());
    f.settle(cx);
    assert!(f.has(cx, "restore.result.Collection.fresh"));
    assert!(!f.has(cx, "restore.to_delete"));
    f.open_jobs(cx);
    assert!(f.has(cx, "jobs.restore_row.0"));
    assert!(!f.has(cx, "restore.job_to_delete"));
}

#[gpui_kit::test]
async fn jobs_show_rows_and_warnings_with_the_tab_closed(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let job = f.start_restore(cx);
    f.close_tab(cx);
    let mut outcome = finished_run();
    outcome.end = JobEnd::Stopped;
    outcome.rows[0].result = RowResult::Failed(RowFailure::Stopped);
    f.fake.finish_restore(job, outcome);
    f.settle(cx);
    f.open_jobs(cx);
    let row = f.label(cx, "jobs.restore_row.0");
    assert!(
        row.contains("fresh") && row.contains("Stopped partway"),
        "{row}"
    );
    assert!(f.label(cx, "jobs.restore_warning.0").contains("shop"));
    assert!(f.label(cx, "restore.job_to_delete").contains("fresh"));
}

#[gpui_kit::test]
async fn a_disconnect_during_a_restore_is_named(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, true).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let connection = cx.read(|cx| f.state.read(cx).connection(f.id, cx).unwrap());
    cx.update(|cx| connection.update(cx, |c, cx| c.unlock(cx)));
    f.choose(cx);
    f.click(cx, "restore.start");
    f.fill(cx, "danger.input", "prod-ch");
    f.click(cx, "danger.ok");
    let job = cx.read(|cx| f.jobs.read(cx).jobs()[0].id);
    cx.update(|cx| f.state.update(cx, |s, cx| s.disconnect(f.id, cx)));
    let failure: RowFailure = tessera_client::Error::ReadOnly {
        method: tessera_client::Method::POST,
    }
    .into();
    let outcome = RestoreOutcome {
        end: JobEnd::Failed(RestoreError::Row {
            kind: RowKind::Collection,
            name: "fresh".into(),
        }),
        rows: vec![RestoreRow {
            kind: RowKind::Collection,
            name: "fresh".into(),
            result: RowResult::Failed(failure),
        }],
        to_delete: Vec::new(),
        warnings: Vec::new(),
    };
    f.fake.finish_restore(job, outcome);
    f.settle(cx);
    cx.read(|cx| {
        let status = &f.jobs.read(cx).jobs()[0].status;
        assert!(
            matches!(status, JobStatus::Failed(t) if t.contains("Locked by disconnect")),
            "{status:?}"
        );
    });
}

#[gpui_kit::test]
async fn a_child_cannot_start_without_its_parent(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["child", "parent"], &[], RestoreView::default());
    let reference = SchemaDraft::from_value(&json!({"name": "child", "fields": [
        {"name": "p", "type": "string", "reference": "parent.id"},
    ]}));
    if let Some(Ok(loaded)) = f.fake.load_answer.lock().unwrap().as_mut() {
        loaded.schemas[0] = reference.unwrap();
    }
    f.choose(cx);
    assert!(!f.has(cx, "restore.unmet"));
    f.click(cx, "restore.row.Collection.parent.tick");
    assert!(f.label(cx, "restore.unmet").contains("child → parent"));
    f.click(cx, "restore.start");
    assert!(!f.has(cx, "confirm.ok"), "Start is off");
    f.click(cx, "restore.row.Collection.child.tick");
    assert!(!f.has(cx, "restore.unmet"));
}

#[gpui_kit::test]
async fn the_plan_is_read_again_when_the_job_ends(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let job = f.start_restore(cx);
    let tick = "restore.row.Collection.fresh.tick";
    assert!(f.has(cx, tick));
    f.fake.server.lock().unwrap().collections = vec!["fresh".into()];
    f.fake.finish_restore(job, finished_run());
    f.settle(cx);
    assert!(!f.has(cx, tick), "the row is blocked now");
    assert!(
        f.has(cx, "restore.result.Collection.fresh"),
        "the result stays"
    );
}

#[gpui_kit::test]
async fn rows_past_the_limit_are_counted_when_they_are_restored(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let names: Vec<String> = (0..1005).map(|i| format!("c{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    // The stopwords row comes first, so 1,000 of its 1,006 rows are drawn. A blocked row stays out.
    let _files = f.offer(&names, &["c1004"], RestoreView::default());
    f.choose(cx);
    assert!(f.label(cx, "restore.rows_unseen").ends_with(": 5"));
}

#[gpui_kit::test]
async fn an_archive_is_held_while_it_unpacks(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let temp = Arc::new(tempfile::tempdir().unwrap());
    let path = temp.path().to_path_buf();
    *f.fake.temp_answer.lock().unwrap() = Some(temp);
    // No answer holds the unpack open.
    f.choose(cx);
    cx.read(|cx| assert_eq!(f.jobs.read(cx).held_temps(), [path]));
}

#[gpui_kit::test]
async fn clear_cache_keeps_a_held_directory_and_reloads_the_lists(cx: &mut TestAppContext) {
    let relevance = fake_relevance(cx);
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let temp = Arc::new(tempfile::tempdir().unwrap());
    let path = temp.path().to_path_buf();
    *f.fake.temp_answer.lock().unwrap() = Some(temp);
    f.choose(cx);
    let lists = *f.data.lists.lock().unwrap();
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.dispatch_action(Box::new(OpenSettings), cx);
    })
    .unwrap();
    f.settle(cx);
    for _ in 0..14 {
        cx.update_window(f.handle.into(), |_, window, cx| window.press("tab", cx))
            .unwrap();
        f.settle(cx);
    }
    f.click(cx, "settings.clear_cache.button");
    assert!(relevance.held.lock().unwrap().is_empty(), "it asks first");
    f.click(cx, "confirm.ok");
    assert_eq!(relevance.held.lock().unwrap().as_slice(), [vec![path]]);
    assert!(f.label(cx, "settings.clear_cache.note").contains('2'));
    assert!(*f.data.lists.lock().unwrap() > lists);
}

#[gpui_kit::test]
async fn a_restore_takes_the_batch_size_of_the_settings(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    cx.update(|cx| {
        let settings = cx.update_entity(&f.state, |s, _| {
            s.settings.batch_size = 77;
            s.settings.clone()
        });
        cx.update_window(f.handle.into(), |_, window, cx| {
            tessera_app::settings::apply(&settings, window, cx);
        })
        .unwrap();
    });
    f.start_restore(cx);
    assert_eq!(f.fake.restores.lock().unwrap()[0].batch_size, 77);
}

#[gpui_kit::test]
async fn a_new_choice_lets_go_of_the_old_temp_directory(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let temp = Arc::new(tempfile::tempdir().unwrap());
    let path = temp.path().to_path_buf();
    *f.fake.temp_answer.lock().unwrap() = Some(temp);
    f.choose(cx);
    assert!(f.has(cx, "restore.start"));
    f.click(cx, "restore.choose");
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/other")]));
    f.settle(cx);
    assert_eq!(f.fake.discarded.lock().unwrap().as_slice(), [path]);
}

#[gpui_kit::test]
async fn closing_the_tab_lets_go_of_the_temp_directory(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    let temp = Arc::new(tempfile::tempdir().unwrap());
    let path = temp.path().to_path_buf();
    *f.fake.temp_answer.lock().unwrap() = Some(temp);
    f.choose(cx);
    assert!(f.has(cx, "restore.start"));
    f.close_tab(cx);
    assert_eq!(f.fake.discarded.lock().unwrap().as_slice(), [path]);
}

#[gpui_kit::test]
async fn the_collection_list_reloads_after_a_restore(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, false).await;
    let _files = f.offer(&["fresh"], &[], RestoreView::default());
    f.choose(cx);
    f.click(cx, "restore.start");
    f.click(cx, "confirm.ok");
    let job = cx.read(|cx| f.jobs.read(cx).jobs()[0].id);
    let lists = *f.data.lists.lock().unwrap();
    f.fake.finish_restore(
        job,
        RestoreOutcome {
            end: JobEnd::Finished,
            rows: Vec::new(),
            to_delete: Vec::new(),
            warnings: Vec::new(),
        },
    );
    f.settle(cx);
    assert!(*f.data.lists.lock().unwrap() > lists);
}
