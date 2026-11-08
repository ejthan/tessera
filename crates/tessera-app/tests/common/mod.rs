// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers shared by the UI test binaries.

// Each test binary uses only some of the helpers.
#![allow(dead_code)]
// Helpers unwrap; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, base::Root, px, size,
};
use secrecy::ExposeSecret as _;
use tessera_app::collections::{CollectionsBackend, CollectionsBackendGlobal};
use tessera_app::connection::{Connected, ConnectorGlobal, ServerInfo};
use tessera_app::documents::{DocumentsBackend, DocumentsBackendGlobal, PageResult};
use tessera_app::poller::{CollectionsFetcherGlobal, NodeSample, PollFetcherGlobal, SampleFuture};
use tessera_app::runtime::Tokio;
use tessera_app::state::AppState;
use tessera_app::views::dock::CollectionPanel;
use tessera_app::views::root::RootView;
use tessera_client::{
    Alias, Client, ClientOptions, Collection, CollectionSummary, Debug, DocumentQuery, Error,
    Health, KeyAccess, Metrics, NodeClient, Origin, SchemaChange, Stats,
};
use tessera_core::documents::page::DocumentPage;
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use tessera_core::secrets::{MemoryStore, SecretStore};
use tessera_core::store::{ConnectionsFile, StoreError};

pub fn open(cx: &mut TestAppContext) -> WindowHandle<Root> {
    open_with(cx, Ok(ConnectionsFile::default()))
}

pub fn open_with(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
) -> WindowHandle<Root> {
    open_state(cx, loaded, Arc::new(MemoryStore::default())).0
}

pub fn open_state(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
    secrets: Arc<dyn SecretStore>,
) -> (WindowHandle<Root>, Entity<AppState>) {
    open_state_at(cx, loaded, None, secrets)
}

/// Like [`open_state`], with profiles saved to `path`.
pub fn open_state_at(
    cx: &mut TestAppContext,
    loaded: Result<ConnectionsFile, StoreError>,
    path: Option<std::path::PathBuf>,
    secrets: Arc<dyn SecretStore>,
) -> (WindowHandle<Root>, Entity<AppState>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        tessera_app::actions::bind_keys(cx);
        install_poll_fakes(cx);
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(800.)),
        };
        let state = cx.new(|cx| {
            let mut state = AppState::new(loaded, path, secrets);
            state.spawn_log_drain(cx);
            state
        });
        let root_state = state.clone();
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| RootView::new(root_state, window, cx)),
        )
        .expect("open test window");
        (window.downcast::<Root>().expect("Base Root"), state)
    })
}

/// Installs a real tokio runtime: keychain and file I/O run on its blocking pool, so the
/// test scheduler must be allowed to park until it answers.
pub fn tokio(cx: &mut TestAppContext) -> tokio::runtime::Runtime {
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

/// A window with a tokio runtime, the given profiles and secret store.
pub fn open_app(
    cx: &mut TestAppContext,
    profiles: Vec<Profile>,
    secrets: Arc<dyn SecretStore>,
) -> (
    WindowHandle<Root>,
    Entity<AppState>,
    tokio::runtime::Runtime,
) {
    let rt = tokio(cx);
    let file = ConnectionsFile {
        profiles,
        ..Default::default()
    };
    let (handle, state) = open_state(cx, Ok(file), secrets);
    (handle, state, rt)
}

/// A node sample where every request succeeded.
pub fn ok_sample() -> NodeSample {
    NodeSample {
        health: Ok(Health { ok: true }),
        debug: Ok(Debug {
            version: "30.2".into(),
            state: tessera_client::RaftState::Leader,
        }),
        metrics: Ok(Metrics {
            cpu_percent: Some(12.5),
            memory_used: Some(1),
            memory_total: Some(4),
            disk_used: Some(1),
            disk_total: Some(2),
            net_received: Some(0),
            net_sent: Some(0),
            typesense_memory_active: Some(1),
            raw: Default::default(),
        }),
        stats: Ok(Stats {
            latency_ms: Default::default(),
            requests_per_second: Default::default(),
            total_requests_per_second: None,
            raw: Default::default(),
        }),
        latency: Duration::from_millis(3),
    }
}

/// Unless the test installed its own, a node fetcher that answers with [`ok_sample`] and a
/// collections loader that answers with no collections.
fn install_poll_fakes(cx: &mut gpui_kit::App) {
    if !cx.has_global::<PollFetcherGlobal>() {
        cx.set_global(PollFetcherGlobal(Arc::new(|_| {
            Box::pin(async { ok_sample() })
        })));
    }
    if !cx.has_global::<CollectionsFetcherGlobal>() {
        cx.set_global(CollectionsFetcherGlobal(Arc::new(|_| {
            Box::pin(async { Ok(Vec::new()) })
        })));
    }
    if !cx.has_global::<CollectionsBackendGlobal>() {
        cx.set_global(CollectionsBackendGlobal(Arc::new(FakeBackend(
            Arc::default(),
        ))));
    }
    if !cx.has_global::<DocumentsBackendGlobal>() {
        cx.set_global(DocumentsBackendGlobal(Arc::new(FakeDocsBackend(
            Arc::default(),
        ))));
    }
}

/// What a [`FakeBackend`] answers and has been asked. `data: None` makes every call time out.
#[derive(Default)]
pub struct FakeData {
    pub data: Mutex<Option<(Vec<Collection>, Vec<Alias>)>>,
    /// `list` fails to load the aliases with this status and Typesense message.
    pub aliases_error: Mutex<Option<(u16, String)>>,
    /// Number of `list` calls.
    pub lists: Mutex<usize>,
    /// Names asked of `get`.
    pub gets: Mutex<Vec<String>>,
    /// Bodies sent to `create`.
    pub creates: Mutex<Vec<serde_json::Value>>,
    /// `create` fails with this status and Typesense message; `None` creates the collection
    /// (and adds it to `data`).
    pub create_error: Mutex<Option<(u16, String)>>,
    /// Names and bodies sent to `update`.
    pub updates: Mutex<Vec<(String, serde_json::Value)>>,
    /// `update` fails with this status and Typesense message.
    pub update_error: Mutex<Option<(u16, String)>>,
    /// When set, the next `update` answers only once the sender fires or is dropped.
    pub update_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Names sent to `truncate`; it sets the document count to 0 in `data`.
    pub truncates: Mutex<Vec<String>>,
    /// Names sent to `delete`; it removes the collection from `data`.
    pub deletes: Mutex<Vec<String>>,
    /// `truncate` and `delete` fail with this status and Typesense message.
    pub danger_error: Mutex<Option<(u16, String)>>,
    /// When set, the next `truncate` or `delete` answers only once the sender fires or is
    /// dropped; its effect on `data` happens then.
    pub danger_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// What `schema_changes` answers.
    pub schema_changes: Mutex<Vec<SchemaChange>>,
    /// Number of `schema_changes` calls.
    pub schema_change_calls: Mutex<usize>,
    /// Origin of the client of each `schema_changes` call.
    pub schema_change_origins: Mutex<Vec<Origin>>,
}

impl FakeData {
    /// Holds the next `update` until the returned sender fires or is dropped.
    pub fn hold_update(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.update_gate.lock().unwrap() = Some(rx);
        tx
    }

    /// Holds the next `truncate` or `delete` until the returned sender fires or is dropped.
    pub fn hold_danger(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.danger_gate.lock().unwrap() = Some(rx);
        tx
    }
}

/// Status that the fake errors below turn into [`Error::Timeout`] instead of an HTTP error.
pub const TIMEOUT: u16 = 0;

/// An HTTP error with `status` and a Typesense message, or a timeout for [`TIMEOUT`].
pub fn http_error((status, message): (u16, String)) -> Error {
    if status == TIMEOUT {
        return Error::Timeout;
    }
    Error::Http {
        status,
        body: serde_json::json!({ "message": message }).to_string(),
        message,
    }
}

/// The installed backend; the test keeps the [`FakeData`] handle.
pub struct FakeBackend(pub Arc<FakeData>);

impl FakeBackend {
    fn danger_answer(&self) -> Result<(), Error> {
        match self.0.danger_error.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(()),
        }
    }
}

impl CollectionsBackend for FakeBackend {
    fn list(
        &self,
        _: Client,
    ) -> tessera_app::collections::BoxFuture<Result<tessera_app::collections::CollectionList, Error>>
    {
        *self.0.lists.lock().unwrap() += 1;
        let aliases_error = self.0.aliases_error.lock().unwrap().clone();
        let answer = self.0.data.lock().unwrap().clone().ok_or(Error::Timeout);
        let answer = answer.map(|(all, aliases)| match aliases_error {
            Some(e) => (all, Err(http_error(e))),
            None => (all, Ok(aliases)),
        });
        Box::pin(async move { answer })
    }

    fn get(
        &self,
        _: Client,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<Collection, Error>> {
        self.0.gets.lock().unwrap().push(name.clone());
        let found = self
            .0
            .data
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|(c, _)| c.iter().find(|c| c.name == name).cloned());
        Box::pin(async move { found.ok_or(Error::Timeout) })
    }

    fn create(
        &self,
        _: Client,
        schema: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<Collection, Error>> {
        self.0.creates.lock().unwrap().push(schema.clone());
        let answer = match self.0.create_error.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => {
                let created: Collection = serde_json::from_value(schema).unwrap();
                if let Some((all, _)) = self.0.data.lock().unwrap().as_mut() {
                    all.push(created.clone());
                }
                Ok(created)
            }
        };
        Box::pin(async move { answer })
    }

    fn update(
        &self,
        _: Client,
        name: String,
        patch: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        self.0.updates.lock().unwrap().push((name, patch));
        let answer = match self.0.update_error.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(()),
        };
        let gate = self.0.update_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.await.ok();
            }
            answer
        })
    }

    fn truncate(
        &self,
        _: Client,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<u64, Error>> {
        self.0.truncates.lock().unwrap().push(name.clone());
        let answer = self.danger_answer().map(|()| {
            let mut data = self.0.data.lock().unwrap();
            let found = data
                .as_mut()
                .and_then(|(all, _)| all.iter_mut().find(|c| c.name == name));
            found.map_or(0, |c| std::mem::take(&mut c.num_documents))
        });
        Box::pin(async move { answer })
    }

    fn delete(
        &self,
        _: Client,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        self.0.deletes.lock().unwrap().push(name.clone());
        let answer = self.danger_answer();
        let gate = self.0.danger_gate.lock().unwrap().take();
        let data = self.0.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.await.ok();
            }
            answer.inspect(|()| {
                if let Some((all, _)) = data.data.lock().unwrap().as_mut() {
                    all.retain(|c| c.name != name);
                }
            })
        })
    }

    fn schema_changes(
        &self,
        client: Client,
    ) -> tessera_app::collections::BoxFuture<Result<Vec<SchemaChange>, Error>> {
        *self.0.schema_change_calls.lock().unwrap() += 1;
        self.0
            .schema_change_origins
            .lock()
            .unwrap()
            .push(client.origin());
        let answer = self.0.schema_changes.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }
}

/// Installs a collections backend answering with `data` (`None`: every call times out) and
/// returns the handle to change the answer and inspect the calls.
pub fn fake_backend(
    cx: &mut TestAppContext,
    data: Option<(Vec<Collection>, Vec<Alias>)>,
) -> Arc<FakeData> {
    let backend = Arc::new(FakeData {
        data: Mutex::new(data),
        ..Default::default()
    });
    let installed = FakeBackend(backend.clone());
    cx.update(|cx| cx.set_global(CollectionsBackendGlobal(Arc::new(installed))));
    backend
}

/// A documents backend that synthesises rows for any page, and records what it was asked.
#[derive(Default)]
pub struct FakeDocs {
    /// Documents matching any query.
    pub found: Mutex<u64>,
    /// Every query asked, in order.
    pub pages: Mutex<Vec<DocumentQuery>>,
    /// Answers fail with this status and Typesense message.
    pub fail: Mutex<Option<(u16, String)>>,
    /// When set, the next answer waits until the sender fires or is dropped.
    pub hold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Every document also carries this key, as a key outside the schema does.
    pub extra_key: Mutex<Option<String>>,
    /// The value of `extra_key`; `null` means "x".
    pub extra_value: Mutex<serde_json::Value>,
    /// What `facet_values` answers; `fail` makes it fail too.
    pub facets: Mutex<Vec<String>>,
    /// Collection, field and prefix of every `facet_values` call.
    pub facet_calls: Mutex<Vec<(String, String, String)>>,
    /// `document` fails with this status and Typesense message.
    pub document_fail: Mutex<Option<(u16, String)>>,
    /// Ids asked of `document`, in order.
    pub document_calls: Mutex<Vec<String>>,
    /// When set, `document` answers this instead of its synthesised document.
    pub stored: Mutex<Option<serde_json::Value>>,
    /// Every write: "create", "upsert" or "patch", the id (empty for create) and the body.
    pub writes: Mutex<Vec<(String, String, serde_json::Value)>>,
    /// Writes fail with this status and Typesense message.
    pub write_fail: Mutex<Option<(u16, String)>>,
    /// Deletes that still pass before `write_fail` applies to them.
    pub pass_deletes: Mutex<usize>,
    /// A filter delete removes this many documents fewer than it names.
    pub filter_short: Mutex<u64>,
    /// Ids of the first rows, in place of their numbers.
    pub ids: Mutex<Vec<String>>,
    /// The filter of every `count`, in order.
    pub count_calls: Mutex<Vec<Option<String>>>,
    /// What `update_by_filter` answers instead of the documents matching.
    pub num_updated: Mutex<Option<u64>>,
    /// The filter of every `export`, in order.
    pub export_calls: Mutex<Vec<String>>,
    /// `export` fails with this status and Typesense message.
    pub export_fail: Mutex<Option<(u16, String)>>,
}

impl FakeDocs {
    /// Holds the next answer until the returned sender fires or is dropped.
    pub fn hold_next(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.hold.lock().unwrap() = Some(rx);
        tx
    }

    /// Records a write and answers with `document`, or with the failure the test set.
    fn write(
        &self,
        kind: &str,
        id: String,
        body: serde_json::Value,
        document: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        self.writes.lock().unwrap().push((kind.into(), id, body));
        let answer = match self.write_fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(document),
        };
        let hold = self.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            answer
        })
    }

    /// Records a delete of `removed` documents and answers with `value`, or with the failure
    /// the test set once `pass_deletes` ran out.
    fn remove<T: Send + 'static>(
        &self,
        kind: &str,
        id: String,
        body: serde_json::Value,
        removed: u64,
        value: T,
    ) -> tessera_app::collections::BoxFuture<Result<T, Error>> {
        self.writes.lock().unwrap().push((kind.into(), id, body));
        let mut pass = self.pass_deletes.lock().unwrap();
        let answer = match self.write_fail.lock().unwrap().clone() {
            Some(e) if *pass == 0 => Err(http_error(e)),
            _ => {
                *pass = pass.saturating_sub(1);
                let mut found = self.found.lock().unwrap();
                *found = found.saturating_sub(removed);
                Ok(value)
            }
        };
        let hold = self.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            answer
        })
    }

    /// Page numbers asked, in order.
    pub fn page_numbers(&self) -> Vec<u32> {
        self.pages.lock().unwrap().iter().map(|q| q.page).collect()
    }

    /// The `sort_by` of the last query.
    pub fn last_sort(&self) -> Option<String> {
        self.pages.lock().unwrap().last()?.sort_by.clone()
    }

    /// The answer to `query`: a descending sort numbers the rows from the end.
    pub fn answer(&self, query: &DocumentQuery) -> PageResult {
        let found = *self.found.lock().unwrap();
        let extra = self.extra_key.lock().unwrap().clone();
        let value = self.extra_value.lock().unwrap().clone();
        let ids = self.ids.lock().unwrap().clone();
        let start = u64::from(query.page - 1) * u64::from(query.per_page);
        let end = (start + u64::from(query.per_page)).min(found);
        let descending = query.sort_by.as_ref().is_some_and(|s| s.ends_with(":desc"));
        let excluded = |key: &String| query.exclude_fields.as_ref() == Some(key);
        let docs = (start..end)
            .map(|row| {
                let n = if descending { found - 1 - row } else { row };
                let id = ids.get(n as usize).cloned();
                let mut doc = serde_json::json!({
                    "id": id.unwrap_or_else(|| n.to_string()),
                    "title": format!("t{n}"),
                    "price": n,
                });
                if let Some(key) = extra.as_ref().filter(|k| !excluded(k)) {
                    doc[key] = if value.is_null() {
                        "x".into()
                    } else {
                        value.clone()
                    };
                }
                doc.as_object().unwrap().clone()
            })
            .collect();
        PageResult {
            found,
            out_of: found,
            search_time_ms: 3,
            page: DocumentPage::from_documents(docs),
        }
    }
}

/// The installed backend; the test keeps the [`FakeDocs`] handle.
pub struct FakeDocsBackend(pub Arc<FakeDocs>);

impl DocumentsBackend for FakeDocsBackend {
    fn page(
        &self,
        _: Client,
        _: String,
        query: DocumentQuery,
    ) -> tessera_app::collections::BoxFuture<Result<PageResult, Error>> {
        self.0.pages.lock().unwrap().push(query.clone());
        let hold = self.0.hold.lock().unwrap().take();
        let answer = match self.0.fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(self.0.answer(&query)),
        };
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            answer
        })
    }

    fn facet_values(
        &self,
        _: Client,
        collection: String,
        field: String,
        prefix: String,
    ) -> tessera_app::collections::BoxFuture<Result<Vec<String>, Error>> {
        let calls = &self.0.facet_calls;
        calls.lock().unwrap().push((collection, field, prefix));
        let answer = match self.0.fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(self.0.facets.lock().unwrap().clone()),
        };
        Box::pin(async move { answer })
    }

    fn document(
        &self,
        _: Client,
        _: String,
        id: String,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        let answer = match self.0.document_fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(self.0.stored.lock().unwrap().clone().unwrap_or_else(|| {
                serde_json::json!({
                    "id": id, "title": format!("t{id}"), "embedding": [0.25, 0.5],
                })
            })),
        };
        self.0.document_calls.lock().unwrap().push(id);
        let hold = self.0.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            answer
        })
    }

    fn create(
        &self,
        _: Client,
        _: String,
        document: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        let mut stored = document.clone();
        stored["id"] = "new".into();
        if self.0.write_fail.lock().unwrap().is_none() {
            *self.0.found.lock().unwrap() += 1;
        }
        self.0.write("create", String::new(), document, stored)
    }

    fn upsert(
        &self,
        _: Client,
        _: String,
        document: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        let id = document["id"].as_str().unwrap_or_default().to_string();
        self.0.write("upsert", id, document.clone(), document)
    }

    fn patch(
        &self,
        _: Client,
        _: String,
        id: String,
        patch: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        let mut stored = self.0.stored.lock().unwrap().clone().unwrap_or_else(|| {
            serde_json::json!({
                "id": id, "title": format!("t{id}"), "embedding": [0.25, 0.5],
            })
        });
        for (key, value) in patch.as_object().unwrap() {
            stored[key] = value.clone();
        }
        self.0.write("patch", id, patch, stored)
    }

    fn delete(
        &self,
        _: Client,
        _: String,
        id: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        self.0.remove("delete", id, serde_json::Value::Null, 1, ())
    }

    fn delete_by_filter(
        &self,
        _: Client,
        _: String,
        filter: String,
    ) -> tessera_app::collections::BoxFuture<Result<u64, Error>> {
        // Two backticks per id; a filter naming no id takes every document.
        let named = match filter.matches('`').count() as u64 / 2 {
            0 => *self.0.found.lock().unwrap(),
            named => named,
        };
        let removed = named.saturating_sub(*self.0.filter_short.lock().unwrap());
        self.0.remove(
            "delete_by_filter",
            filter.clone(),
            filter.into(),
            removed,
            removed,
        )
    }

    fn count(
        &self,
        _: Client,
        _: String,
        filter: Option<String>,
    ) -> tessera_app::collections::BoxFuture<Result<u64, Error>> {
        self.0.count_calls.lock().unwrap().push(filter);
        let answer = match self.0.fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(*self.0.found.lock().unwrap()),
        };
        let hold = self.0.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            answer
        })
    }

    fn update_by_filter(
        &self,
        _: Client,
        _: String,
        filter: String,
        partial: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<u64, Error>> {
        let found = *self.0.found.lock().unwrap();
        let updated = self.0.num_updated.lock().unwrap().unwrap_or(found);
        self.0
            .remove("update_by_filter", filter, partial, 0, updated)
    }

    fn export(
        &self,
        _: Client,
        _: String,
        filter: String,
    ) -> tessera_app::collections::BoxFuture<Result<String, Error>> {
        self.0.export_calls.lock().unwrap().push(filter.clone());
        let answer = match self.0.export_fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            // The ids between backticks, each as the page shows it.
            None => Ok(filter
                .split('`')
                .skip(1)
                .step_by(2)
                .map(|id| {
                    let n: u64 = id.parse().unwrap_or(0);
                    serde_json::json!({"id": id, "title": format!("t{id}"), "price": n}).to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"),
        };
        Box::pin(async move { answer })
    }
}

/// Installs a documents backend with no documents; the test sets what it answers.
pub fn fake_documents(cx: &mut TestAppContext) -> Arc<FakeDocs> {
    let docs = Arc::new(FakeDocs::default());
    let installed = FakeDocsBackend(docs.clone());
    cx.update(|cx| cx.set_global(DocumentsBackendGlobal(Arc::new(installed))));
    docs
}

/// Installs a node fetcher that answers with `answer` and records every node URL it polls.
pub fn fake_poll(
    cx: &mut TestAppContext,
    answer: impl Fn(&NodeClient) -> NodeSample + Send + Sync + 'static,
) -> Arc<Mutex<Vec<String>>> {
    fake_poll_async(cx, move |node| {
        let sample = answer(node);
        Box::pin(async move { sample })
    })
}

/// Like [`fake_poll`], with an answer that may take time or never come.
pub fn fake_poll_async(
    cx: &mut TestAppContext,
    answer: impl Fn(&NodeClient) -> SampleFuture + Send + Sync + 'static,
) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(PollFetcherGlobal(Arc::new(move |node: NodeClient| {
            record.lock().unwrap().push(node.url().to_string());
            answer(&node)
        })));
    });
    seen
}

/// Installs a collections loader that records the origin of each client it loads through.
pub fn fake_collections(cx: &mut TestAppContext) -> Arc<Mutex<Vec<Origin>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(CollectionsFetcherGlobal(Arc::new(move |client: Client| {
            record.lock().unwrap().push(client.origin());
            Box::pin(async {
                Ok(vec![CollectionSummary {
                    name: "books".into(),
                    num_documents: 7,
                }])
            })
        })));
    });
    seen
}

/// A connection to the nodes of `profile`, as the fake connector answers it.
pub fn server_for(profile: &Profile) -> Connected {
    let (nodes, options) = tessera_app::connection::client_options(profile).unwrap();
    let nodes: Vec<&str> = nodes.iter().map(String::as_str).collect();
    let mut connected = server("30.2", true);
    connected.client = Client::new(&nodes, "k".to_string().into(), options).unwrap();
    connected.info.url = connected.client.base_url().clone();
    connected
}

pub const fn key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}
pub const SELECT_ALL: &str = key("cmd-a", "ctrl-a");
pub const DISCONNECT: &str = key("cmd-shift-d", "ctrl-shift-d");
pub const NEW_CONNECTION: &str = key("cmd-n", "ctrl-n");
pub const EDIT: &str = key("cmd-e", "ctrl-e");
pub const TEST: &str = key("cmd-t", "ctrl-t");
pub const SAVE: &str = key("cmd-enter", "ctrl-enter");
pub const TOGGLE_KEY: &str = key("alt-cmd-k", "ctrl-alt-k");
pub const DELETE: &str = key("cmd-backspace", "ctrl-backspace");
pub const TOGGLE_LOG: &str = key("cmd-j", "ctrl-j");
pub const CLOSE_TAB: &str = key("cmd-w", "ctrl-w");
pub const COPY_CURL: &str = key("cmd-shift-c", "ctrl-shift-c");
pub const REFRESH: &str = key("cmd-r", "ctrl-r");
pub const LOCK_AGAIN: &str = key("cmd-shift-l", "ctrl-shift-l");
pub const DELETE_QUERY: &str = key("cmd-shift-k", "ctrl-shift-k");
pub const UPDATE_QUERY: &str = key("cmd-shift-u", "ctrl-shift-u");
pub const SHOW_DOCUMENTS: &str = key("cmd-1", "ctrl-1");
pub const SHOW_SCHEMA: &str = key("cmd-2", "ctrl-2");
pub const SORT: &str = key("cmd-shift-s", "ctrl-shift-s");
pub const FIND: &str = key("cmd-f", "ctrl-f");
pub const CLEAR_FILTER: &str = key("cmd-shift-f", "ctrl-shift-f");
pub const PIN: &str = key("cmd-shift-p", "ctrl-shift-p");
pub const NARROWER: &str = key("cmd-shift-left", "ctrl-shift-left");
pub const WIDER: &str = key("cmd-shift-right", "ctrl-shift-right");
pub const INSPECTOR: &str = key("cmd-i", "ctrl-i");
pub const GO_TO: &str = key("cmd-g", "ctrl-g");
pub const COPY_CELL: &str = key("cmd-c", "ctrl-c");
pub const COPY_ROW: &str = key("cmd-alt-c", "ctrl-alt-c");
pub const COPY_JSONL: &str = key("cmd-alt-j", "ctrl-alt-j");
pub const COPY_CSV: &str = key("cmd-alt-s", "ctrl-alt-s");
pub const COPY_DOC_CURL: &str = key("cmd-alt-u", "ctrl-alt-u");
pub const NEW_DOCUMENT: &str = key("cmd-shift-n", "ctrl-shift-n");
pub const SAVE_MODE: &str = key("cmd-shift-m", "ctrl-shift-m");
pub const REVERT: &str = key("cmd-shift-r", "ctrl-shift-r");

/// Installs a connector that answers immediately with `result` (or derives it from the profile).
pub fn fake_connector(
    cx: &mut TestAppContext,
    answer: impl Fn(&Profile) -> Result<Connected, Error> + Send + Sync + 'static,
) {
    let answer = Arc::new(answer);
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(
            move |profile: Profile, _key, _hook| {
                let result = answer(&profile);
                Box::pin(async move { result })
            },
        )));
    });
}

/// A connector that records the keys it was called with and answers with `info`.
pub fn recording_connector(cx: &mut TestAppContext, info: Connected) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.set_global(ConnectorGlobal(Arc::new(move |_: Profile, key, _hook| {
            record.lock().unwrap().push(key.expose_secret().to_string());
            let info = info.clone();
            Box::pin(async move { Ok(info) })
        })));
    });
    seen
}

/// The open tab of collection `name`.
pub fn collection_panel(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    name: &str,
) -> Entity<CollectionPanel> {
    handle
        .update(cx, |root, _, cx| {
            let view = root.view().clone().downcast::<RootView>().unwrap();
            let area = view.read(cx).dock().read(cx);
            area.layout(DockPlacement::Center)
                .unwrap()
                .panels()
                .filter_map(|id| area.panel(id)?.view().downcast::<CollectionPanel>().ok())
                .find(|panel| panel.read(cx).name() == name)
                .unwrap()
        })
        .unwrap()
}

/// Switches the focused collection tab to its schema.
pub fn show_schema(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.press(SHOW_SCHEMA, cx);
}

pub async fn open_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(NEW_CONNECTION, cx);
    })
    .unwrap();
    wait_for_dialog(cx, handle).await;
}

pub async fn wait_for_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window
            .try_find("connect.name")
            .is_some_and(|name| name.visible())
    })
    .await;
}

pub async fn wait_for_close(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.name").is_none()
    })
    .await;
}

/// Replaces the text of input `id`.
pub fn fill(window: &mut gpui_kit::Window, id: &str, text: &str, cx: &mut gpui_kit::App) {
    window.click(id.to_string(), cx);
    window.press(SELECT_ALL, cx);
    window.press("backspace", cx);
    window.input(text, cx);
}

pub fn server(version: &str, healthy: bool) -> Connected {
    let client = Client::new(
        &["http://localhost:8108"],
        "k".to_string().into(),
        ClientOptions::default(),
    )
    .unwrap();
    Connected {
        info: ServerInfo {
            url: client.base_url().clone(),
            version: version.into(),
            state: tessera_client::RaftState::Leader,
            healthy,
            capabilities: tessera_core::ServerCapabilities::from_version(version),
            latency: Duration::from_millis(11),
        },
        access: KeyAccess::Admin,
        client,
    }
}

pub async fn connect(
    cx: &mut TestAppContext,
    info: Connected,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    fake_connector(cx, move |_| Ok(info.clone()));
    let (handle, _, rt) = open_app(cx, Vec::new(), Arc::new(MemoryStore::default()));
    open_dialog(cx, handle).await;
    cx.update_window(handle.into(), |_, window, cx| {
        fill(window, "connect.key", "k", cx);
        window.press(SAVE, cx);
    })
    .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
        window.try_find("connect.name").is_none() && window.try_find("figure.version").is_some()
    })
    .await;
    (handle, rt)
}

pub fn label(window: &mut gpui_kit::Window, id: &'static str) -> String {
    window.find(id).label().unwrap_or_default().to_string()
}

pub fn node() -> Node {
    Node {
        protocol: Protocol::Http,
        host: "localhost".into(),
        port: 8108,
        path: String::new(),
    }
}

/// A staging and a read-only prod profile, both with a stored key.
pub fn two_profiles() -> (Profile, Profile, Arc<MemoryStore>) {
    let mut staging = Profile::new("staging-eu", vec![node()]);
    staging.environment = Environment::Staging;
    staging.color = ProfileColor::Amber;
    let mut prod = Profile::new("prod-ch", vec![node()]);
    prod.environment = Environment::Prod;
    prod.color = ProfileColor::Red;
    prod.read_only = true;
    let secrets = Arc::new(MemoryStore::default());
    for p in [&staging, &prod] {
        secrets.set(p.id, &"k".to_string().into()).unwrap();
    }
    (staging, prod, secrets)
}

/// Opens a window with saved `profiles`.
pub fn open_profiles(
    cx: &mut TestAppContext,
    profiles: Vec<Profile>,
    secrets: Arc<MemoryStore>,
) -> (WindowHandle<Root>, tokio::runtime::Runtime) {
    let (handle, _, rt) = open_app(cx, profiles, secrets);
    (handle, rt)
}

pub fn entry(profile: &Profile) -> String {
    format!("sidebar.profile.{}", profile.id)
}

/// Presses Tab until `id` has keyboard focus.
pub fn focus(window: &mut gpui_kit::Window, id: &str, cx: &mut gpui_kit::App) {
    window.render_frame(cx);
    for _ in 0..20 {
        if window.find(id.to_string()).focused() == Some(true) {
            return;
        }
        window.press("tab", cx);
    }
    panic!("Tab never focused {id}");
}

pub async fn wait_for_label(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    id: String,
    part: &'static str,
) {
    cx.wait_for(
        handle.into(),
        Duration::from_millis(10_000),
        move |window, _| {
            window
                .try_find(id.clone())
                .and_then(|e| e.label().map(|l| l.contains(part)))
                .unwrap_or(false)
        },
    )
    .await;
}

/// Opens the entry's context menu and clicks its first item, which must read `label`.
pub async fn menu_first_item(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entry: String,
    label: &'static str,
) {
    menu_item(cx, handle, entry, 0, label).await;
}

/// Opens the entry's context menu and clicks item `index`, which must read `label`.
pub async fn menu_item(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entry: String,
    index: usize,
    label: &'static str,
) {
    cx.update_window(handle.into(), |_, window, cx| window.right_click(entry, cx))
        .unwrap();
    cx.wait_for(handle.into(), Duration::from_millis(500), |window, _| {
        window.try_find("popup-menu").is_some()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let mut menu = window.within("popup-menu");
        assert_eq!(menu.find(index).label(), Some(label));
        menu.click(index, cx);
    })
    .unwrap();
}

/// Waits until element `id` is (`true`) or is not (`false`) in the window.
pub async fn wait_for_element(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    id: String,
    present: bool,
) {
    cx.wait_for(
        handle.into(),
        Duration::from_millis(1000),
        move |window, _| window.try_find(id.clone()).is_some() == present,
    )
    .await;
}
