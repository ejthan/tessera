// SPDX-License-Identifier: GPL-3.0-or-later

//! A language change. In its own binary, because the locale is global and the other UI tests
//! read English from threads of the same process.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::component::table::{Column, TableDelegate, TableState};
use gpui_kit::{App, AppContext as _, Context, IntoElement, TestAppContext, Window};
use tessera_app::settings::{apply, follow_language};
use tessera_core::secrets::MemoryStore;
use tessera_core::settings::{Language, Settings};
use tessera_core::store::ConnectionsFile;

use common::*;

/// Counts how often the kit reads its columns, which it caches.
struct CountedColumns(Arc<AtomicUsize>);

impl TableDelegate for CountedColumns {
    fn columns_count(&self, _: &App) -> usize {
        1
    }

    fn rows_count(&self, _: &App) -> usize {
        0
    }

    fn column(&self, _: usize, _: &App) -> Column {
        self.0.fetch_add(1, Ordering::Relaxed);
        Column::new("a", "a")
    }

    fn render_td(
        &mut self,
        _: usize,
        _: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        "a"
    }
}

#[gpui_kit::test]
async fn open_tables_read_their_headers_again_after_a_language_change(cx: &mut TestAppContext) {
    let (handle, _state) = open_state(
        cx,
        Ok(ConnectionsFile::default()),
        Arc::new(MemoryStore::default()),
    );
    cx.run_until_parked();
    let reads = Arc::new(AtomicUsize::new(0));
    let delegate = CountedColumns(reads.clone());
    let _table = cx
        .update_window(handle.into(), |_, window, cx| {
            cx.new(|cx| {
                follow_language(cx);
                TableState::new(delegate, window, cx)
            })
        })
        .unwrap();
    cx.run_until_parked();
    let before = reads.load(Ordering::Relaxed);

    let german = Settings {
        language: Language::De,
        ..Settings::default()
    };
    cx.update_window(handle.into(), |_, window, cx| apply(&german, window, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(reads.load(Ordering::Relaxed), before, "the same language");

    tessera_app::i18n::init(Language::De);
    cx.update_window(handle.into(), |_, window, cx| apply(&german, window, cx))
        .unwrap();
    cx.run_until_parked();
    assert!(reads.load(Ordering::Relaxed) > before);
}
