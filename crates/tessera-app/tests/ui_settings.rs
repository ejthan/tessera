// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the settings dialog: appearance and defaults.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::component::table::{Column, TableDelegate, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Size, Theme, ThemeMode, ThemeRegistry, WindowExt as _,
};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, Point, TestAppContext, Window,
    WindowHandle, base::Root, px, size,
};
use tessera_app::actions::{OpenPalette, OpenSettings};
use tessera_app::settings::{appearance_changed, follow_language, table_size};
use tessera_app::state::AppState;
use tessera_core::secrets::MemoryStore;
use tessera_core::settings::{Density, Language, Settings};
use tessera_core::store::ConnectionsFile;

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn open(cx: &mut TestAppContext, file: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        if let Some(text) = file {
            std::fs::write(&path, text).unwrap();
        }
        let (handle, state) = open_state(
            cx,
            Ok(ConnectionsFile::default()),
            Arc::new(MemoryStore::default()),
        );
        let loaded = path.clone();
        cx.update(|cx| state.update(cx, |s, _| s.load_settings(Some(loaded))));
        let f = Self {
            handle,
            state,
            path,
            _dir: dir,
        };
        f.settle(cx);
        f
    }

    fn act<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
    ) -> R {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            f(window, cx)
        })
        .unwrap()
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        self.act(cx, |_, _| {});
    }

    fn open_dialog(&self, cx: &mut TestAppContext) {
        self.act(cx, |window, cx| {
            window.dispatch_action(Box::new(OpenSettings), cx)
        });
        self.settle(cx);
        cx.run_until_parked();
        assert!(self.has(cx, "settings"));
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.act(cx, |window, _| window.try_find(id).is_some())
    }

    /// Picks the `down`-th option below the current one of the select `id`, by keyboard.
    fn pick(&self, cx: &mut TestAppContext, id: &str, down: usize) {
        self.act(cx, |window, cx| window.click(id.to_string(), cx));
        self.settle(cx);
        for _ in 0..down {
            self.act(cx, |window, cx| window.press("down", cx));
            self.settle(cx);
        }
        self.act(cx, |window, cx| window.press("enter", cx));
        self.settle(cx);
    }

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.act(cx, |window, cx| fill(window, id, text, cx));
        self.settle(cx);
    }

    fn settings(&self, cx: &mut TestAppContext) -> Settings {
        cx.update(|cx| self.state.read(cx).settings.clone())
    }

    /// A restart: what a new `AppState` reads from the file.
    fn restarted(&self) -> Settings {
        let empty = Ok(ConnectionsFile::default());
        let mut state = AppState::new(empty, None, Arc::new(MemoryStore::default()));
        state.load_settings(Some(self.path.clone()));
        assert_eq!(state.settings_error, None);
        state.settings
    }
}

#[gpui_kit::test]
async fn opens_by_action_and_title_bar_and_only_once(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, None);
    f.act(cx, |window, cx| {
        window.click("titlebar.settings".to_string(), cx)
    });
    f.settle(cx);
    assert!(f.has(cx, "settings"));
    f.act(cx, |window, cx| {
        window.dispatch_action(Box::new(OpenSettings), cx);
        assert!(window.has_active_dialog(cx));
    });
}

#[gpui_kit::test]
async fn each_control_changes_the_state_and_survives_a_restart(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, None);
    f.open_dialog(cx);
    f.pick(cx, "settings.theme", 2);
    f.pick(cx, "settings.font_size", 5);
    f.pick(cx, "settings.density", 1);
    f.pick(cx, "settings.language", 1);
    f.pick(cx, "settings.poll", 2);
    f.fill(cx, "settings.per_page", "50");
    f.fill(cx, "settings.batch_size", "200");
    f.fill(cx, "settings.confirm_above", "5");
    let want = Settings {
        theme: "dark".into(),
        font_size: Some(16),
        density: Density::Compact,
        language: Language::En,
        per_page: 50,
        batch_size: 200,
        poll_seconds: 30,
        confirm_above: 5,
        update_check: false,
    };
    assert_eq!(f.settings(cx), want);
    assert_eq!(f.restarted(), want);
    cx.update(|cx| {
        assert!(cx.theme().mode.is_dark());
        assert_eq!(cx.theme().font_size, px(16.));
        assert_eq!(table_size(cx), Size::XSmall);
    });
    // Restored, since the mode and font are the app's and the tests share nothing else.
    f.pick(cx, "settings.language", 0);
}

#[gpui_kit::test]
async fn a_number_out_of_range_changes_nothing_and_says_so(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, None);
    f.open_dialog(cx);
    f.fill(cx, "settings.per_page", "5");
    f.fill(cx, "settings.confirm_above", "abc");
    f.fill(cx, "settings.batch_size", "0");
    assert_eq!(f.settings(cx), Settings::default());
    assert!(!f.path.exists());
    assert!(f.has(cx, "settings.per_page.note"));
}

#[gpui_kit::test]
async fn an_explicit_theme_survives_an_appearance_change(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, Some("theme = \"dark\"\n"));
    let settings = f.settings(cx);
    f.act(cx, |window, cx| {
        tessera_app::settings::apply(&settings, window, cx);
        assert!(cx.theme().mode.is_dark());
        // The test window's appearance is light.
        appearance_changed(&settings, window, cx);
        assert!(cx.theme().mode.is_dark());
        let system = Settings::default();
        appearance_changed(&system, window, cx);
        assert!(!cx.theme().mode.is_dark());
    });
}

#[gpui_kit::test]
async fn a_theme_of_the_registry_applies_and_is_offered(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, Some("theme = \"Nord\"\n"));
    let settings = f.settings(cx);
    f.act(cx, |window, cx| {
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(
                r#"{"name":"t","themes":[{"name":"Nord","mode":"dark","colors":{}}]}"#,
            )
            .unwrap();
        tessera_app::settings::apply(&settings, window, cx);
        assert!(cx.theme().mode.is_dark());
        assert_eq!(cx.theme().theme_name().as_ref(), "Nord");
        // Gone from the registry, it follows the system and keeps the setting.
        let gone = Settings {
            theme: "Removed".into(),
            ..settings.clone()
        };
        tessera_app::settings::apply(&gone, window, cx);
        assert!(!cx.theme().mode.is_dark());
    });
}

#[gpui_kit::test]
async fn a_new_connection_starts_on_the_poll_setting(cx: &mut TestAppContext) {
    let profile = tessera_core::profile::Profile::new("test", vec![node()]);
    fake_connector(cx, |_| Ok(server("30.2", true)));
    let (handle, state, _rt) =
        open_app(cx, vec![profile.clone()], Arc::new(MemoryStore::default()));
    cx.update(|cx| {
        state.update(cx, |s, cx| {
            s.change_settings(|s| s.poll_seconds = 30, cx);
            s.connect(profile.id, Some("k".to_string().into()), cx);
        })
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    let interval = cx.update(|cx| {
        let conn = state.read(cx).connection(profile.id, cx).unwrap();
        conn.read(cx).interval
    });
    assert_eq!(interval, std::time::Duration::from_secs(30));
}

#[gpui_kit::test]
async fn a_bad_file_shows_inline_and_saves_nothing(cx: &mut TestAppContext) {
    let text = "density = \"tight\"\n";
    let f = Fixture::open(cx, Some(text));
    assert_eq!(f.settings(cx), Settings::default());
    f.open_dialog(cx);
    let label = f.act(cx, |window, _| {
        window.find("settings.error").label().unwrap().to_string()
    });
    assert!(label.contains("density"), "{label}");
    f.pick(cx, "settings.density", 1);
    cx.run_until_parked();
    assert_eq!(f.settings(cx).density, Density::Compact);
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), text);
}

#[gpui_kit::test]
async fn a_failed_write_shows_inline_and_stops_saving(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, None);
    std::fs::create_dir(f.path.with_extension("json.tmp")).unwrap();
    f.open_dialog(cx);
    f.pick(cx, "settings.density", 1);
    assert!(f.has(cx, "settings.error"));
    let label = f.act(cx, |window, _| {
        window.find("settings.error").label().unwrap().to_string()
    });
    assert!(label.contains("could not be written"), "{label}");

    std::fs::remove_dir(f.path.with_extension("json.tmp")).unwrap();
    f.pick(cx, "settings.font_size", 1);
    assert!(
        f.settings(cx).font_size.is_some(),
        "the change still applies"
    );
    assert!(!f.path.exists(), "nothing is written after a failure");
}

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
async fn open_tables_reread_their_headers_after_a_language_change(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, None);
    let reads = Arc::new(AtomicUsize::new(0));
    let delegate = CountedColumns(reads.clone());
    let _table = f.act(cx, |window, cx| {
        cx.new(|cx| {
            follow_language(cx);
            TableState::new(delegate, window, cx)
        })
    });
    f.settle(cx);
    f.open_dialog(cx);
    f.pick(cx, "settings.density", 1);
    let before = reads.load(Ordering::Relaxed);
    f.pick(cx, "settings.language", 2);
    assert!(reads.load(Ordering::Relaxed) > before);
    f.pick(cx, "settings.language", 0);
}

#[gpui_kit::test]
async fn a_removed_theme_keeps_following_the_system(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, Some("theme = \"gone\"\n"));
    let settings = f.settings(cx);
    assert_eq!(settings.theme, "gone");
    f.act(cx, |window, cx| {
        Theme::change(ThemeMode::Dark, Some(window), cx);
        // The test window's appearance is light.
        appearance_changed(&settings, window, cx);
        assert!(!cx.theme().mode.is_dark());
    });
}

#[gpui_kit::test]
async fn the_palette_lists_settings(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, None);
    f.act(cx, |window, cx| {
        window.dispatch_action(Box::new(OpenPalette), cx)
    });
    f.settle(cx);
    f.act(cx, |window, cx| window.input("settings", cx));
    f.settle(cx);
    assert!(f.has(cx, "palette.action.tessera::OpenSettings"));
}

fn inside(outer: Bounds<gpui_kit::Pixels>, inner: Bounds<gpui_kit::Pixels>) {
    assert!(inner.origin.y >= outer.origin.y, "{inner:?} in {outer:?}");
    assert!(
        inner.origin.y + inner.size.height <= outer.origin.y + outer.size.height,
        "{inner:?} in {outer:?}"
    );
}

#[gpui_kit::test]
async fn font_size_20_stays_inside_the_smallest_window(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, Some("font_size = 20\n"));
    let settings = f.settings(cx);
    f.act(cx, |window, cx| {
        tessera_app::settings::apply(&settings, window, cx)
    });
    cx.simulate_window_resize(f.handle.into(), size(px(960.), px(600.)));
    f.settle(cx);
    let (bar, status, title) = f.act(cx, |window, _| {
        (
            window.find("titlebar.settings").bounds(),
            window.find("status.text").bounds(),
            window.find("titlebar.connection").bounds(),
        )
    });
    let frame = Bounds {
        origin: Point::default(),
        size: size(px(960.), px(600.)),
    };
    inside(frame, bar);
    inside(frame, status);
    inside(frame, title);
    assert!(bar.origin.x + bar.size.width <= px(960.));
}

#[gpui_kit::test]
async fn tab_reaches_every_control_and_shows_it(cx: &mut TestAppContext) {
    let f = Fixture::open(cx, Some("font_size = 20\n"));
    let settings = f.settings(cx);
    f.act(cx, |window, cx| {
        tessera_app::settings::apply(&settings, window, cx)
    });
    cx.simulate_window_resize(f.handle.into(), size(px(960.), px(600.)));
    f.settle(cx);
    f.open_dialog(cx);
    let ids = [
        "settings.font_size",
        "settings.density",
        "settings.language",
        "settings.per_page",
        "settings.batch_size",
        "settings.poll",
        "settings.confirm_above",
    ];
    for id in ids {
        f.act(cx, |window, cx| window.press("tab", cx));
        f.settle(cx);
        let id = id.to_string();
        let (focused, visible) = f.act(cx, |window, _| {
            let control = window.find(id.clone());
            (control.focused(), control.visible())
        });
        assert_eq!(focused, Some(true), "{id}");
        assert!(visible, "{id}");
    }
}
