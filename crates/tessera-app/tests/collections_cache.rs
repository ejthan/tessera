// SPDX-License-Identifier: GPL-3.0-or-later

//! The collection cache on the connection state, through a fake backend.

#![allow(clippy::unwrap_used)]

mod common;

use gpui_kit::{AppContext as _, TestAppContext};
use tessera_app::connection::ConnectionState;
use tessera_client::{Alias, Collection};
use tessera_core::profile::Profile;

use common::*;

fn books() -> (Vec<Collection>, Vec<Alias>) {
    let collection = Collection {
        name: "books".into(),
        ..Default::default()
    };
    let alias = Alias {
        name: "library".into(),
        collection_name: "books".into(),
    };
    (vec![collection], vec![alias])
}

fn connected_state(cx: &mut TestAppContext) -> gpui_kit::Entity<ConnectionState> {
    let profile = Profile::new("t", vec![node()]);
    let connected = server_for(&profile);
    cx.new(|_| {
        let mut state = ConnectionState::new(profile);
        state.connected = Some(connected);
        state
    })
}

#[gpui_kit::test]
fn cache_load_fills_list_and_aliases(cx: &mut TestAppContext) {
    open(cx);
    let backend = fake_backend(cx, Some(books()));
    let state = connected_state(cx);
    state.update(cx, |s, cx| s.load_collection_list(cx));
    assert!(state.read_with(cx, |s, _| s.collection_cache.loading));
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        let cache = &s.collection_cache;
        assert!(!cache.loading);
        assert_eq!(cache.collections.as_ref().unwrap()[0].name, "books");
        assert_eq!(cache.aliases[0].name, "library");
        assert!(cache.loaded_at.is_some() && cache.error.is_none());
    });
    assert_eq!(*backend.lists.lock().unwrap(), 1);
}

#[gpui_kit::test]
fn cache_error_is_kept_inline(cx: &mut TestAppContext) {
    open(cx);
    let backend = fake_backend(cx, Some(books()));
    let state = connected_state(cx);
    state.update(cx, |s, cx| s.load_collection_list(cx));
    cx.run_until_parked();
    *backend.data.lock().unwrap() = None;
    state.update(cx, |s, cx| s.load_collection_list(cx));
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        let cache = &s.collection_cache;
        assert!(!cache.loading);
        let error = cache.error.as_ref().expect("the failure is shown inline");
        assert!(
            error.contains("localhost"),
            "full text names the host: {error}"
        );
        assert!(cache.collections.is_some(), "the old list stays");
    });
}

#[gpui_kit::test]
fn invalidate_clears_and_reloads(cx: &mut TestAppContext) {
    open(cx);
    let backend = fake_backend(cx, Some(books()));
    let state = connected_state(cx);
    state.update(cx, |s, cx| s.load_collection_list(cx));
    cx.run_until_parked();
    state.update(cx, |s, cx| s.invalidate_collections(cx));
    state.read_with(cx, |s, _| {
        assert!(s.collection_cache.collections.is_none() && s.collection_cache.loading);
    });
    cx.run_until_parked();
    state.read_with(cx, |s, _| assert!(s.collection_cache.collections.is_some()));
    assert_eq!(*backend.lists.lock().unwrap(), 2);
}
