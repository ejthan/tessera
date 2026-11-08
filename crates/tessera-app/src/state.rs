// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide state: saved profiles, the secret store and open connections.
//!
//! Keychain and file I/O never run on the GPUI thread: they go to tokio's `spawn_blocking`
//! through the [`Tokio`] global (`docs/architecture/async.md`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Subscription, Task};
use rust_i18n::t;
use secrecy::{ExposeSecret as _, SecretString};
use tessera_client::{RequestEvent, RequestHook};
use tessera_core::cli::Launch;
use tessera_core::documents::views::{self, CollectionView, ViewsFile};
use tessera_core::import;
use tessera_core::log::{LogEntry, RequestLog};
use tessera_core::profile::Profile;
use tessera_core::search::history::{self, History, HistoryItem};
use tessera_core::search::params::SearchDraft;
use tessera_core::search::saved::{self, SavedFile, SavedQuery};
use tessera_core::secrets::{SecretError, SecretStore};
use tessera_core::store::{self, ConnectionsFile, SCHEMA_VERSION, StoreError};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::connection::{Connected, ConnectionState, ConnectorGlobal, client_options, error_text};
use crate::runtime::Tokio;
use crate::views::status_bar;

/// One file's writes: each waits for the one before, on the background executor, and quitting
/// waits for the last.
#[derive(Default)]
struct WriteChain {
    task: Option<Task<()>>,
    /// Registered with the first write.
    on_quit: Option<Subscription>,
}

impl WriteChain {
    /// Queues `write`, owned by the chain: closing a tab must not cancel it. `chain` finds this
    /// chain again on quit.
    fn push<R: Send + 'static>(
        &mut self,
        write: impl FnOnce() -> R + Send + 'static,
        chain: fn(&mut AppState) -> &mut Self,
        cx: &mut Context<AppState>,
    ) -> Task<Option<R>> {
        let previous = self.task.take();
        let (done, result) = tokio::sync::oneshot::channel();
        self.task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            done.send(write()).ok();
        }));
        if self.on_quit.is_none() {
            self.on_quit = Some(cx.on_app_quit(move |s, _| {
                let pending = chain(s).task.take();
                async move {
                    if let Some(pending) = pending {
                        pending.await;
                    }
                }
            }));
        }
        cx.background_spawn(async move { result.await.ok() })
    }
}

/// Why saving a profile failed. Neither variant holds the key.
#[derive(Debug)]
pub enum SaveError {
    /// `connections.json` could not be written; the text is the reason.
    File(String),
    /// The key could not be stored in the secret store.
    Keychain(SecretError),
}

/// Asks the window to show the overview of a connected profile: open its tab, or focus the
/// existing one.
pub struct OpenOverview(pub Uuid);

/// Asks the window to show the collections list of a connected profile.
pub struct OpenCollections(pub Uuid);

/// Asks the window to show the tab of one collection of a profile.
pub struct OpenCollection(pub Uuid, pub String);

/// Asks the window to show the search tab of a profile, with `.1` loaded into it when given.
pub struct OpenSearch(pub Uuid, pub Option<SearchDraft>);

/// Asks the window to close the tab of one collection of a profile, which no longer exists.
pub struct CloseCollection(pub Uuid, pub String);

/// Profiles, keys and open connections of the app.
pub struct AppState {
    /// Saved profiles, in sidebar order.
    pub profiles: Vec<Profile>,
    /// The profile connected last.
    pub last_used: Option<Uuid>,
    /// Reopen `last_used` at startup.
    pub reopen_last: bool,
    /// Where `connections.json` lives; `None` means profiles are not persisted.
    pub store_path: Option<PathBuf>,
    /// Where API keys of saved profiles live.
    pub secrets: Arc<dyn SecretStore>,
    /// Open connections, one per profile or temporary connection.
    pub connections: Vec<Entity<ConnectionState>>,
    /// The connection the main area shows.
    pub selected: Option<Uuid>,
    /// The connection from CLI flags or `TYPESENSE_*`. Never saved; its key lives in memory.
    /// An empty key means none was given.
    pub temporary: Option<(Profile, SecretString)>,
    /// Keys of saved profiles that live in memory only, because the keychain failed.
    pub session_keys: HashMap<Uuid, SecretString>,
    /// Why `connections.json` could not be loaded. While set, the file is never written.
    pub load_error: Option<String>,
    /// The last save. Each save awaits the previous one, so writes land in order.
    save_task: Option<Task<()>>,
    /// Waits for `save_task` on quit; registered with the first save.
    on_quit: Option<Subscription>,
    /// Column preferences of the document tables, by profile and collection.
    pub document_views: ViewsFile,
    /// Where `document_views.json` lives; `None` means preferences are not persisted.
    pub views_path: Option<PathBuf>,
    /// The last write of `document_views.json`; each write awaits the previous one.
    views_save_task: Option<Task<()>>,
    /// Waits for `views_save_task` on quit; registered with the first write.
    views_on_quit: Option<Subscription>,
    /// Searches and console requests that ran.
    pub history: History,
    /// `None` keeps the history for this session only.
    pub history_path: Option<PathBuf>,
    /// Lines the history file gained beyond what it held after the last rewrite.
    history_lines: usize,
    history_chain: WriteChain,
    /// Saved queries and console favorites.
    pub saved: SavedFile,
    /// `None` keeps them for this session only.
    pub saved_path: Option<PathBuf>,
    /// Why the saved file did not load; while set, it is not written.
    pub saved_error: Option<String>,
    saved_chain: WriteChain,
    log: RequestLog,
    log_tx: UnboundedSender<LogEntry>,
    /// Taken by [`AppState::spawn_log_drain`].
    log_rx: Option<UnboundedReceiver<LogEntry>>,
    log_drain: Option<Task<()>>,
}

impl AppState {
    /// State from the result of [`store::load`]. A failed load keeps the profiles in memory only.
    pub fn new(
        loaded: Result<ConnectionsFile, StoreError>,
        store_path: Option<PathBuf>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        let (file, load_error) = match loaded {
            Ok(file) => (file, None),
            Err(e) => {
                tracing::warn!("connections file not loaded, it will not be written: {e}");
                (ConnectionsFile::default(), Some(e.to_string()))
            }
        };
        let (log_tx, log_rx) = unbounded_channel();
        Self {
            log: RequestLog::default(),
            log_tx,
            log_rx: Some(log_rx),
            log_drain: None,
            profiles: file.profiles,
            last_used: file.last_used,
            reopen_last: file.reopen_last,
            store_path,
            secrets,
            connections: Vec::new(),
            selected: None,
            temporary: None,
            session_keys: HashMap::new(),
            load_error,
            save_task: None,
            on_quit: None,
            document_views: ViewsFile::default(),
            views_path: None,
            views_save_task: None,
            views_on_quit: None,
            history: History::default(),
            history_path: None,
            history_lines: 0,
            history_chain: WriteChain::default(),
            saved: SavedFile::default(),
            saved_path: None,
            saved_error: None,
            saved_chain: WriteChain::default(),
        }
    }

    /// The saved column preferences of `collection`, or the default.
    pub fn document_view(&self, profile: Uuid, collection: &str) -> CollectionView {
        self.document_views.view(profile, collection)
    }

    /// Stores the column preferences and writes the file in the background. A temporary
    /// connection keeps them in memory only. The task reports the save; callers must show
    /// an error inline (the preferences stay set in memory).
    pub fn set_document_view(
        &mut self,
        profile: Uuid,
        collection: &str,
        view: CollectionView,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.document_views.set_view(profile, collection, view);
        let temporary = self
            .temporary
            .as_ref()
            .is_some_and(|(p, _)| p.id == profile);
        let Some(path) = self.views_path.clone().filter(|_| !temporary) else {
            return Task::ready(Ok(()));
        };
        let file = self.document_views.clone();
        let previous = self.views_save_task.take();
        let (done, result) = tokio::sync::oneshot::channel();
        // Owned by the state, not the caller: closing a tab must not cancel the write. On the
        // background executor so the quit handler can wait for it.
        self.views_save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            done.send(views::save(&path, &file).map_err(|e| e.to_string()))
                .ok();
        }));
        if self.views_on_quit.is_none() {
            self.views_on_quit = Some(cx.on_app_quit(|s, _| {
                let pending = s.views_save_task.take();
                async move {
                    if let Some(pending) = pending {
                        pending.await;
                    }
                }
            }));
        }
        cx.background_spawn(async move {
            result
                .await
                .unwrap_or_else(|_| Err("the save was dropped".into()))
        })
    }

    /// Loads history and saved queries. Reads the disk, so only before the window opens.
    pub fn load_queries(&mut self, history_path: Option<PathBuf>, saved_path: Option<PathBuf>) {
        if let Some(path) = &history_path {
            (self.history, self.history_lines) = history::load(path);
        }
        if let Some(path) = &saved_path {
            match saved::load(path) {
                Ok(file) => self.saved = file,
                Err(e) => {
                    tracing::warn!("saved queries not loaded, they will not be written: {e}");
                    self.saved_error = Some(e.to_string());
                }
            }
        }
        self.history_path = history_path;
        self.saved_path = saved_path;
    }

    fn is_temporary(&self, profile: Uuid) -> bool {
        self.temporary
            .as_ref()
            .is_some_and(|(p, _)| p.id == profile)
    }

    /// Adds `item` to the history and the file. A temporary connection stays in memory.
    pub fn record_history(&mut self, item: HistoryItem, cx: &mut Context<Self>) {
        let temporary = self.is_temporary(item.profile);
        if !self.history.push(item.clone()) || temporary {
            return;
        }
        self.history_lines += 1;
        if self.history_lines > 2 * history::HISTORY_CAP {
            return self.rewrite_history(cx);
        }
        let Some(path) = self.history_path.clone() else {
            return;
        };
        let write = move || {
            if let Err(e) = history::append(&path, &item) {
                tracing::warn!("history not written: {e}");
            }
        };
        self.history_chain
            .push(write, |s| &mut s.history_chain, cx)
            .detach();
    }

    /// Forgets the history of `profile`, also in the file.
    pub fn clear_history(&mut self, profile: Uuid, cx: &mut Context<Self>) {
        self.history.clear(profile);
        self.rewrite_history(cx);
    }

    fn rewrite_history(&mut self, cx: &mut Context<Self>) {
        self.history_lines = 0;
        let Some(path) = self.history_path.clone() else {
            return;
        };
        let mut file = self.history.clone();
        if let Some((profile, _)) = &self.temporary {
            file.clear(profile.id);
        }
        let write = move || {
            if let Err(e) = history::rewrite(&path, &file) {
                tracing::warn!("history not written: {e}");
            }
        };
        self.history_chain
            .push(write, |s| &mut s.history_chain, cx)
            .detach();
    }

    /// Adds `query`, or replaces the one with its id, and saves. The task reports the save;
    /// callers must show an error inline.
    pub fn save_query(
        &mut self,
        query: SavedQuery,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.change_saved(
            |saved| match saved.queries.iter_mut().find(|q| q.id == query.id) {
                Some(old) => *old = query,
                None => saved.queries.push(query),
            },
            cx,
        )
    }

    /// Deletes query `id` and saves.
    pub fn delete_query(&mut self, id: Uuid, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.change_saved(|saved| saved.queries.retain(|q| q.id != id), cx)
    }

    /// Replaces the console favorites and saves.
    pub fn set_favorites(
        &mut self,
        favorites: Vec<HistoryItem>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.change_saved(|saved| saved.favorites = favorites, cx)
    }

    /// Applies `change` and writes the file without what a temporary connection holds. Refused
    /// while the file on disk could not be read: it may hold work this build cannot see.
    fn change_saved(
        &mut self,
        change: impl FnOnce(&mut SavedFile),
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        if let Some(error) = &self.saved_error {
            return Task::ready(Err(error.clone()));
        }
        change(&mut self.saved);
        let Some(path) = self.saved_path.clone() else {
            return Task::ready(Ok(()));
        };
        let mut file = self.saved.clone();
        if let Some((profile, _)) = &self.temporary {
            file.queries.retain(|q| q.profile != profile.id);
            file.favorites.retain(|f| f.profile != profile.id);
        }
        let write = move || saved::save(&path, &file).map_err(|e| e.to_string());
        let done = self.saved_chain.push(write, |s| &mut s.saved_chain, cx);
        cx.background_spawn(async move {
            let done = done.await;
            done.unwrap_or_else(|| Err("the save was dropped".into()))
        })
    }

    /// The request log of all connections.
    pub fn log(&self) -> &RequestLog {
        &self.log
    }

    /// Where request events enter the log. Hooks send here from tokio; tests inject events here.
    pub fn log_sender(&self) -> UnboundedSender<LogEntry> {
        self.log_tx.clone()
    }

    /// A hook that sends the events of `profile`'s requests to the log. Never carries the key.
    pub fn log_hook(&self, profile: &Profile) -> RequestHook {
        debug_assert!(
            self.log_drain.is_some(),
            "spawn_log_drain must run before connecting"
        );
        let tx = self.log_tx.clone();
        let (connection, connection_name): (Uuid, Arc<str>) =
            (profile.id, profile.name.as_str().into());
        Arc::new(move |event: &RequestEvent| {
            // A closed channel means the app is quitting.
            tx.send(LogEntry {
                connection,
                connection_name: connection_name.clone(),
                event: event.clone(),
            })
            .ok();
        })
    }

    /// Starts the task that moves queued events into the log, one `notify` per batch.
    /// Call once, right after the entity is created: `cx.new(|cx| { s.spawn_log_drain(cx); s })`.
    pub fn spawn_log_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.log_rx.take() else {
            return;
        };
        self.log_drain = Some(cx.spawn(async move |this, cx| {
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(entry) = rx.try_recv() {
                    batch.push(entry);
                }
                if this
                    .update(cx, |s, cx| {
                        batch.into_iter().for_each(|e| s.log.push(e));
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// Applies the launch mode and returns the profile to connect at startup, if any.
    /// An unknown `--profile` name is an error for `main` to print.
    pub fn resolve(&mut self, launch: Launch) -> Result<Option<Uuid>, String> {
        match launch {
            Launch::Normal => Ok(self
                .last_used
                .filter(|id| self.reopen_last && self.profiles.iter().any(|p| p.id == *id))),
            Launch::Help => Ok(None),
            Launch::Profile(name) => match self.profiles.iter().find(|p| p.name == name) {
                Some(p) => Ok(Some(p.id)),
                None => Err(match &self.load_error {
                    Some(e) => format!("cannot open profile {name:?}: {e}"),
                    None => format!("no saved profile is named {name:?}"),
                }),
            },
            Launch::Temporary {
                profile,
                key_from_env,
                ..
            } => {
                let id = profile.id;
                let key = key_from_env.unwrap_or_else(|| SecretString::from(""));
                self.temporary = Some((profile, key));
                Ok(Some(id))
            }
        }
    }

    /// The open connection of profile `id`.
    pub fn connection(&self, id: Uuid, cx: &App) -> Option<Entity<ConnectionState>> {
        self.connections
            .iter()
            .find(|c| c.read(cx).profile.id == id)
            .cloned()
    }

    /// Writes the profiles to `store_path` on tokio's `spawn_blocking`, after the previous save.
    /// Does nothing without a path or after a failed load (the file is not overwritten).
    /// The task reports the result; dropping it does not stop the write.
    pub fn save(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let Some(path) = self.store_path.clone() else {
            return Task::ready(Ok(()));
        };
        if self.load_error.is_some() {
            return Task::ready(Ok(()));
        }
        let file = ConnectionsFile {
            schema_version: SCHEMA_VERSION,
            profiles: self.profiles.clone(),
            last_used: self.last_used,
            reopen_last: self.reopen_last,
        };
        let tokio = cx.global::<Tokio>().0.clone();
        let previous = self.save_task.take();
        let (done, result) = tokio::sync::oneshot::channel();
        // On the background executor: the quit handler blocks the main thread, which then
        // runs no foreground tasks, so a foreground save chain could not finish.
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            let outcome = match tokio
                .spawn_blocking(move || store::save(&path, &file))
                .await
            {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = &outcome {
                // shortcut: only the dialog shows save errors; other saves are logged until
                // M1 has a place for background errors (toast).
                tracing::error!("saving connections failed: {e}");
            }
            done.send(outcome).ok();
        }));
        if self.on_quit.is_none() {
            // GPUI waits up to SHUTDOWN_TIMEOUT (200 ms) for this before the process exits.
            self.on_quit = Some(cx.on_app_quit(|s, _| {
                let pending = s.save_task.take();
                async move {
                    if let Some(pending) = pending {
                        pending.await;
                    }
                }
            }));
        }
        cx.background_spawn(async move {
            result
                .await
                .unwrap_or_else(|_| Err("the save was dropped".into()))
        })
    }

    /// Adds or replaces a profile and saves. With `key`, stores it in the secret store. The
    /// task reports both writes, the keychain first (dropping it does not stop them).
    /// Saving the temporary connection's id turns it into this saved profile.
    pub fn upsert_profile(
        &mut self,
        profile: Profile,
        key: Option<SecretString>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), SaveError>> {
        let id = profile.id;
        if self.temporary.as_ref().is_some_and(|(p, _)| p.id == id) {
            self.temporary = None;
        }
        match self.profiles.iter_mut().find(|p| p.id == id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
        let saved = self.save(cx);
        cx.notify();
        let stored = key.map(|key| {
            self.session_keys.remove(&id);
            let secrets = self.secrets.clone();
            secret_task(blocking(cx, move || secrets.set(id, &key)), cx)
        });
        cx.spawn(async move |_, _| {
            if let Some(stored) = stored {
                stored.await.map_err(SaveError::Keychain)?;
            }
            saved.await.map_err(SaveError::File)
        })
    }

    /// Stars or unstars `collection` of profile `profile` and saves. A temporary connection
    /// keeps the favorite in memory only. The task reports the save; callers must show an
    /// error inline (the favorite stays set in memory).
    pub fn toggle_favorite(
        &mut self,
        profile: Uuid,
        collection: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let favorite = self
            .profiles
            .iter()
            .chain(self.temporary.as_ref().map(|(p, _)| p))
            .find(|p| p.id == profile)
            .is_some_and(|p| p.favorite_collections.iter().any(|c| c == collection));
        self.set_favorite(profile, collection, !favorite, cx)
    }

    /// Stars (`favorite`) or unstars `collection` of profile `profile` and saves, like
    /// [`Self::toggle_favorite`]. Nothing to change is `Ok` without a save.
    pub fn set_favorite(
        &mut self,
        profile: Uuid,
        collection: &str,
        favorite: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let saved = self.profiles.iter_mut().find(|p| p.id == profile);
        let temporary = self
            .temporary
            .as_mut()
            .map(|(p, _)| p)
            .filter(|p| p.id == profile);
        let (Some(p), saved) = (match saved {
            Some(p) => (Some(p), true),
            None => (temporary, false),
        }) else {
            return Task::ready(Ok(()));
        };
        match (
            p.favorite_collections.iter().position(|c| c == collection),
            favorite,
        ) {
            (Some(i), false) => {
                p.favorite_collections.remove(i);
            }
            (None, true) => p.favorite_collections.push(collection.to_owned()),
            _ => return Task::ready(Ok(())),
        }
        let favorites = p.favorite_collections.clone();
        if let Some(conn) = self.connection(profile, cx) {
            conn.update(cx, |c, cx| {
                c.profile.favorite_collections = favorites;
                cx.notify();
            });
        }
        cx.notify();
        if !saved {
            return Task::ready(Ok(()));
        }
        // `save` skips the write after a failed load; the favorite would be lost silently.
        if let (Some(error), Some(path)) = (&self.load_error, &self.store_path) {
            let path = path.display().to_string();
            let text = t!("collections.favorite_unsaved", path = path, error = error);
            return Task::ready(Err(text.to_string()));
        }
        self.save(cx)
    }

    /// Removes a profile, its connection and its stored key, and saves.
    pub fn delete_profile(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), SecretError>> {
        self.profiles.retain(|p| p.id != id);
        if self.last_used == Some(id) {
            self.last_used = None;
        }
        self.session_keys.remove(&id);
        self.disconnect(id, cx);
        self.save(cx).detach();
        let secrets = self.secrets.clone();
        secret_task(blocking(cx, move || secrets.delete(id)), cx)
    }

    /// Writes the saved profiles, without keys, to `path` on tokio's `spawn_blocking`.
    /// Writes a temporary file next to it and renames it, so a failed export leaves no
    /// half-written file.
    pub fn export_to(&self, path: PathBuf, cx: &App) -> Task<Result<(), String>> {
        let text = import::export_profiles(&self.profiles);
        let job = blocking(cx, move || {
            let mut tmp = path.clone().into_os_string();
            tmp.push(".tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path)
        });
        cx.background_spawn(async move {
            match job.await {
                Ok(result) => result.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    /// Connects profile `id` (saved or temporary) and selects it. `key` is used instead of
    /// the stored one, so a key that was just saved need not be read back from the keychain.
    /// A connected profile is only selected; a failed or running attempt is retried in place
    /// with the current profile, cancelling the previous attempt.
    pub fn connect(&mut self, id: Uuid, key: Option<SecretString>, cx: &mut Context<Self>) {
        if let Some(conn) = self.connection(id, cx)
            && conn.read(cx).connected.is_some()
        {
            self.selected = Some(id);
            cx.emit(OpenOverview(id));
            cx.notify();
            return;
        }
        let temporary = self.temporary.as_ref().filter(|(p, _)| p.id == id);
        let saved = temporary.is_none();
        let (profile, key) = match temporary {
            Some((p, temp_key)) => (p.clone(), key.or_else(|| Some(temp_key.clone()))),
            None => match self.profiles.iter().find(|p| p.id == id) {
                Some(p) => (
                    p.clone(),
                    key.or_else(|| self.session_keys.get(&id).cloned()),
                ),
                None => return,
            },
        };
        let conn = match self.connection(id, cx) {
            Some(conn) => {
                conn.update(cx, |c, _| c.profile = profile.clone());
                conn
            }
            None => {
                let conn = cx.new(|_| ConnectionState::new(profile.clone()));
                observe_connection(&conn, cx);
                self.connections.push(conn.clone());
                conn
            }
        };
        self.selected = Some(id);
        cx.notify();
        if let Err(e) = client_options(&profile) {
            conn.update(cx, |c, cx| {
                c.task = None;
                c.connecting = false;
                c.missing_key = false;
                c.error = Some(
                    t!("connect.error.invalid_node", reason = e.0)
                        .to_string()
                        .into(),
                );
                cx.notify();
            });
            return;
        }
        // Without a key in memory, read the stored one.
        let stored_key = key.is_none().then(|| {
            let secrets = self.secrets.clone();
            blocking(cx, move || secrets.get(id))
        });
        let connect = cx.global::<ConnectorGlobal>().0.clone();
        let hook = self.log_hook(&profile);
        let weak = conn.downgrade();
        let task = cx.spawn(async move |this, cx| {
            let key = match stored_key {
                None => Ok(key),
                Some(job) => job
                    .await
                    .unwrap_or_else(|e| Err(SecretError::new(e.to_string()))),
            };
            let missing_key =
                matches!(&key, Ok(k) if k.as_ref().is_none_or(|k| k.expose_secret().is_empty()));
            let result = match key {
                Ok(Some(key)) if !key.expose_secret().is_empty() => {
                    connect(profile.clone(), key, hook)
                        .await
                        .map_err(|e| error_text(&e, &profile))
                }
                Ok(_) => Err(t!("connect.error.no_key").to_string()),
                Err(e) => Err(t!("connect.error.keychain", detail = e.to_string()).to_string()),
            };
            let ok = result.is_ok();
            // A connection closed meanwhile gets no tab.
            let alive = weak
                .update(cx, |c, cx| {
                    c.connecting = false;
                    c.missing_key = missing_key;
                    match result {
                        Ok(connected) => {
                            // A new client starts with the profile's guard.
                            c.writes_unlocked = false;
                            c.connected = Some(connected);
                        }
                        Err(text) => c.error = Some(text.into()),
                    }
                    cx.notify();
                })
                .is_ok();
            if ok && alive {
                this.update(cx, |s, cx| {
                    cx.emit(OpenOverview(id));
                    if saved {
                        s.last_used = Some(id);
                        s.save(cx).detach();
                    }
                })
                .ok();
            }
        });
        conn.update(cx, |c, cx| {
            c.connecting = true;
            c.error = None;
            c.missing_key = false;
            c.task = Some(task);
            cx.notify();
        });
    }

    /// Adds a connection that was established elsewhere (the connect dialog) and selects it.
    pub fn add_connected(
        &mut self,
        profile: Profile,
        connected: Connected,
        cx: &mut Context<Self>,
    ) {
        let id = profile.id;
        self.disconnect(id, cx);
        let mut state = ConnectionState::new(profile);
        state.writes_unlocked = false;
        state.connected = Some(connected);
        let conn = cx.new(|_| state);
        observe_connection(&conn, cx);
        self.connections.push(conn);
        self.selected = Some(id);
        cx.emit(OpenOverview(id));
        cx.notify();
    }

    /// Asks for the collections tab of profile `id`.
    pub fn open_collections(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.selected = Some(id);
        cx.emit(OpenCollections(id));
        cx.notify();
    }

    /// Asks for the tab of collection `name` of profile `id`.
    pub fn open_collection(&mut self, id: Uuid, name: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        cx.emit(OpenCollection(id, name));
        cx.notify();
    }

    /// Asks for the search tab of profile `id`, showing `draft` when given.
    pub fn open_search(&mut self, id: Uuid, draft: Option<SearchDraft>, cx: &mut Context<Self>) {
        self.selected = Some(id);
        cx.emit(OpenSearch(id, draft));
        cx.notify();
    }

    /// Asks for the tab of collection `name` of profile `id` to close.
    pub fn close_collection(&mut self, id: Uuid, name: String, cx: &mut Context<Self>) {
        cx.emit(CloseCollection(id, name));
    }

    /// Closes the connection of profile `id`. Dropping it cancels a running attempt.
    pub fn disconnect(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(conn) = self.connection(id, cx) {
            conn.update(cx, |c, cx| {
                c.closed = true;
                c.lock(cx);
            });
        }
        self.connections.retain(|c| c.read(cx).profile.id != id);
        if self.selected == Some(id) {
            self.selected = self.connections.last().map(|c| c.read(cx).profile.id);
        }
        cx.notify();
    }
}

/// Redraws the window when `conn` changes what it shows: sidebar entry, title bar or status
/// bar. Polling notifies once per node result, which mostly changes only the overview tab.
fn observe_connection(conn: &Entity<ConnectionState>, cx: &mut Context<AppState>) {
    let mut shown = None;
    cx.observe(conn, move |_, conn, cx| {
        let c = conn.read(cx);
        let now = Some((
            c.connecting,
            c.missing_key,
            c.writes_unlocked,
            c.error.clone(),
            status_bar::summary(c),
        ));
        if now != shown {
            shown = now;
            cx.notify();
        }
    })
    .detach();
}

impl EventEmitter<OpenOverview> for AppState {}
impl EventEmitter<OpenCollections> for AppState {}
impl EventEmitter<OpenCollection> for AppState {}
impl EventEmitter<OpenSearch> for AppState {}
impl EventEmitter<CloseCollection> for AppState {}

/// Runs a blocking secret-store or file call on tokio's blocking pool.
fn blocking<T: Send + 'static>(cx: &App, f: impl FnOnce() -> T + Send + 'static) -> JoinHandle<T> {
    cx.global::<Tokio>().0.spawn_blocking(f)
}

/// Awaits a secret-store job from a GPUI task.
fn secret_task(
    job: JoinHandle<Result<(), SecretError>>,
    cx: &mut Context<AppState>,
) -> Task<Result<(), SecretError>> {
    cx.spawn(async move |_, _| {
        job.await
            .unwrap_or_else(|e| Err(SecretError::new(e.to_string())))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::{Arc, Mutex};

    use gpui_kit::TestAppContext;
    use secrecy::SecretString;
    use tessera_client::{Client, ClientOptions, Error, KeyAccess, RaftState};
    use tessera_core::ServerCapabilities;
    use tessera_core::profile::{Node, Protocol};
    use tessera_core::search::history::HistoryEntry;
    use tessera_core::secrets::MemoryStore;

    use super::*;
    use crate::connection::ServerInfo;
    use crate::runtime::Tokio;

    fn node() -> Node {
        Node {
            protocol: Protocol::Http,
            host: "localhost".into(),
            port: 8108,
            path: String::new(),
        }
    }

    fn connected() -> Connected {
        let client = Client::new(
            &["http://localhost:8108"],
            SecretString::from("k"),
            ClientOptions::default(),
        )
        .unwrap();
        Connected {
            info: ServerInfo {
                url: client.base_url().clone(),
                version: "30.2".into(),
                state: RaftState::Leader,
                healthy: true,
                capabilities: ServerCapabilities::from_version("30.2"),
                latency: std::time::Duration::from_millis(3),
            },
            access: KeyAccess::Admin,
            client,
        }
    }

    /// Installs a real tokio runtime. Keychain and file I/O finish on its threads, so the
    /// test scheduler must be allowed to park until they wake it.
    fn tokio(cx: &mut TestAppContext) -> tokio::runtime::Runtime {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();
        cx.update(|cx| cx.set_global(Tokio(handle)));
        cx.executor().allow_parking();
        rt
    }

    /// Installs a fake connector that records (profile name, key) and answers with `result`.
    fn fake(
        cx: &mut TestAppContext,
        result: impl Fn() -> Result<Connected, Error> + Send + Sync + 'static,
    ) -> Arc<Mutex<Vec<(String, String)>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let result = Arc::new(result);
        cx.update(|cx| {
            cx.set_global(ConnectorGlobal(Arc::new(
                move |profile: Profile, key, _hook| {
                    record
                        .lock()
                        .unwrap()
                        .push((profile.name, key.expose_secret().to_string()));
                    let r = result();
                    Box::pin(async move { r })
                },
            )));
        });
        seen
    }

    fn new_state(
        cx: &mut TestAppContext,
        loaded: Result<ConnectionsFile, store::StoreError>,
        path: Option<std::path::PathBuf>,
        secrets: Arc<MemoryStore>,
    ) -> Entity<AppState> {
        cx.new(|cx| {
            let mut s = AppState::new(loaded, path, secrets);
            s.spawn_log_drain(cx);
            s
        })
    }

    #[gpui_kit::test]
    async fn saves_are_serialized(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            Some(path.clone()),
            Arc::default(),
        );
        let mut profile = Profile::new("first", vec![node()]);
        state.update(cx, |s, cx| {
            s.upsert_profile(profile.clone(), None, cx).detach();
            // Many edits in a row make an unserialized write order visible.
            for i in 0..20 {
                profile.name = format!("edit {i}");
                s.upsert_profile(profile.clone(), None, cx).detach();
            }
            profile.name = "second".into();
            s.upsert_profile(profile.clone(), None, cx).detach();
        });
        let last = state
            .update(cx, |s, _| s.save_task.take())
            .expect("a save is pending");
        last.await;
        cx.run_until_parked();
        let file = store::load(&path).unwrap();
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.profiles[0].name, "second");
    }

    #[gpui_kit::test]
    fn unparseable_file_is_never_overwritten(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, "{ not json").unwrap();
        let state = new_state(cx, store::load(&path), Some(path.clone()), Arc::default());
        state.update(cx, |s, cx| {
            assert!(s.load_error.is_some());
            s.upsert_profile(Profile::new("x", vec![node()]), None, cx)
                .detach();
            assert_eq!(s.profiles.len(), 1, "kept in memory");
            assert!(s.save_task.is_none(), "no save was started");
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[gpui_kit::test]
    fn temporary_launch_connects_without_saving(cx: &mut TestAppContext) {
        let seen = fake(cx, || Ok(connected()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let secrets = Arc::new(MemoryStore::default());
        let mut s = AppState::new(
            Ok(ConnectionsFile::default()),
            Some(path.clone()),
            secrets.clone(),
        );
        let id = s
            .resolve(Launch::Temporary {
                profile: Profile::new("tmp", vec![node()]),
                key_from_stdin: false,
                key_from_env: Some(SecretString::from("k-tmp")),
            })
            .unwrap()
            .expect("temporary connection opens");
        let state = cx.new(|cx| {
            s.spawn_log_drain(cx);
            s
        });
        state.update(cx, |s, cx| s.connect(id, None, cx));
        cx.run_until_parked();
        state.read_with(cx, |s, cx| {
            assert!(s.profiles.is_empty());
            assert_eq!(s.selected, Some(id));
            let c = s.connection(id, cx).expect("connection exists");
            let c = c.read(cx);
            assert!(c.connected.is_some());
            assert!(!c.connecting);
            assert!(c.error.is_none());
        });
        assert_eq!(
            *seen.lock().unwrap(),
            [("tmp".to_string(), "k-tmp".to_string())]
        );
        assert!(!path.exists(), "nothing written");
        assert!(secrets.get(id).unwrap().is_none(), "no keychain write");
    }

    #[gpui_kit::test]
    async fn saved_profile_connects_with_stored_key(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        let seen = fake(cx, || Ok(connected()));
        let profile = Profile::new("saved", vec![node()]);
        let id = profile.id;
        let secrets = Arc::new(MemoryStore::default());
        secrets.set(id, &SecretString::from("k-saved")).unwrap();
        let file = ConnectionsFile {
            profiles: vec![profile],
            ..Default::default()
        };
        let state = new_state(cx, Ok(file), None, secrets);
        let attempt = state.update(cx, |s, cx| {
            s.connect(id, None, cx);
            let c = s.connection(id, cx).unwrap();
            assert!(c.read(cx).connecting);
            c.update(cx, |c, _| c.task.take()).unwrap()
        });
        attempt.await;
        state.update(cx, |s, cx| {
            let c = s.connection(id, cx).unwrap();
            assert!(c.read(cx).connected.is_some());
            assert_eq!(s.last_used, Some(id));
            s.connect(id, None, cx);
            assert!(
                !c.read(cx).connecting,
                "a connected profile is only selected"
            );
        });
        assert_eq!(
            *seen.lock().unwrap(),
            [("saved".to_string(), "k-saved".to_string())]
        );
    }

    #[gpui_kit::test]
    fn connect_failure_is_kept_on_the_connection(cx: &mut TestAppContext) {
        rust_i18n::set_locale("en");
        fake(cx, || {
            Err(Error::Http {
                status: 401,
                message: "Forbidden - a valid `x-typesense-api-key` header must be sent.".into(),
                body: String::new(),
            })
        });
        let mut s = AppState::new(
            Ok(ConnectionsFile::default()),
            None,
            Arc::new(MemoryStore::default()),
        );
        let id = s
            .resolve(Launch::Temporary {
                profile: Profile::new("tmp", vec![node()]),
                key_from_stdin: false,
                key_from_env: Some(SecretString::from("k")),
            })
            .unwrap()
            .unwrap();
        let state = cx.new(|cx| {
            s.spawn_log_drain(cx);
            s
        });
        state.update(cx, |s, cx| s.connect(id, None, cx));
        cx.run_until_parked();
        state.read_with(cx, |s, cx| {
            let c = s.connection(id, cx).unwrap();
            let c = c.read(cx);
            assert!(c.connected.is_none());
            assert!(!c.connecting);
            let error = c.error.clone().unwrap();
            assert!(error.contains("x-typesense-api-key"), "{error}");
        });
    }

    #[gpui_kit::test]
    async fn delete_removes_profile_key_and_connection(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        fake(cx, || Ok(connected()));
        let profile = Profile::new("gone", vec![node()]);
        let id = profile.id;
        let secrets = Arc::new(MemoryStore::default());
        let state = new_state(cx, Ok(ConnectionsFile::default()), None, secrets.clone());
        let stored = state.update(cx, |s, cx| {
            s.upsert_profile(profile, Some(SecretString::from("k")), cx)
        });
        stored.await.unwrap();
        assert!(secrets.get(id).unwrap().is_some());
        let attempt = state.update(cx, |s, cx| {
            s.connect(id, None, cx);
            s.connection(id, cx)
                .unwrap()
                .update(cx, |c, _| c.task.take())
                .unwrap()
        });
        attempt.await;
        let deleted = state.update(cx, |s, cx| s.delete_profile(id, cx));
        deleted.await.unwrap();
        state.read_with(cx, |s, cx| {
            assert!(s.profiles.is_empty());
            assert!(s.connection(id, cx).is_none());
            assert_eq!(s.selected, None);
            assert_eq!(s.last_used, None);
        });
        assert!(secrets.get(id).unwrap().is_none());
    }

    /// A store whose reads always fail, to prove a key was not read back.
    struct NoReads;

    impl SecretStore for NoReads {
        fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretError> {
            Err(SecretError::new("read attempted"))
        }
        fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretError> {
            Ok(())
        }
        fn delete(&self, _: Uuid) -> Result<(), SecretError> {
            Ok(())
        }
    }

    fn error_of(state: &Entity<AppState>, id: Uuid, cx: &mut TestAppContext) -> Option<String> {
        state.read_with(cx, |s, cx| {
            let c = s.connection(id, cx).unwrap();
            let c = c.read(cx);
            assert!(!c.connecting);
            c.error.as_ref().map(|e| e.to_string())
        })
    }

    #[gpui_kit::test]
    async fn missing_key_does_not_call_the_connector(cx: &mut TestAppContext) {
        rust_i18n::set_locale("en");
        let _rt = tokio(cx);
        let seen = fake(cx, || Ok(connected()));
        let saved = Profile::new("saved", vec![node()]);
        let saved_id = saved.id;
        let file = ConnectionsFile {
            profiles: vec![saved],
            ..Default::default()
        };
        let mut s = AppState::new(Ok(file), None, Arc::new(MemoryStore::default()));
        let temp_id = s
            .resolve(Launch::Temporary {
                profile: Profile::new("tmp", vec![node()]),
                key_from_stdin: false,
                key_from_env: None,
            })
            .unwrap()
            .unwrap();
        let state = cx.new(|cx| {
            s.spawn_log_drain(cx);
            s
        });
        let attempt = state.update(cx, |s, cx| {
            s.connect(temp_id, None, cx);
            s.connect(saved_id, None, cx);
            s.connection(saved_id, cx)
                .unwrap()
                .update(cx, |c, _| c.task.take())
                .unwrap()
        });
        attempt.await;
        cx.run_until_parked();
        for id in [temp_id, saved_id] {
            let error = error_of(&state, id, cx).expect("inline error");
            assert!(error.contains("No API key"), "{error}");
        }
        assert!(seen.lock().unwrap().is_empty(), "connector was called");
    }

    #[gpui_kit::test]
    fn given_key_skips_the_keychain(cx: &mut TestAppContext) {
        let seen = fake(cx, || Ok(connected()));
        let profile = Profile::new("saved", vec![node()]);
        let id = profile.id;
        let file = ConnectionsFile {
            profiles: vec![profile],
            ..Default::default()
        };
        let state = cx.new(|cx| {
            let mut s = AppState::new(Ok(file), None, Arc::new(NoReads));
            s.spawn_log_drain(cx);
            s
        });
        state.update(cx, |s, cx| {
            s.connect(id, Some(SecretString::from("k-new")), cx)
        });
        cx.run_until_parked();
        assert_eq!(error_of(&state, id, cx), None);
        assert_eq!(
            *seen.lock().unwrap(),
            [("saved".to_string(), "k-new".to_string())]
        );
    }

    #[gpui_kit::test]
    fn reconnect_uses_the_edited_profile(cx: &mut TestAppContext) {
        let seen = fake(cx, || Err(Error::Timeout));
        let mut profile = Profile::new("before", vec![node()]);
        let id = profile.id;
        let state = new_state(cx, Ok(ConnectionsFile::default()), None, Arc::default());
        state.update(cx, |s, cx| {
            s.upsert_profile(profile.clone(), None, cx).detach();
            s.connect(id, Some(SecretString::from("k")), cx);
        });
        cx.run_until_parked();
        profile.name = "after".into();
        state.update(cx, |s, cx| {
            s.upsert_profile(profile.clone(), None, cx).detach();
            s.connect(id, Some(SecretString::from("k")), cx);
        });
        cx.run_until_parked();
        let names: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        assert_eq!(names, ["before", "after"]);
        state.read_with(cx, |s, cx| {
            assert_eq!(s.connection(id, cx).unwrap().read(cx).profile.name, "after");
        });
    }

    #[gpui_kit::test]
    fn invalid_node_shows_its_reason(cx: &mut TestAppContext) {
        rust_i18n::set_locale("en");
        let seen = fake(cx, || Ok(connected()));
        let mut bad = node();
        bad.host = "ts.example.com/x".into();
        let profile = Profile::new("bad", vec![bad]);
        let id = profile.id;
        let file = ConnectionsFile {
            profiles: vec![profile],
            ..Default::default()
        };
        let state = cx.new(|cx| {
            let mut s = AppState::new(Ok(file), None, Arc::new(NoReads));
            s.spawn_log_drain(cx);
            s
        });
        state.update(cx, |s, cx| s.connect(id, Some(SecretString::from("k")), cx));
        cx.run_until_parked();
        let error = error_of(&state, id, cx).expect("inline error");
        assert!(error.contains("ts.example.com/x"), "{error}");
        assert!(
            !error.contains("http://"),
            "not the URL format text: {error}"
        );
        assert!(seen.lock().unwrap().is_empty());
    }

    #[gpui_kit::test]
    fn pending_save_finishes_on_quit(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        // Poll the quit handler for the whole SHUTDOWN_TIMEOUT, not a random number of ticks.
        cx.executor().set_block_on_ticks(usize::MAX..=usize::MAX);
        // The write runs on a real tokio thread; without parking the test scheduler gives up
        // as soon as nothing is runnable instead of waiting out SHUTDOWN_TIMEOUT (flaky on CI).
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            Some(path.clone()),
            Arc::default(),
        );
        state.update(cx, |s, cx| {
            s.upsert_profile(Profile::new("kept", vec![node()]), None, cx)
                .detach();
        });
        cx.quit();
        let file = store::load(&path).unwrap();
        assert_eq!(file.profiles.len(), 1, "save finished before quit returned");
    }

    #[test]
    fn unknown_profile_name_is_an_error() {
        let mut s = AppState::new(
            Ok(ConnectionsFile {
                profiles: vec![Profile::new("prod-eu", vec![node()])],
                ..Default::default()
            }),
            None,
            Arc::new(MemoryStore::default()),
        );
        assert!(s.resolve(Launch::Profile("prod".into())).is_err());
        assert!(
            s.resolve(Launch::Profile("prod-eu".into()))
                .unwrap()
                .is_some()
        );
    }

    #[gpui_kit::test]
    async fn favorite_is_saved(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let profile = Profile::new("saved", vec![node()]);
        let id = profile.id;
        let file = ConnectionsFile {
            profiles: vec![profile],
            ..Default::default()
        };
        let state = new_state(
            cx,
            Ok(file),
            Some(path.clone()),
            Arc::new(MemoryStore::default()),
        );
        let save = state.update(cx, |s, cx| s.toggle_favorite(id, "books", cx));
        save.await.unwrap();
        let saved = store::load(&path).unwrap();
        assert_eq!(saved.profiles[0].favorite_collections, ["books"]);
        let save = state.update(cx, |s, cx| s.toggle_favorite(id, "books", cx));
        save.await.unwrap();
        assert!(
            store::load(&path).unwrap().profiles[0]
                .favorite_collections
                .is_empty()
        );
    }

    #[gpui_kit::test]
    fn temporary_favorite_is_not_saved(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let profile = Profile::new("tmp", vec![node()]);
        let id = profile.id;
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            Some(path.clone()),
            Arc::new(MemoryStore::default()),
        );
        state.update(cx, |s, cx| {
            s.temporary = Some((profile, SecretString::from("k")));
            s.toggle_favorite(id, "books", cx).detach();
        });
        cx.run_until_parked();
        state.read_with(cx, |s, _| {
            assert_eq!(
                s.temporary.as_ref().unwrap().0.favorite_collections,
                ["books"]
            );
        });
        assert!(!path.exists(), "nothing written");
    }

    #[gpui_kit::test]
    async fn favorite_after_a_failed_load_says_it_is_not_saved(cx: &mut TestAppContext) {
        rust_i18n::set_locale("en");
        let _rt = tokio(cx);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, "{ not json").unwrap();
        let state = new_state(cx, store::load(&path), Some(path.clone()), Arc::default());
        let profile = Profile::new("kept in memory", vec![node()]);
        let id = profile.id;
        state.update(cx, |s, cx| s.upsert_profile(profile, None, cx).detach());
        let save = state.update(cx, |s, cx| s.toggle_favorite(id, "books", cx));
        let error = save.await.unwrap_err();
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("could not be read"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[gpui_kit::test]
    async fn failed_favorite_save_is_reported(cx: &mut TestAppContext) {
        let _rt = tokio(cx);
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "").unwrap();
        // The parent of the store path is a file, so the write fails.
        let path = blocker.join("connections.json");
        let profile = Profile::new("saved", vec![node()]);
        let id = profile.id;
        let file = ConnectionsFile {
            profiles: vec![profile],
            ..Default::default()
        };
        let state = new_state(cx, Ok(file), Some(path), Arc::new(MemoryStore::default()));
        let save = state.update(cx, |s, cx| s.toggle_favorite(id, "books", cx));
        assert!(save.await.is_err());
    }

    #[gpui_kit::test]
    async fn document_view_is_saved_for_a_saved_profile(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document_views.json");
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            None,
            Arc::new(MemoryStore::default()),
        );
        let id = Uuid::new_v4();
        let mut view = CollectionView::default();
        view.hidden.push("price".into());
        state.update(cx, |s, _| s.views_path = Some(path.clone()));
        let save = state.update(cx, |s, cx| {
            s.set_document_view(id, "books", view.clone(), cx)
        });
        save.await.unwrap();
        assert_eq!(views::load(&path).view(id, "books"), view);
        state.read_with(cx, |s, _| {
            assert_eq!(s.document_view(id, "books"), view);
            assert_eq!(s.document_view(id, "other"), CollectionView::default());
        });
    }

    #[gpui_kit::test]
    async fn quick_document_view_saves_land_in_order(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document_views.json");
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            None,
            Arc::new(MemoryStore::default()),
        );
        let id = Uuid::new_v4();
        state.update(cx, |s, _| s.views_path = Some(path.clone()));
        let mut saves = Vec::new();
        for n in 0..20 {
            let mut view = CollectionView::default();
            view.widths.insert("title".into(), 100. + n as f32);
            // The caller drops the task, as a closed tab does.
            saves.push(state.update(cx, |s, cx| s.set_document_view(id, "books", view, cx)));
        }
        let last = saves.pop().unwrap();
        drop(saves);
        last.await.unwrap();
        assert_eq!(views::load(&path).view(id, "books").widths["title"], 119.);
    }

    fn search_item(profile: Uuid, q: &str) -> HistoryItem {
        let mut draft = SearchDraft::new("books");
        draft.params.insert("q".into(), q.into());
        HistoryItem {
            at: 1,
            profile,
            entry: HistoryEntry::Search {
                union: false,
                searches: vec![draft],
            },
        }
    }

    fn query(profile: Uuid, name: &str) -> SavedQuery {
        SavedQuery {
            id: Uuid::new_v4(),
            profile,
            name: name.into(),
            tags: Vec::new(),
            union: false,
            searches: vec![SearchDraft::new("books")],
        }
    }

    /// State with both files in a temporary directory, already loaded.
    fn query_state(cx: &mut TestAppContext, dir: &tempfile::TempDir) -> Entity<AppState> {
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            None,
            Arc::new(MemoryStore::default()),
        );
        state.update(cx, |s, _| {
            s.load_queries(
                Some(dir.path().join("history.jsonl")),
                Some(dir.path().join("saved_queries.json")),
            );
        });
        state
    }

    #[gpui_kit::test]
    async fn history_lands_on_disk_and_clear_rewrites(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let state = query_state(cx, &dir);
        let (id, other) = (Uuid::new_v4(), Uuid::new_v4());
        state.update(cx, |s, cx| {
            s.record_history(search_item(id, "a"), cx);
            s.record_history(search_item(id, "a"), cx);
            s.record_history(search_item(other, "b"), cx);
        });
        cx.run_until_parked();
        let (loaded, lines) = history::load(&path);
        assert_eq!((loaded.searches(id, "").len(), lines), (1, 2));
        state.update(cx, |s, cx| s.clear_history(id, cx));
        cx.run_until_parked();
        let (loaded, lines) = history::load(&path);
        assert_eq!(loaded.searches(id, "").len(), 0);
        assert_eq!((loaded.searches(other, "").len(), lines), (1, 1));
    }

    #[gpui_kit::test]
    async fn a_long_file_is_rewritten(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let id = Uuid::new_v4();
        let line = |n| serde_json::to_string(&search_item(id, &format!("{n}"))).unwrap();
        let text: Vec<String> = (0..=2 * history::HISTORY_CAP).map(line).collect();
        std::fs::write(&path, text.join("\n")).unwrap();
        let state = query_state(cx, &dir);
        state.update(cx, |s, cx| s.record_history(search_item(id, "new"), cx));
        cx.run_until_parked();
        let (loaded, lines) = history::load(&path);
        assert_eq!(lines, history::HISTORY_CAP);
        assert_eq!(loaded.searches(id, "new").len(), 1);
    }

    #[gpui_kit::test]
    fn temporary_connection_writes_no_history(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (path, saved) = (
            dir.path().join("history.jsonl"),
            dir.path().join("saved_queries.json"),
        );
        let state = query_state(cx, &dir);
        let profile = Profile::new("tmp", vec![node()]);
        let (id, kept) = (profile.id, Uuid::new_v4());
        state.update(cx, |s, cx| {
            s.temporary = Some((profile, SecretString::from("k")));
            s.record_history(search_item(id, "a"), cx);
            s.save_query(query(id, "tmp"), cx).detach();
        });
        cx.run_until_parked();
        assert!(!path.exists(), "nothing written");
        state.read_with(cx, |s, _| assert_eq!(s.history.searches(id, "").len(), 1));
        // A rewrite for another connection must not carry it along.
        state.update(cx, |s, cx| {
            s.record_history(search_item(kept, "b"), cx);
            s.clear_history(kept, cx);
        });
        cx.run_until_parked();
        assert_eq!(history::load(&path).1, 0);
        assert!(saved::load(&saved).unwrap().queries.is_empty());
    }

    #[gpui_kit::test]
    async fn corrupt_saved_file_is_not_overwritten(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved_queries.json");
        std::fs::write(&path, "{not json").unwrap();
        let state = query_state(cx, &dir);
        let id = Uuid::new_v4();
        let shown = state.read_with(cx, |s, _| s.saved_error.clone()).unwrap();
        assert!(shown.contains("cannot parse"), "{shown}");
        let refused = [
            state.update(cx, |s, cx| s.save_query(query(id, "a"), cx)),
            state.update(cx, |s, cx| s.delete_query(id, cx)),
            state.update(cx, |s, cx| s.set_favorites(Vec::new(), cx)),
        ];
        for task in refused {
            assert_eq!(task.await.unwrap_err(), shown);
        }
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        state.read_with(cx, |s, _| assert!(s.saved.queries.is_empty()));
    }

    #[gpui_kit::test]
    async fn saved_queries_replace_and_delete(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved_queries.json");
        let state = query_state(cx, &dir);
        let id = Uuid::new_v4();
        let mut q = query(id, "a");
        let save = state.update(cx, |s, cx| s.save_query(q.clone(), cx));
        save.await.unwrap();
        q.name = "b".into();
        let save = state.update(cx, |s, cx| s.save_query(q.clone(), cx));
        save.await.unwrap();
        assert_eq!(saved::load(&path).unwrap().queries, [q.clone()]);
        let delete = state.update(cx, |s, cx| s.delete_query(q.id, cx));
        delete.await.unwrap();
        assert!(saved::load(&path).unwrap().queries.is_empty());
        let favorites = vec![search_item(id, "a")];
        let save = state.update(cx, |s, cx| s.set_favorites(favorites.clone(), cx));
        save.await.unwrap();
        assert_eq!(saved::load(&path).unwrap().favorites, favorites);
    }

    #[gpui_kit::test]
    async fn saves_land_in_order(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved_queries.json");
        let state = query_state(cx, &dir);
        let id = Uuid::new_v4();
        let mut saves = Vec::new();
        for n in 0..20 {
            // The caller drops the task, as a closed tab does.
            let q = query(id, &n.to_string());
            saves.push(state.update(cx, |s, cx| s.save_query(q, cx)));
        }
        let last = saves.pop().unwrap();
        drop(saves);
        last.await.unwrap();
        assert_eq!(saved::load(&path).unwrap().queries.len(), 20);
    }

    #[gpui_kit::test]
    fn quit_flushes_history(cx: &mut TestAppContext) {
        // Poll the quit handler for the whole SHUTDOWN_TIMEOUT, not a random number of ticks.
        cx.executor().set_block_on_ticks(usize::MAX..=usize::MAX);
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let state = query_state(cx, &dir);
        let id = Uuid::new_v4();
        state.update(cx, |s, cx| s.record_history(search_item(id, "a"), cx));
        cx.quit();
        assert_eq!(history::load(&path).1, 1, "written before quit returned");
    }

    #[gpui_kit::test]
    fn temporary_connection_writes_no_file(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document_views.json");
        let state = new_state(
            cx,
            Ok(ConnectionsFile::default()),
            None,
            Arc::new(MemoryStore::default()),
        );
        let profile = Profile::new("tmp", vec![node()]);
        let id = profile.id;
        let mut view = CollectionView::default();
        view.hidden.push("price".into());
        state.update(cx, |s, cx| {
            s.views_path = Some(path.clone());
            s.temporary = Some((profile, SecretString::from("k")));
            s.set_document_view(id, "books", view.clone(), cx).detach();
        });
        cx.run_until_parked();
        assert!(!path.exists(), "nothing written");
        state.read_with(cx, |s, _| assert_eq!(s.document_view(id, "books"), view));
    }
}
