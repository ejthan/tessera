// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of unlocking writes on a read-only connection (D10).

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use tessera_app::collections::CollectionsBackendGlobal;
use tessera_app::connection::ConnectionState;
use tessera_app::state::AppState;
use tessera_core::profile::Profile;
use tessera_core::store::{ConnectionsFile, save};

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    conn: Entity<ConnectionState>,
    prod: Profile,
    backend: Arc<FakeData>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    /// Connects the read-only prod profile through the sidebar.
    async fn new(cx: &mut TestAppContext) -> Self {
        Self::at(cx, None).await
    }

    async fn at(cx: &mut TestAppContext, path: Option<std::path::PathBuf>) -> Self {
        fake_connector(cx, |p| Ok(server_for(p)));
        let backend = fake_backend(cx, Some((Vec::new(), Vec::new())));
        let (staging, prod, secrets) = two_profiles();
        let rt = tokio(cx);
        let file = ConnectionsFile {
            profiles: vec![staging, prod.clone()],
            ..Default::default()
        };
        if let Some(path) = &path {
            save(path, &file).unwrap();
        }
        let (handle, state) = open_state_at(cx, Ok(file), path, secrets);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(entry(&prod), cx);
        })
        .unwrap();
        wait_for_label(cx, handle, "status.text".into(), "prod-ch").await;
        let conn = cx.update(|cx| state.read(cx).connection(prod.id, cx).unwrap());
        Self {
            handle,
            state,
            conn,
            prod,
            backend,
            _rt: rt,
        }
    }

    /// A "write": asks the backend for a collection once writes are allowed.
    fn write(&self, cx: &mut TestAppContext) {
        let client = cx.update(|cx| self.conn.read(cx).connected.clone().unwrap().client);
        let conn = self.conn.clone();
        cx.update_window(self.handle.into(), |_, window, cx| {
            conn.update(cx, |c, cx| {
                c.ensure_writable(window, cx, move |_, cx| {
                    let backend = cx.global::<CollectionsBackendGlobal>().0.clone();
                    drop(backend.get(client, "written".into()));
                });
            });
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn writes(&self) -> usize {
        self.backend.gets.lock().unwrap().len()
    }

    async fn type_name_and_confirm(&self, cx: &mut TestAppContext) {
        wait_for_element(cx, self.handle, "danger.input".into(), true).await;
        let name = self.prod.name.clone();
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.click("danger.input".to_string(), cx);
            window.input(&name, cx);
            window.render_frame(cx);
            window.click("danger.ok".to_string(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        wait_for_element(cx, self.handle, "danger.input".into(), false).await;
    }

    fn shown(&self, cx: &mut TestAppContext, id: String) -> bool {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    }
}

#[gpui_kit::test]
async fn write_on_read_only_opens_unlock_and_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.write(cx);
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    assert_eq!(f.writes(), 0);
    assert!(cx.update(|cx| {
        f.conn
            .read(cx)
            .connected
            .clone()
            .unwrap()
            .client
            .is_read_only()
    }));
    assert!(!f.shown(cx, "unlocked.banner".into()));
    cx.update_window(f.handle.into(), |_, window, cx| {
        let text = label(window, "danger.consequence.0");
        assert!(text.contains("prod-ch (PROD)"), "{text}");
        window.click("danger.cancel".to_string(), cx);
    })
    .unwrap();
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    assert_eq!(f.writes(), 0);
}

#[gpui_kit::test]
async fn typing_the_name_unlocks_and_continues(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.write(cx);
    f.type_name_and_confirm(cx).await;
    assert_eq!(f.writes(), 1);
    cx.update(|cx| {
        let c = f.conn.read(cx);
        assert!(c.writes_unlocked);
        assert!(!c.connected.as_ref().unwrap().client.is_read_only());
    });
    // Unlocked: the next write goes straight through.
    f.write(cx);
    assert_eq!(f.writes(), 2);
    assert!(!f.shown(cx, "danger.input".into()));
}

#[gpui_kit::test]
async fn banner_and_open_lock_show_while_unlocked(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    let id = f.prod.id;
    let locks = [
        format!("{}.readonly", entry(&f.prod)),
        "titlebar.readonly".to_string(),
    ];
    for lock in &locks {
        assert!(f.shown(cx, lock.clone()), "{lock}");
    }
    assert!(!f.shown(cx, "unlocked.banner".into()));
    f.write(cx);
    f.type_name_and_confirm(cx).await;
    assert!(f.shown(cx, "unlocked.banner".into()));
    for lock in &locks {
        assert!(!f.shown(cx, lock.clone()), "{lock}");
    }
    for open in [
        format!("{}.unlocked", entry(&f.prod)),
        "titlebar.unlocked".to_string(),
        format!("tab.{id}.overview.unlocked"),
    ] {
        assert!(f.shown(cx, open.clone()), "{open}");
    }
}

#[gpui_kit::test]
async fn lock_again_restores_the_guard(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.write(cx);
    f.type_name_and_confirm(cx).await;
    assert_eq!(f.writes(), 1);
    // Focus is in the tab, so the shortcut of its key context is dispatched.
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("unlocked.lock".to_string(), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!f.shown(cx, "unlocked.banner".into()));
    cx.update(|cx| {
        assert!(
            f.conn
                .read(cx)
                .connected
                .as_ref()
                .unwrap()
                .client
                .is_read_only()
        )
    });
    f.write(cx);
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    assert_eq!(f.writes(), 1);
}

#[gpui_kit::test]
async fn lock_again_shortcut_works_from_the_tab(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.write(cx);
    f.type_name_and_confirm(cx).await;
    let tab = format!("tab.{}.overview", f.prod.id);
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(tab, cx);
        window.press(LOCK_AGAIN, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!f.shown(cx, "unlocked.banner".into()));
    cx.update(|cx| assert!(!f.conn.read(cx).writes_unlocked));
}

#[gpui_kit::test]
async fn disconnect_locks(cx: &mut TestAppContext) {
    let f = Fixture::new(cx).await;
    f.write(cx);
    f.type_name_and_confirm(cx).await;
    let client = cx.update(|cx| f.conn.read(cx).connected.clone().unwrap().client);
    assert!(!client.is_read_only());
    let id = f.prod.id;
    cx.update(|cx| f.state.update(cx, |s, cx| s.disconnect(id, cx)));
    cx.run_until_parked();
    assert!(client.is_read_only());
    cx.update(|cx| assert!(!f.conn.read(cx).writes_unlocked));
}

#[gpui_kit::test]
async fn unlock_is_never_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let f = Fixture::at(cx, Some(path.clone())).await;
    let before = std::fs::read(&path).unwrap();
    f.write(cx);
    f.type_name_and_confirm(cx).await;
    cx.update(|cx| assert!(f.conn.read(cx).writes_unlocked));
    // Let any save that was queued finish.
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
