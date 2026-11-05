// SPDX-License-Identifier: GPL-3.0-or-later

//! Tessera desktop app.

pub mod actions;
pub mod connection;
pub mod i18n;
pub mod runtime;
pub mod views;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Size, WindowOptions, px};
use rust_i18n::t;
use tracing_subscriber::EnvFilter;

rust_i18n::i18n!("../../assets/locales", fallback = "en");

/// Smallest window size the layout is designed for.
const MIN_WINDOW: Size<gpui_kit::Pixels> = Size {
    width: px(960.),
    height: px(600.),
};

/// Starts the app and blocks until it quits.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to start the tokio runtime");
    let smoke = std::env::var("TESSERA_SMOKE").is_ok_and(|v| v == "1");
    let handle = runtime.handle().clone();

    gpui_kit::application().run(move |cx| {
        cx.set_global(connection::ConnectorGlobal(connection::tokio_connector(
            handle.clone(),
        )));
        cx.set_global(runtime::Tokio(handle));
        gpui_kit::init(cx);
        i18n::init();
        actions::bind_keys(cx);
        let options = WindowOptions {
            titlebar: Some(TitleBar::title_bar_options()),
            app_owns_titlebar_drag: true,
            window_min_size: Some(MIN_WINDOW),
            ..Default::default()
        };
        let (window, _) = gpui_kit::open_window(options, cx, |window, cx| {
            window.set_window_title(&t!("app.title"));
            cx.new(|cx| views::root::RootView::new(window, cx))
        })
        .expect("failed to open the main window");
        if smoke {
            // CI smoke test: prove the window opened and rendered, then exit.
            window
                .update(cx, |_, window, _| {
                    window.on_next_frame(|_, cx| cx.quit());
                })
                .expect("main window is open");
        }
        cx.activate(true);
    });

    runtime.shutdown_background();
}
