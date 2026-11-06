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
use tessera_app::actions::ToggleJobs;
use tessera_app::jobs::{JobKind, JobStatus, Jobs, JobsGlobal};
use tessera_app::state::{AppState, ResourceKind};
use tessera_client::{Collection, SetKind};
use tessera_core::backup::run::{
    BackupOutcome, BackupProgress, BackupStep, BackupTarget, BackupWarning,
};
use tessera_core::profile::Profile;
use tessera_core::transfer::JobEnd;
use uuid::Uuid;

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    jobs: Entity<Jobs>,
    fake: Arc<FakeTransfer>,
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
        fake_backend(cx, Some((collections(), Vec::new())));
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
