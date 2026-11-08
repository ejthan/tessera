// SPDX-License-Identifier: GPL-3.0-or-later

//! Tessera desktop app.

pub mod actions;
pub mod collections;
pub mod connection;
pub mod documents;
pub mod i18n;
pub mod poller;
pub mod runtime;
pub mod search;
pub mod state;
pub mod views;

use std::io::BufRead;
use std::sync::Arc;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Size, WindowOptions, px};
use rust_i18n::t;
use secrecy::SecretString;
use tessera_core::cli::Launch;
use tessera_core::secrets::KeyringStore;
use tessera_core::store;
use tracing_subscriber::EnvFilter;

rust_i18n::i18n!("../../assets/locales", fallback = "en");

/// Smallest window size the layout is designed for.
const MIN_WINDOW: Size<gpui_kit::Pixels> = Size {
    width: px(960.),
    height: px(600.),
};

/// For `--api-key-stdin`, reads the key as one line from `input` and puts it into the launch.
/// A missing or empty line is an error for `main` to print.
pub fn read_stdin_key(launch: Launch, input: &mut dyn BufRead) -> Result<Launch, String> {
    let Launch::Temporary {
        profile,
        key_from_stdin: true,
        ..
    } = launch
    else {
        return Ok(launch);
    };
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|e| format!("cannot read the API key from stdin: {e}"))?;
    let len = line.trim_end_matches(['\r', '\n']).len();
    line.truncate(len);
    if line.is_empty() {
        return Err("--api-key-stdin: stdin did not hold a key".into());
    }
    Ok(Launch::Temporary {
        profile,
        key_from_stdin: false,
        key_from_env: Some(SecretString::from(line)),
    })
}

/// Loads the profiles, applies `launch` and runs the app until it quits. Fails before the
/// window opens when `launch` names a profile that does not exist.
pub fn run(launch: Launch) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let store_path = store::config_dir().map(|dir| dir.join("connections.json"));
    if store_path.is_none() {
        tracing::warn!("no home directory: profiles are kept for this session only");
    }
    // Once, before the window opens; spec §4.2.
    let loaded = store_path
        .as_deref()
        .map_or_else(|| Ok(store::ConnectionsFile::default()), store::load);
    let mut state = state::AppState::new(loaded, store_path, Arc::new(KeyringStore));
    state.views_path = tessera_core::documents::views::views_path();
    if let Some(path) = &state.views_path {
        state.document_views = tessera_core::documents::views::load(path);
    }
    state.load_queries(
        tessera_core::search::history::history_path(),
        tessera_core::search::saved::saved_path(),
    );
    let open = state.resolve(launch)?;

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
        cx.set_global(poller::PollFetcherGlobal(poller::tokio_fetcher(
            handle.clone(),
        )));
        cx.set_global(poller::CollectionsFetcherGlobal(poller::tokio_collections(
            handle.clone(),
        )));
        cx.set_global(collections::CollectionsBackendGlobal(
            collections::tokio_backend(handle.clone()),
        ));
        cx.set_global(documents::DocumentsBackendGlobal(documents::tokio_backend(
            handle.clone(),
        )));
        cx.set_global(search::SearchBackendGlobal(search::tokio_backend(
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
        let state = cx.new(|cx| {
            state.spawn_log_drain(cx);
            state
        });
        if let Some(id) = open {
            state.update(cx, |s, cx| s.connect(id, None, cx));
        }
        let (window, _) = gpui_kit::open_window(options, cx, |window, cx| {
            window.set_window_title(&t!("app.title"));
            cx.new(|cx| views::root::RootView::new(state, window, cx))
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret as _;
    use tessera_core::profile::Profile;

    use super::*;

    fn stdin_launch() -> Launch {
        Launch::Temporary {
            profile: Profile::new("t", Vec::new()),
            key_from_stdin: true,
            key_from_env: Some(SecretString::from("from-env")),
        }
    }

    #[test]
    fn stdin_key_wins_and_loses_its_newline() {
        let launch = read_stdin_key(stdin_launch(), &mut &b"xyz\r\nrest"[..]);
        let Ok(Launch::Temporary {
            key_from_env: Some(key),
            key_from_stdin: false,
            ..
        }) = launch
        else {
            panic!("expected a temporary launch with the key");
        };
        assert_eq!(key.expose_secret(), "xyz");
    }

    #[test]
    fn empty_stdin_is_an_error() {
        assert!(read_stdin_key(stdin_launch(), &mut &b"\n"[..]).is_err());
        assert!(read_stdin_key(stdin_launch(), &mut &b""[..]).is_err());
    }

    #[test]
    fn other_launches_do_not_read_stdin() {
        let launch = read_stdin_key(Launch::Normal, &mut &b"xyz\n"[..]);
        assert!(matches!(launch, Ok(Launch::Normal)));
    }
}
