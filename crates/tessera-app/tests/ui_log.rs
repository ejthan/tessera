// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the request log in headless GPUI windows.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::TestAppContext;
use secrecy::SecretString;
use tessera_app::connection::ConnectorGlobal;
use tessera_client::{Method, Origin, RequestEvent};
use tessera_core::log::{Filter, LogEntry};
use tessera_core::profile::Profile;
use tessera_core::secrets::{MemoryStore, SecretStore};
use uuid::Uuid;

use common::*;

#[gpui_kit::test]
async fn events_reach_the_log(cx: &mut TestAppContext) {
    let (_handle, state, _rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    let tx = state.read_with(cx, |s, _| s.log_sender());
    for id in 0..2 {
        tx.send(LogEntry {
            connection: Uuid::nil(),
            connection_name: "dev".into(),
            event: RequestEvent {
                id,
                started_at: std::time::SystemTime::UNIX_EPOCH,
                node: "http://localhost:8108".parse().unwrap(),
                method: Method::GET,
                path: "/health".into(),
                query: None,
                request_headers: vec![],
                key_prefix: String::new(),
                request_body: None,
                response_body: None,
                status: Some(200),
                duration: Duration::ZERO,
                size: None,
                origin: Origin::User,
                error: None,
            },
        })
        .unwrap();
    }
    cx.run_until_parked();
    let n = state.read_with(cx, |s, _| s.log().entries(&Filter::default()).len());
    assert_eq!(n, 2);
}

#[gpui_kit::test]
async fn connect_hook_logs_with_profile_id_and_name(cx: &mut TestAppContext) {
    let (staging, _, _) = two_profiles();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(staging.id, &SecretString::from("k")).unwrap();
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, _, hook| {
            hook(&RequestEvent {
                id: 1,
                started_at: std::time::SystemTime::UNIX_EPOCH,
                node: "http://localhost:8108".parse().unwrap(),
                method: Method::GET,
                path: "/health".into(),
                query: None,
                request_headers: vec![],
                key_prefix: String::new(),
                request_body: None,
                response_body: None,
                status: Some(200),
                duration: Duration::ZERO,
                size: None,
                origin: Origin::User,
                error: None,
            });
            let info = server("30.2", true);
            Box::pin(async move { Ok(info) })
        })));
    });
    let (handle, state, _rt) = open_app(cx, vec![staging.clone()], secrets);
    state.update(cx, |s, cx| s.connect(staging.id, None, cx));
    wait_for_label(cx, handle, "status.text".into(), "staging-eu").await;
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        let entries = s.log().entries(&Filter::default());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].connection, staging.id);
        assert_eq!(&*entries[0].connection_name, "staging-eu");
    });
}
