// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide state: saved profiles, the secret store and open connections.
//!
//! Keychain and file I/O never run on the GPUI thread: they go to tokio's `spawn_blocking`
//! through the [`Tokio`] global (`docs/architecture/async.md`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Context, Entity, Subscription, Task};
use rust_i18n::t;
use secrecy::{ExposeSecret as _, SecretString};
use tessera_core::cli::Launch;
use tessera_core::import;
use tessera_core::profile::Profile;
use tessera_core::secrets::{SecretError, SecretStore};
use tessera_core::store::{self, ConnectionsFile, SCHEMA_VERSION, StoreError};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::connection::{Connected, ConnectionState, ConnectorGlobal, client_options, error_text};
use crate::runtime::Tokio;

/// Why saving a profile failed. Neither variant holds the key.
#[derive(Debug)]
pub enum SaveError {
    /// `connections.json` could not be written; the text is the reason.
    File(String),
    /// The key could not be stored in the secret store.
    Keychain(SecretError),
}

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
        Self {
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
        }
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
                cx.observe(&conn, |_, _, cx| cx.notify()).detach();
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
                Ok(Some(key)) if !key.expose_secret().is_empty() => connect(profile.clone(), key)
                    .await
                    .map_err(|e| error_text(&e, &profile)),
                Ok(_) => Err(t!("connect.error.no_key").to_string()),
                Err(e) => Err(t!("connect.error.keychain", detail = e.to_string()).to_string()),
            };
            let ok = result.is_ok();
            weak.update(cx, |c, cx| {
                c.connecting = false;
                c.missing_key = missing_key;
                match result {
                    Ok(connected) => c.connected = Some(connected),
                    Err(text) => c.error = Some(text.into()),
                }
                cx.notify();
            })
            .ok();
            if ok && saved {
                this.update(cx, |s, cx| {
                    s.last_used = Some(id);
                    s.save(cx).detach();
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
        state.connected = Some(connected);
        let conn = cx.new(|_| state);
        cx.observe(&conn, |_, _, cx| cx.notify()).detach();
        self.connections.push(conn);
        self.selected = Some(id);
        cx.notify();
    }

    /// Closes the connection of profile `id`. Dropping it cancels a running attempt.
    pub fn disconnect(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.connections.retain(|c| c.read(cx).profile.id != id);
        if self.selected == Some(id) {
            self.selected = self.connections.last().map(|c| c.read(cx).profile.id);
        }
        cx.notify();
    }
}

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
            cx.set_global(ConnectorGlobal(Arc::new(move |profile: Profile, key| {
                record
                    .lock()
                    .unwrap()
                    .push((profile.name, key.expose_secret().to_string()));
                let r = result();
                Box::pin(async move { r })
            })));
        });
        seen
    }

    fn new_state(
        cx: &mut TestAppContext,
        loaded: Result<ConnectionsFile, store::StoreError>,
        path: Option<std::path::PathBuf>,
        secrets: Arc<MemoryStore>,
    ) -> Entity<AppState> {
        cx.new(|_| AppState::new(loaded, path, secrets))
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
        let state = cx.new(|_| s);
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
        let state = cx.new(|_| s);
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
        let state = cx.new(|_| s);
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
        let state = cx.new(|_| AppState::new(Ok(file), None, Arc::new(NoReads)));
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
        let state = cx.new(|_| AppState::new(Ok(file), None, Arc::new(NoReads)));
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
}
