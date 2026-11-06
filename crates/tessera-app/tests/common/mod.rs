// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers shared by the UI test binaries.

// Each test binary uses only some of the helpers.
#![allow(dead_code)]
// Helpers unwrap; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
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
use tessera_app::jobs::{self, JobId, JobsGlobal};
use tessera_app::poller::{CollectionsFetcherGlobal, NodeSample, PollFetcherGlobal, SampleFuture};
use tessera_app::relevance::{
    AttachError, AttachOutcome, RelevanceBackend, RelevanceBackendGlobal,
};
use tessera_app::runtime::Tokio;
use tessera_app::search::{MultiAnswer, SearchAnswer, SearchBackend, SearchBackendGlobal};
use tessera_app::state::AppState;
use tessera_app::transfer::{FileStamp, TransferBackend, TransferBackendGlobal, file_stamp};
use tessera_app::views::dock::CollectionPanel;
use tessera_app::views::root::RootView;
use tessera_client::{
    Alias, Client, ClientOptions, Collection, CollectionSummary, Debug, DocumentQuery, Error,
    Health, ItemSet, KeyAccess, Metrics, NodeClient, Origin, SchemaChange, SetKind, Stats,
    StopwordSet,
};
use tessera_core::documents::page::DocumentPage;
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use tessera_core::relevance::synonym_file::SetFileError;
use tessera_core::search::results::{parse_multi, parse_result};
use tessera_core::secrets::{MemoryStore, SecretStore};
use tessera_core::store::{ConnectionsFile, StoreError};
use tessera_core::transfer::export::{ExportJob, ExportOutcome, ExportProgress};
use tessera_core::transfer::import::{ImportJob, ImportOutcome, ImportProgress};
use tessera_core::transfer::infer::InferredSchema;
use tessera_core::transfer::preview::Preview;
use tessera_core::transfer::report::{Report, ReportRow};
use tessera_core::transfer::source::{ReadError, ReadOptions, Source};
use tessera_core::transfer::{JobControl, JobEnd};
use tokio::sync::{oneshot, watch};

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
        if !cx.has_global::<JobsGlobal>() {
            jobs::init(cx);
        }
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
    if !cx.has_global::<TransferBackendGlobal>() {
        cx.set_global(TransferBackendGlobal(Arc::new(FakeTransferBackend(
            Arc::default(),
        ))));
    }
    if !cx.has_global::<RelevanceBackendGlobal>() {
        cx.set_global(RelevanceBackendGlobal(Arc::new(FakeRelevanceBackend(
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
    /// When set, the next `create` answers only once the sender fires or is dropped.
    pub create_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
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

    /// Holds the next `create` until the returned sender fires or is dropped.
    pub fn hold_create(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.create_gate.lock().unwrap() = Some(rx);
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
        let gate = self.0.create_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.await.ok();
            }
            answer
        })
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

/// A running job of a [`FakeTransfer`]: its progress sender and how to end it.
enum FakeJob {
    Import(
        watch::Sender<ImportProgress>,
        oneshot::Sender<ImportOutcome>,
    ),
    Export(
        watch::Sender<ExportProgress>,
        oneshot::Sender<ExportOutcome>,
    ),
}

/// What a transfer fake answers and has been asked. Its jobs run until the test ends them.
#[derive(Default)]
pub struct FakeTransfer {
    jobs: Mutex<HashMap<u64, FakeJob>>,
    /// What `preview` answers; `None` holds it open.
    pub preview: Mutex<Option<Result<Preview, ReadError>>>,
    /// What `infer` answers; `None` holds it open.
    pub infer: Mutex<Option<Result<InferredSchema, ReadError>>>,
    /// What `save_report` answers; `None` answers with [`Self::saved_lines`].
    pub save_error: Mutex<Option<String>>,
    /// Lines a successful `save_report` says it wrote.
    pub saved_lines: Mutex<u64>,
    /// `preview` calls.
    pub previews: Mutex<Vec<(Source, ReadOptions, Option<Collection>)>>,
    /// `infer` calls.
    pub infers: Mutex<Vec<(Source, ReadOptions)>>,
    /// `import` calls.
    pub imports: Mutex<Vec<ImportJob>>,
    /// `export` calls.
    pub exports: Mutex<Vec<ExportJob>>,
    /// `save_report` calls: the path and whether only failed lines.
    pub saves: Mutex<Vec<(PathBuf, bool)>>,
}

impl FakeTransfer {
    /// Publishes import progress of job `id`.
    pub fn progress(&self, id: JobId, progress: ImportProgress) {
        match self.jobs.lock().unwrap().get(&id.0) {
            Some(FakeJob::Import(tx, _)) => {
                tx.send_replace(progress);
            }
            _ => panic!("no import {id:?}"),
        }
    }

    /// Publishes export progress of job `id`.
    pub fn export_progress(&self, id: JobId, progress: ExportProgress) {
        match self.jobs.lock().unwrap().get(&id.0) {
            Some(FakeJob::Export(tx, _)) => {
                tx.send_replace(progress);
            }
            _ => panic!("no export {id:?}"),
        }
    }

    /// Ends import `id` with `outcome`.
    pub fn finish(&self, id: JobId, outcome: ImportOutcome) {
        match self.jobs.lock().unwrap().remove(&id.0) {
            Some(FakeJob::Import(_, done)) => drop(done.send(outcome)),
            _ => panic!("no import {id:?}"),
        }
    }

    /// Ends export `id` with `outcome`.
    pub fn finish_export(&self, id: JobId, outcome: ExportOutcome) {
        match self.jobs.lock().unwrap().remove(&id.0) {
            Some(FakeJob::Export(_, done)) => drop(done.send(outcome)),
            _ => panic!("no export {id:?}"),
        }
    }
}

/// The installed backend; the test keeps the [`FakeTransfer`] handle.
pub struct FakeTransferBackend(pub Arc<FakeTransfer>);

impl TransferBackend for FakeTransferBackend {
    fn file_stamp(
        &self,
        path: PathBuf,
    ) -> tessera_app::collections::BoxFuture<Result<FileStamp, ReadError>> {
        let answer = file_stamp(&path).map_err(|e| ReadError::Io(e.to_string()));
        Box::pin(async move { answer })
    }

    fn preview(
        &self,
        source: Source,
        options: ReadOptions,
        schema: Option<Collection>,
    ) -> tessera_app::collections::BoxFuture<Result<Preview, ReadError>> {
        self.0
            .previews
            .lock()
            .unwrap()
            .push((source, options, schema));
        let answer = self.0.preview.lock().unwrap().clone();
        Box::pin(async move {
            match answer {
                Some(answer) => answer,
                None => std::future::pending().await,
            }
        })
    }

    fn infer(
        &self,
        source: Source,
        options: ReadOptions,
    ) -> tessera_app::collections::BoxFuture<Result<InferredSchema, ReadError>> {
        self.0.infers.lock().unwrap().push((source, options));
        let answer = self.0.infer.lock().unwrap().clone();
        Box::pin(async move {
            match answer {
                Some(answer) => answer,
                None => std::future::pending().await,
            }
        })
    }

    fn import(
        &self,
        _: Client,
        job: ImportJob,
        _: JobControl,
        progress: watch::Sender<ImportProgress>,
    ) -> tessera_app::collections::BoxFuture<ImportOutcome> {
        let (done, outcome) = oneshot::channel();
        let id = {
            let mut imports = self.0.imports.lock().unwrap();
            imports.push(job);
            let id = imports.len() + self.0.exports.lock().unwrap().len() - 1;
            id as u64
        };
        self.0
            .jobs
            .lock()
            .unwrap()
            .insert(id, FakeJob::Import(progress, done));
        Box::pin(async move { outcome.await.unwrap() })
    }

    fn export(
        &self,
        _: Client,
        job: ExportJob,
        _: JobControl,
        progress: watch::Sender<ExportProgress>,
    ) -> tessera_app::collections::BoxFuture<ExportOutcome> {
        let (done, outcome) = oneshot::channel();
        let id = {
            let mut exports = self.0.exports.lock().unwrap();
            exports.push(job);
            let id = exports.len() + self.0.imports.lock().unwrap().len() - 1;
            id as u64
        };
        self.0
            .jobs
            .lock()
            .unwrap()
            .insert(id, FakeJob::Export(progress, done));
        Box::pin(async move { outcome.await.unwrap() })
    }

    fn save_report(
        &self,
        _: Arc<Report>,
        path: PathBuf,
        failed_only: bool,
    ) -> tessera_app::collections::BoxFuture<Result<u64, String>> {
        self.0.saves.lock().unwrap().push((path, failed_only));
        let answer = match self.0.save_error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(*self.0.saved_lines.lock().unwrap()),
        };
        Box::pin(async move { answer })
    }
}

/// Installs a transfer backend with scripted answers and the jobs entity.
pub fn fake_transfer(cx: &mut TestAppContext) -> Arc<FakeTransfer> {
    let fake = Arc::new(FakeTransfer::default());
    let installed = FakeTransferBackend(fake.clone());
    cx.update(|cx| {
        cx.set_global(TransferBackendGlobal(Arc::new(installed)));
        if !cx.has_global::<JobsGlobal>() {
            jobs::init(cx);
        }
    });
    fake
}

/// A report holding `rows`.
pub fn report_with(rows: Vec<ReportRow>) -> Arc<Report> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut report = Report::new().await.unwrap();
        for row in rows {
            report.add(row).await.unwrap();
        }
        report.finish().await.unwrap();
        Arc::new(report)
    })
}

/// The outcome of an import that ended with `end` after `progress`.
pub fn import_outcome(
    end: JobEnd<tessera_core::transfer::TransferError>,
    progress: ImportProgress,
) -> ImportOutcome {
    ImportOutcome {
        end,
        progress,
        report: report_with(Vec::new()),
        stopped_at_line: None,
        count: None,
    }
}

/// The outcome of an export that ended with `end`, having written `docs` of `expected`.
pub fn export_outcome(
    end: JobEnd<tessera_core::transfer::TransferError>,
    docs: u64,
    expected: Option<u64>,
    unlisted_values: u64,
) -> ExportOutcome {
    ExportOutcome {
        end,
        progress: ExportProgress {
            docs,
            expected,
            ..Default::default()
        },
        unlisted_values,
        kept_existing: false,
    }
}

/// An export that found nothing and left the file that was there.
pub fn export_kept_outcome() -> ExportOutcome {
    ExportOutcome {
        kept_existing: true,
        ..export_outcome(JobEnd::Finished, 0, None, 0)
    }
}

/// One search the fake was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCall {
    /// The collection.
    pub collection: String,
    /// The parameters as sent.
    pub params: Vec<(String, String)>,
}

impl SearchCall {
    /// The value of parameter `key`.
    pub fn param(&self, key: &str) -> Option<&str> {
        let found = self.params.iter().find(|(k, _)| k == key);
        found.map(|(_, v)| v.as_str())
    }
}

/// A search backend that answers with a set text and records what it was asked.
pub struct FakeSearch {
    /// The body of each answer.
    pub body: Mutex<String>,
    /// Every search, in order.
    pub calls: Mutex<Vec<SearchCall>>,
    /// Answers fail with this status and Typesense message.
    pub fail: Mutex<Option<(u16, String)>>,
    /// Searches that carry this parameter fail with this status and message.
    pub fail_with_param: Mutex<Option<(String, (u16, String))>>,
    /// When set, the next answer waits until the sender fires or is dropped.
    pub hold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Every multi-search, in order.
    pub multi_calls: Mutex<Vec<tessera_client::MultiSearch>>,
    /// The body of each multi-search answer; by default [`search_body`] of 12 per search.
    pub multi_body: Mutex<Option<String>>,
    /// The presets on the server.
    pub presets: Mutex<Vec<tessera_client::Preset>>,
    /// Every preset saved, in order.
    pub saved_presets: Mutex<Vec<(String, serde_json::Value)>>,
    /// `save_preset` stores this instead of the value sent.
    pub stored_preset: Mutex<Option<serde_json::Value>>,
    /// Every raw request, in order.
    pub raw_calls: Mutex<Vec<tessera_client::RawRequest>>,
    /// The status and body of each raw answer.
    pub raw_answer: Mutex<(u16, String)>,
    /// Raw requests that ran to their end.
    pub raw_done: std::sync::atomic::AtomicUsize,
}

impl FakeSearch {
    /// Holds the next answer until the returned sender fires. The sender reports closed
    /// once the view dropped the search.
    pub fn hold_next(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.hold.lock().unwrap() = Some(rx);
        tx
    }

    /// The searches asked so far.
    pub fn calls(&self) -> Vec<SearchCall> {
        self.calls.lock().unwrap().clone()
    }

    /// The multi-searches asked so far.
    pub fn multi_calls(&self) -> Vec<tessera_client::MultiSearch> {
        self.multi_calls.lock().unwrap().clone()
    }
}

/// The body of a search that found `found` documents.
pub fn search_body(found: u64) -> String {
    format!(r#"{{"found":{found},"out_of":100,"page":1,"search_time_ms":3,"hits":[]}}"#)
}

pub struct FakeSearchBackend(pub Arc<FakeSearch>);

impl SearchBackend for FakeSearchBackend {
    fn search(
        &self,
        _: Client,
        collection: String,
        params: Vec<(String, String)>,
        tags: (String, String),
    ) -> tessera_app::collections::BoxFuture<Result<SearchAnswer, Error>> {
        let fake = &self.0;
        let by_param = fake.fail_with_param.lock().unwrap().clone();
        let by_param = by_param.filter(|(key, _)| params.iter().any(|(k, _)| k == key));
        fake.calls
            .lock()
            .unwrap()
            .push(SearchCall { collection, params });
        let body = fake.body.lock().unwrap().clone();
        let fail = by_param
            .map(|(_, e)| e)
            .or_else(|| fake.fail.lock().unwrap().clone());
        let hold = fake.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            if let Some(e) = fail {
                return Err(http_error(e));
            }
            let result = parse_result(&body, (&tags.0, &tags.1)).map_err(Error::Decode)?;
            Ok(SearchAnswer {
                text: body.into(),
                result,
            })
        })
    }

    fn multi(
        &self,
        _: Client,
        request: tessera_client::MultiSearch,
        tags: Vec<(String, String)>,
    ) -> tessera_app::collections::BoxFuture<Result<MultiAnswer, Error>> {
        let fake = &self.0;
        fake.multi_calls.lock().unwrap().push(request.clone());
        let body = fake.multi_body.lock().unwrap().clone().unwrap_or_else(|| {
            let results = vec![search_body(12); request.searches.len()].join(",");
            if request.union {
                search_body(12)
            } else {
                format!(r#"{{"results":[{results}]}}"#)
            }
        });
        let fail = fake.fail.lock().unwrap().clone();
        let hold = fake.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            if let Some(e) = fail {
                return Err(http_error(e));
            }
            let (start, end) = &tags[0];
            let result = parse_multi(&body, request.union, (start, end)).map_err(Error::Decode)?;
            Ok(MultiAnswer {
                text: body.into(),
                result,
            })
        })
    }

    fn presets(
        &self,
        _: Client,
    ) -> tessera_app::collections::BoxFuture<Result<Vec<tessera_client::Preset>, Error>> {
        let fake = &self.0;
        let presets = fake.presets.lock().unwrap().clone();
        let fail = fake.fail.lock().unwrap().clone();
        Box::pin(async move { fail.map_or(Ok(presets), |e| Err(http_error(e))) })
    }

    fn save_preset(
        &self,
        _: Client,
        name: String,
        value: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        let fake = &self.0;
        fake.saved_presets
            .lock()
            .unwrap()
            .push((name.clone(), value.clone()));
        let stored = fake.stored_preset.lock().unwrap().clone().unwrap_or(value);
        let fail = fake.fail.lock().unwrap().clone();
        if fail.is_none() {
            let mut presets = fake.presets.lock().unwrap();
            presets.retain(|p| p.name != name);
            presets.push(tessera_client::Preset {
                name,
                value: stored.clone(),
            });
        }
        Box::pin(async move { fail.map_or(Ok(stored), |e| Err(http_error(e))) })
    }

    fn raw(
        &self,
        _: Client,
        request: tessera_client::RawRequest,
    ) -> tessera_app::collections::BoxFuture<Result<tessera_client::RawResponse, Error>> {
        let fake = self.0.clone();
        fake.raw_calls.lock().unwrap().push(request);
        let (status, body) = fake.raw_answer.lock().unwrap().clone();
        let fail = fake.fail.lock().unwrap().clone();
        let hold = fake.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            fake.raw_done
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(e) = fail {
                return Err(http_error(e));
            }
            Ok(tessera_client::RawResponse {
                status,
                headers: vec![("x-demo".into(), "yes".into())],
                body,
                duration: Duration::from_millis(12),
            })
        })
    }
}

/// Installs a search backend answering with [`search_body`] of 12 documents.
pub fn install_search_fake(cx: &mut TestAppContext) -> Arc<FakeSearch> {
    let fake = Arc::new(FakeSearch {
        body: Mutex::new(search_body(12)),
        calls: Mutex::default(),
        fail: Mutex::default(),
        fail_with_param: Mutex::default(),
        hold: Mutex::default(),
        multi_calls: Mutex::default(),
        multi_body: Mutex::default(),
        presets: Mutex::default(),
        saved_presets: Mutex::default(),
        stored_preset: Mutex::default(),
        raw_calls: Mutex::default(),
        raw_answer: Mutex::new((200, "{}".to_string())),
        raw_done: Default::default(),
    });
    let installed = FakeSearchBackend(fake.clone());
    cx.update(|cx| cx.set_global(SearchBackendGlobal(Arc::new(installed))));
    fake
}

/// A relevance backend that records each call as text.
#[derive(Default)]
pub struct FakeRelevance {
    /// Every call, in order, as `method arg…`.
    pub calls: Mutex<Vec<String>>,
    /// Writes fail with this status and Typesense message.
    pub fail: Mutex<Option<(u16, String)>>,
    /// `save_alias` answers with this target instead of the one sent.
    pub stored_target: Mutex<Option<String>>,
    /// The stopword sets on the server.
    pub stopwords: Mutex<Vec<StopwordSet>>,
    /// `save_stopwords` stores these words instead of the ones sent.
    pub stored_words: Mutex<Option<Vec<String>>>,
    /// The sets on the server.
    pub sets: Mutex<Vec<ItemSet>>,
    /// `save_item` reads back this instead of the item sent.
    pub stored_item: Mutex<Option<serde_json::Value>>,
    /// `save_set` keeps only this many items.
    pub keep_items: Mutex<Option<usize>>,
    /// Calls that start with this fail with this message, whatever `fail` says.
    pub fail_call: Mutex<Option<(String, String)>>,
    /// `sets` and `set` fail.
    pub fail_reads: Mutex<bool>,
    /// The collections `attach` edits and `list` reads, while `attach_ignored` is off.
    pub data: Mutex<Option<Arc<FakeData>>>,
    /// `attach` changes nothing on the server.
    pub attach_ignored: Mutex<bool>,
    /// `attach` reports these names as left out.
    pub attach_dropped: Mutex<Vec<String>>,
    /// `attach` reports a list unlike the one sent.
    pub attach_mismatch: Mutex<bool>,
    /// What `read_set_file` answers.
    pub file_text: Mutex<Option<Result<String, SetFileError>>>,
    /// What `write_set_file` was given.
    pub written: Mutex<Vec<(PathBuf, String)>>,
    /// The next `save_stopwords` answers once this resolves.
    pub hold: Mutex<Option<oneshot::Receiver<()>>>,
}

impl FakeRelevance {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    /// `outcome` for `call`, which is recorded.
    fn write(&self, call: String) -> Result<(), Error> {
        let refused = self.fail_call.lock().unwrap().clone();
        let refused = refused.filter(|(start, _)| call.starts_with(start));
        self.record(call);
        match refused {
            Some((_, message)) => Err(http_error((500, message))),
            None => self.outcome(),
        }
    }

    fn outcome(&self) -> Result<(), Error> {
        match self.fail.lock().unwrap().clone() {
            Some(e) => Err(http_error(e)),
            None => Ok(()),
        }
    }
}

pub struct FakeRelevanceBackend(pub Arc<FakeRelevance>);

impl RelevanceBackend for FakeRelevanceBackend {
    fn sets(
        &self,
        _: Client,
        _: SetKind,
    ) -> tessera_app::collections::BoxFuture<Result<Vec<ItemSet>, Error>> {
        let sets = self.0.sets.lock().unwrap().clone();
        let fail = *self.0.fail_reads.lock().unwrap();
        Box::pin(async move {
            if fail {
                return Err(http_error((500, "no list".into())));
            }
            Ok(sets)
        })
    }

    fn set(
        &self,
        _: Client,
        _: SetKind,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<ItemSet, Error>> {
        let set = self
            .0
            .sets
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.name == name)
            .cloned();
        Box::pin(async move { set.ok_or_else(|| http_error((404, "Not found.".into()))) })
    }

    fn save_set(
        &self,
        _: Client,
        _: SetKind,
        name: String,
        mut items: Vec<serde_json::Value>,
    ) -> tessera_app::collections::BoxFuture<Result<ItemSet, Error>> {
        let answer = self.0.write(format!("save_set {name} {}", items.len()));
        let answer = answer.map(|()| {
            if let Some(keep) = *self.0.keep_items.lock().unwrap() {
                items.truncate(keep);
            }
            let set = ItemSet { name, items };
            let mut sets = self.0.sets.lock().unwrap();
            sets.retain(|s| s.name != set.name);
            sets.push(set.clone());
            set
        });
        Box::pin(async move { answer })
    }

    fn delete_set(
        &self,
        _: Client,
        _: SetKind,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        let answer = self.0.write(format!("delete_set {name}"));
        if answer.is_ok() {
            self.0.sets.lock().unwrap().retain(|s| s.name != name);
        }
        Box::pin(async move { answer })
    }

    fn attach(
        &self,
        _: Client,
        _: SetKind,
        collection: String,
        set: String,
        attach: bool,
    ) -> tessera_app::collections::BoxFuture<Result<AttachOutcome, AttachError>> {
        let answer = self.0.write(format!("attach {collection} {set} {attach}"));
        let mut list = Vec::new();
        if let Some(data) = &*self.0.data.lock().unwrap() {
            let mut data = data.data.lock().unwrap();
            let all = &mut data.as_mut().unwrap().0;
            if let Some(found) = all.iter_mut().find(|c| c.name == collection) {
                if answer.is_ok() && !*self.0.attach_ignored.lock().unwrap() {
                    found.synonym_sets.retain(|n| *n != set);
                    if attach {
                        found.synonym_sets.push(set);
                    }
                }
                list.clone_from(&found.synonym_sets);
            }
        }
        let outcome = AttachOutcome {
            list,
            dropped: self.0.attach_dropped.lock().unwrap().clone(),
            mismatch: *self.0.attach_mismatch.lock().unwrap(),
        };
        Box::pin(async move { answer.map(|()| outcome).map_err(Into::into) })
    }

    fn read_set_file(
        &self,
        _: PathBuf,
    ) -> tessera_app::collections::BoxFuture<Result<String, SetFileError>> {
        let text = self.0.file_text.lock().unwrap().clone();
        Box::pin(async move { text.unwrap_or_else(|| Ok(String::new())) })
    }

    fn write_set_file(
        &self,
        path: PathBuf,
        text: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), String>> {
        self.0.written.lock().unwrap().push((path, text));
        Box::pin(async move { Ok(()) })
    }

    fn save_item(
        &self,
        _: Client,
        _: SetKind,
        name: String,
        id: String,
        item: serde_json::Value,
    ) -> tessera_app::collections::BoxFuture<Result<serde_json::Value, Error>> {
        let answer = self.0.write(format!("save_item {name} {id}"));
        let stored = self.0.stored_item.lock().unwrap().clone().unwrap_or(item);
        let answer = answer.map(|()| {
            let mut sets = self.0.sets.lock().unwrap();
            if let Some(set) = sets.iter_mut().find(|s| s.name == name) {
                set.items.retain(|i| i["id"] != id.as_str());
                set.items.push(stored.clone());
            }
            stored
        });
        Box::pin(async move { answer })
    }

    fn delete_item(
        &self,
        _: Client,
        _: SetKind,
        name: String,
        id: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        let answer = self.0.write(format!("delete_item {name} {id}"));
        if answer.is_ok() {
            let mut sets = self.0.sets.lock().unwrap();
            if let Some(set) = sets.iter_mut().find(|s| s.name == name) {
                set.items.retain(|i| i["id"] != id.as_str());
            }
        }
        Box::pin(async move { answer })
    }

    fn stopwords(
        &self,
        _: Client,
    ) -> tessera_app::collections::BoxFuture<Result<Vec<StopwordSet>, Error>> {
        let answer = self.0.stopwords.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }

    fn save_stopwords(
        &self,
        _: Client,
        id: String,
        words: Vec<String>,
        locale: Option<String>,
    ) -> tessera_app::collections::BoxFuture<Result<StopwordSet, Error>> {
        self.0.record(format!("save_stopwords {id}"));
        let stored = self.0.stored_words.lock().unwrap().clone();
        let set = StopwordSet {
            id,
            stopwords: stored.unwrap_or(words),
            locale,
        };
        let answer = self.0.outcome().map(|()| {
            let mut sets = self.0.stopwords.lock().unwrap();
            sets.retain(|s| s.id != set.id);
            sets.push(set.clone());
            set
        });
        let hold = self.0.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.await.ok();
            }
            answer
        })
    }

    fn delete_stopwords(
        &self,
        _: Client,
        id: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        self.0.record(format!("delete_stopwords {id}"));
        let answer = self.0.outcome();
        if answer.is_ok() {
            self.0.stopwords.lock().unwrap().retain(|s| s.id != id);
        }
        Box::pin(async move { answer })
    }

    fn save_alias(
        &self,
        _: Client,
        name: String,
        collection: String,
    ) -> tessera_app::collections::BoxFuture<Result<Alias, Error>> {
        self.0.record(format!("save_alias {name} {collection}"));
        let stored = self.0.stored_target.lock().unwrap().clone();
        let answer = self.0.outcome().map(|()| Alias {
            name,
            collection_name: stored.unwrap_or(collection),
        });
        Box::pin(async move { answer })
    }

    fn alias(
        &self,
        _: Client,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<Alias, Error>> {
        self.0.record(format!("alias {name}"));
        let answer = self.0.outcome().map(|()| Alias {
            name,
            collection_name: String::new(),
        });
        Box::pin(async move { answer })
    }

    fn delete_alias(
        &self,
        _: Client,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        self.0.record(format!("delete_alias {name}"));
        let answer = self.0.outcome();
        Box::pin(async move { answer })
    }

    fn delete_preset(
        &self,
        _: Client,
        name: String,
    ) -> tessera_app::collections::BoxFuture<Result<(), Error>> {
        self.0.record(format!("delete_preset {name}"));
        let answer = self.0.outcome();
        Box::pin(async move { answer })
    }
}

/// Installs a relevance backend and returns the handle to inspect it.
pub fn fake_relevance(cx: &mut TestAppContext) -> Arc<FakeRelevance> {
    let fake = Arc::new(FakeRelevance::default());
    let installed = FakeRelevanceBackend(fake.clone());
    cx.update(|cx| cx.set_global(RelevanceBackendGlobal(Arc::new(installed))));
    fake
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
pub const OPEN_SEARCH: &str = key("cmd-shift-e", "ctrl-shift-e");
pub const OPEN_HISTORY: &str = key("cmd-shift-h", "ctrl-shift-h");
pub const OPEN_SAVED: &str = key("cmd-o", "ctrl-o");
pub const SAVE_QUERY: &str = key("cmd-s", "ctrl-s");
pub const LEAVE_PRESET: &str = key("cmd-shift-g", "ctrl-shift-g");
pub const DETACH_MISSING: &str = key("cmd-shift-k", "ctrl-shift-k");
pub const OPEN_PRESETS_TAB: &str = key("alt-cmd-5", "ctrl-alt-5");
pub const OPEN_PRESETS: &str = key("cmd-alt-p", "ctrl-alt-p");
pub const OPEN_EXPORT: &str = key("cmd-alt-e", "ctrl-alt-e");
pub const SHOW_FORM: &str = key("cmd-1", "ctrl-1");
pub const SHOW_JSON: &str = key("cmd-2", "ctrl-2");
pub const SHOW_HITS: &str = key("cmd-3", "ctrl-3");
pub const SHOW_RAW: &str = key("cmd-4", "ctrl-4");
pub const TOGGLE_INSTANT: &str = key("cmd-alt-l", "ctrl-alt-l");
pub const ADD_SEARCH: &str = key("cmd-t", "ctrl-t");
pub const REMOVE_SEARCH: &str = key("cmd-shift-w", "ctrl-shift-w");
pub const NEXT_SEARCH: &str = key("cmd-alt-right", "ctrl-alt-right");
pub const PREVIOUS_SEARCH: &str = key("cmd-alt-left", "ctrl-alt-left");
pub const TOGGLE_UNION: &str = key("cmd-shift-m", "ctrl-shift-m");
pub const NEXT_PAGE: &str = key("cmd-alt-down", "ctrl-alt-down");

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
